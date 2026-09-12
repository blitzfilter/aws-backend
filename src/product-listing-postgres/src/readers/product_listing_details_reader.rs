use crate::{
    object_id::{PersistedObjectIdError, try_from_uuid},
    product_listing_auction::{ProductListingAuctionParts, auction_from_parts},
    url::referral_configuration,
};
use application::personalized::Personalized;

use indexmap::IndexSet;
use listing_source_core::{ListingSourceName, ListingSourceSlugId, outbound_url};
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};

use platform_postgres::SqlxTransaction;
use product_listing_core::content_policy::{ContentPolicyDecision, SensitiveContentCategory};
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::{ListingSaleObservation, ProductListingPricing};

use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::source_listing_id::SourceListingId;

use product_listing_core::title::Title;
use product_listing_service::ports::{
    ListingSourceSummary, PersonalizedProductListingDetailsReadModel,
    ProductListingDetailsReadError, ProductListingDetailsReadModel,
    ProductListingDetailsReadRequest, ProductListingDetailsReader,
    ProductListingDetailsReaderFactory,
};
use product_listing_service::use_cases::queries::get_product_listing::ProductListingLookup;
use product_listing_service::user_state::{
    ContentVisibilityUserState, NotificationUserState, ProductListingUserState,
    SearchFilterUserState, WatchlistUserState,
};
use search_filter_core::{
    enhanced_match_reason::EnhancedMatchReason, user_search_filter_name::UserSearchFilterName,
};
use serde::Deserialize;
use sqlx::{PgConnection, Postgres, QueryBuilder};

use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingDetailsReaderFactory;

struct SqlxProductListingDetailsReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct ProductListingDetailsRow {
    pub(super) product_listing_id: uuid::Uuid,
    product_listing_title_slug_id: String,
    current_event_id: uuid::Uuid,
    listing_source_id: uuid::Uuid,
    source_listing_id: String,
    listing_source_name: String,
    listing_source_slug_id: String,
    listing_source_referral_configuration: Option<serde_json::Value>,
    product_title_text: Option<String>,
    product_title_language: Option<String>,
    product_description_text: Option<String>,
    product_description_language: Option<String>,
    title_text: Option<String>,
    title_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
    price_kind: Option<String>,
    price_amount: Option<i64>,
    price_currency: Option<String>,
    price_estimate_min_amount: Option<i64>,
    price_estimate_min_currency: Option<String>,
    price_estimate_max_amount: Option<i64>,
    price_estimate_max_currency: Option<String>,
    sale_observation_fx_rate_id: Option<uuid::Uuid>,
    sale_observed_at: Option<OffsetDateTime>,
    availability: Option<String>,
    lifecycle: String,
    url: String,
    product_images: serde_json::Value,
    content_policy_decision: Option<String>,
    content_policy_category: Option<String>,
    auction_context_product_listing_id: Option<uuid::Uuid>,
    auction_lot_number: Option<String>,
    auction_catalogue_position: Option<i64>,
    auction_timing_product_listing_id: Option<uuid::Uuid>,
    auction_bidding_opens_precision: Option<String>,
    auction_bidding_opens_instant_at: Option<OffsetDateTime>,
    auction_bidding_opens_date_on: Option<time::Date>,
    auction_bidding_opens_source_timezone: Option<String>,
    auction_scheduled_closes_precision: Option<String>,
    auction_scheduled_closes_instant_at: Option<OffsetDateTime>,
    auction_scheduled_closes_date_on: Option<time::Date>,
    auction_scheduled_closes_source_timezone: Option<String>,
    auction_reported_closed_at: Option<OffsetDateTime>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    personalization_user_id: Option<uuid::Uuid>,
    user_show_unassessed_or_sensitive_content: Option<bool>,
    user_tier: Option<String>,
    watchlist_notifications: Option<bool>,
    selected_match_user_search_filter_id: Option<uuid::Uuid>,
    selected_match_user_search_filter_name: Option<String>,
    selected_match_reason: Option<String>,
    selected_match_feedback: Option<bool>,
    selected_match_month_position: Option<i64>,
    unseen_notification_ids: Option<Vec<uuid::Uuid>>,
}

#[derive(Debug, Deserialize)]
struct ProductListingImageJson {
    url: String,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ProductListingDetailsRowMappingError {
    #[error("product details row contains an invalid persisted object ID")]
    InvalidObjectId(#[source] PersistedObjectIdError),
    #[error("product details row contains an invalid persisted value")]
    InvalidValue,
}

impl From<()> for ProductListingDetailsRowMappingError {
    fn from((): ()) -> Self {
        Self::InvalidValue
    }
}

impl SqlxProductListingDetailsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingDetailsReaderFactory<SqlxTransaction>
    for SqlxProductListingDetailsReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingDetailsReader + 'tx {
        SqlxProductListingDetailsReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingDetailsReader for SqlxProductListingDetailsReader<'_> {
    async fn find_details(
        &mut self,
        request: &ProductListingDetailsReadRequest,
    ) -> Result<Option<PersonalizedProductListingDetailsReadModel>, ProductListingDetailsReadError>
    {
        let requested_language = request.language.as_str();
        let user_id = request.user_id.map(|id| id.into_uuid());
        let row = match &request.lookup {
            ProductListingLookup::ById(product_listing_id) => {
                let mut query = QueryBuilder::<Postgres>::new(product_details_select(
                    DEFAULT_NOTIFICATION_STATES,
                ));
                query.push(" WHERE p.product_listing_id = $3");
                query
                    .build_query_as::<ProductListingDetailsRow>()
                    .bind(requested_language)
                    .bind(user_id)
                    .bind(product_listing_id.as_uuid())
                    .fetch_optional(&mut *self.connection)
                    .await
            }

            ProductListingLookup::ByTitleSlug(product_listing_title_slug_id) => {
                let mut query = QueryBuilder::<Postgres>::new(product_details_select(
                    DEFAULT_NOTIFICATION_STATES,
                ));
                query.push(" WHERE p.product_listing_title_slug_id = $3");
                query
                    .build_query_as::<ProductListingDetailsRow>()
                    .bind(requested_language)
                    .bind(user_id)
                    .bind(product_listing_title_slug_id.as_ref())
                    .fetch_optional(&mut *self.connection)
                    .await
            }
        }
        .map_err(|_| ProductListingDetailsReadError::ProductListingDetailsQueryFailed)?;

        row.map(PersonalizedProductListingDetailsReadModel::try_from)
            .transpose()
            .map_err(|_| ProductListingDetailsReadError::ProductListingDetailsReadModelInvalid)
    }
}

pub(super) const DEFAULT_NOTIFICATION_STATES: &str = r#"
    notification_states AS (
        SELECT
            notification.product_listing_id,
            array_agg(
                notification.notification_id
                ORDER BY notification.created DESC, notification.notification_id DESC
            ) AS unseen_notification_ids
        FROM notifications notification
        WHERE notification.user_id = $2
            AND notification.seen = false
        GROUP BY notification.product_listing_id
    )
"#;

pub(super) fn product_details_select(notification_states: &str) -> String {
    SELECT_PRODUCT_DETAILS.replace("/* NOTIFICATION_STATES */", notification_states)
}

pub(super) const SELECT_PRODUCT_DETAILS: &str = r#"
    WITH /* NOTIFICATION_STATES */
    SELECT
        p.product_listing_id, p.product_listing_title_slug_id, p.current_event_id,
        p.listing_source_id, p.source_listing_id,
        listing_source.name AS listing_source_name,
        listing_source.listing_source_slug_id,
        listing_source.referral_configuration AS listing_source_referral_configuration,
        p.title_text AS product_title_text, p.title_language AS product_title_language,
        p.description_text AS product_description_text,
        p.description_language AS product_description_language,
        selected_text.title_text, selected_text.title_language,
        selected_text.description_text, selected_text.description_language,
        p.price_kind, p.price_amount, p.price_currency, p.price_estimate_min_amount,
        p.price_estimate_min_currency, p.price_estimate_max_amount,
        p.price_estimate_max_currency, p.sale_observation_fx_rate_id, p.sale_observed_at, p.availability, p.lifecycle, p.url,
        p.product_images,
        assessment.decision AS content_policy_decision,
        assessment.category AS content_policy_category,
        auction_context.product_listing_id AS auction_context_product_listing_id,
        auction_context.lot_number AS auction_lot_number,
        auction_context.catalogue_position AS auction_catalogue_position,
        auction_timing.product_listing_id AS auction_timing_product_listing_id,
        auction_timing.bidding_opens_precision AS auction_bidding_opens_precision,
        auction_timing.bidding_opens_instant_at AS auction_bidding_opens_instant_at,
        auction_timing.bidding_opens_date_on AS auction_bidding_opens_date_on,
        auction_timing.bidding_opens_source_timezone AS auction_bidding_opens_source_timezone,
        auction_timing.scheduled_closes_precision AS auction_scheduled_closes_precision,
        auction_timing.scheduled_closes_instant_at AS auction_scheduled_closes_instant_at,
        auction_timing.scheduled_closes_date_on AS auction_scheduled_closes_date_on,
        auction_timing.scheduled_closes_source_timezone AS auction_scheduled_closes_source_timezone,
        auction_timing.reported_closed_at AS auction_reported_closed_at,
        p.created, p.updated,
        $2::uuid AS personalization_user_id,
        authenticated_user.show_unassessed_or_sensitive_content AS user_show_unassessed_or_sensitive_content,
        authenticated_user.tier AS user_tier,
        watchlist.notifications AS watchlist_notifications,
        selected_match.user_search_filter_id AS selected_match_user_search_filter_id,
        selected_match.user_search_filter_name AS selected_match_user_search_filter_name,
        selected_match.enhanced_match_reason AS selected_match_reason,
        selected_match.feedback AS selected_match_feedback,
        selected_match.month_position AS selected_match_month_position,
        notification_state.unseen_notification_ids
    FROM product_listings p
    JOIN listing_sources listing_source
        ON listing_source.listing_source_id = p.listing_source_id
    LEFT JOIN product_listing_auction_contexts auction_context
        ON auction_context.product_listing_id = p.product_listing_id
    LEFT JOIN product_listing_lot_auction_timings auction_timing
        ON auction_timing.product_listing_id = auction_context.product_listing_id
    LEFT JOIN product_listing_content_assessments assessment
        ON assessment.product_listing_id = p.product_listing_id
        AND assessment.source_event_id = p.content_source_event_id
    LEFT JOIN users authenticated_user ON authenticated_user.user_id = $2
    LEFT JOIN product_listing_watchlist watchlist
        ON watchlist.user_id = $2
        AND watchlist.product_listing_id = p.product_listing_id
    LEFT JOIN LATERAL (
        SELECT
            matched.user_search_filter_id,
            matched.user_search_filter_name,
            matched.enhanced_match_reason,
            matched.feedback,
            CASE
                WHEN authenticated_user.tier = 'FREE' THEN (
                    SELECT COUNT(*)
                    FROM search_filter_matches monthly_match
                    WHERE monthly_match.user_id = $2
                        AND monthly_match.created >= (
                            date_trunc('month', matched.created AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                        )
                        AND (
                            monthly_match.created < matched.created
                            OR (
                                monthly_match.created = matched.created
                                AND (
                                    monthly_match.user_search_filter_id < matched.user_search_filter_id
                                    OR (
                                        monthly_match.user_search_filter_id = matched.user_search_filter_id
                                        AND monthly_match.product_listing_id <= matched.product_listing_id
                                    )
                                )
                            )
                        )
                )
                ELSE NULL
            END AS month_position
        FROM search_filter_matches matched
        WHERE matched.user_id = $2
            AND matched.product_listing_id = p.product_listing_id
        ORDER BY matched.created ASC, matched.user_search_filter_id ASC
        LIMIT 1
    ) AS selected_match ON TRUE
    LEFT JOIN notification_states notification_state
        ON notification_state.product_listing_id = p.product_listing_id
    LEFT JOIN LATERAL (
        SELECT
            (
                array_agg(
                    candidates.title_text
                    ORDER BY
                        CASE lower(candidates.title_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.title_language),
                        candidates.source_priority,
                        candidates.title_language,
                        candidates.title_text
                ) FILTER (WHERE candidates.title_text IS NOT NULL AND candidates.title_language IS NOT NULL)
            )[1] AS title_text,
            (
                array_agg(
                    candidates.title_language
                    ORDER BY
                        CASE lower(candidates.title_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.title_language),
                        candidates.source_priority,
                        candidates.title_language,
                        candidates.title_text
                ) FILTER (WHERE candidates.title_text IS NOT NULL AND candidates.title_language IS NOT NULL)
            )[1] AS title_language,
            (
                array_agg(
                    candidates.description_text
                    ORDER BY
                        CASE lower(candidates.description_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.description_language),
                        candidates.source_priority,
                        candidates.description_language,
                        candidates.description_text
                ) FILTER (
                    WHERE candidates.description_text IS NOT NULL
                        AND candidates.description_language IS NOT NULL
                )
            )[1] AS description_text,
            (
                array_agg(
                    candidates.description_language
                    ORDER BY
                        CASE lower(candidates.description_language)
                            WHEN lower($1) THEN 0
                            WHEN 'en' THEN 1
                            WHEN 'de' THEN 2
                            ELSE 3
                        END,
                        lower(candidates.description_language),
                        candidates.source_priority,
                        candidates.description_language,
                        candidates.description_text
                ) FILTER (
                    WHERE candidates.description_text IS NOT NULL
                        AND candidates.description_language IS NOT NULL
                )
            )[1] AS description_language
        FROM (
            SELECT
                translation.title AS title_text,
                translation.language AS title_language,
                translation.description AS description_text,
                translation.language AS description_language,
                0 AS source_priority
            FROM product_listing_translations translation
            WHERE translation.product_listing_id = p.product_listing_id

            UNION ALL

            SELECT
                p.title_text,
                p.title_language,
                p.description_text,
                p.description_language,
                1 AS source_priority
        ) AS candidates
    ) AS selected_text ON TRUE
"#;

impl TryFrom<ProductListingDetailsRow> for PersonalizedProductListingDetailsReadModel {
    type Error = ProductListingDetailsRowMappingError;

    fn try_from(row: ProductListingDetailsRow) -> Result<Self, Self::Error> {
        let source_listing_id =
            SourceListingId::try_from(row.source_listing_id.clone()).map_err(|_| ())?;
        let parsed_images = images(row.product_images.clone())?;
        let user_state = user_state(&row, &parsed_images)?;
        let content_policy = content_policy(
            row.content_policy_decision.as_deref(),
            row.content_policy_category.as_deref(),
        )?;
        let auction = auction_from_parts(ProductListingAuctionParts {
            context_product_listing_id: row.auction_context_product_listing_id,
            lot_number: row.auction_lot_number,
            catalogue_position: row.auction_catalogue_position,
            timing_product_listing_id: row.auction_timing_product_listing_id,
            bidding_opens_precision: row.auction_bidding_opens_precision,
            bidding_opens_instant_at: row.auction_bidding_opens_instant_at,
            bidding_opens_date_on: row.auction_bidding_opens_date_on,
            bidding_opens_source_timezone: row.auction_bidding_opens_source_timezone,
            scheduled_closes_precision: row.auction_scheduled_closes_precision,
            scheduled_closes_instant_at: row.auction_scheduled_closes_instant_at,
            scheduled_closes_date_on: row.auction_scheduled_closes_date_on,
            scheduled_closes_source_timezone: row.auction_scheduled_closes_source_timezone,
            reported_closed_at: row.auction_reported_closed_at,
        })
        .map_err(|_| ())?;
        let product_title = localized_title(row.product_title_text, row.product_title_language)?;
        let product_description = localized_description(
            row.product_description_text,
            row.product_description_language,
        )?;
        let title = localized_title(row.title_text, row.title_language)?;
        let description = localized_description(row.description_text, row.description_language)?;
        let product_price =
            product_listing_price(row.price_kind, row.price_amount, row.price_currency)?;
        let product_price_estimate_min = price(
            row.price_estimate_min_amount,
            row.price_estimate_min_currency,
        )?;
        let product_price_estimate_max = price(
            row.price_estimate_max_amount,
            row.price_estimate_max_currency,
        )?;
        let sale_observation =
            sale_observation(row.sale_observed_at, row.sale_observation_fx_rate_id)?;
        let url = Url::parse(&row.url).map_err(|_| ())?;
        let view_url = outbound_url(
            referral_configuration(row.listing_source_referral_configuration.as_ref())?.as_ref(),
            &url,
        )
        .map_err(|_| ())?;

        Ok(Personalized {
            item: ProductListingDetailsReadModel {
                product_listing_id: try_from_uuid(row.product_listing_id, "ProductListing ID")
                    .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
                product_listing_title_slug_id: ProductListingSlugId::raw(
                    &row.product_listing_title_slug_id,
                )
                .map_err(|_| ())?,
                event_id: try_from_uuid(row.current_event_id, "current event ID")
                    .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
                source: ListingSourceSummary {
                    listing_source_id: try_from_uuid(row.listing_source_id, "ListingSource ID")
                        .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
                    name: ListingSourceName::try_from(row.listing_source_name).map_err(|_| ())?,
                    slug_id: ListingSourceSlugId::raw(&row.listing_source_slug_id)
                        .map_err(|_| ())?,
                },
                source_listing_id,
                product_title,
                product_description,
                title,
                description,
                pricing: ProductListingPricing {
                    price: product_price,
                    price_estimate_min: product_price_estimate_min,
                    price_estimate_max: product_price_estimate_max,
                },
                sale_observation,
                availability: availability(row.availability.as_deref())?,
                lifecycle: lifecycle(&row.lifecycle)?,
                view_url,
                url,
                images: parsed_images,
                content_policy,
                auction,
                created: row.created,
                updated: row.updated,
            },
            user_state,
        })
    }
}

fn sale_observation(
    observed_at: Option<OffsetDateTime>,
    fx_rate_id: Option<uuid::Uuid>,
) -> Result<Option<ListingSaleObservation>, ProductListingDetailsRowMappingError> {
    match (observed_at, fx_rate_id) {
        (Some(observed_at), Some(fx_rate_id)) => Ok(Some(ListingSaleObservation::new(
            observed_at,
            try_from_uuid(fx_rate_id, "sale observation FX rate ID")
                .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductListingDetailsRowMappingError::InvalidValue),
    }
}

fn user_state(
    row: &ProductListingDetailsRow,
    _images: &IndexSet<ProductListingImage>,
) -> Result<Option<ProductListingUserState>, ProductListingDetailsRowMappingError> {
    let Some(personalization_user_id) = row.personalization_user_id else {
        return Ok(None);
    };
    try_from_uuid::<user_core::user_id::UserId>(personalization_user_id, "personalization user ID")
        .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?;

    let (stored_consent, tier) = match (
        row.user_show_unassessed_or_sensitive_content,
        row.user_tier.as_deref(),
    ) {
        (Some(consent), Some("FREE")) => (consent, "FREE"),
        (Some(consent), Some("PRO")) => (consent, "PRO"),
        (Some(consent), Some("ULTIMATE")) => (consent, "ULTIMATE"),
        _ => return Err(ProductListingDetailsRowMappingError::InvalidValue),
    };
    let search_filter = search_filter_user_state(row, Some(tier))?;

    Ok(Some(ProductListingUserState {
        watchlist: WatchlistUserState {
            watching: row.watchlist_notifications.is_some(),
            notifications: row.watchlist_notifications.unwrap_or(false),
        },
        content_visibility: ContentVisibilityUserState {
            show_unassessed_or_sensitive_content: stored_consent,
        },
        notification: NotificationUserState {
            unseen_notification_ids: row
                .unseen_notification_ids
                .as_deref()
                .unwrap_or_default()
                .iter()
                .copied()
                .map(|id| {
                    try_from_uuid(id, "notification ID")
                        .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)
                })
                .collect::<Result<_, _>>()?,
        },
        search_filter,
    }))
}

fn search_filter_user_state(
    row: &ProductListingDetailsRow,
    tier: Option<&str>,
) -> Result<SearchFilterUserState, ProductListingDetailsRowMappingError> {
    let Some(user_search_filter_id) = row.selected_match_user_search_filter_id else {
        if row.selected_match_user_search_filter_name.is_some()
            || row.selected_match_reason.is_some()
            || row.selected_match_feedback.is_some()
            || row.selected_match_month_position.is_some()
        {
            return Err(ProductListingDetailsRowMappingError::InvalidValue);
        }
        return Ok(SearchFilterUserState::default());
    };

    let hidden = match tier.ok_or(())? {
        "FREE" => row.selected_match_month_position.ok_or(())? > 10,
        "PRO" | "ULTIMATE" => {
            if row.selected_match_month_position.is_some() {
                return Err(ProductListingDetailsRowMappingError::InvalidValue);
            }
            false
        }
        _ => return Err(ProductListingDetailsRowMappingError::InvalidValue),
    };

    Ok(SearchFilterUserState {
        matched: true,
        hidden,
        user_search_filter_id: Some(
            try_from_uuid(user_search_filter_id, "user search filter ID")
                .map_err(ProductListingDetailsRowMappingError::InvalidObjectId)?,
        ),
        user_search_filter_name: row
            .selected_match_user_search_filter_name
            .clone()
            .map(UserSearchFilterName::from),
        match_reason: row
            .selected_match_reason
            .clone()
            .map(EnhancedMatchReason::from),
        match_feedback: row.selected_match_feedback,
    })
}

fn localized_title(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Title>>, ()> {
    match (text, language) {
        (Some(text), Some(language)) => {
            let title = Title::from(text.as_str());
            if title.as_ref().is_empty() || title.as_ref() != text.as_str() {
                return Err(());
            }
            Ok(Some(Localized::new(parse_language(&language)?, title)))
        }
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn localized_description(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Description>>, ()> {
    match (text, language) {
        (Some(text), Some(language)) => {
            let description = Description::from(text.as_str());
            if description.as_ref().is_empty() || description.as_ref() != text.as_str() {
                return Err(());
            }
            Ok(Some(Localized::new(
                parse_language(&language)?,
                description,
            )))
        }
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

fn product_listing_price(
    kind: Option<String>,
    amount: Option<i64>,
    currency: Option<String>,
) -> Result<Option<product_listing_core::product_listing_price::ProductListingPrice>, ()> {
    match (kind.as_deref(), amount, currency) {
        (None, None, None) => Ok(None),
        (Some("MONETARY"), Some(amount), Some(currency)) => Ok(Some(
            product_listing_core::product_listing_price::ProductListingPrice::Monetary(Price::new(
                MonetaryAmount::from(u64::try_from(amount).map_err(|_| ())?),
                parse_currency(&currency)?,
            )),
        )),
        (Some("ON_REQUEST"), None, None) => Ok(Some(
            product_listing_core::product_listing_price::ProductListingPrice::OnRequest,
        )),
        _ => Err(()),
    }
}

fn price(amount: Option<i64>, currency: Option<String>) -> Result<Option<Price>, ()> {
    match (amount, currency) {
        (Some(amount), Some(currency)) => Ok(Some(Price::new(
            MonetaryAmount::from(u64::try_from(amount).map_err(|_| ())?),
            parse_currency(&currency)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

pub(crate) fn images(value: serde_json::Value) -> Result<IndexSet<ProductListingImage>, ()> {
    serde_json::from_value::<Vec<ProductListingImageJson>>(value)
        .map_err(|_| ())?
        .into_iter()
        .map(|image| {
            Ok(ProductListingImage::new(
                Url::parse(&image.url).map_err(|_| ())?,
            ))
        })
        .collect()
}

fn parse_language(value: &str) -> Result<Language, ()> {
    Language::from_code(value).ok_or(())
}

fn parse_currency(value: &str) -> Result<Currency, ()> {
    Currency::from_code(value).ok_or(())
}

fn content_policy(
    decision: Option<&str>,
    category: Option<&str>,
) -> Result<Option<ContentPolicyDecision>, ()> {
    match (decision, category) {
        (None, None) => Ok(None),
        (Some("ALLOWED"), None) => Ok(Some(ContentPolicyDecision::Allowed)),
        (Some("REQUIRES_CONSENT"), Some("NAZI_GERMANY")) => Ok(Some(
            ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany),
        )),
        _ => Err(()),
    }
}

fn availability(value: Option<&str>) -> Result<Option<ListingAvailability>, ()> {
    value
        .map(|value| ListingAvailability::from_code(value).ok_or(()))
        .transpose()
}

fn lifecycle(value: &str) -> Result<ListingLifecycle, ()> {
    ListingLifecycle::from_code(value).ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxrate_core::FxRateId;

    #[test]
    fn should_map_complete_sale_observation() {
        let fx_rate_id = FxRateId::new();
        let observed_at = OffsetDateTime::UNIX_EPOCH;

        let result = sale_observation(Some(observed_at), Some(fx_rate_id.into_uuid()));

        assert!(matches!(
            result,
            Ok(Some(observation))
                if observation
                    == ListingSaleObservation::new(
                        observed_at,
                        fx_rate_id,
                    )
        ));
    }

    #[test]
    fn should_reject_incomplete_sale_observation() {
        let result = sale_observation(Some(OffsetDateTime::UNIX_EPOCH), None);

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_selected_title_when_text_is_not_canonical() {
        let result = localized_title(Some("title".to_owned()), Some("en".to_owned()));

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_selected_description_when_text_is_empty() {
        let result = localized_description(Some(" ".to_owned()), Some("en".to_owned()));

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_selected_text_when_language_is_unrecognized() {
        let result = localized_title(Some("Title".to_owned()), Some("xx".to_owned()));

        assert!(result.is_err());
    }
}

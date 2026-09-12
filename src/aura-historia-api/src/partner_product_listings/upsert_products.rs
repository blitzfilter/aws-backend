use super::types::{
    PartnerProductFailureData, UpsertProductListingData, parse_listing_source_id,
    parse_partner_product_batch,
};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::PartnerProductListingsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

pub async fn upsert_products(
    State(state): State<PartnerProductListingsState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: String,
) -> Response {
    let listing_source_id = match parse_listing_source_id(&raw_listing_source_id) {
        Ok(listing_source_id) => listing_source_id,
        Err(error) => return error.into_response(),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let products: Vec<UpsertProductListingData> = match parse_partner_product_batch(&body) {
        Ok(products) => products,
        Err(error) => return error.into_response(),
    };

    let mapped = match products
        .into_iter()
        .map(|product| {
            let raw_source_listing_id = product.source_listing_id.clone();
            product
                .into_command(listing_source_id)
                .map(|command| (raw_source_listing_id, command))
        })
        .collect::<Result<Vec<_>, ApiError>>()
    {
        Ok(mapped) => mapped,
        Err(error) => return error.into_response(),
    };

    let mut failures = Vec::new();
    let mut first_error = None;
    let mut successes = 0;
    for (source_listing_id, command) in mapped {
        match state.upsert.execute(&context, command).await {
            Ok(_) => successes += 1,
            Err(error) => {
                let error = ApiError::from(error);
                let error_code = error.code();
                first_error.get_or_insert(error);
                failures.push(PartnerProductFailureData::new(
                    listing_source_id,
                    source_listing_id,
                    error_code,
                ));
            }
        }
    }

    if successes == 0
        && let Some(error) = first_error
    {
        return error.into_response();
    }

    (StatusCode::OK, Json(failures)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::{CredentialCapability, OperationContext};
    use application::patch_field::PatchField;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, header};
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use product_listing_service::use_cases::{
        CreateProductListingCommand, CreateProductListingError, CreateProductListingResult,
        CreateProductListingUseCase, UpdateProductListingCommand, UpdateProductListingError,
        UpdateProductListingResult, UpdateProductListingUseCase, UpsertProductListingCommand,
        UpsertProductListingError, UpsertProductListingResult, UpsertProductListingUseCase,
        WithdrawProductListingError, WithdrawProductListingResult, WithdrawProductListingUseCase,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    mockall::mock! { CreateUseCase {} #[async_trait::async_trait] impl CreateProductListingUseCase for CreateUseCase { async fn execute(&self, context: &OperationContext, command: CreateProductListingCommand) -> Result<CreateProductListingResult, CreateProductListingError>; } }
    mockall::mock! { UpdateUseCase {} #[async_trait::async_trait] impl UpdateProductListingUseCase for UpdateUseCase { async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>; } }
    mockall::mock! { UpsertUseCase {} #[async_trait::async_trait] impl UpsertProductListingUseCase for UpsertUseCase { async fn execute(&self, context: &OperationContext, command: UpsertProductListingCommand) -> Result<UpsertProductListingResult, UpsertProductListingError>; } }
    mockall::mock! { WithdrawUseCase {} #[async_trait::async_trait] impl WithdrawProductListingUseCase for WithdrawUseCase { async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId) -> Result<WithdrawProductListingResult, WithdrawProductListingError>; async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey) -> Result<WithdrawProductListingResult, WithdrawProductListingError>; } }
    mockall::mock! { Authenticator {} #[async_trait::async_trait] impl TokenAuthenticator for Authenticator { async fn authenticate(&self, bearer_token: &str, metadata: &RequestMetadata) -> Result<TransportPrincipal, AuthError>; } }

    #[tokio::test]
    async fn should_upsert_each_batch_item() -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert
            .expect_execute()
            .times(2)
            .withf(|_, command| {
                matches!(
                    (command.source_listing_id.as_ref(), &command.availability),
                    ("first", application::patch_field::PatchField::Unchanged)
                        | ("second", application::patch_field::PatchField::Clear)
                )
            })
            .returning(|_, _| Ok(created()));
        let app = app(upsert);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[{"sourceListingId":"first"},{"sourceListingId":"second","availability":null}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_omitted_null_and_value_price_to_explicit_upsert_intent()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert
            .expect_execute()
            .times(3)
            .withf(|_, command| {
                matches!(
                    (command.source_listing_id.as_ref(), &command.price),
                    ("omitted", application::patch_field::PatchField::Unchanged)
                        | ("clear", application::patch_field::PatchField::Clear)
                        | ("set", application::patch_field::PatchField::Set(_))
                )
            })
            .returning(|_, _| Ok(created()));
        let app = app(upsert);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[
                {"sourceListingId":"omitted"},
                {"sourceListingId":"clear","price":null},
                {"sourceListingId":"set","price":{"type":"MONETARY","amount":12000,"currency":"EUR"}}
            ]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_upsert_patch_values() -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert
            .expect_execute()
            .times(1)
            .withf(|_, command| {
                matches!(&command.price_estimate_min, PatchField::Set(_))
                    && matches!(&command.price_estimate_max, PatchField::Set(_))
                    && matches!(&command.images, PatchField::Set(images) if images.len() == 1)
                    && matches!(
                        &command.auction,
                        PatchField::Set(auction)
                            if auction.lot_number().is_some_and(|number| number.as_str() == "42")
                                && auction.catalogue_position().is_some_and(|position| position.value() == 7)
                                && auction.timing().is_some_and(|timing| {
                                    timing.bidding_opens().is_some()
                                        && timing.scheduled_closes().is_some()
                                        && timing.reported_closed_at().is_some()
                                })
                    )
            })
            .returning(|_, _| Ok(created()));
        let app = app(upsert);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[{
                "sourceListingId":"patched",
                "priceEstimateMin":{"amount":10000,"currency":"EUR"},
                "priceEstimateMax":{"amount":20000,"currency":"EUR"},
                "images":["https://example.com/image.jpg"],
                "auction":{
                    "lotNumber":"42",
                    "cataloguePosition":7,
                    "timing":{
                        "biddingOpens":{"precision":"DATE","on":"2026-08-23","sourceTimezone":"Europe/Berlin"},
                        "scheduledCloses":{"precision":"INSTANT","at":"2026-08-24T12:00:00Z"},
                        "reportedClosedAt":"2026-08-24T12:30:00Z"
                    }
                }
            }]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_empty_images_to_explicit_upsert_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert
            .expect_execute()
            .times(1)
            .withf(|_, command| {
                matches!(&command.images, PatchField::Set(images) if images.is_empty())
            })
            .returning(|_, _| Ok(created()));
        let app = app(upsert);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[{"sourceListingId":"empty-images","images":[]}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_omitted_and_null_upsert_patch_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert
            .expect_execute()
            .times(2)
            .withf(|_, command| {
                matches!(
                    (
                        command.source_listing_id.as_ref(),
                        &command.price_estimate_min,
                        &command.price_estimate_max,
                        &command.images,
                        &command.auction,
                    ),
                    (
                        "omitted",
                        PatchField::Unchanged,
                        PatchField::Unchanged,
                        PatchField::Unchanged,
                        PatchField::Unchanged,
                    ) | (
                        "clear",
                        PatchField::Clear,
                        PatchField::Clear,
                        PatchField::Unchanged,
                        PatchField::Unchanged,
                    )
                )
            })
            .returning(|_, _| Ok(created()));
        let app = app(upsert);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[
                {"sourceListingId":"omitted"},
                {
                    "sourceListingId":"clear",
                    "priceEstimateMin":null,
                    "priceEstimateMax":null
                }
            ]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_later_upsert_member_before_any_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert.expect_execute().never();
        let app = app(upsert);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[
                {"sourceListingId":"valid"},
                {"sourceListingId":"invalid","images":null}
            ]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!("BAD_BODY_VALUE", body_json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_failed_key_when_upsert_batch_partially_succeeds()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert.expect_execute().times(2).returning(|_, command| {
            if command.source_listing_id.as_ref() == "failed" {
                Err(UpsertProductListingError::InvalidProductListing {
                    source: application::error::static_error("invalid test product listing"),
                })
            } else {
                Ok(created())
            }
        });
        let app = app(upsert);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[{"sourceListingId":"created"},{"sourceListingId":"failed"}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            json!([{
                "listingSourceId": listing_source_id.to_string(),
                "sourceListingId": "failed",
                "error": "BAD_BODY_VALUE"
            }]),
            body_json(response).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_all_invalid_upserts_to_bad_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut upsert = MockUpsertUseCase::new();
        upsert.expect_execute().times(1).returning(|_, _| {
            Err(UpsertProductListingError::InvalidProductListing {
                source: application::error::static_error("invalid test product listing"),
            })
        });
        let app = app(upsert);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            r#"[{"sourceListingId":"invalid"}]"#,
            true,
        )
        .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        Ok(())
    }

    fn app(upsert: MockUpsertUseCase) -> Router {
        let state = PartnerProductListingsState::new(
            Arc::new(MockCreateUseCase::new()),
            Arc::new(MockUpdateUseCase::new()),
            Arc::new(upsert),
            Arc::new(MockWithdrawUseCase::new()),
            Arc::new(authenticator()),
        );
        Router::new()
            .route(
                "/api/v1/listing-sources/{listing_source_id}/product-listings",
                axum::routing::put(upsert_products),
            )
            .with_state(state)
    }

    fn authenticator() -> MockAuthenticator {
        let mut authenticator = MockAuthenticator::new();
        authenticator.expect_authenticate().returning(|_, _| {
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
            })
        });
        authenticator
    }

    fn created() -> UpsertProductListingResult {
        UpsertProductListingResult::Created(CreateProductListingResult {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("upserted-product-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
        })
    }

    async fn request(
        app: &Router,
        uri: &str,
        body: &str,
        authenticated: bool,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut builder = Request::builder().method("PUT").uri(uri);
        if authenticated {
            builder = builder.header(header::AUTHORIZATION, "Bearer valid");
        }
        Ok(app
            .clone()
            .oneshot(builder.body(Body::from(body.to_owned()))?)
            .await?)
    }

    async fn body_json(response: Response) -> Result<Value, Box<dyn std::error::Error>> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

use crate::{
    product_listing_document::{
        ProductListingDocument, SalePricesDocument, SourcePriceDocument, TextDocument,
        listing_availability,
    },
    product_listing_image_document::ProductListingImageDocument,
};
use domain_primitives::event_id::EventId;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::Language;
use money::Currency;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_id::ProductListingId,
    product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
};
use product_listing_service::ports::{
    ProductListingPercolationInput, ProductListingPricesByCurrency,
    ProductListingSearchFilterMatchSource,
};
use serde::Serialize;
use serde_json::Value;

use time::OffsetDateTime;
use url::Url;

/// Closed-world prices for one temporary ProductListing percolation document.
///
/// Iteration 6B fills these values from one event-time persisted FX snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProductListingPercolationPricesDocument {
    eur: u64,
    gbp: u64,
    usd: u64,
    aud: u64,
    cad: u64,
    nzd: u64,
    cny: u64,
    brl: u64,
    pln: u64,
    r#try: u64,
    jpy: u64,
    czk: u64,
    rub: u64,
    aed: u64,
    sar: u64,
    hkd: u64,
    sgd: u64,
    chf: u64,
    zar: u64,
}

/// Private temporary ProductListing representation used only as a percolator input.
///
/// It deliberately does not reuse the persistent ProductListing OpenSearch document.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingPercolationDocument {
    product_listing_id: ProductListingId,
    product_listing_title_slug_id: ProductListingSlugId,
    #[serde(with = "crate::product_listing_document::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(with = "crate::product_listing_document::source_listing_id")]
    source_listing_id: SourceListingId,
    event_id: EventId,
    title: TextDocument,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_it: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_by_currency: Option<ProductListingPercolationPricesDocument>,
    #[serde(
        with = "listing_availability::option",
        skip_serializing_if = "Option::is_none"
    )]
    availability: Option<ListingAvailability>,
    url: Url,
    #[serde(skip_serializing_if = "IndexSet::is_empty")]
    images: IndexSet<ProductListingImageDocument>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    auction_start: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    auction_end: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingPercolationDocumentError {
    #[error("sale valuation is missing its immutable FX snapshot")]
    MissingSaleSnapshot,
    #[error("active product source must not receive a sale FX snapshot")]
    UnexpectedSaleSnapshot,
    #[error("sale valuation FX snapshot ID does not match the supplied snapshot")]
    SaleSnapshotMismatch {
        valuation_fx_rate_id: FxRateId,
        snapshot_fx_rate_id: FxRateId,
    },
    #[error("sale FX snapshot cannot convert the native source price")]
    InvalidSaleSnapshot {
        #[source]
        source: FxRateSnapshotError,
    },
    #[error("product percolation document serialization failed")]
    Serialize {
        #[source]
        source: serde_json::Error,
    },
}

/// Builds the private temporary ProductListing JSON consumed by saved-filter percolation.
///
/// The application owns valuation selection and checked conversion before this
/// adapter maps the closed-world prices to OpenSearch JSON.
pub fn product_listing_percolation_document(
    input: &ProductListingPercolationInput,
) -> Result<Value, ProductListingPercolationDocumentError> {
    serde_json::to_value(build_product_listing_percolation_document(input))
        .map_err(|source| ProductListingPercolationDocumentError::Serialize { source })
}

fn build_product_listing_percolation_document(
    input: &ProductListingPercolationInput,
) -> ProductListingPercolationDocument {
    let product = &input.source;
    let (title, language) = selected_title(product);

    ProductListingPercolationDocument {
        product_listing_id: product.product_listing_id,
        product_listing_title_slug_id: product.product_listing_title_slug_id.clone(),
        listing_source_id: product.source.listing_source_id,
        source_listing_id: product.source_listing_id.clone(),
        event_id: product.current_event_id,
        title: TextDocument::new(title, language),
        title_de: translated_title(product, Language::De),
        title_en: translated_title(product, Language::En),
        title_fr: translated_title(product, Language::Fr),
        title_es: translated_title(product, Language::Es),
        title_it: translated_title(product, Language::It),
        price_by_currency: input
            .valuation
            .as_ref()
            .map(|valuation| percolation_prices(valuation.prices)),
        availability: product.availability,
        url: product.url.clone(),
        images: product
            .images
            .iter()
            .cloned()
            .map(ProductListingImageDocument::from)
            .collect(),
        auction_start: product.auction.start,
        auction_end: product.auction.end,
        created: product.created,
        updated: product.updated,
    }
}

fn percolation_prices(
    prices: ProductListingPricesByCurrency,
) -> ProductListingPercolationPricesDocument {
    ProductListingPercolationPricesDocument {
        eur: prices.amount_in(Currency::Eur),
        gbp: prices.amount_in(Currency::Gbp),
        usd: prices.amount_in(Currency::Usd),
        aud: prices.amount_in(Currency::Aud),
        cad: prices.amount_in(Currency::Cad),
        nzd: prices.amount_in(Currency::Nzd),
        cny: prices.amount_in(Currency::Cny),
        brl: prices.amount_in(Currency::Brl),
        pln: prices.amount_in(Currency::Pln),
        r#try: prices.amount_in(Currency::Try),
        jpy: prices.amount_in(Currency::Jpy),
        czk: prices.amount_in(Currency::Czk),
        rub: prices.amount_in(Currency::Rub),
        aed: prices.amount_in(Currency::Aed),
        sar: prices.amount_in(Currency::Sar),
        hkd: prices.amount_in(Currency::Hkd),
        sgd: prices.amount_in(Currency::Sgd),
        chf: prices.amount_in(Currency::Chf),
        zar: prices.amount_in(Currency::Zar),
    }
}

pub(crate) fn product_listing_document(
    product: &ProductListingSearchFilterMatchSource,
    sale_snapshot: Option<&FxRateSnapshot>,
) -> Result<ProductListingDocument, ProductListingPercolationDocumentError> {
    let (sale_prices, sale_observation_fx_rate_id, sale_observed_at) =
        sale_projection(product, sale_snapshot)?;
    let (title, language) = selected_title(product);

    Ok(ProductListingDocument {
        product_listing_id: product.product_listing_id,
        product_listing_title_slug_id: product.product_listing_title_slug_id.clone(),
        listing_source_id: product.source.listing_source_id,
        source_listing_id: product.source_listing_id.clone(),
        event_id: product.current_event_id,
        title: TextDocument::new(title, language),
        title_de: translated_title(product, Language::De),
        title_en: translated_title(product, Language::En),
        title_fr: translated_title(product, Language::Fr),
        title_es: translated_title(product, Language::Es),
        title_it: translated_title(product, Language::It),
        source_price: product.pricing.price.map(|price| SourcePriceDocument {
            amount: price.monetary_amount.into(),
            currency: price.currency,
        }),
        sale_prices,
        sale_observation_fx_rate_id,
        sale_observed_at,
        availability: product.availability,
        url: product.url.clone(),
        images: product
            .images
            .iter()
            .cloned()
            .map(ProductListingImageDocument::from)
            .collect(),
        embedding: product.embedding.clone(),
        auction_start: product.auction.start,
        auction_end: product.auction.end,
        created: product.created,
        updated: product.updated,
    })
}

type SaleProjection = (
    Option<SalePricesDocument>,
    Option<FxRateId>,
    Option<OffsetDateTime>,
);

fn sale_projection(
    product: &ProductListingSearchFilterMatchSource,
    sale_snapshot: Option<&FxRateSnapshot>,
) -> Result<SaleProjection, ProductListingPercolationDocumentError> {
    let observation = if product.availability == Some(ListingAvailability::SoldOut) {
        product.sale_observation
    } else {
        None
    };
    match (observation, sale_snapshot) {
        (None, None) => Ok((None, None, None)),
        (None, Some(_)) => Err(ProductListingPercolationDocumentError::UnexpectedSaleSnapshot),
        (Some(observation), None) if product.pricing.price.is_none() => Ok((
            None,
            Some(observation.fx_rate_id()),
            Some(observation.observed_at()),
        )),
        (Some(_), None) => Err(ProductListingPercolationDocumentError::MissingSaleSnapshot),
        (Some(observation), Some(snapshot)) if observation.fx_rate_id() != snapshot.id() => Err(
            ProductListingPercolationDocumentError::SaleSnapshotMismatch {
                valuation_fx_rate_id: observation.fx_rate_id(),
                snapshot_fx_rate_id: snapshot.id(),
            },
        ),
        (Some(observation), Some(snapshot)) => Ok((
            product
                .pricing
                .price
                .map(|price| sale_prices(snapshot, price))
                .transpose()?,
            Some(observation.fx_rate_id()),
            Some(observation.observed_at()),
        )),
    }
}

fn sale_prices(
    snapshot: &FxRateSnapshot,
    source_price: money::Price,
) -> Result<SalePricesDocument, ProductListingPercolationDocumentError> {
    let amount_in = |currency| {
        snapshot
            .convert(source_price, currency, RoundingMode::HalfUp)
            .map(|price| u64::from(price.monetary_amount))
            .map_err(
                |source| ProductListingPercolationDocumentError::InvalidSaleSnapshot { source },
            )
    };

    Ok(SalePricesDocument {
        eur: amount_in(Currency::Eur)?,
        gbp: amount_in(Currency::Gbp)?,
        usd: amount_in(Currency::Usd)?,
        aud: amount_in(Currency::Aud)?,
        cad: amount_in(Currency::Cad)?,
        nzd: amount_in(Currency::Nzd)?,
        cny: amount_in(Currency::Cny)?,
        brl: amount_in(Currency::Brl)?,
        pln: amount_in(Currency::Pln)?,
        r#try: amount_in(Currency::Try)?,
        jpy: amount_in(Currency::Jpy)?,
        czk: amount_in(Currency::Czk)?,
        rub: amount_in(Currency::Rub)?,
        aed: amount_in(Currency::Aed)?,
        sar: amount_in(Currency::Sar)?,
        hkd: amount_in(Currency::Hkd)?,
        sgd: amount_in(Currency::Sgd)?,
        chf: amount_in(Currency::Chf)?,
        zar: amount_in(Currency::Zar)?,
    })
}

fn selected_title(product: &ProductListingSearchFilterMatchSource) -> (&str, Language) {
    product
        .product_title
        .as_ref()
        .map(|title| (title.payload.as_ref(), title.localization))
        .or_else(|| {
            product
                .titles
                .get(&Language::En)
                .map(|title| (title.as_ref(), Language::En))
        })
        .or_else(|| {
            product
                .titles
                .iter()
                .min_by_key(|(language, _)| language.as_str())
                .map(|(language, title)| (title.as_ref(), *language))
        })
        .unwrap_or(("", Language::En))
}

fn translated_title(
    product: &ProductListingSearchFilterMatchSource,
    language: Language,
) -> Option<String> {
    product
        .titles
        .get(&language)
        .map(|title| title.as_ref().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::event_id::EventId;
    use domain_primitives::query::range_query::RangeQuery;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Localized;
    use product_listing_core::product_listing_search::ProductListingSearch;
    use product_listing_core::{
        listing_availability::ListingAvailability,
        listing_lifecycle::ListingLifecycle,
        product_listing::{
            ListingSaleObservation, ProductListingAuction, ProductListingPriceValuationBasis,
            ProductListingPricing,
        },
        product_listing_image::ProductListingImage,
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
        title::Title,
    };
    use product_listing_service::ports::ListingSourceSummary;
    use product_listing_service::ports::{
        ProductListingPercolationValuation, ProductListingPriceFilterPlan,
        ProductListingPricesByCurrency, ProductListingSearchFilterMatchSourceEventKind,
    };
    use std::collections::{BTreeSet, HashMap};
    use strum::IntoEnumIterator;
    use url::Url;

    fn source() -> Result<ProductListingSearchFilterMatchSource, url::ParseError> {
        let title = Title::from("Blue vase");
        let url = Url::parse("https://shop.example.test/product_listings/blue-vase")?;
        let event_id = EventId::new();
        Ok(ProductListingSearchFilterMatchSource {
            event_id,
            event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 1,
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("blue-vase-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                slug_id: ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            },
            source_listing_id: SourceListingId::try_from("sku-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            product_title: Some(Localized::new(Language::En, title.clone())),
            product_description: None,
            titles: HashMap::from([(Language::En, title)]),
            descriptions: HashMap::new(),
            pricing: ProductListingPricing::default(),
            sale_observation: None,
            availability: Some(ListingAvailability::Available),
            lifecycle: ListingLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: ProductListingAuction::default(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn snapshot() -> Result<FxRateSnapshot, FxRateSnapshotError> {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    match currency {
                        Currency::Eur => FX_RATE_SCALE,
                        Currency::Gbp => 850_000,
                        Currency::Usd => 1_100_000,
                        Currency::Jpy => 160_000_000,
                        _ => 1_250_000,
                    },
                )
            }),
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(FxRateGeneration::try_from(1)?)))
    }

    fn boundary_amounts(lower: u64, upper: Option<u64>) -> Vec<u64> {
        let mut amounts = vec![
            0,
            1,
            lower.saturating_sub(1),
            lower,
            lower.saturating_add(1),
        ];
        if let Some(upper) = upper {
            amounts.extend([upper.saturating_sub(1), upper, upper.saturating_add(1)]);
        }
        amounts.sort_unstable();
        amounts.dedup();
        amounts
    }

    fn matches_inclusive_range(amount: u64, bounds: &Value) -> bool {
        bounds
            .get("gte")
            .and_then(Value::as_u64)
            .is_none_or(|minimum| amount >= minimum)
            && bounds
                .get("lte")
                .and_then(Value::as_u64)
                .is_none_or(|maximum| amount <= maximum)
    }

    fn normal_search_membership(
        price_clause: &Value,
        source_currency: Currency,
        source_amount: u64,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let source_currency = serde_json::json!(source_currency.as_str());
        let active_ranges = price_clause
            .pointer("/bool/should")
            .and_then(Value::as_array)
            .ok_or("normal ProductListing price query has no branches")?;
        let bounds = active_ranges.iter().find_map(|branch| {
            (branch.pointer("/bool/filter/0/bool/should")?.as_array()?)
                .iter()
                .find_map(|range| {
                    (range.pointer("/bool/filter/0/term/sourcePrice.currency")
                        == Some(&source_currency))
                    .then(|| range.pointer("/bool/filter/1/range/sourcePrice.amount"))
                    .flatten()
                })
        });

        Ok(bounds.is_some_and(|bounds| matches_inclusive_range(source_amount, bounds)))
    }

    fn saved_filter_percolation_membership(
        product_listing_percolator_query: &Value,
        product_listing_percolation_document: &Value,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let filters = product_listing_percolator_query
            .pointer("/bool/filter")
            .and_then(Value::as_array)
            .ok_or("saved-filter percolator query has no filters")?;
        let (field, bounds) = filters
            .iter()
            .find_map(|filter| {
                filter
                    .get("range")
                    .and_then(Value::as_object)
                    .and_then(|ranges| {
                        ranges.iter().find_map(|(field, bounds)| {
                            field
                                .strip_prefix("priceByCurrency.")
                                .map(|field| (field, bounds))
                        })
                    })
            })
            .ok_or("saved-filter percolator query has no price range")?;
        let amount = product_listing_percolation_document
            .get("priceByCurrency")
            .and_then(Value::as_object)
            .and_then(|prices| prices.get(field))
            .and_then(Value::as_u64)
            .ok_or("percolation document has no mapped target price")?;

        Ok(matches_inclusive_range(amount, bounds))
    }

    #[test]
    fn should_keep_every_maximal_temporary_percolation_document_path_mapping_compatible()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = snapshot()?;
        let source_price = money::Price::new(12_500_u64.into(), Currency::Gbp);
        let prices = ProductListingPricesByCurrency::convert_all(&snapshot, source_price)?;
        let mut product = source()?;
        product.pricing.price = Some(source_price);
        product.titles = HashMap::from([
            (Language::De, Title::from("Blaue Vase")),
            (Language::En, Title::from("Blue vase")),
            (Language::Fr, Title::from("Vase bleu")),
            (Language::Es, Title::from("Jarrón azul")),
            (Language::It, Title::from("Vaso blu")),
        ]);
        product.images = IndexSet::from([ProductListingImage::new(Url::parse(
            "https://shop.example.test/product_listings/blue-vase/image.jpg",
        )?)]);
        product.auction = ProductListingAuction {
            start: Some(OffsetDateTime::UNIX_EPOCH),
            end: Some(OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1)),
        };
        product.created = OffsetDateTime::UNIX_EPOCH + time::Duration::days(1);
        product.updated = OffsetDateTime::UNIX_EPOCH + time::Duration::days(2);

        let document = product_listing_percolation_document(&ProductListingPercolationInput {
            source: product,
            valuation: Some(ProductListingPercolationValuation {
                basis: ProductListingPriceValuationBasis::Event,
                fx_rate_id: snapshot.id(),
                effective_at: snapshot.captured_at(),
                prices,
            }),
        })?;
        let mapping: Value = serde_json::from_str(include_str!(
            "../../../opensearch/mappings/user_search_filters.json"
        ))?;

        let mut paths = BTreeSet::new();
        collect_mapping_compatible_paths(&document, "", &mapping, &mut paths)?;
        assert!(paths.contains("priceByCurrency.chf"));
        assert!(paths.contains("images.url"));
        assert!(paths.contains("titleIt"));
        Ok(())
    }

    fn collect_mapping_compatible_paths(
        value: &Value,
        path: &str,
        mapping: &Value,
        paths: &mut BTreeSet<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match value {
            Value::Object(values) => {
                for (key, value) in values {
                    let field = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    mapping_field(mapping, &field).ok_or_else(|| {
                        format!("percolation field `{field}` is missing from mapping")
                    })?;
                    collect_mapping_compatible_paths(value, &field, mapping, paths)?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect_mapping_compatible_paths(value, path, mapping, paths)?;
                }
            }
            Value::Null => {}
            _ => {
                let field_mapping = mapping_field(mapping, path)
                    .ok_or_else(|| format!("percolation field `{path}` is missing from mapping"))?;
                if !mapping_accepts_value(field_mapping, value) {
                    return Err(
                        format!("mapping for percolation field `{path}` rejects {value}").into(),
                    );
                }
                paths.insert(path.to_owned());
            }
        }
        Ok(())
    }

    fn mapping_field<'a>(mapping: &'a Value, path: &str) -> Option<&'a Value> {
        let mut properties = mapping.pointer("/mappings/properties")?;
        let mut segments = path.split('.').peekable();
        while let Some(segment) = segments.next() {
            let field = properties.get(segment)?;
            if segments.peek().is_none() {
                return Some(field);
            }
            properties = field.get("properties")?;
        }
        None
    }

    fn mapping_accepts_value(mapping: &Value, value: &Value) -> bool {
        match mapping.get("type").and_then(Value::as_str) {
            Some("keyword" | "text" | "date") => value.is_string(),
            Some("unsigned_long") => value.is_number(),
            _ => false,
        }
    }

    #[test]
    fn should_use_the_private_percolation_shape_without_persistent_price_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = product_listing_percolation_document(&ProductListingPercolationInput {
            source: source()?,
            valuation: None,
        })?;

        assert!(document.get("priceByCurrency").is_none());
        assert!(document.get("sourcePrice").is_none());
        assert!(document.get("salePrices").is_none());
        assert!(document.get("priceEstimateMin").is_none());
        assert!(document.get("priceEstimateMax").is_none());
        assert!(document.get("productListingTitleSlugId").is_some());
        assert!(document.get("productListingSlugId").is_none());
        assert!(document.get("listingSourceId").is_some());
        assert!(document.get("sourceListingId").is_some());
        assert!(document.get("sourceListingSlugId").is_none());
        for field in [
            "listingSourceName",
            "listingSourceSlugId",
            "shopSlugId",
            "sellerSlugId",
            "shopId",
            "sellerId",
            "shopListingId",
            "shopName",
            "sellerName",
            "shopType",
        ] {
            assert!(
                document.get(field).is_none(),
                "retired field {field} is present"
            );
        }
        Ok(())
    }

    #[test]
    fn should_use_identical_sale_snapshot_values_for_persistent_and_temporary_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    match currency {
                        Currency::Eur => FX_RATE_SCALE,
                        Currency::Gbp => 850_000,
                        Currency::Usd => 1_100_000,
                        Currency::Jpy => 160_000_000,
                        _ => 1_250_000,
                    },
                )
            }),
        )?
        .into_persisted(FxRateGeneration::try_from(1)?);
        let mut product = source()?;
        let source_price = money::Price::new(12_500_u64.into(), Currency::Gbp);
        product.pricing.price = Some(source_price);
        product.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            snapshot.id(),
        ));
        product.availability = Some(ListingAvailability::SoldOut);
        let prices = ProductListingPricesByCurrency::convert_all(&snapshot, source_price)?;

        let persistent =
            serde_json::to_value(product_listing_document(&product, Some(&snapshot))?)?;
        let temporary = product_listing_percolation_document(&ProductListingPercolationInput {
            source: product,
            valuation: Some(ProductListingPercolationValuation {
                basis: ProductListingPriceValuationBasis::SaleObservation,
                fx_rate_id: snapshot.id(),
                effective_at: snapshot.captured_at(),
                prices,
            }),
        })?;

        assert_eq!(
            persistent.get("salePrices"),
            temporary.get("priceByCurrency"),
        );
        Ok(())
    }

    #[test]
    fn should_preserve_raw_urls_without_view_urls_in_projection_documents()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = source()?;
        let persistent = serde_json::to_value(product_listing_document(&product, None)?)?;
        let temporary = product_listing_percolation_document(&ProductListingPercolationInput {
            source: product,
            valuation: None,
        })?;

        for document in [&persistent, &temporary] {
            assert_eq!(
                Some(&serde_json::json!(
                    "https://shop.example.test/product_listings/blue-vase"
                )),
                document.get("url"),
            );
            assert!(document.get("viewUrl").is_none());
        }
        Ok(())
    }

    #[test]
    fn should_project_sold_product_without_main_price_or_sale_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = source()?;
        product.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            FxRateId::new(),
        ));
        product.availability = Some(ListingAvailability::SoldOut);

        let document = serde_json::to_value(product_listing_document(&product, None)?)?;

        assert!(document.get("sourcePrice").is_none());
        assert!(document.get("salePrices").is_none());
        assert_eq!(
            product
                .sale_observation
                .map(|observation| observation.fx_rate_id().to_string()),
            document
                .get("saleObservationFxRateId")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        );
        assert!(document.get("saleObservedAt").is_some());
        Ok(())
    }

    #[test]
    fn should_omit_sale_observation_metadata_for_active_relisted_product()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = source()?;
        product.pricing.price = Some(money::Price::new(12_500_u64.into(), Currency::Gbp));
        product.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            FxRateId::new(),
        ));

        let document = serde_json::to_value(product_listing_document(&product, None)?)?;

        assert!(document.get("saleObservationFxRateId").is_none());
        assert!(document.get("saleObservedAt").is_none());
        assert!(document.get("salePrices").is_none());
        assert!(document.get("sourcePrice").is_some());
        Ok(())
    }

    #[test]
    fn should_match_normal_search_and_saved_filter_percolation_for_all_price_pairs_and_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = snapshot()?;
        let ranges = [
            RangeQuery {
                min: None,
                max: Some(5_u64.into()),
            },
            RangeQuery {
                min: Some(3_u64.into()),
                max: None,
            },
            RangeQuery {
                min: Some(3_u64.into()),
                max: Some(12_u64.into()),
            },
            RangeQuery {
                min: Some(0_u64.into()),
                max: Some(1_u64.into()),
            },
        ];
        let mut covered_pairs = 0;
        let mut covered_jpy_pairs = 0;

        for source_currency in Currency::iter() {
            for target_currency in Currency::iter() {
                covered_pairs += 1;
                if source_currency == Currency::Jpy || target_currency == Currency::Jpy {
                    covered_jpy_pairs += 1;
                }
                for range in ranges {
                    let price_filter = ProductListingPriceFilterPlan::compile(
                        snapshot.clone(),
                        target_currency,
                        Some(range),
                    )?;
                    let normal_price_clause =
                        crate::product_listing_search_reader::build_product_index_price_clause(
                            &price_filter,
                        )
                        .ok_or("normal ProductListing price clause missing")?;
                    let saved_filter = ProductListingSearch::new(Language::En, target_currency)
                        .with_price_query(range);
                    let product_listing_percolator_query =
                        crate::build_percolator_query(&saved_filter)?;
                    let native_range = price_filter
                        .active_native_ranges
                        .iter()
                        .find(|native| native.source_currency == source_currency)
                        .ok_or("normal ProductListing price query misses a source currency")?;

                    for source_amount in boundary_amounts(native_range.lower, native_range.upper) {
                        let source_price = money::Price::new(source_amount.into(), source_currency);
                        let prices =
                            ProductListingPricesByCurrency::convert_all(&snapshot, source_price)?;
                        let mut product = source()?;
                        product.pricing.price = Some(source_price);
                        let product_listing_percolation_document =
                            product_listing_percolation_document(
                                &ProductListingPercolationInput {
                                    source: product,
                                    valuation: Some(ProductListingPercolationValuation {
                                        basis: ProductListingPriceValuationBasis::Event,
                                        fx_rate_id: snapshot.id(),
                                        effective_at: snapshot.captured_at(),
                                        prices,
                                    }),
                                },
                            )?;
                        let normal_membership = normal_search_membership(
                            &normal_price_clause,
                            source_currency,
                            source_amount,
                        )?;
                        let saved_filter_membership = saved_filter_percolation_membership(
                            &product_listing_percolator_query,
                            &product_listing_percolation_document,
                        )?;

                        assert_eq!(
                            normal_membership, saved_filter_membership,
                            "{source_currency:?} -> {target_currency:?}, source amount {source_amount}, range {range:?}",
                        );
                    }
                }
            }
        }

        let supported_currency_count = Currency::iter().count();
        assert_eq!(
            supported_currency_count * supported_currency_count,
            covered_pairs
        );
        assert_eq!(
            supported_currency_count * 2 - 1,
            covered_jpy_pairs,
            "all JPY source and target pairs must be covered"
        );
        Ok(())
    }

    #[test]
    fn should_serialize_every_supported_currency_in_closed_world_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(ProductListingPercolationPricesDocument {
            eur: 1,
            gbp: 1,
            usd: 1,
            aud: 1,
            cad: 1,
            nzd: 1,
            cny: 1,
            brl: 1,
            pln: 1,
            r#try: 1,
            jpy: 1,
            czk: 1,
            rub: 1,
            aed: 1,
            sar: 1,
            hkd: 1,
            sgd: 1,
            chf: 1,
            zar: 1,
        })?;

        assert_eq!(19, value.as_object().map_or(0, serde_json::Map::len));
        assert!(value.get("jpy").is_some());
        assert!(value.get("zar").is_some());
        assert!(value.get("priceEstimateMin").is_none());
        Ok(())
    }
}

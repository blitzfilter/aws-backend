use crate::product_listing_image_document::ProductListingImageDocument;
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::Language;
use money::Currency;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_price::ProductListingPrice;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::source_listing_id::SourceListingId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;
use url::Url;

fn serialize_code<T, S>(
    value: &T,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: serde::Serializer,
{
    serializer.serialize_str(code(*value))
}

fn deserialize_code<'de, T, D>(deserializer: D, parse: fn(&str) -> Option<T>) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse(&value).ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
}

pub(crate) mod currency {
    use super::*;

    pub(crate) fn serialize<S>(value: &Currency, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, Currency::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Currency, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, Currency::from_code)
    }
}

pub(crate) mod language {
    use super::*;
    use strum::IntoEnumIterator;

    fn code(value: Language) -> &'static str {
        match value {
            Language::De => "DE",
            Language::En => "EN",
            Language::Fr => "FR",
            Language::Es => "ES",
            Language::It => "IT",
            Language::Zh => "ZH",
            Language::Pt => "PT",
            Language::Pl => "PL",
            Language::Tr => "TR",
            Language::Nl => "NL",
            Language::Cs => "CS",
            Language::Ja => "JA",
            Language::Ru => "RU",
            Language::Ar => "AR",
        }
    }

    fn parse(value: &str) -> Option<Language> {
        Language::iter().find(|language| code(*language) == value)
    }

    pub(crate) fn serialize<S>(value: &Language, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, code)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Language, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, parse)
    }
}

pub(crate) mod source_listing_id {
    use super::*;

    pub(crate) fn serialize<S>(value: &SourceListingId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(value.as_ref())
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<SourceListingId, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SourceListingId::try_from(String::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

pub(crate) mod listing_source_id {
    use super::*;

    pub(crate) fn serialize<S>(value: &ListingSourceId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<ListingSourceId, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value
            .parse::<uuid::Uuid>()
            .map(ListingSourceId::from)
            .map_err(serde::de::Error::custom)
    }
}

pub(crate) mod listing_availability {
    use super::*;

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<ListingAvailability>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            match value {
                Some(value) => serializer.serialize_some(value.as_str()),
                None => serializer.serialize_none(),
            }
        }

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<Option<ListingAvailability>, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            Option::<String>::deserialize(deserializer)?
                .map(|value| {
                    ListingAvailability::from_code(&value).ok_or_else(|| {
                        serde::de::Error::custom(format!("unsupported code `{value}`"))
                    })
                })
                .transpose()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TextDocument {
    pub(crate) text: String,
    #[serde(with = "language")]
    pub(crate) language: Language,
}

impl TextDocument {
    pub(crate) fn new(text: impl Into<String>, language: Language) -> Self {
        Self {
            text: text.into(),
            language,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub(crate) enum SourcePriceDocument {
    Monetary {
        amount: u64,
        #[serde(with = "currency")]
        currency: Currency,
    },
    OnRequest,
}

impl From<ProductListingPrice> for SourcePriceDocument {
    fn from(value: ProductListingPrice) -> Self {
        match value {
            ProductListingPrice::Monetary(price) => Self::Monetary {
                amount: u64::from(price.monetary_amount),
                currency: price.currency,
            },
            ProductListingPrice::OnRequest => Self::OnRequest,
        }
    }
}

impl From<SourcePriceDocument> for ProductListingPrice {
    fn from(value: SourcePriceDocument) -> Self {
        match value {
            SourcePriceDocument::Monetary { amount, currency } => {
                Self::Monetary(money::Price::new(amount.into(), currency))
            }
            SourcePriceDocument::OnRequest => Self::OnRequest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SalePricesDocument {
    pub(crate) eur: u64,
    pub(crate) gbp: u64,
    pub(crate) usd: u64,
    pub(crate) aud: u64,
    pub(crate) cad: u64,
    pub(crate) nzd: u64,
    pub(crate) cny: u64,
    pub(crate) brl: u64,
    pub(crate) pln: u64,
    pub(crate) r#try: u64,
    pub(crate) jpy: u64,
    pub(crate) czk: u64,
    pub(crate) rub: u64,
    pub(crate) aed: u64,
    pub(crate) sar: u64,
    pub(crate) hkd: u64,
    pub(crate) sgd: u64,
    pub(crate) chf: u64,
    pub(crate) zar: u64,
}

impl SalePricesDocument {
    fn amount_in(&self, currency: Currency) -> u64 {
        match currency {
            Currency::Eur => self.eur,
            Currency::Gbp => self.gbp,
            Currency::Usd => self.usd,
            Currency::Aud => self.aud,
            Currency::Cad => self.cad,
            Currency::Nzd => self.nzd,
            Currency::Cny => self.cny,
            Currency::Brl => self.brl,
            Currency::Pln => self.pln,
            Currency::Try => self.r#try,
            Currency::Jpy => self.jpy,
            Currency::Czk => self.czk,
            Currency::Rub => self.rub,
            Currency::Aed => self.aed,
            Currency::Sar => self.sar,
            Currency::Hkd => self.hkd,
            Currency::Sgd => self.sgd,
            Currency::Chf => self.chf,
            Currency::Zar => self.zar,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ProductListingDocumentValidationError {
    #[error("product sale projection metadata must be complete when present")]
    PartialSaleProjection,
    #[error("product sale prices require sale projection metadata")]
    SalePricesWithoutSaleObservation,
    #[error("product sale prices require a monetary source price")]
    SalePricesRequireMonetarySourcePrice,
    #[error("a monetary source price with sale metadata requires sale prices")]
    MissingSalePricesForMonetarySaleObservation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingDocument {
    pub product_listing_id: ProductListingId,
    pub product_listing_title_slug_id: ProductListingSlugId,
    #[serde(with = "listing_source_id")]
    pub listing_source_id: ListingSourceId,
    #[serde(with = "source_listing_id")]
    pub source_listing_id: SourceListingId,
    pub event_id: EventId,
    pub title: TextDocument,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_it: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) source_price: Option<SourcePriceDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) sale_prices: Option<SalePricesDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(rename = "saleObservationFxRateId")]
    pub(crate) sale_observation_fx_rate_id: Option<FxRateId>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    #[serde(rename = "saleObservedAt")]
    pub(crate) sale_observed_at: Option<OffsetDateTime>,
    #[serde(
        with = "listing_availability::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub availability: Option<ListingAvailability>,
    pub url: Url,
    #[serde(skip_serializing_if = "IndexSet::is_empty", default)]
    pub images: IndexSet<ProductListingImageDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_start: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_end: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl ProductListingDocument {
    pub(crate) fn _id(&self) -> ProductListingId {
        self.product_listing_id
    }

    pub(crate) fn validate(&self) -> Result<(), ProductListingDocumentValidationError> {
        let has_sale_metadata = match (self.sale_observation_fx_rate_id, self.sale_observed_at) {
            (None, None) => false,
            (Some(_), Some(_)) => true,
            _ => return Err(ProductListingDocumentValidationError::PartialSaleProjection),
        };
        let has_monetary_source_price = matches!(
            self.source_price,
            Some(SourcePriceDocument::Monetary { .. })
        );

        match (
            has_sale_metadata,
            self.sale_prices.is_some(),
            has_monetary_source_price,
        ) {
            (false, true, _) => {
                Err(ProductListingDocumentValidationError::SalePricesWithoutSaleObservation)
            }
            (true, true, false) => {
                Err(ProductListingDocumentValidationError::SalePricesRequireMonetarySourcePrice)
            }
            (true, false, true) => Err(
                ProductListingDocumentValidationError::MissingSalePricesForMonetarySaleObservation,
            ),
            _ => Ok(()),
        }
    }

    pub(crate) fn source_price(&self) -> Option<ProductListingPrice> {
        self.source_price.map(Into::into)
    }

    pub(crate) fn sale_price(&self, currency: Currency) -> Option<u64> {
        self.sale_prices
            .as_ref()
            .map(|prices| prices.amount_in(currency))
    }

    pub(crate) fn has_sale_observation(&self) -> bool {
        self.sale_observation_fx_rate_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rstest::rstest;
    use time::macros::datetime;

    fn document() -> Result<ProductListingDocument, url::ParseError> {
        Ok(ProductListingDocument {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("vase-abcdef")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("sku-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            event_id: EventId::new(),
            title: TextDocument::new("Vase", Language::En),
            title_de: None,
            title_en: Some("Vase".to_owned()),
            title_fr: None,
            title_es: None,
            title_it: None,
            source_price: Some(SourcePriceDocument::Monetary {
                amount: 100,
                currency: Currency::Eur,
            }),
            sale_prices: None,
            sale_observation_fx_rate_id: None,
            sale_observed_at: None,
            availability: Some(ListingAvailability::Available),
            url: Url::parse("https://shop.example/product_listings/sku-1")?,
            images: IndexSet::new(),
            embedding: None,
            auction_start: None,
            auction_end: None,
            created: datetime!(2025-01-01 0:00 UTC),
            updated: datetime!(2025-01-02 0:00 UTC),
        })
    }

    #[test]
    fn should_preserve_historical_uppercase_language_codec()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(TextDocument::new("Vase", Language::En))?;

        assert_eq!(
            serde_json::json!({
                "text": "Vase",
                "language": "EN",
            }),
            value
        );

        let restored = serde_json::from_value::<TextDocument>(value)?;
        assert_eq!(TextDocument::new("Vase", Language::En), restored);

        Ok(())
    }

    #[test]
    fn should_serialize_source_price_and_availability_for_active_product()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(document()?)?;

        assert_eq!(
            Some(&serde_json::json!("MONETARY")),
            value.pointer("/sourcePrice/type")
        );
        assert_eq!(
            Some(&serde_json::json!(100)),
            value.pointer("/sourcePrice/amount")
        );
        assert_eq!(
            Some(&serde_json::json!("EUR")),
            value.pointer("/sourcePrice/currency")
        );
        assert_eq!(
            Some(&serde_json::json!("AVAILABLE")),
            value.get("availability")
        );
        assert!(value.get("productListingTitleSlugId").is_some());
        assert!(value.get("productListingSlugId").is_none());
        assert!(value.get("sourceListingSlugId").is_none());
        assert!(value.get("salePrices").is_none());
        assert!(value.get("priceEur").is_none());
        assert!(value.get("priceUsd").is_none());
        assert!(value.get("priceEstimateMinEur").is_none());
        assert!(value.get("priceEstimateMaxUsd").is_none());
        assert!(value.as_object().is_some_and(|fields| {
            fields.keys().all(|field| {
                !field.starts_with("priceEstimate") && field != "priceEur" && field != "priceUsd"
            })
        }));
        Ok(())
    }

    #[test]
    fn should_omit_absent_availability() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.availability = None;

        assert!(
            serde_json::to_value(document)?
                .get("availability")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn should_roundtrip_on_request_source_price() -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(document()?)?;
        value["sourcePrice"] = serde_json::json!({ "type": "ON_REQUEST" });

        let restored = serde_json::from_value::<ProductListingDocument>(value)?;

        assert_eq!(
            Some(ProductListingPrice::OnRequest),
            restored.source_price()
        );
        Ok(())
    }

    #[test]
    fn should_serialize_listing_source_identity_without_retired_source_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = document()?;
        let value = serde_json::to_value(&document)?;

        assert_eq!(
            Some(&serde_json::json!(document.listing_source_id.to_string())),
            value.get("listingSourceId")
        );
        assert_eq!(
            Some(&serde_json::json!(document.source_listing_id.to_string())),
            value.get("sourceListingId")
        );
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
            "viewUrl",
        ] {
            assert!(
                value.get(field).is_none(),
                "retired field {field} is present"
            );
        }

        let round_tripped = serde_json::from_value::<ProductListingDocument>(value)?;
        assert_eq!(document, round_tripped);
        Ok(())
    }

    #[rstest]
    #[case::active_without_source_price(None, false, false, true)]
    #[case::active_on_request(Some(SourcePriceDocument::OnRequest), false, false, true)]
    #[case::active_monetary(
        Some(SourcePriceDocument::Monetary { amount: 100, currency: Currency::Eur }),
        false,
        false,
        true
    )]
    #[case::sold_without_source_price(None, true, false, true)]
    #[case::sold_on_request(Some(SourcePriceDocument::OnRequest), true, false, true)]
    #[case::sold_monetary(
        Some(SourcePriceDocument::Monetary { amount: 100, currency: Currency::Eur }),
        true,
        true,
        true
    )]
    #[case::sale_prices_without_source_price(None, true, true, false)]
    #[case::sale_prices_with_on_request(Some(SourcePriceDocument::OnRequest), true, true, false)]
    #[case::sold_monetary_without_sale_prices(
        Some(SourcePriceDocument::Monetary { amount: 100, currency: Currency::Eur }),
        true,
        false,
        false
    )]
    #[case::sale_prices_without_sale_metadata(
        Some(SourcePriceDocument::Monetary { amount: 100, currency: Currency::Eur }),
        false,
        true,
        false
    )]
    fn should_validate_sale_prices_against_source_price_and_sale_metadata(
        #[case] source_price: Option<SourcePriceDocument>,
        #[case] has_sale_metadata: bool,
        #[case] has_sale_prices: bool,
        #[case] valid: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.source_price = source_price;
        document.sale_observation_fx_rate_id = has_sale_metadata.then(FxRateId::new);
        document.sale_observed_at = has_sale_metadata.then_some(OffsetDateTime::UNIX_EPOCH);
        document.sale_prices = has_sale_prices.then_some(SalePricesDocument {
            eur: 100,
            gbp: 100,
            usd: 100,
            aud: 100,
            cad: 100,
            nzd: 100,
            cny: 100,
            brl: 100,
            pln: 100,
            r#try: 100,
            jpy: 100,
            czk: 100,
            rub: 100,
            aed: 100,
            sar: 100,
            hkd: 100,
            sgd: 100,
            chf: 100,
            zar: 100,
        });

        assert_eq!(valid, document.validate().is_ok());
        Ok(())
    }

    #[test]
    fn should_reject_partial_sale_projection() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.sale_observation_fx_rate_id = Some(FxRateId::new());

        assert_eq!(
            Err(ProductListingDocumentValidationError::PartialSaleProjection),
            document.validate()
        );
        Ok(())
    }

    #[test]
    fn should_reject_incomplete_sale_prices_during_deserialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(document()?)?;
        value["salePrices"] = serde_json::json!({ "eur": 100 });
        value["saleObservationFxRateId"] = serde_json::json!(FxRateId::new());
        value["saleObservedAt"] = serde_json::json!("1970-01-01T00:00:00Z");

        assert!(serde_json::from_value::<ProductListingDocument>(value).is_err());
        Ok(())
    }
}

use crate::error::{ApiError, INVALID_OBJECT_ID};
use crate::patch_value::PatchValue;
use serde::{Deserialize, Deserializer, Serializer};
use std::collections::HashSet;
use std::hash::Hash;
use std::str::FromStr;

pub(crate) fn parse_path_object_id<T>(
    value: &str,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<T, ApiError>
where
    T: FromStr,
{
    value
        .parse()
        .map_err(|_| invalid_object_id(semantic_type).with_path_field(field))
}

pub(crate) fn parse_query_object_id<T>(
    value: &str,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<T, ApiError>
where
    T: FromStr,
{
    value
        .parse()
        .map_err(|_| invalid_object_id(semantic_type).with_query_field(field))
}

pub(crate) fn parse_body_object_id<T>(
    value: &str,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<T, ApiError>
where
    T: FromStr,
{
    value
        .parse()
        .map_err(|_| invalid_object_id(semantic_type).with_body_field(field))
}

fn invalid_object_id(semantic_type: &'static str) -> ApiError {
    ApiError::bad_request(INVALID_OBJECT_ID)
        .with_detail(format!("must be a valid {semantic_type} ID"))
}

fn serialize_code<T, S>(
    value: &T,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: Serializer,
{
    serializer.serialize_str(code(*value))
}

fn deserialize_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
    expected: Option<&'static [&'static str]>,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse(&value).ok_or_else(|| invalid_code(&value, expected))
}

fn serialize_option_code<T, S>(
    value: &Option<T>,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(code(*value)),
        None => serializer.serialize_none(),
    }
}

fn deserialize_option_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
    expected: Option<&'static [&'static str]>,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(None), |value| {
        parse(&value)
            .map(Some)
            .ok_or_else(|| invalid_code(&value, expected))
    })
}

fn serialize_set_code<T, S>(
    values: &HashSet<T>,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy + Eq + Hash,
    S: Serializer,
{
    serializer.collect_seq(values.iter().map(|value| code(*value)))
}

fn deserialize_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
    expected: Option<&'static [&'static str]>,
) -> Result<HashSet<T>, D::Error>
where
    T: Eq + Hash,
    D: Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .into_iter()
        .map(|value| parse(&value).ok_or_else(|| invalid_code(&value, expected)))
        .collect()
}

fn deserialize_patch_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
    expected: Option<&'static [&'static str]>,
) -> Result<PatchValue<T>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |value| {
        parse(&value)
            .map(PatchValue::Value)
            .ok_or_else(|| invalid_code(&value, expected))
    })
}

fn deserialize_patch_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
    expected: Option<&'static [&'static str]>,
) -> Result<PatchValue<HashSet<T>>, D::Error>
where
    T: Eq + Hash,
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |values| {
        values
            .into_iter()
            .map(|value| parse(&value).ok_or_else(|| invalid_code(&value, expected)))
            .collect::<Result<HashSet<_>, D::Error>>()
            .map(PatchValue::Value)
    })
}

fn invalid_code<E>(value: &str, expected: Option<&'static [&'static str]>) -> E
where
    E: serde::de::Error,
{
    match expected {
        Some(expected) => E::unknown_variant(value, expected),
        None => E::custom(format!("unsupported code `{value}`")),
    }
}

pub(crate) mod source_listing_id {
    use product_listing_core::source_listing_id::SourceListingId;

    use super::*;

    pub(crate) fn serialize<S>(value: &SourceListingId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(value.as_ref())
    }
}

pub(crate) mod currency {
    use super::*;
    use money::Currency;

    pub(crate) fn serialize<S>(value: &Currency, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, Currency::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Currency, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, Currency::from_code, None)
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<Currency>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_option_code(value, serializer, Currency::as_str)
        }

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Currency>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_option_code(deserializer, Currency::from_code, None)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<Currency>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, Currency::from_code, None)
        }
    }
}

pub(crate) mod language {
    use super::*;
    use localization::Language;

    fn parse(value: &str) -> Option<Language> {
        match value {
            "de-DE" | "de-AT" | "de-CH" | "de-LU" | "de-LI" => Some(Language::De),
            "en-US" | "en-GB" | "en-AU" | "en-CA" | "en-NZ" | "en_IE" => Some(Language::En),
            "fr-FR" | "fr-CA" | "fr-BE" | "fr-CH" | "fr-LU" => Some(Language::Fr),
            "es-ES" | "es-MX" | "es-AR" | "es-CO" | "es-CL" | "es-PE" | "es-VE" => {
                Some(Language::Es)
            }
            "it-IT" | "it-CH" => Some(Language::It),
            "zh-CN" | "zh-Hans" => Some(Language::Zh),
            "pt-PT" | "pt-BR" => Some(Language::Pt),
            "pl-PL" => Some(Language::Pl),
            "tr-TR" => Some(Language::Tr),
            "nl-NL" | "nl-BE" => Some(Language::Nl),
            "cs-CZ" => Some(Language::Cs),
            "ja-JP" => Some(Language::Ja),
            "ru-RU" => Some(Language::Ru),
            "ar-SA" | "ar-EG" | "ar-AE" => Some(Language::Ar),
            _ => Language::from_code(value),
        }
    }

    pub(crate) fn serialize<S>(value: &Language, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, Language::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Language, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, parse, None)
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<Language>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_option_code(value, serializer, Language::as_str)
        }

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Option<Language>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_option_code(deserializer, parse, None)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<Language>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, parse, None)
        }
    }
}

pub(crate) mod notification_kind {
    use super::*;
    use notification_core::notification_kind::NotificationKind;

    pub(crate) fn serialize<S>(value: &NotificationKind, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, NotificationKind::as_str)
    }
}

pub(crate) mod listing_availability {
    use super::*;
    use product_listing_core::listing_availability::ListingAvailability;

    fn parse(value: &str) -> Option<ListingAvailability> {
        ListingAvailability::from_code(value)
    }

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<ListingAvailability>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_option_code(value, serializer, ListingAvailability::as_str)
        }

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<Option<ListingAvailability>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_option_code(deserializer, parse, None)
        }
    }

    pub(crate) mod patch_set {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<PatchValue<HashSet<ListingAvailability>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_set_code(deserializer, parse, None)
        }
    }

    pub(crate) mod set_option {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<Option<HashSet<ListingAvailability>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(None), |values| {
                values
                    .into_iter()
                    .map(|value| parse(&value).ok_or_else(|| invalid_code(&value, None)))
                    .collect::<Result<HashSet<_>, D::Error>>()
                    .map(Some)
            })
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<PatchValue<ListingAvailability>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, parse, None)
        }
    }
}

pub(crate) mod listing_orderability {
    use super::*;
    use product_listing_core::listing_orderability::ListingOrderability;

    pub(crate) mod set_option {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<Option<HashSet<ListingOrderability>>, D::Error>
        where
            D: Deserializer<'de>,
        {
            Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(None), |values| {
                values
                    .into_iter()
                    .map(|value| {
                        ListingOrderability::from_code(&value)
                            .ok_or_else(|| invalid_code(&value, None))
                    })
                    .collect::<Result<HashSet<_>, D::Error>>()
                    .map(Some)
            })
        }
    }
}

pub(crate) mod listing_lifecycle {
    use super::*;
    use product_listing_core::listing_lifecycle::ListingLifecycle;

    pub(crate) fn serialize<S>(value: &ListingLifecycle, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, ListingLifecycle::as_str)
    }
}

pub(crate) mod measurement_unit {
    use super::*;
    use user_core::measurement_unit::MeasurementUnit;

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<MeasurementUnit>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_option_code(value, serializer, MeasurementUnit::as_str)
        }
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<PatchValue<MeasurementUnit>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, MeasurementUnit::from_code, None)
        }
    }
}

pub(crate) mod user_tier {
    use super::*;
    use user_core::tier::UserTier;

    pub(crate) fn serialize<S>(value: &UserTier, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, UserTier::as_str)
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<UserTier>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, UserTier::from_code, None)
        }
    }
}

pub(crate) mod user_role {
    use super::*;
    use user_core::role::UserRole;

    pub(crate) fn serialize<S>(value: &UserRole, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, UserRole::as_str)
    }

    pub(crate) mod patch {
        use super::*;

        pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<UserRole>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_patch_code(deserializer, UserRole::from_code, None)
        }
    }
}

pub(crate) mod search_filter_state {
    use super::*;
    use search_filter_core::search_filter_state::SearchFilterState;

    pub(crate) fn serialize<S>(value: &SearchFilterState, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, SearchFilterState::as_str)
    }
}

pub(crate) mod watchlist_state {
    use super::*;
    use watchlist_core::watchlist_state::WatchlistState;

    pub(crate) fn serialize<S>(value: &WatchlistState, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, WatchlistState::as_str)
    }
}

pub(crate) mod partnership_application_state {
    use super::*;
    use partnership_core::partnership_application_state::PartnershipApplicationState;

    pub(crate) fn serialize<S>(
        value: &PartnershipApplicationState,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, PartnershipApplicationState::as_str)
    }
}

pub(crate) mod ingestion_method {
    use super::*;
    use listing_source_core::ListingIngestionMethod;
    use std::str::FromStr;

    pub(crate) mod set {
        use super::*;

        pub(crate) fn serialize<S>(
            values: &HashSet<ListingIngestionMethod>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serialize_set_code(values, serializer, ListingIngestionMethod::as_str)
        }

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<HashSet<ListingIngestionMethod>, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_set_code(
                deserializer,
                |value| ListingIngestionMethod::from_str(value).ok(),
                Some(&["WEB_CRAWL", "SHOPIFY", "WOOCOMMERCE", "PARTNER_API"]),
            )
        }
    }
}

pub(crate) mod partnership_application_decision {
    use super::*;
    use notification_core::notification::PartnershipApplicationDecision;

    fn code(value: PartnershipApplicationDecision) -> &'static str {
        match value {
            PartnershipApplicationDecision::Approved => "APPROVED",
            PartnershipApplicationDecision::Rejected => "REJECTED",
        }
    }

    pub(crate) fn serialize<S>(
        value: &PartnershipApplicationDecision,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_code(value, serializer, code)
    }
}

pub(crate) mod billing_plan {
    use super::*;
    use billing_service::use_cases::BillingPlan;

    const EXPECTED: &[&str] = &["PRO", "ULTIMATE"];

    fn parse(value: &str) -> Option<BillingPlan> {
        match value {
            "PRO" => Some(BillingPlan::Pro),
            "ULTIMATE" => Some(BillingPlan::Ultimate),
            _ => None,
        }
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<BillingPlan, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, parse, Some(EXPECTED))
    }
}

pub(crate) mod billing_cycle {
    use super::*;
    use billing_service::use_cases::BillingCycle;

    const EXPECTED: &[&str] = &["MONTHLY", "YEARLY"];

    fn parse(value: &str) -> Option<BillingCycle> {
        match value {
            "MONTHLY" => Some(BillingCycle::Monthly),
            "YEARLY" => Some(BillingCycle::Yearly),
            _ => None,
        }
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<BillingCycle, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_code(deserializer, parse, Some(EXPECTED))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use serde_json::{Value, json};
    use user_core::user_id::UserId;

    async fn problem(error: ApiError) -> Result<Value, Box<dyn std::error::Error>> {
        let response = error.into_response();
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    #[test]
    fn should_parse_object_id_with_domain_from_str() {
        let user_id = UserId::new();

        assert!(matches!(
            parse_path_object_id::<UserId>(&user_id.to_string(), "userId", "User"),
            Ok(parsed) if parsed == user_id
        ));
    }

    #[tokio::test]
    async fn should_map_invalid_path_object_id_without_codec_detail()
    -> Result<(), Box<dyn std::error::Error>> {
        let Err(error) = parse_path_object_id::<UserId>("not-an-object-id", "userId", "User")
        else {
            return Err("invalid object ID was accepted".into());
        };

        assert_eq!(
            json!({
                "status": 400,
                "title": "Bad Request",
                "error": "INVALID_OBJECT_ID",
                "source": {"field": "userId", "type": "PATH"},
                "detail": "must be a valid User ID"
            }),
            problem(error).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_invalid_query_object_id_with_query_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let Err(error) = parse_query_object_id::<UserId>("not-an-object-id", "userId", "User")
        else {
            return Err("invalid object ID was accepted".into());
        };

        assert_eq!(
            json!({
                "status": 400,
                "title": "Bad Request",
                "error": "INVALID_OBJECT_ID",
                "source": {"field": "userId", "type": "QUERY"},
                "detail": "must be a valid User ID"
            }),
            problem(error).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_invalid_body_object_id_with_body_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let Err(error) = parse_body_object_id::<UserId>("not-an-object-id", "userId", "User")
        else {
            return Err("invalid object ID was accepted".into());
        };

        assert_eq!(
            json!({
                "status": 400,
                "title": "Bad Request",
                "error": "INVALID_OBJECT_ID",
                "source": {"field": "userId", "type": "BODY"},
                "detail": "must be a valid User ID"
            }),
            problem(error).await?
        );
        Ok(())
    }
}

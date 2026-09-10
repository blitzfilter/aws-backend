use localization::Language;
use money::Currency;
use serde_email::Email;
use sqlx::FromRow;
use time::OffsetDateTime;
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::measurement_unit::MeasurementUnit;
use user_core::role::UserRole;
use user_core::sort_user_field::SortUserField;
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::tier::UserTier;
use user_core::user::{RehydratedUserState, User, UserAccount, UserPreferences, UserProfile};
use user_core::user_id::UserId;
use user_service::ports::{UserDetailsView, UserStorageVersion, VersionedUser};
use user_service::use_cases::queries::find_user_by_stripe_customer_id::UserStripeLookupView;
use user_service::use_cases::queries::search_users::UserSummary;

#[allow(dead_code)]
#[derive(Debug, FromRow)]
pub(crate) struct UserRow {
    pub user_id: uuid::Uuid,
    pub email: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub language: Option<String>,
    pub currency: Option<String>,
    pub measurement_unit: Option<String>,
    pub show_unassessed_or_sensitive_content: bool,
    pub suspended: bool,
    pub tier: String,
    pub role: String,
    pub stripe_customer_id: Option<String>,
    pub version: i64,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub(crate) fn user_columns() -> &'static str {
    "user_id, email, first_name, last_name, language, currency, measurement_unit, show_unassessed_or_sensitive_content, suspended, tier, role, stripe_customer_id, version, created, updated"
}

impl TryFrom<UserRow> for VersionedUser {
    type Error = UserRowMappingError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        let version = UserStorageVersion::try_from(row.version)?;
        let value = User::rehydrate(RehydratedUserState {
            id: UserId::try_from(row.user_id).map_err(UserRowMappingError::InvalidUserId)?,
            email: parse_email(&row.email)?,
            profile: profile_from_row(&row)?,
            preferences: preferences_from_row(&row)?,
            account: account_from_row(&row)?,
            suspended: row.suspended,
        })?;

        Ok(VersionedUser::new(value, version))
    }
}

impl TryFrom<UserRow> for UserDetailsView {
    type Error = UserRowMappingError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::try_from(row.user_id).map_err(UserRowMappingError::InvalidUserId)?,
            email: parse_email(&row.email)?,
            first_name: row.first_name.clone().map(FirstName::from),
            last_name: row.last_name.clone().map(LastName::from),
            language: parse_optional_language(row.language.as_deref())?,
            currency: parse_optional_currency(row.currency.as_deref())?,
            measurement_unit: parse_optional_measurement_unit(row.measurement_unit.as_deref())?,
            show_unassessed_or_sensitive_content: row.show_unassessed_or_sensitive_content,
            tier: parse_tier(&row.tier)?,
            role: parse_role(&row.role)?,
            stripe_customer_id: row.stripe_customer_id.clone().map(StripeCustomerId::from),
        })
    }
}

impl TryFrom<UserRow> for UserStripeLookupView {
    type Error = UserRowMappingError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::try_from(row.user_id).map_err(UserRowMappingError::InvalidUserId)?,
            email: parse_email(&row.email)?,
            tier: parse_tier(&row.tier)?,
            role: parse_role(&row.role)?,
            stripe_customer_id: row
                .stripe_customer_id
                .ok_or(UserRowMappingError::MissingStripeCustomerId)
                .map(StripeCustomerId::from)?,
        })
    }
}

impl TryFrom<UserRow> for UserSummary {
    type Error = UserRowMappingError;

    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::try_from(row.user_id).map_err(UserRowMappingError::InvalidUserId)?,
            email: parse_email(&row.email)?,
            first_name: row.first_name.clone().map(FirstName::from),
            last_name: row.last_name.clone().map(LastName::from),
            tier: parse_tier(&row.tier)?,
            role: parse_role(&row.role)?,
            stripe_customer_id: row.stripe_customer_id.map(StripeCustomerId::from),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UserRowMappingError {
    #[error("invalid persisted user identifier")]
    InvalidUserId(#[source] domain_primitives::object_id::ObjectIdError),
    #[error("invalid user email")]
    InvalidEmail,
    #[error("invalid user language")]
    InvalidLanguage,
    #[error("invalid user currency")]
    InvalidCurrency,
    #[error("invalid user measurement unit")]
    InvalidMeasurementUnit,
    #[error("invalid user tier")]
    InvalidTier,
    #[error("invalid user role")]
    InvalidRole,
    #[error("missing user stripe customer id")]
    MissingStripeCustomerId,

    #[error("invalid user version")]
    InvalidVersion(#[from] domain_primitives::version::InvalidVersionError),
    #[error("invalid rehydrated user")]
    InvalidUser(#[from] user_core::user::RehydrateUserError),
}

pub(crate) fn bind_language(value: Option<Language>) -> Option<&'static str> {
    value.map(|language| language.as_str())
}

pub(crate) fn bind_currency(value: Option<Currency>) -> Option<&'static str> {
    value.map(|currency| currency.as_str())
}

pub(crate) fn bind_measurement_unit(value: Option<MeasurementUnit>) -> Option<&'static str> {
    value.map(MeasurementUnit::as_str)
}

pub(crate) fn bind_tier(value: UserTier) -> &'static str {
    value.as_str()
}

pub(crate) fn bind_role(value: UserRole) -> &'static str {
    value.as_str()
}

pub(crate) fn version_to_i64(version: UserStorageVersion) -> i64 {
    i64::try_from(version.into_inner()).unwrap_or(i64::MAX)
}

pub(crate) fn sort_user_field_columns(field: SortUserField) -> &'static [&'static str] {
    match field {
        SortUserField::Name => &["first_name", "last_name"],
        SortUserField::Email => &["email"],
        SortUserField::FirstName => &["first_name"],
        SortUserField::LastName => &["last_name"],
        SortUserField::Tier => &["tier"],
        SortUserField::Role => &["role"],
        SortUserField::Created => &["created"],
        SortUserField::Updated => &["updated"],
    }
}

fn profile_from_row(row: &UserRow) -> Result<UserProfile, UserRowMappingError> {
    Ok(UserProfile {
        first_name: row.first_name.clone().map(FirstName::from),
        last_name: row.last_name.clone().map(LastName::from),
    })
}

fn preferences_from_row(row: &UserRow) -> Result<UserPreferences, UserRowMappingError> {
    Ok(UserPreferences {
        language: parse_optional_language(row.language.as_deref())?,
        currency: parse_optional_currency(row.currency.as_deref())?,
        measurement_unit: parse_optional_measurement_unit(row.measurement_unit.as_deref())?,
        show_unassessed_or_sensitive_content: row.show_unassessed_or_sensitive_content,
    })
}

fn account_from_row(row: &UserRow) -> Result<UserAccount, UserRowMappingError> {
    Ok(UserAccount {
        tier: parse_tier(&row.tier)?,
        role: parse_role(&row.role)?,
        stripe_customer_id: row.stripe_customer_id.clone().map(StripeCustomerId::from),
    })
}

fn parse_email(value: &str) -> Result<Email, UserRowMappingError> {
    Email::try_from(value).map_err(|_| UserRowMappingError::InvalidEmail)
}

pub(crate) fn parse_optional_language(
    value: Option<&str>,
) -> Result<Option<Language>, UserRowMappingError> {
    value.map(parse_language).transpose()
}

fn parse_language(value: &str) -> Result<Language, UserRowMappingError> {
    Language::from_code(value).ok_or(UserRowMappingError::InvalidLanguage)
}

pub(crate) fn parse_optional_currency(
    value: Option<&str>,
) -> Result<Option<Currency>, UserRowMappingError> {
    value.map(parse_currency).transpose()
}

fn parse_currency(value: &str) -> Result<Currency, UserRowMappingError> {
    Currency::from_code(value).ok_or(UserRowMappingError::InvalidCurrency)
}

fn parse_optional_measurement_unit(
    value: Option<&str>,
) -> Result<Option<MeasurementUnit>, UserRowMappingError> {
    value.map(parse_measurement_unit).transpose()
}

fn parse_measurement_unit(value: &str) -> Result<MeasurementUnit, UserRowMappingError> {
    MeasurementUnit::from_code(value).ok_or(UserRowMappingError::InvalidMeasurementUnit)
}

pub(crate) fn parse_tier(value: &str) -> Result<UserTier, UserRowMappingError> {
    UserTier::from_code(value).ok_or(UserRowMappingError::InvalidTier)
}

fn parse_role(value: &str) -> Result<UserRole, UserRowMappingError> {
    UserRole::from_code(value).ok_or(UserRowMappingError::InvalidRole)
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::version::InvalidVersionError;
    use strum::IntoEnumIterator;
    use user_service::use_cases::queries::find_user_by_stripe_customer_id::UserStripeLookupView;

    #[test]
    fn should_bind_domain_values_for_sql() {
        assert_eq!(Some("en"), bind_language(Some(Language::En)));
        assert_eq!(Some("GBP"), bind_currency(Some(Currency::Gbp)));
        assert_eq!(
            Some("METRIC"),
            bind_measurement_unit(Some(MeasurementUnit::Metric))
        );
        assert_eq!(
            Some("IMPERIAL"),
            bind_measurement_unit(Some(MeasurementUnit::Imperial))
        );
        assert_eq!("FREE", bind_tier(UserTier::Free));
        assert_eq!("PRO", bind_tier(UserTier::Pro));
        assert_eq!("ULTIMATE", bind_tier(UserTier::Ultimate));
        assert_eq!("USER", bind_role(UserRole::User));
        assert_eq!("ADMIN", bind_role(UserRole::Admin));
    }

    #[test]
    fn should_parse_all_canonical_enum_identifiers() {
        for expected in Language::iter() {
            assert!(matches!(
                parse_language(expected.as_str()),
                Ok(value) if value == expected
            ));
        }
        for expected in Currency::iter() {
            assert!(matches!(
                parse_currency(expected.as_str()),
                Ok(value) if value == expected
            ));
        }
        for expected in MeasurementUnit::iter() {
            assert!(matches!(
                parse_measurement_unit(expected.as_str()),
                Ok(value) if value == expected
            ));
        }
        for expected in UserTier::iter() {
            assert!(matches!(
                parse_tier(expected.as_str()),
                Ok(value) if value == expected
            ));
        }
        for expected in UserRole::iter() {
            assert!(matches!(
                parse_role(expected.as_str()),
                Ok(value) if value == expected
            ));
        }
    }

    #[test]
    fn should_reject_noncanonical_enum_identifiers() {
        assert!(matches!(
            parse_language("EN"),
            Err(UserRowMappingError::InvalidLanguage)
        ));
        assert!(matches!(
            parse_currency("gbp"),
            Err(UserRowMappingError::InvalidCurrency)
        ));
        assert!(matches!(
            parse_measurement_unit("metric"),
            Err(UserRowMappingError::InvalidMeasurementUnit)
        ));
        assert!(matches!(
            parse_tier("pro"),
            Err(UserRowMappingError::InvalidTier)
        ));
        assert!(matches!(
            parse_role("admin"),
            Err(UserRowMappingError::InvalidRole)
        ));
    }

    #[test]
    fn should_map_user_sort_fields_to_postgres_columns() {
        assert_eq!(
            &["first_name", "last_name"],
            sort_user_field_columns(SortUserField::Name)
        );
        assert_eq!(&["email"], sort_user_field_columns(SortUserField::Email));
        assert_eq!(
            &["first_name"],
            sort_user_field_columns(SortUserField::FirstName)
        );
        assert_eq!(
            &["last_name"],
            sort_user_field_columns(SortUserField::LastName)
        );
        assert_eq!(&["tier"], sort_user_field_columns(SortUserField::Tier));
        assert_eq!(&["role"], sort_user_field_columns(SortUserField::Role));
        assert_eq!(
            &["created"],
            sort_user_field_columns(SortUserField::Created)
        );
        assert_eq!(
            &["updated"],
            sort_user_field_columns(SortUserField::Updated)
        );
    }

    #[test]
    fn should_map_full_user_row_to_views() {
        let row = user_row();

        let details = UserDetailsView::try_from(row).unwrap_or_else(|error| {
            panic!("failed to map details: {error}");
        });

        assert_eq!(Email::try_from("ada@example.com").ok(), Some(details.email));
        assert_eq!(Some(Language::En), details.language);
        assert_eq!(Some(Currency::Gbp), details.currency);
        assert_eq!(Some(MeasurementUnit::Imperial), details.measurement_unit);
        assert_eq!(UserTier::Pro, details.tier);
        assert_eq!(UserRole::Admin, details.role);
    }

    #[test]
    fn should_map_empty_optional_address_and_preferences() {
        let row = UserRow {
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
            measurement_unit: None,
            ..user_row()
        };

        let summary = UserSummary::try_from(row).unwrap_or_else(|error| {
            panic!("failed to map summary: {error}");
        });

        assert!(summary.first_name.is_none());
        assert!(summary.last_name.is_none());
    }

    #[test]
    fn should_report_mapping_errors_for_invalid_scalar_values() {
        assert!(matches!(
            UserDetailsView::try_from(UserRow {
                user_id: uuid::Uuid::new_v4(),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidUserId(_))
        ));
        assert!(matches!(
            UserDetailsView::try_from(UserRow {
                email: "not-an-email".to_owned(),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidEmail)
        ));
        assert!(matches!(
            VersionedUser::try_from(UserRow {
                language: Some("xx".to_owned()),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidLanguage)
        ));
        assert!(matches!(
            UserDetailsView::try_from(UserRow {
                currency: Some("BAD".to_owned()),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidCurrency)
        ));
        assert!(matches!(
            UserDetailsView::try_from(UserRow {
                measurement_unit: Some("BAD".to_owned()),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidMeasurementUnit)
        ));
        assert!(matches!(
            UserSummary::try_from(UserRow {
                tier: "BAD".to_owned(),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidTier)
        ));
        assert!(matches!(
            UserSummary::try_from(UserRow {
                role: "BAD".to_owned(),
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidRole)
        ));
    }

    #[test]
    fn should_report_mapping_errors_for_stripe_and_version_branches() {
        assert!(matches!(
            UserStripeLookupView::try_from(UserRow {
                stripe_customer_id: None,
                ..user_row()
            }),
            Err(UserRowMappingError::MissingStripeCustomerId)
        ));
        assert!(matches!(
            VersionedUser::try_from(UserRow {
                version: 0,
                ..user_row()
            }),
            Err(UserRowMappingError::InvalidVersion(
                InvalidVersionError::Zero
            ))
        ));
    }

    fn user_row() -> UserRow {
        UserRow {
            user_id: uuid::Uuid::parse_str("01890a5d-ac96-774b-bf1d-d5586c639f75")
                .unwrap_or_else(|error| panic!("invalid UUIDv7 fixture: {error}")),
            email: "ada@example.com".to_owned(),
            first_name: Some("Ada".to_owned()),
            last_name: Some("Lovelace".to_owned()),
            language: Some("en".to_owned()),
            currency: Some("GBP".to_owned()),
            measurement_unit: Some("IMPERIAL".to_owned()),
            show_unassessed_or_sensitive_content: true,
            suspended: false,
            tier: "PRO".to_owned(),
            role: "ADMIN".to_owned(),
            stripe_customer_id: Some("cus_test".to_owned()),
            version: 1,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }
}

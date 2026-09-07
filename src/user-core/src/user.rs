use crate::{
    first_name::FirstName, last_name::LastName, measurement_unit::MeasurementUnit, name::Name,
    role::UserRole, tier::UserTier,
};
use crate::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use domain_primitives::change_outcome::ChangeOutcome;
use localization::Language;
use money::Currency;
use serde_email::Email;

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    id: UserId,
    email: Email,
    profile: UserProfile,
    preferences: UserPreferences,
    account: UserAccount,
    suspended: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewUser {
    pub id: UserId,
    pub email: Email,
    pub profile: UserProfile,
    pub preferences: UserPreferences,
    pub account: UserAccount,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedUserState {
    pub id: UserId,
    pub email: Email,
    pub profile: UserProfile,
    pub preferences: UserPreferences,
    pub account: UserAccount,
    pub suspended: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserProfile {
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserPreferences {
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub measurement_unit: Option<MeasurementUnit>,
    pub show_unassessed_or_sensitive_content: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserAccount {
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
}

impl Default for UserAccount {
    fn default() -> Self {
        Self {
            tier: UserTier::Free,
            role: UserRole::User,
            stripe_customer_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RehydrateUserError {}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssociateStripeCustomerIdError {
    #[error("a different Stripe customer is already associated")]
    DifferentCustomerAlreadyAssociated,
}

impl User {
    pub fn create(input: NewUser) -> Result<Self, RehydrateUserError> {
        Self::rehydrate(RehydratedUserState {
            id: input.id,
            email: input.email,
            profile: input.profile,
            preferences: input.preferences,
            account: input.account,
            suspended: false,
        })
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn rehydrate(state: RehydratedUserState) -> Result<Self, RehydrateUserError> {
        Ok(Self {
            id: state.id,
            email: state.email,
            profile: state.profile,
            preferences: state.preferences,
            account: state.account,
            suspended: state.suspended,
        })
    }

    pub fn change_email(&mut self, email: Email) -> ChangeOutcome {
        replace_if_changed(&mut self.email, email)
    }

    pub fn replace_profile(
        &mut self,
        profile: UserProfile,
    ) -> Result<ChangeOutcome, RehydrateUserError> {
        Ok(replace_if_changed(&mut self.profile, profile))
    }

    pub fn replace_preferences(&mut self, preferences: UserPreferences) -> ChangeOutcome {
        replace_if_changed(&mut self.preferences, preferences)
    }

    pub fn change_tier(&mut self, tier: UserTier) -> ChangeOutcome {
        replace_if_changed(&mut self.account.tier, tier)
    }

    pub fn change_role(&mut self, role: UserRole) -> ChangeOutcome {
        replace_if_changed(&mut self.account.role, role)
    }

    pub fn suspend(&mut self) -> ChangeOutcome {
        replace_if_changed(&mut self.suspended, true)
    }

    pub fn unsuspend(&mut self) -> ChangeOutcome {
        replace_if_changed(&mut self.suspended, false)
    }

    pub fn change_stripe_customer_id(
        &mut self,
        stripe_customer_id: Option<StripeCustomerId>,
    ) -> ChangeOutcome {
        replace_if_changed(&mut self.account.stripe_customer_id, stripe_customer_id)
    }

    pub fn associate_stripe_customer_id(
        &mut self,
        stripe_customer_id: StripeCustomerId,
    ) -> Result<ChangeOutcome, AssociateStripeCustomerIdError> {
        match self.account.stripe_customer_id.as_ref() {
            None => Ok(replace_if_changed(
                &mut self.account.stripe_customer_id,
                Some(stripe_customer_id),
            )),
            Some(current) if current == &stripe_customer_id => Ok(ChangeOutcome::Unchanged),
            Some(_) => Err(AssociateStripeCustomerIdError::DifferentCustomerAlreadyAssociated),
        }
    }

    pub fn id(&self) -> UserId {
        self.id
    }

    pub fn email(&self) -> &Email {
        &self.email
    }

    pub fn profile(&self) -> &UserProfile {
        &self.profile
    }

    pub fn preferences(&self) -> &UserPreferences {
        &self.preferences
    }

    pub fn account(&self) -> &UserAccount {
        &self.account
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    pub fn name(&self) -> Option<Name> {
        match (
            self.profile.first_name.as_ref(),
            self.profile.last_name.as_ref(),
        ) {
            (Some(first), Some(last)) => Some(Name::from(format!("{first} {last}"))),
            (Some(first), None) => Some(Name::from(first.as_ref())),
            (None, Some(last)) => Some(Name::from(last.as_ref())),
            (None, None) => None,
        }
    }
}

fn replace_if_changed<T: PartialEq>(target: &mut T, value: T) -> ChangeOutcome {
    if *target == value {
        ChangeOutcome::Unchanged
    } else {
        *target = value;
        ChangeOutcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(value: &str) -> Email {
        match Email::try_from(value) {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }

    fn new_user() -> NewUser {
        NewUser {
            id: UserId::new(),
            email: email("ada@example.com"),
            profile: UserProfile {
                first_name: Some("Ada".into()),
                last_name: Some("Lovelace".into()),
            },
            preferences: UserPreferences::default(),
            account: UserAccount::default(),
        }
    }

    #[test]
    fn should_create_user_with_private_state_and_defaults() {
        let input = new_user();
        let id = input.id;
        let email = input.email.clone();
        let result = User::create(input);

        assert!(
            matches!(result, Ok(ref user) if user.id() == id && user.email() == &email && user.account().tier == UserTier::Free && !user.is_suspended())
        );
    }

    #[test]
    fn should_default_user_profile_to_empty_fields() {
        let profile = UserProfile::default();

        assert_eq!(None, profile.first_name);
        assert_eq!(None, profile.last_name);
    }

    #[test]
    fn should_default_user_preferences_to_empty_fields_and_no_consent() {
        let preferences = UserPreferences::default();

        assert_eq!(None, preferences.language);
        assert_eq!(None, preferences.currency);
        assert_eq!(None, preferences.measurement_unit);
        assert!(!preferences.show_unassessed_or_sensitive_content);
    }

    #[test]
    fn should_default_user_account_to_free_user_without_stripe_customer() {
        let account = UserAccount::default();

        assert_eq!(UserTier::Free, account.tier);
        assert_eq!(UserRole::User, account.role);
        assert_eq!(None, account.stripe_customer_id);
    }

    #[test]
    fn should_return_user_name_when_profile_has_names() {
        let result = User::create(new_user());

        assert!(matches!(
            result,
            Ok(ref user) if user.name().is_some_and(|name| name.as_ref() == "Ada Lovelace")
        ));
    }

    #[test]
    fn should_change_tier_when_tier_differs() {
        let mut user = match User::create(new_user()) {
            Ok(user) => user,
            Err(error) => panic!("user create failed: {error}"),
        };

        let outcome = user.change_tier(UserTier::Pro);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(UserTier::Pro, user.account().tier);
    }

    #[test]
    fn should_report_unchanged_when_tier_same() {
        let mut user = match User::create(new_user()) {
            Ok(user) => user,
            Err(error) => panic!("user create failed: {error}"),
        };

        let outcome = user.change_tier(UserTier::Free);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_change_email_when_email_differs() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));

        let outcome = user.change_email(email("grace@example.com"));

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(email("grace@example.com"), *user.email());
    }

    #[test]
    fn should_report_unchanged_when_email_same() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let email = user.email().clone();

        let outcome = user.change_email(email);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_replace_profile_when_profile_differs() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let profile = UserProfile {
            first_name: Some("Grace".into()),
            last_name: None,
        };

        let outcome = user.replace_profile(profile.clone());

        assert_eq!(Ok(ChangeOutcome::Changed), outcome);
        assert_eq!(&profile, user.profile());
    }

    #[test]
    fn should_report_unchanged_when_profile_same() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let profile = user.profile().clone();

        let outcome = user.replace_profile(profile);

        assert_eq!(Ok(ChangeOutcome::Unchanged), outcome);
    }

    #[test]
    fn should_replace_preferences_when_preferences_differ() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let preferences = UserPreferences {
            show_unassessed_or_sensitive_content: true,
            ..Default::default()
        };

        let outcome = user.replace_preferences(preferences.clone());

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(&preferences, user.preferences());
    }

    #[test]
    fn should_report_unchanged_when_preferences_same() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let preferences = user.preferences().clone();

        let outcome = user.replace_preferences(preferences);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_change_role_when_role_differs() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));

        let outcome = user.change_role(UserRole::Admin);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(UserRole::Admin, user.account().role);
    }

    #[test]
    fn should_report_unchanged_when_role_same() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));

        let outcome = user.change_role(UserRole::User);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_suspend_user_without_changing_role_or_tier() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let role = user.account().role;
        let tier = user.account().tier;

        let outcome = user.suspend();

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(user.is_suspended());
        assert_eq!(role, user.account().role);
        assert_eq!(tier, user.account().tier);
    }

    #[test]
    fn should_report_unchanged_when_user_already_suspended() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let _ = user.suspend();

        let outcome = user.suspend();

        assert_eq!(ChangeOutcome::Unchanged, outcome);
        assert!(user.is_suspended());
    }

    #[test]
    fn should_unsuspend_user_without_changing_role_or_tier() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let role = user.account().role;
        let tier = user.account().tier;
        let _ = user.suspend();

        let outcome = user.unsuspend();

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(!user.is_suspended());
        assert_eq!(role, user.account().role);
        assert_eq!(tier, user.account().tier);
    }

    #[test]
    fn should_report_unchanged_when_user_already_active() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));

        let outcome = user.unsuspend();

        assert_eq!(ChangeOutcome::Unchanged, outcome);
        assert!(!user.is_suspended());
    }

    #[test]
    fn should_restore_suspension_state_when_rehydrating() {
        let input = new_user();
        let result = User::rehydrate(RehydratedUserState {
            id: input.id,
            email: input.email,
            profile: input.profile,
            preferences: input.preferences,
            account: input.account,
            suspended: true,
        });

        assert!(matches!(result, Ok(user) if user.is_suspended()));
    }

    #[test]
    fn should_change_stripe_customer_id_when_value_differs() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let stripe_customer_id = StripeCustomerId::from("cus_test");

        let outcome = user.change_stripe_customer_id(Some(stripe_customer_id.clone()));

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(Some(stripe_customer_id), user.account().stripe_customer_id);
    }

    #[test]
    fn should_associate_stripe_customer_id_when_customer_is_absent() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));
        let stripe_customer_id = StripeCustomerId::from("cus_test");

        let outcome = user.associate_stripe_customer_id(stripe_customer_id.clone());

        assert_eq!(Ok(ChangeOutcome::Changed), outcome);
        assert_eq!(Some(stripe_customer_id), user.account().stripe_customer_id);
    }

    #[test]
    fn should_reject_association_when_different_customer_already_exists() {
        let mut input = new_user();
        input.account.stripe_customer_id = Some(StripeCustomerId::from("cus_existing"));
        let mut user =
            User::create(input).unwrap_or_else(|error| panic!("user create failed: {error}"));

        let outcome = user.associate_stripe_customer_id(StripeCustomerId::from("cus_other"));

        assert_eq!(
            Err(AssociateStripeCustomerIdError::DifferentCustomerAlreadyAssociated),
            outcome
        );
    }

    #[test]
    fn should_clear_stripe_customer_id_when_value_exists() {
        let mut input = new_user();
        input.account.stripe_customer_id = Some(StripeCustomerId::from("cus_test"));
        let mut user =
            User::create(input).unwrap_or_else(|error| panic!("user create failed: {error}"));

        let outcome = user.change_stripe_customer_id(None);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(None, user.account().stripe_customer_id);
    }

    #[test]
    fn should_report_unchanged_when_stripe_customer_id_same() {
        let mut user =
            User::create(new_user()).unwrap_or_else(|error| panic!("user create failed: {error}"));

        let outcome = user.change_stripe_customer_id(None);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_return_user_name_when_only_first_name_set() {
        let mut input = new_user();
        input.profile.last_name = None;
        let user =
            User::create(input).unwrap_or_else(|error| panic!("user create failed: {error}"));

        assert!(matches!(user.name(), Some(name) if name.as_ref() == "Ada"));
    }

    #[test]
    fn should_return_user_name_when_only_last_name_set() {
        let mut input = new_user();
        input.profile.first_name = None;
        let user =
            User::create(input).unwrap_or_else(|error| panic!("user create failed: {error}"));

        assert!(matches!(user.name(), Some(name) if name.as_ref() == "Lovelace"));
    }

    #[test]
    fn should_return_no_user_name_when_names_missing() {
        let mut input = new_user();
        input.profile.first_name = None;
        input.profile.last_name = None;
        let user =
            User::create(input).unwrap_or_else(|error| panic!("user create failed: {error}"));

        assert_eq!(None, user.name());
    }
}

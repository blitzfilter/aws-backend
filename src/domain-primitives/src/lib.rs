pub mod object_id;
pub mod query;
pub mod slug_id;
pub mod sort;

pub mod change_outcome {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ChangeOutcome {
        Changed,
        Unchanged,
    }

    impl ChangeOutcome {
        pub fn changed(self) -> bool {
            matches!(self, Self::Changed)
        }

        pub fn combine(self, other: Self) -> Self {
            if self.changed() || other.changed() {
                Self::Changed
            } else {
                Self::Unchanged
            }
        }
    }

    impl From<bool> for ChangeOutcome {
        fn from(changed: bool) -> Self {
            if changed {
                Self::Changed
            } else {
                Self::Unchanged
            }
        }
    }
}

pub mod event_id {
    crate::uuid_v7_newtype!(EventId);

    impl From<EventId> for uuid::Uuid {
        fn from(id: EventId) -> Self {
            id.0
        }
    }
}

pub mod event {
    use crate::event_id::EventId;
    use time::OffsetDateTime;

    #[derive(Debug, Clone, PartialEq)]
    pub struct Event<AggregateId, Payload> {
        pub aggregate_id: AggregateId,
        pub event_id: EventId,
        pub timestamp: OffsetDateTime,
        pub payload: Payload,
    }

    impl<AggregateId, Payload> Event<AggregateId, Payload> {
        pub fn map_payload<R, F>(self, mut f: F) -> Event<AggregateId, R>
        where
            F: FnMut(Payload) -> R,
        {
            Event {
                aggregate_id: self.aggregate_id,
                event_id: self.event_id,
                timestamp: self.timestamp,
                payload: f(self.payload),
            }
        }
    }

    #[cfg(feature = "test-data")]
    impl<AggregateId: fake::Dummy<fake::Faker>, Payload: fake::Dummy<fake::Faker>>
        fake::Dummy<fake::Faker> for Event<AggregateId, Payload>
    {
        fn dummy_with_rng<R: fake::RngExt + ?Sized>(config: &fake::Faker, rng: &mut R) -> Self {
            use fake::Fake;

            Self {
                aggregate_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                timestamp: OffsetDateTime::now_utc(),
                payload: config.fake_with_rng(rng),
            }
        }
    }
}

pub mod version {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
    pub enum InvalidVersionError {
        #[error("version must be greater than zero")]
        Zero,
    }
}

#[doc(hidden)]
pub mod __private {
    pub use serde;
    pub use strong_id::prefix as object_id_prefix;
    pub use uuid::Uuid;

    #[cfg(feature = "test-data")]
    pub use fake::{Dummy, Faker, RngExt};
}

#[cfg(feature = "test-data")]
#[doc(hidden)]
#[macro_export]
macro_rules! __object_id_dummy {
    ($name:ident) => {
        impl $crate::__private::Dummy<$crate::__private::Faker> for $name {
            fn dummy_with_rng<R: $crate::__private::RngExt + ?Sized>(
                _config: &$crate::__private::Faker,
                _rng: &mut R,
            ) -> Self {
                Self::new()
            }
        }
    };
}

#[cfg(not(feature = "test-data"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __object_id_dummy {
    ($name:ident) => {};
}

pub mod versioned {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Versioned<T, V> {
        pub value: T,
        pub version: V,
    }

    impl<T, V> Versioned<T, V> {
        pub fn new(value: T, version: V) -> Self {
            Self { value, version }
        }

        pub fn into_value(self) -> T {
            self.value
        }
    }
}

#[macro_export]
macro_rules! uuid_v4_newtype {
    ($name:ident) => {
        #[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            PartialOrd,
            Eq,
            Ord,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[serde(into = "String", try_from = "String")]
        pub struct $name(::uuid::Uuid);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(::uuid::Uuid::new_v4())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<::uuid::Uuid> for $name {
            fn from(uuid: ::uuid::Uuid) -> Self {
                Self(uuid)
            }
        }

        impl TryFrom<String> for $name {
            type Error = ::uuid::Error;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(&value).map(Self)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0.to_string()
            }
        }

        impl TryFrom<&str> for $name {
            type Error = ::uuid::Error;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(value).map(Self)
            }
        }

        impl TryFrom<&String> for $name {
            type Error = ::uuid::Error;

            fn try_from(value: &String) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(value).map(Self)
            }
        }
    };
}

#[macro_export]
macro_rules! uuid_v7_newtype {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            PartialOrd,
            Eq,
            Ord,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[serde(into = "String", try_from = "String")]
        pub struct $name(::uuid::Uuid);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(::uuid::Uuid::now_v7())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<::uuid::Uuid> for $name {
            fn from(uuid: ::uuid::Uuid) -> Self {
                Self(uuid)
            }
        }

        impl TryFrom<String> for $name {
            type Error = ::uuid::Error;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(&value).map(Self)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0.to_string()
            }
        }

        impl TryFrom<&str> for $name {
            type Error = ::uuid::Error;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(value).map(Self)
            }
        }

        impl TryFrom<&String> for $name {
            type Error = ::uuid::Error;

            fn try_from(value: &String) -> Result<Self, Self::Error> {
                ::uuid::Uuid::parse_str(value).map(Self)
            }
        }

        #[cfg(feature = "test-data")]
        impl<T> ::fake::Dummy<T> for $name {
            fn dummy_with_rng<R: ::fake::RngExt + ?Sized>(_config: &T, _rng: &mut R) -> Self {
                Self::new()
            }
        }
    };
}

#[macro_export]
macro_rules! version_newtype {
    ($name:ident) => {
        $crate::version_newtype!(@define $name, serde);
    };
    ($name:ident, no_serde) => {
        $crate::version_newtype!(@define $name, no_serde);
    };
    (@define $name:ident, serde) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            PartialOrd,
            Eq,
            Ord,
            Hash,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[serde(try_from = "u64", into = "u64")]
        pub struct $name(u64);
        $crate::version_newtype!(@impl $name);
    };
    (@define $name:ident, no_serde) => {
        #[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash)]
        pub struct $name(u64);
        $crate::version_newtype!(@impl $name);
    };
    (@impl $name:ident) => {
        impl Default for $name {
            fn default() -> Self {
                Self::INITIAL
            }
        }

        impl $name {
            pub const INITIAL: Self = Self(1);

            pub fn into_inner(self) -> u64 {
                self.0
            }

            pub fn next(self) -> Self {
                Self(self.0 + 1)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl TryFrom<u64> for $name {
            type Error = $crate::version::InvalidVersionError;

            fn try_from(value: u64) -> Result<Self, Self::Error> {
                if value == 0 {
                    Err($crate::version::InvalidVersionError::Zero)
                } else {
                    Ok(Self(value))
                }
            }
        }

        impl TryFrom<i64> for $name {
            type Error = $crate::version::InvalidVersionError;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                let unsigned =
                    u64::try_from(value).map_err(|_| $crate::version::InvalidVersionError::Zero)?;
                Self::try_from(unsigned)
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        #[cfg(feature = "test-data")]
        impl<T> ::fake::Dummy<T> for $name {
            fn dummy_with_rng<R: ::fake::RngExt + ?Sized>(_config: &T, _rng: &mut R) -> Self {
                Self::INITIAL
            }
        }
    };
}

#[macro_export]
macro_rules! string_newtype {
    ($name:ident, max_length($max:expr), no_fake $(, derives($($derive:path),* $(,)?))? ) => {
        $crate::string_newtype!(@inner_no_fake $name $(, derives($($derive),*))?);
        $crate::string_newtype!(@max_length_from $name, $max);
    };
    ($name:ident, max_length($max:expr) $(, derives($($derive:path),* $(,)?))? ) => {
        $crate::string_newtype!(@inner $name $(, derives($($derive),*))?);
        $crate::string_newtype!(@max_length_from $name, $max);
    };
    ($name:ident, struct_only $(, derives($($derive:path),* $(,)?))? ) => {
        $crate::string_newtype!(@inner_no_fake $name $(, derives($($derive),*))?);
    };
    ($name:ident $(, derives($($derive:path),* $(,)?))? ) => {
        $crate::string_newtype!(@inner $name $(, derives($($derive),*))?);

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&String> for $name {
            fn from(value: &String) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
    (@max_length_from $name:ident, $max:expr) => {
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::from(value.as_str())
            }
        }

        impl From<&String> for $name {
            fn from(value: &String) -> Self {
                Self::from(value.as_str())
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                let trimmed = value.trim();
                if trimmed.len() > $max {
                    if $max >= 3 {
                        let truncate_at = $max - 3;
                        match trimmed.split_at_checked(truncate_at) {
                            Some((truncated, _)) => Self(format!("{}...", truncated)),
                            None => Self(trimmed.into()),
                        }
                    } else {
                        match trimmed.split_at_checked($max) {
                            Some((truncated, _)) => Self(truncated.into()),
                            None => Self(trimmed.into()),
                        }
                    }
                } else {
                    Self(trimmed.into())
                }
            }
        }
    };
    (@inner $name:ident $(, derives($($derive:path),* $(,)?))? ) => {
        #[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
        #[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash $(, $($derive),*)?)]
        pub struct $name(String);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }
    };
    (@inner_no_fake $name:ident $(, derives($($derive:path),* $(,)?))? ) => {
        #[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash $(, $($derive),*)?)]
        pub struct $name(String);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use crate::change_outcome::ChangeOutcome;
    use crate::event::Event;
    use crate::event_id::EventId;
    use crate::version::InvalidVersionError;
    use crate::versioned::Versioned;

    crate::version_newtype!(TestVersion);
    crate::uuid_v4_newtype!(TestId);
    crate::uuid_v7_newtype!(TestEventId);
    crate::string_newtype!(BoundedText, max_length(10));

    #[test]
    fn should_combine_changed_outcomes() {
        assert_eq!(
            ChangeOutcome::Changed,
            ChangeOutcome::Unchanged.combine(ChangeOutcome::Changed)
        );
    }

    #[test]
    fn should_map_event_payload_without_changing_metadata() {
        let event = Event {
            aggregate_id: TestId::new(),
            event_id: EventId::new(),
            timestamp: time::OffsetDateTime::now_utc(),
            payload: "old",
        };
        let event_id = event.event_id;

        let mapped = event.map_payload(str::len);

        assert_eq!(event_id, mapped.event_id);
        assert_eq!(3, mapped.payload);
    }

    #[test]
    fn should_reject_zero_version() {
        assert_eq!(
            InvalidVersionError::Zero,
            TestVersion::try_from(0_u64).unwrap_err()
        );
    }

    #[test]
    fn should_wrap_versioned_value() {
        assert_eq!(
            "value",
            Versioned::new("value", TestVersion::INITIAL).into_value()
        );
    }

    #[test]
    fn should_keep_bounded_text_behavior() {
        assert_eq!("this is...", BoundedText::from("this is too long").as_ref());
    }

    #[test]
    fn should_create_uuid_newtypes() {
        assert_ne!(TestId::new().to_string(), TestEventId::new().to_string());
    }
}

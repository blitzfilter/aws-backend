use strong_id::Id;
use uuid::Uuid;

const TYPE_ID_SUFFIX_LENGTH: usize = 26;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ObjectIdError {
    #[error("malformed object ID `{value}`")]
    Malformed { value: String },
    #[error("wrong object ID prefix: expected `{expected}`, found `{actual}`")]
    WrongPrefix {
        expected: &'static str,
        actual: String,
    },
    #[error("noncanonical object ID `{value}`")]
    NonCanonical { value: String },
    #[error("unsupported UUID version {actual}; expected version 7")]
    UnsupportedUuidVersion { actual: u8 },
    #[error("unsupported UUID variant; expected RFC variant")]
    UnsupportedUuidVariant,
}

#[doc(hidden)]
pub fn format_uuid(prefix: &str, uuid: &Uuid) -> String {
    format!("{prefix}_{}", uuid.encode())
}

#[doc(hidden)]
pub fn parse_uuid(value: &str, expected_prefix: &'static str) -> Result<Uuid, ObjectIdError> {
    let Some((actual_prefix, suffix)) = value.rsplit_once('_') else {
        return Err(malformed(value));
    };

    if !is_structurally_valid_typeid_prefix(actual_prefix)
        || !is_structurally_valid_typeid_suffix(suffix)
    {
        return Err(malformed(value));
    }

    if !actual_prefix.eq_ignore_ascii_case(expected_prefix) {
        return Err(ObjectIdError::WrongPrefix {
            expected: expected_prefix,
            actual: actual_prefix.to_owned(),
        });
    }

    if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(ObjectIdError::NonCanonical {
            value: value.to_owned(),
        });
    }

    let uuid = Uuid::decode(suffix).map_err(|_| malformed(value))?;
    validate_uuid(uuid)?;

    if format_uuid(expected_prefix, &uuid) != value {
        return Err(ObjectIdError::NonCanonical {
            value: value.to_owned(),
        });
    }

    Ok(uuid)
}

#[doc(hidden)]
pub fn validate_uuid(uuid: Uuid) -> Result<Uuid, ObjectIdError> {
    if !matches!(uuid.get_variant(), uuid::Variant::RFC4122) {
        return Err(ObjectIdError::UnsupportedUuidVariant);
    }

    let version = uuid.as_bytes()[6] >> 4;
    if version != 7 {
        return Err(ObjectIdError::UnsupportedUuidVersion { actual: version });
    }

    Ok(uuid)
}

fn malformed(value: &str) -> ObjectIdError {
    ObjectIdError::Malformed {
        value: value.to_owned(),
    }
}

fn is_structurally_valid_typeid_prefix(prefix: &str) -> bool {
    let bytes = prefix.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.first().is_some_and(u8::is_ascii_alphabetic)
        && bytes.last().is_some_and(u8::is_ascii_alphabetic)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
}

fn is_structurally_valid_typeid_suffix(suffix: &str) -> bool {
    suffix.len() == TYPE_ID_SUFFIX_LENGTH
        && suffix
            .as_bytes()
            .first()
            .is_some_and(|byte| matches!(byte, b'0'..=b'7'))
        && suffix.bytes().all(is_typeid_alphabet_character)
}

fn is_typeid_alphabet_character(byte: u8) -> bool {
    byte.is_ascii_digit()
        || matches!(
            byte.to_ascii_lowercase(),
            b'a' | b'b'
                | b'c'
                | b'd'
                | b'e'
                | b'f'
                | b'g'
                | b'h'
                | b'j'
                | b'k'
                | b'm'
                | b'n'
                | b'p'
                | b'q'
                | b'r'
                | b's'
                | b't'
                | b'v'
                | b'w'
                | b'x'
                | b'y'
                | b'z'
        )
}

/// Defines a strict, UUIDv7-backed Aura object identifier.
///
/// The prefix is checked at compile time against the TypeID v0.3 prefix grammar.
/// Raw UUID construction remains fallible; there is intentionally no `From<Uuid>`.
///
/// ```compile_fail
/// use domain_primitives::object_id_newtype;
/// use uuid::Uuid;
///
/// object_id_newtype!(ExampleId, "ex");
/// let _: ExampleId = Uuid::now_v7().into();
/// ```
///
/// ```compile_fail
/// use domain_primitives::object_id_newtype;
///
/// object_id_newtype!(InvalidId, "INVALID");
/// ```
#[macro_export]
macro_rules! object_id_newtype {
    ($name:ident, $prefix:literal) => {
        const _: &str = $crate::__private::object_id_prefix!($prefix);

        #[derive(Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash)]
        pub struct $name($crate::__private::Uuid);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub fn new() -> Self {
                Self($crate::__private::Uuid::now_v7())
            }

            pub const fn as_uuid(&self) -> &$crate::__private::Uuid {
                &self.0
            }

            pub const fn into_uuid(self) -> $crate::__private::Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(&$crate::object_id::format_uuid(Self::PREFIX, &self.0))
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter
                    .debug_tuple(::core::stringify!($name))
                    .field(&self.to_string())
                    .finish()
            }
        }

        impl ::core::str::FromStr for $name {
            type Err = $crate::object_id::ObjectIdError;

            fn from_str(value: &str) -> ::core::result::Result<Self, Self::Err> {
                $crate::object_id::parse_uuid(value, Self::PREFIX).map(Self)
            }
        }

        impl ::core::convert::TryFrom<&str> for $name {
            type Error = $crate::object_id::ObjectIdError;

            fn try_from(value: &str) -> ::core::result::Result<Self, Self::Error> {
                value.parse()
            }
        }

        impl ::core::convert::TryFrom<::std::string::String> for $name {
            type Error = $crate::object_id::ObjectIdError;

            fn try_from(value: ::std::string::String) -> ::core::result::Result<Self, Self::Error> {
                value.parse()
            }
        }

        impl ::core::convert::TryFrom<$crate::__private::Uuid> for $name {
            type Error = $crate::object_id::ObjectIdError;

            fn try_from(
                value: $crate::__private::Uuid,
            ) -> ::core::result::Result<Self, Self::Error> {
                $crate::object_id::validate_uuid(value).map(Self)
            }
        }

        impl ::core::convert::From<$name> for $crate::__private::Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl $crate::__private::serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error>
            where
                S: $crate::__private::serde::Serializer,
            {
                serializer.collect_str(self)
            }
        }

        impl<'de> $crate::__private::serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> ::core::result::Result<Self, D::Error>
            where
                D: $crate::__private::serde::Deserializer<'de>,
            {
                let value =
                    <::std::string::String as $crate::__private::serde::Deserialize>::deserialize(
                        deserializer,
                    )?;
                value
                    .parse()
                    .map_err($crate::__private::serde::de::Error::custom)
            }
        }

        $crate::__object_id_dummy!($name);
    };
}

#[cfg(test)]
mod tests {
    use super::{format_uuid, is_structurally_valid_typeid_prefix};
    use serde::Deserialize;
    use std::error::Error;
    use uuid::Uuid;

    #[derive(Deserialize)]
    struct ValidVector {
        typeid: String,
        prefix: String,
        uuid: Uuid,
    }

    #[derive(Deserialize)]
    struct InvalidVector {
        typeid: String,
    }

    // Faithful fixtures from TypeID spec v0.3 at commit
    // 26a84e85491203834ed0d1c3423ba5f679a67114:
    // https://github.com/jetify-com/typeid/tree/26a84e85491203834ed0d1c3423ba5f679a67114/spec
    #[test]
    fn should_match_all_official_valid_typeid_vectors() -> Result<(), Box<dyn Error>> {
        let vectors: Vec<ValidVector> =
            serde_json::from_str(include_str!("../tests/fixtures/typeid_v0_3_valid.json"))?;

        for vector in vectors {
            let suffix = if vector.prefix.is_empty() {
                vector.typeid.as_str()
            } else {
                vector
                    .typeid
                    .strip_prefix(&format!("{}_", vector.prefix))
                    .ok_or("valid vector prefix must match")?
            };
            let decoded = <Uuid as strong_id::Id>::decode(suffix)?;

            assert_eq!(vector.uuid, decoded);
            if vector.prefix.is_empty() {
                assert_eq!(vector.typeid, strong_id::Id::encode(&decoded));
            } else {
                assert!(is_structurally_valid_typeid_prefix(&vector.prefix));
                assert_eq!(vector.typeid, format_uuid(&vector.prefix, &decoded));
            }
        }

        Ok(())
    }

    #[test]
    fn should_reject_all_official_invalid_typeid_vectors() -> Result<(), Box<dyn Error>> {
        let vectors: Vec<InvalidVector> =
            serde_json::from_str(include_str!("../tests/fixtures/typeid_v0_3_invalid.json"))?;

        for vector in vectors {
            assert!(super::parse_uuid(&vector.typeid, "prefix").is_err());
        }

        Ok(())
    }
}

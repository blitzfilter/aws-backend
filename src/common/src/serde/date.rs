use serde::{Deserialize, Deserializer, Serializer};
use time::{Date, format_description::FormatItem};

/// Partial ISO 8601, date-only
pub const FORMAT: &[FormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]");

pub fn serialize<S>(date: &Date, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let s = date.format(FORMAT).map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&s)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Date, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Date::parse(&s, FORMAT).map_err(serde::de::Error::custom)
}

pub mod option {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(date: &Option<Date>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match date {
            Some(d) => super::serialize(d, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Date>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<String>::deserialize(deserializer)?;
        match opt {
            Some(s) => {
                let d = Date::parse(&s, FORMAT).map_err(serde::de::Error::custom)?;
                Ok(Some(d))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde::{Deserialize, Serialize};
    use serde_json;
    use time::{Date, macros::date};

    #[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
    struct TestStruct {
        #[serde(with = "super")]
        date: Date,
    }

    #[rstest]
    #[case(date!(2025 - 06 - 02), "{\"date\":\"2025-06-02\"}")]
    #[case(date!(1990 - 01 - 01), "{\"date\":\"1990-01-01\"}")]
    #[case(date!(2000 - 12 - 31), "{\"date\":\"2000-12-31\"}")]
    fn should_serialize_date(#[case] date: Date, #[case] expected: &str) {
        let datum = TestStruct { date };
        let actual = serde_json::to_string(&datum).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("{\"date\":\"2025-06-02\"}", date!(2025 - 06 - 02), )]
    #[case("{\"date\":\"1990-01-01\"}", date!(1990 - 01 - 01), )]
    #[case("{\"date\":\"2000-12-31\"}", date!(2000 - 12 - 31), )]
    fn should_deserialize_date(#[case] json: &str, #[case] expected: Date) {
        let actual: TestStruct = serde_json::from_str(json).unwrap();
        assert_eq!(actual, TestStruct { date: expected });
    }

    mod option {
        use rstest::rstest;
        use serde::{Deserialize, Serialize};
        use time::{Date, macros::date};

        #[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq)]
        struct TestStructOption {
            #[serde(with = "super::super::option")]
            date: Option<Date>,
        }

        #[rstest]
        #[case(Some(date!(2025 - 06 - 02)), "{\"date\":\"2025-06-02\"}")]
        #[case(Some(date!(1990 - 01 - 01)), "{\"date\":\"1990-01-01\"}")]
        #[case(Some(date!(2000 - 12 - 31)), "{\"date\":\"2000-12-31\"}")]
        #[case(None, "{\"date\":null}")]
        fn should_serialize_date(#[case] date: Option<Date>, #[case] expected: &str) {
            let datum = TestStructOption { date };
            let actual = serde_json::to_string(&datum).unwrap();
            assert_eq!(actual, expected);
        }

        #[rstest]
        #[case("{\"date\":\"2025-06-02\"}", Some(date!(2025 - 06 - 02)))]
        #[case("{\"date\":\"1990-01-01\"}", Some(date!(1990 - 01 - 01)))]
        #[case("{\"date\":\"2000-12-31\"}", Some(date!(2000 - 12 - 31)))]
        #[case("{\"date\":null}", None)]
        fn should_deserialize_date(#[case] json: &str, #[case] expected: Option<Date>) {
            let actual: TestStructOption = serde_json::from_str(json).unwrap();
            assert_eq!(actual, TestStructOption { date: expected });
        }
    }
}

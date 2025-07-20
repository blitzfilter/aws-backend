use crate::shop_id::ShopId;
use crate::shops_item_id::ShopsItemId;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use time::OffsetDateTime;
use uuid::Uuid;

pub type ItemKey = (ShopId, ShopsItemId);

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ItemId(Uuid);

impl Default for ItemId {
    fn default() -> Self {
        Self::now()
    }
}

impl ItemId {
    /// Creates a new `ItemId` using the current system time.
    ///
    /// This method generates a UUIDv7 based on the current timestamp, providing
    /// both temporal ordering and uniqueness. UUIDv7 is designed to be sortable
    /// and encodes the Unix timestamp in milliseconds in the most significant bits.
    ///
    /// Internally, this uses `Uuid::now_v7()` from the `uuid` crate.
    ///
    /// # Returns
    /// A `ItemId` containing a time-ordered UUIDv7 value.
    pub fn now() -> Self {
        Self(Uuid::now_v7())
    }

    /// Returns the timestamp portion of the UUIDv7 as a hexadecimal prefix.
    ///
    /// This method extracts the most significant 48 bits (6 bytes) of the UUID,
    /// which represent the timestamp in a UUIDv7 format (according to the spec).
    ///
    /// The extracted bits are formatted as a 12-character lowercase hexadecimal string,
    /// and returned in the format `xxxxxxxx-xxxx`, where:
    /// - `xxxxxxxx` are the first 8 hex digits,
    /// - `xxxx` are the remaining 4 hex digits.
    ///
    /// # Returns
    /// A string in the format `"xxxxxxxx-xxxx"` representing the timestamp prefix.
    pub fn timestamp_prefix(&self) -> String {
        let time_bits = (self.0.as_u128() >> 80) & 0xFFFFFFFFFFFF;
        let time_prefix = format!("{time_bits:012x}");
        let (eight, four) = time_prefix.split_at(8);
        format!("{eight}-{four}")
    }
}

impl Display for ItemId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<String> for ItemId {
    type Error = uuid::Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&s).map(Self)
    }
}

impl From<ItemId> for String {
    fn from(id: ItemId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<&str> for ItemId {
    type Error = uuid::Error;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<OffsetDateTime> for ItemId {
    fn from(offset_date_time: OffsetDateTime) -> Self {
        ItemId(
            uuid::Builder::from_unix_timestamp_millis(
                offset_date_time.unix_timestamp() as u64 * 1000
                    + offset_date_time.millisecond() as u64,
                &rand::rng().random(),
            )
            .into_uuid(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ItemId;
    use quickcheck::{Arbitrary, Gen, quickcheck};
    use std::cmp::Ordering;
    use time::macros::datetime;
    use time::{Duration, OffsetDateTime};

    #[derive(Clone, Debug)]
    struct ValidUnixTime(OffsetDateTime);

    impl Arbitrary for ValidUnixTime {
        fn arbitrary(g: &mut Gen) -> Self {
            // Choose a range between e.g. 1970-01-01 and 3000-01-01
            let start = OffsetDateTime::UNIX_EPOCH;
            let end = datetime!(3000-01-01 00:00 UTC);

            let seconds_range = (end - start).whole_seconds();
            let rand_seconds = i64::arbitrary(g).rem_euclid(seconds_range);
            let dt = start + Duration::seconds(rand_seconds);

            ValidUnixTime(dt)
        }
    }

    quickcheck! {
        fn should_preserve_uuid_v7_ordering(dt1: ValidUnixTime, dt2: ValidUnixTime) -> bool {
            let id1 = ItemId::from(dt1.0);
            let id2 = ItemId::from(dt2.0);

            match dt1.0.cmp(&dt2.0) {
                Ordering::Less => id1 < id2,
                Ordering::Equal => id1.timestamp_prefix() == id2.timestamp_prefix(),
                Ordering::Greater => id1 > id2,
            }
        }
    }

    quickcheck! {
        fn should_strip_uuid_v7_timestamp_prefix(datetime: ValidUnixTime) -> bool {
            let item_id = ItemId::from(datetime.0);
            let id = item_id.to_string();
            let prefix = item_id.timestamp_prefix();

            id.starts_with(&prefix)
        }
    }
}

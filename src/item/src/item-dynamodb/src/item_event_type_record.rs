use item_core::domain::ItemEventPayload;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemEventTypeRecord {
    Created,
    StateListed,
    StateAvailable,
    StateReserved,
    StateSold,
    StateRemoved,
    PriceDiscovered,
    PriceDropped,
    PriceIncreased,
}

impl From<&ItemEventPayload> for ItemEventTypeRecord {
    fn from(domain: &ItemEventPayload) -> Self {
        match domain {
            ItemEventPayload::Created(_) => ItemEventTypeRecord::Created,
            ItemEventPayload::StateListed(_) => ItemEventTypeRecord::StateListed,
            ItemEventPayload::StateAvailable(_) => ItemEventTypeRecord::StateAvailable,
            ItemEventPayload::StateReserved(_) => ItemEventTypeRecord::StateReserved,
            ItemEventPayload::StateSold(_) => ItemEventTypeRecord::StateSold,
            ItemEventPayload::StateRemoved(_) => ItemEventTypeRecord::StateRemoved,
            ItemEventPayload::PriceDiscovered(_) => ItemEventTypeRecord::PriceDiscovered,
            ItemEventPayload::PriceDropped(_) => ItemEventTypeRecord::PriceDropped,
            ItemEventPayload::PriceIncreased(_) => ItemEventTypeRecord::PriceIncreased,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ItemEventTypeRecord;
    use rstest::rstest;

    #[rstest]
    #[case(ItemEventTypeRecord::Created, "\"CREATED\"")]
    #[case(ItemEventTypeRecord::StateListed, "\"STATE_LISTED\"")]
    #[case(ItemEventTypeRecord::StateAvailable, "\"STATE_AVAILABLE\"")]
    #[case(ItemEventTypeRecord::StateReserved, "\"STATE_RESERVED\"")]
    #[case(ItemEventTypeRecord::StateSold, "\"STATE_SOLD\"")]
    #[case(ItemEventTypeRecord::StateRemoved, "\"STATE_REMOVED\"")]
    #[case(ItemEventTypeRecord::PriceDiscovered, "\"PRICE_DISCOVERED\"")]
    #[case(ItemEventTypeRecord::PriceDropped, "\"PRICE_DROPPED\"")]
    #[case(ItemEventTypeRecord::PriceIncreased, "\"PRICE_INCREASED\"")]
    fn should_serialize_item_event_type_record_in_screaming_snake_case(
        #[case] item_state_record: ItemEventTypeRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&item_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"CREATED\"", ItemEventTypeRecord::Created)]
    #[case("\"STATE_LISTED\"", ItemEventTypeRecord::StateListed)]
    #[case("\"STATE_AVAILABLE\"", ItemEventTypeRecord::StateAvailable)]
    #[case("\"STATE_RESERVED\"", ItemEventTypeRecord::StateReserved)]
    #[case("\"STATE_SOLD\"", ItemEventTypeRecord::StateSold)]
    #[case("\"STATE_REMOVED\"", ItemEventTypeRecord::StateRemoved)]
    #[case("\"PRICE_DISCOVERED\"", ItemEventTypeRecord::PriceDiscovered)]
    #[case("\"PRICE_DROPPED\"", ItemEventTypeRecord::PriceDropped)]
    #[case("\"PRICE_INCREASED\"", ItemEventTypeRecord::PriceIncreased)]
    fn should_deserialize_item_event_type_record_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ItemEventTypeRecord,
    ) {
        let actual = serde_json::from_str::<ItemEventTypeRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

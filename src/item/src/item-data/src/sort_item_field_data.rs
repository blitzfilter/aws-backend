use item_core::sort_item_field::SortItemField;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortItemFieldData {
    Price,
    Updated,
    Created,
}

impl SortItemFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortItemFieldData::Price => "price",
            SortItemFieldData::Updated => "updated",
            SortItemFieldData::Created => "created",
        }
    }
}

impl From<SortItemFieldData> for &'static str {
    fn from(value: SortItemFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortItemFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "price" => Ok(SortItemFieldData::Price),
            "updated" => Ok(SortItemFieldData::Updated),
            "created" => Ok(SortItemFieldData::Created),
            invalid => Err(format!(
                "Expected any of: 'price', 'updated', 'created'. Got: '{invalid}'"
            )),
        }
    }
}

impl From<SortItemFieldData> for SortItemField {
    fn from(value: SortItemFieldData) -> Self {
        match value {
            SortItemFieldData::Price => SortItemField::Price,
            SortItemFieldData::Updated => SortItemField::Updated,
            SortItemFieldData::Created => SortItemField::Created,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sort_item_field_data::SortItemFieldData;

    #[rstest::rstest]
    #[case(SortItemFieldData::Price)]
    #[case(SortItemFieldData::Created)]
    #[case(SortItemFieldData::Updated)]
    fn should_match_as_str_serialize(#[case] field: SortItemFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}

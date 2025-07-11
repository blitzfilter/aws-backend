use aws_sdk_dynamodb::types::{PutRequest, WriteRequest};
use serde::Serialize;
use std::ops::Deref;
use thiserror::Error;
use tracing::error;

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamoDbBatchConstructionError {
    #[error("DynamoDB must not be empty")]
    DynamoDbBatchEmpty,

    #[error("DynamoDB batch size exceeded: got {0}, max is 25")]
    DynamoDbBatchSizeExceeded(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct DynamoDbBatch<T>(Vec<T>);

impl<T> DynamoDbBatch<T> {
    pub fn from_iter_safe<I: IntoIterator<Item = T>>(
        iter: I,
    ) -> Result<Self, DynamoDbBatchConstructionError> {
        let vec: Vec<T> = iter.into_iter().collect();
        Self::try_from(vec)
    }
}

// region fixed-size array

impl<T> From<[T; 1]> for DynamoDbBatch<T> {
    fn from(value: [T; 1]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 2]> for DynamoDbBatch<T> {
    fn from(value: [T; 2]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 3]> for DynamoDbBatch<T> {
    fn from(value: [T; 3]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 4]> for DynamoDbBatch<T> {
    fn from(value: [T; 4]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 5]> for DynamoDbBatch<T> {
    fn from(value: [T; 5]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 6]> for DynamoDbBatch<T> {
    fn from(value: [T; 6]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 7]> for DynamoDbBatch<T> {
    fn from(value: [T; 7]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 8]> for DynamoDbBatch<T> {
    fn from(value: [T; 8]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 9]> for DynamoDbBatch<T> {
    fn from(value: [T; 9]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 10]> for DynamoDbBatch<T> {
    fn from(value: [T; 10]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 11]> for DynamoDbBatch<T> {
    fn from(value: [T; 11]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 12]> for DynamoDbBatch<T> {
    fn from(value: [T; 12]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 13]> for DynamoDbBatch<T> {
    fn from(value: [T; 13]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 14]> for DynamoDbBatch<T> {
    fn from(value: [T; 14]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 15]> for DynamoDbBatch<T> {
    fn from(value: [T; 15]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 16]> for DynamoDbBatch<T> {
    fn from(value: [T; 16]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 17]> for DynamoDbBatch<T> {
    fn from(value: [T; 17]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 18]> for DynamoDbBatch<T> {
    fn from(value: [T; 18]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 19]> for DynamoDbBatch<T> {
    fn from(value: [T; 19]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 20]> for DynamoDbBatch<T> {
    fn from(value: [T; 20]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 21]> for DynamoDbBatch<T> {
    fn from(value: [T; 21]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 22]> for DynamoDbBatch<T> {
    fn from(value: [T; 22]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 23]> for DynamoDbBatch<T> {
    fn from(value: [T; 23]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 24]> for DynamoDbBatch<T> {
    fn from(value: [T; 24]) -> Self {
        DynamoDbBatch(value.into())
    }
}
impl<T> From<[T; 25]> for DynamoDbBatch<T> {
    fn from(value: [T; 25]) -> Self {
        DynamoDbBatch(value.into())
    }
}

// endregion

impl<T> TryFrom<Vec<T>> for DynamoDbBatch<T> {
    type Error = DynamoDbBatchConstructionError;

    fn try_from(v: Vec<T>) -> Result<Self, DynamoDbBatchConstructionError> {
        match v.len() {
            0 => Err(DynamoDbBatchConstructionError::DynamoDbBatchEmpty),
            1..=25 => Ok(Self(v)),
            size => Err(DynamoDbBatchConstructionError::DynamoDbBatchSizeExceeded(
                size,
            )),
        }
    }
}

impl<T> Deref for DynamoDbBatch<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> AsRef<[T]> for DynamoDbBatch<T> {
    fn as_ref(&self) -> &[T] {
        &self.0
    }
}

impl<T> IntoIterator for DynamoDbBatch<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<T: std::fmt::Display> std::fmt::Display for DynamoDbBatch<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[")?;
        for (i, item) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{item}")?;
        }
        write!(f, "]")
    }
}

impl<T> From<DynamoDbBatch<T>> for Vec<T> {
    fn from(v: DynamoDbBatch<T>) -> Self {
        v.0
    }
}

impl<T> DynamoDbBatch<T> {
    pub fn singleton(v: T) -> Self {
        DynamoDbBatch(vec![v])
    }
}

impl<T: Serialize> DynamoDbBatch<T> {
    pub fn into_write_requests(self) -> Vec<WriteRequest> {
        self.into_iter()
            .filter_map(|record| match serde_dynamo::to_item(record) {
                Ok(item) => Some(
                    WriteRequest::builder()
                        .put_request(PutRequest::builder().set_item(Some(item)).build().expect(
                            "should always succeed because PutRequest::set_item() \
                                                is always called before PutRequest::build()",
                        ))
                        .build(),
                ),
                Err(err) => {
                    error!(
                        error = %err,
                        type = %std::any::type_name::<T>(),
                        "Failed to serialize record."
                    );
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::dynamodb_batch::{DynamoDbBatch, DynamoDbBatchConstructionError};

    #[rstest::rstest]
    #[case::empty(DynamoDbBatch::try_from(vec![]), DynamoDbBatchConstructionError::DynamoDbBatchEmpty)]
    #[case::exceeded(DynamoDbBatch::try_from([1].repeat(26)), DynamoDbBatchConstructionError::DynamoDbBatchSizeExceeded(26))]
    #[case::exceeded(DynamoDbBatch::try_from([1].repeat(27)), DynamoDbBatchConstructionError::DynamoDbBatchSizeExceeded(27))]
    #[case::exceeded(DynamoDbBatch::try_from([1].repeat(100)), DynamoDbBatchConstructionError::DynamoDbBatchSizeExceeded(100))]
    fn should_err_from(
        #[case] batch: Result<DynamoDbBatch<u32>, DynamoDbBatchConstructionError>,
        #[case] err: DynamoDbBatchConstructionError,
    ) {
        assert!(batch.is_err());
        assert_eq!(batch.unwrap_err(), err);
    }

    #[rstest::rstest]
    #[case::one(1)]
    #[case::two(2)]
    #[case::three(3)]
    #[case::four(4)]
    #[case::five(5)]
    #[case::six(6)]
    #[case::seven(7)]
    #[case::eight(8)]
    #[case::nine(9)]
    #[case::ten(10)]
    #[case::eleven(11)]
    #[case::twelve(12)]
    #[case::thirteen(13)]
    #[case::fourteen(14)]
    #[case::fifteen(15)]
    #[case::sixteen(16)]
    #[case::seventeen(17)]
    #[case::eighteen(18)]
    #[case::nineteen(19)]
    #[case::twenty(20)]
    #[case::twentyone(21)]
    #[case::twentytwo(22)]
    #[case::twentythree(23)]
    #[case::twentyfour(24)]
    #[case::twentyfive(25)]
    fn should_ok_from(#[case] size: usize) {
        let batch: Result<DynamoDbBatch<&str>, DynamoDbBatchConstructionError> =
            DynamoDbBatch::try_from(["foo"].repeat(size));

        assert!(batch.is_ok());
        assert_eq!(batch.unwrap().len(), size);
    }
}

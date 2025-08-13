use itertools::Itertools;
use std::ops::Deref;
use thiserror::Error;

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchConstructionError<const N: usize> {
    #[error("Batch must not be empty")]
    BatchEmpty,

    #[error("Batch size exceeded: got {0}, max is {N}")]
    BatchSizeExceeded(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct Batch<T, const N: usize>(Vec<T>);

impl<T, const N: usize> Batch<T, N> {
    pub fn try_from_iter<I: IntoIterator<Item = T>>(
        iter: I,
    ) -> Result<Self, BatchConstructionError<N>> {
        let vec: Vec<T> = iter.into_iter().collect();
        Self::try_from(vec)
    }

    pub fn chunked_from<I: Itertools<Item = T>>(iter: I) -> Vec<Batch<T, N>> {
        iter.chunks(N)
            .into_iter()
            .map(|chunk| Batch(chunk.collect()))
            .collect()
    }
}

// region fixed-size array

impl<T, const N: usize> From<[T; 1]> for Batch<T, N> {
    fn from(value: [T; 1]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 2]> for Batch<T, N> {
    fn from(value: [T; 2]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 3]> for Batch<T, N> {
    fn from(value: [T; 3]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 4]> for Batch<T, N> {
    fn from(value: [T; 4]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 5]> for Batch<T, N> {
    fn from(value: [T; 5]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 6]> for Batch<T, N> {
    fn from(value: [T; 6]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 7]> for Batch<T, N> {
    fn from(value: [T; 7]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 8]> for Batch<T, N> {
    fn from(value: [T; 8]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 9]> for Batch<T, N> {
    fn from(value: [T; 9]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 10]> for Batch<T, N> {
    fn from(value: [T; 10]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 11]> for Batch<T, N> {
    fn from(value: [T; 11]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 12]> for Batch<T, N> {
    fn from(value: [T; 12]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 13]> for Batch<T, N> {
    fn from(value: [T; 13]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 14]> for Batch<T, N> {
    fn from(value: [T; 14]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 15]> for Batch<T, N> {
    fn from(value: [T; 15]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 16]> for Batch<T, N> {
    fn from(value: [T; 16]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 17]> for Batch<T, N> {
    fn from(value: [T; 17]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 18]> for Batch<T, N> {
    fn from(value: [T; 18]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 19]> for Batch<T, N> {
    fn from(value: [T; 19]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 20]> for Batch<T, N> {
    fn from(value: [T; 20]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 21]> for Batch<T, N> {
    fn from(value: [T; 21]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 22]> for Batch<T, N> {
    fn from(value: [T; 22]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 23]> for Batch<T, N> {
    fn from(value: [T; 23]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 24]> for Batch<T, N> {
    fn from(value: [T; 24]) -> Self {
        Batch(value.into())
    }
}
impl<T, const N: usize> From<[T; 25]> for Batch<T, N> {
    fn from(value: [T; 25]) -> Self {
        Batch(value.into())
    }
}

impl<T: Clone, const N: usize> From<&[T; 1]> for Batch<T, N> {
    fn from(value: &[T; 1]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 2]> for Batch<T, N> {
    fn from(value: &[T; 2]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 3]> for Batch<T, N> {
    fn from(value: &[T; 3]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 4]> for Batch<T, N> {
    fn from(value: &[T; 4]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 5]> for Batch<T, N> {
    fn from(value: &[T; 5]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 6]> for Batch<T, N> {
    fn from(value: &[T; 6]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 7]> for Batch<T, N> {
    fn from(value: &[T; 7]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 8]> for Batch<T, N> {
    fn from(value: &[T; 8]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 9]> for Batch<T, N> {
    fn from(value: &[T; 9]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 10]> for Batch<T, N> {
    fn from(value: &[T; 10]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 11]> for Batch<T, N> {
    fn from(value: &[T; 11]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 12]> for Batch<T, N> {
    fn from(value: &[T; 12]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 13]> for Batch<T, N> {
    fn from(value: &[T; 13]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 14]> for Batch<T, N> {
    fn from(value: &[T; 14]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 15]> for Batch<T, N> {
    fn from(value: &[T; 15]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 16]> for Batch<T, N> {
    fn from(value: &[T; 16]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 17]> for Batch<T, N> {
    fn from(value: &[T; 17]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 18]> for Batch<T, N> {
    fn from(value: &[T; 18]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 19]> for Batch<T, N> {
    fn from(value: &[T; 19]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 20]> for Batch<T, N> {
    fn from(value: &[T; 20]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 21]> for Batch<T, N> {
    fn from(value: &[T; 21]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 22]> for Batch<T, N> {
    fn from(value: &[T; 22]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 23]> for Batch<T, N> {
    fn from(value: &[T; 23]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 24]> for Batch<T, N> {
    fn from(value: &[T; 24]) -> Self {
        Batch(value.into())
    }
}
impl<T: Clone, const N: usize> From<&[T; 25]> for Batch<T, N> {
    fn from(value: &[T; 25]) -> Self {
        Batch(value.into())
    }
}

// endregion

impl<T, const N: usize> TryFrom<Vec<T>> for Batch<T, N> {
    type Error = BatchConstructionError<N>;

    fn try_from(v: Vec<T>) -> Result<Self, BatchConstructionError<N>> {
        match v.len() {
            0 => Err(BatchConstructionError::BatchEmpty),
            x if x <= N => Ok(Self(v)),
            size => Err(BatchConstructionError::BatchSizeExceeded(size)),
        }
    }
}

impl<T, const N: usize> Deref for Batch<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T, const N: usize> AsRef<[T]> for Batch<T, N> {
    fn as_ref(&self) -> &[T] {
        &self.0
    }
}

impl<T, const N: usize> IntoIterator for Batch<T, N> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<T: std::fmt::Display, const N: usize> std::fmt::Display for Batch<T, N> {
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

impl<T, const N: usize> From<Batch<T, N>> for Vec<T> {
    fn from(v: Batch<T, N>) -> Self {
        v.0
    }
}

#[cfg(feature = "dynamodb")]
pub mod dynamodb {
    use crate::{batch::Batch, has::HasKey};
    use aws_sdk_dynamodb::{
        operation::batch_write_item::BatchWriteItemOutput,
        types::{PutRequest, WriteRequest},
    };
    use serde::{Deserialize, Serialize};
    use tracing::error;

    impl<T: Serialize> Batch<T, 25> {
        pub fn into_dynamodb_write_requests(self) -> Vec<WriteRequest> {
            self.0
                .into_iter()
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

    pub struct BatchGetItemResult<T, Key> {
        pub items: Vec<T>,
        pub unprocessed: Option<Batch<Key, 100>>,
    }

    pub fn handle_batch_output<T>(output: BatchWriteItemOutput, failures: &mut Vec<T::Key>)
    where
        T: HasKey + for<'de> Deserialize<'de>,
    {
        let unprocessed = output
            .unprocessed_items
            .unwrap_or_default()
            .into_iter()
            .next()
            .map(|(_, unprocessed)| unprocessed)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|write_req| write_req.put_request)
            .map(|put_req| put_req.item)
            .filter_map(|ddb_item| {
                let record_res = serde_dynamo::from_item::<_, T>(ddb_item);
                match record_res {
                    Ok(record_event) => Some(record_event),
                    Err(err) => {
                        error!(
                            error = %err,
                            type = %std::any::type_name::<T>(),
                            "Failed converting DynamoDB-JSON to target-type from failed BatchWriteItemOutput."
                        );
                        None
                    }
                }
            })
            .map(|t| t.key());

        failures.extend(unprocessed);
    }
}

#[cfg(feature = "sqs")]
pub mod sqs {
    use crate::batch::Batch;
    use aws_sdk_sqs::types::SendMessageBatchRequestEntry;
    use itertools::Itertools;
    use serde::Serialize;
    use tracing::error;

    impl<T: Serialize> Batch<T, 10> {
        pub fn into_sqs_message_entries(self) -> Vec<SendMessageBatchRequestEntry> {
            self.0
                .into_iter()
                .enumerate()
                .filter_map(|(i, x)| match serde_json::to_string(&x) {
                    Ok(payload) => Some(
                        SendMessageBatchRequestEntry::builder()
                            .message_body(payload)
                            .id(i.to_string())
                            .build()
                            .expect("shouldn't fail because 'id' and 'message_body' have been set"),
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
                .collect_vec()
        }
    }

    pub struct BatchGetItemResult<T, Key> {
        pub items: Vec<T>,
        pub unprocessed: Option<Batch<Key, 100>>,
    }
}

#[cfg(test)]
mod tests {
    use crate::batch::{Batch, BatchConstructionError};

    #[rstest::rstest]
    #[case::empty(
        Batch::try_from(vec![]),
        BatchConstructionError::BatchEmpty
    )]
    #[case::exceeded(
        Batch::try_from([1].repeat(26)),
        BatchConstructionError::BatchSizeExceeded(26)
    )]
    #[case::exceeded(
        Batch::try_from([1].repeat(27)),
        BatchConstructionError::BatchSizeExceeded(27)
    )]
    #[case::exceeded(
        Batch::try_from([1].repeat(100)),
        BatchConstructionError::BatchSizeExceeded(100)
    )]
    fn should_err_from(
        #[case] batch: Result<Batch<u32, 25>, BatchConstructionError<25>>,
        #[case] err: BatchConstructionError<25>,
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
        let batch: Result<Batch<&str, 25>, BatchConstructionError<25>> =
            Batch::try_from(["foo"].repeat(size));

        assert!(batch.is_ok());
        assert_eq!(batch.unwrap().len(), size);
    }
}

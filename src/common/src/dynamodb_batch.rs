use std::ops::Deref;
use thiserror::Error;

#[derive(Error, Debug, Clone, Copy)]
#[error("DynamoDB batch size exceeded: got {size}, max is 25")]
pub struct DynamoDbBatchSizeExceeded {
    size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct DynamoDbBatch<T>(Vec<T>);

impl<T> DynamoDbBatch<T> {
    pub fn from_iter_safe<I: IntoIterator<Item = T>>(
        iter: I,
    ) -> Result<Self, DynamoDbBatchSizeExceeded> {
        let vec: Vec<T> = iter.into_iter().collect();
        Self::try_from(vec)
    }
}

impl<T> Default for DynamoDbBatch<T> {
    fn default() -> Self {
        Self(Vec::default())
    }
}

impl<T> TryFrom<Vec<T>> for DynamoDbBatch<T> {
    type Error = DynamoDbBatchSizeExceeded;

    fn try_from(v: Vec<T>) -> Result<Self, DynamoDbBatchSizeExceeded> {
        let size = v.len();
        if size > 25 {
            Err(DynamoDbBatchSizeExceeded { size })
        } else {
            Ok(Self(v))
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
            write!(f, "{}", item)?;
        }
        write!(f, "]")
    }
}

impl<T> From<DynamoDbBatch<T>> for Vec<T> {
    fn from(v: DynamoDbBatch<T>) -> Self {
        v.0
    }
}

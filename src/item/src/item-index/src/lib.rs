pub mod bulk;

use crate::bulk::BulkResponse;
use async_trait::async_trait;

#[async_trait]
pub trait IndexItemDocuments {}

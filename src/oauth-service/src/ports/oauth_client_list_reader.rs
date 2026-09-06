use super::OAuthClientReadError;
use crate::use_cases::list_clients::{ListOAuthClientsRequest, ListOAuthClientsResult};

#[async_trait::async_trait]
pub trait OAuthClientListReader: Send + Sync {
    async fn search(
        &self,
        request: &ListOAuthClientsRequest,
    ) -> Result<ListOAuthClientsResult, OAuthClientReadError>;
}

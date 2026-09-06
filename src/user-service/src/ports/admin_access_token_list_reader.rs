use crate::ports::AccessTokenDetails;
use crate::use_cases::queries::list_admin_access_tokens::AccessTokenSearchCursor;
use application::error::BoxError;
use application::pagination::{Cursor, CursoredResult};
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum AdminAccessTokenListReadError {
    #[error("temporary admin access token list read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid admin access token list read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal admin access token list read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AdminAccessTokenListReader: Send {
    async fn list_for_user(
        &mut self,
        user_id: UserId,
        cursor: Cursor<AccessTokenSearchCursor>,
    ) -> Result<
        CursoredResult<AccessTokenDetails, AccessTokenSearchCursor>,
        AdminAccessTokenListReadError,
    >;
}

pub trait AdminAccessTokenListReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl AdminAccessTokenListReader + 'tx;
}

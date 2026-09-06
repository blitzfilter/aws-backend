use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientDetailsReader, OAuthClientView};
use crate::use_cases::support::{authorize_oauth_client_admin, authorize_oauth_client_read};
use application::operation_context::OperationContext;
use credential_core::oauth_client_id::OAuthClientId;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase;

#[async_trait::async_trait]
pub trait GetOAuthClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<OAuthClientView, OAuthServiceError>;
}

pub struct GetOAuthClientHandler<R, A> {
    reader: R,
    check_user_admin: A,
}
impl<R, A> GetOAuthClientHandler<R, A> {
    pub fn new(reader: R, check_user_admin: A) -> Self {
        Self {
            reader,
            check_user_admin,
        }
    }
}
#[async_trait::async_trait]
impl<R, A> GetOAuthClientUseCase for GetOAuthClientHandler<R, A>
where
    R: OAuthClientDetailsReader,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "get_oauth_client",
        skip_all,
        fields(
            client_id = %client_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<OAuthClientView, OAuthServiceError> {
        authorize_oauth_client_read(context)?;
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        authorize_oauth_client_admin(context, &self.check_user_admin).await?;
        self.reader
            .find(client_id)
            .await?
            .ok_or(OAuthServiceError::ClientNotFound)
    }
}

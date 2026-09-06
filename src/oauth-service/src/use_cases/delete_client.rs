use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientRepository, OAuthClientRepositoryFactory};
use crate::use_cases::support::{authorize_oauth_admin, authorize_oauth_client_admin};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteOAuthClientResult {
    pub client_id: OAuthClientId,
}

#[async_trait::async_trait]
pub trait DeleteOAuthClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<DeleteOAuthClientResult, OAuthServiceError>;
}

pub struct DeleteOAuthClientHandler<U, C, A> {
    unit_of_work: U,
    clients: C,
    check_user_admin: A,
}
impl<U, C, A> DeleteOAuthClientHandler<U, C, A> {
    pub fn new(unit_of_work: U, clients: C, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            clients,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, A> DeleteOAuthClientUseCase for DeleteOAuthClientHandler<U, C, A>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "delete_oauth_client",
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
    ) -> Result<DeleteOAuthClientResult, OAuthServiceError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        authorize_oauth_admin(context)?;
        authorize_oauth_client_admin(context, &self.check_user_admin).await?;
        let mut tx = self.unit_of_work.begin().await?;
        let deleted = self
            .clients
            .in_transaction(&mut tx)
            .delete_by_id(*client_id)
            .await?;
        if !deleted {
            tracing::warn!(
                event = "oauth_client.deleted",
                actor_id = %context.principal.label(),
                client_id = %client_id,
                outcome = "not_found",
            );
            return Err(OAuthServiceError::ClientNotFound);
        }
        tx.commit().await?;

        tracing::info!(event = "oauth_client.deleted", actor_id = %context.principal.label(), client_id = %client_id, outcome = "success");
        Ok(DeleteOAuthClientResult {
            client_id: *client_id,
        })
    }
}

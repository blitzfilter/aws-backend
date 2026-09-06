use crate::error::OAuthServiceError;
use crate::ports::{
    OAuthClientRepository, OAuthClientRepositoryFactory, OAuthClientView, PersistedOAuthClient,
};
use crate::use_cases::support::{authorize_oauth_admin, authorize_oauth_client_admin};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;
use domain_primitives::change_outcome::ChangeOutcome;
use oauth_core::client::{OAuthClient, OAuthClientName, OAuthRedirectUris};
use std::collections::HashSet;
use url::Url;
use user_core::access_token::Scope;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateOAuthClientCommand {
    pub name: Option<OAuthClientName>,
    pub redirect_uris: Option<HashSet<Url>>,
    pub tos_uri: Option<Url>,
    pub policy_uri: Option<Url>,
    pub client_uri: Option<Url>,
    pub logo_uri: Option<Url>,
    pub scopes: Option<HashSet<Scope>>,
}

#[async_trait::async_trait]
pub trait UpdateOAuthClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
        command: UpdateOAuthClientCommand,
    ) -> Result<OAuthClientView, OAuthServiceError>;
}

pub struct UpdateOAuthClientHandler<U, C, A> {
    unit_of_work: U,
    clients: C,
    check_user_admin: A,
}
impl<U, C, A> UpdateOAuthClientHandler<U, C, A> {
    pub fn new(unit_of_work: U, clients: C, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            clients,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, A> UpdateOAuthClientUseCase for UpdateOAuthClientHandler<U, C, A>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "update_oauth_client",
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
        command: UpdateOAuthClientCommand,
    ) -> Result<OAuthClientView, OAuthServiceError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        authorize_oauth_admin(context)?;
        authorize_oauth_client_admin(context, &self.check_user_admin).await?;
        let redirect_uris = command
            .redirect_uris
            .clone()
            .map(OAuthRedirectUris::try_from)
            .transpose()
            .map_err(|error| OAuthServiceError::InvalidClientMetadata(error.to_string()))?;

        let mut tx = self.unit_of_work.begin().await?;
        let loaded = self
            .clients
            .in_transaction(&mut tx)
            .find_by_id(*client_id)
            .await?
            .ok_or(OAuthServiceError::ClientNotFound)?;
        let loaded_version = loaded.version;
        let mut client = loaded.value;
        let outcome = apply_client_metadata_changes(&mut client, command, redirect_uris);
        let changed = outcome.changed();
        let persisted = if changed {
            self.clients
                .in_transaction(&mut tx)
                .update(&client, loaded_version)
                .await?
        } else {
            PersistedOAuthClient {
                value: client,
                version: loaded_version,
                created: loaded.created,
                updated: loaded.updated,
            }
        };
        let result = OAuthClientView::from(persisted);

        tx.commit().await?;
        tracing::info!(
            event = "oauth_client.updated",
            actor_id = %context.principal.label(),
            client_id = %result.client_id,
            changed,
            outcome = if changed { "changed" } else { "no_op" },
        );
        Ok(result)
    }
}

fn apply_client_metadata_changes(
    client: &mut OAuthClient,
    command: UpdateOAuthClientCommand,
    redirect_uris: Option<OAuthRedirectUris>,
) -> ChangeOutcome {
    let mut outcome = ChangeOutcome::Unchanged;
    if let Some(name) = command.name {
        outcome = outcome.combine(client.change_name(name));
    }
    if let Some(redirect_uris) = redirect_uris {
        outcome = outcome.combine(client.replace_redirect_uris(redirect_uris));
    }
    if let Some(tos_uri) = command.tos_uri {
        outcome = outcome.combine(client.change_tos_uri(tos_uri));
    }
    if let Some(policy_uri) = command.policy_uri {
        outcome = outcome.combine(client.change_policy_uri(policy_uri));
    }
    if let Some(client_uri) = command.client_uri {
        outcome = outcome.combine(client.change_client_uri(client_uri));
    }
    if let Some(logo_uri) = command.logo_uri {
        outcome = outcome.combine(client.change_logo_uri(logo_uri));
    }
    if let Some(scopes) = command.scopes {
        outcome = outcome.combine(client.replace_scopes(scopes));
    }
    outcome
}

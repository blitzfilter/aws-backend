pub mod create;
pub mod delete;
pub mod list;
pub(crate) mod types;
pub mod update;
pub(crate) mod util;

pub use create::post_watchlist;
pub use delete::delete_watchlist;
pub use list::list_watchlist;
pub use update::patch_watchlist;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::state::WatchlistState;
    use application::error::static_error;
    use application::operation_context::OperationContext;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use product_listing_core::product_listing_id::ProductListingId;
    use std::sync::Arc;
    use tower::ServiceExt;
    use user_core::user_id::UserId;
    use watchlist_service::use_cases::{
        ListWatchlistError, ListWatchlistRequest, ListWatchlistResult, ListWatchlistUseCase,
        UnwatchProductListingCommand, UnwatchProductListingError, UnwatchProductListingResult,
        UnwatchProductListingUseCase, UpdateWatchlistProductListingCommand,
        UpdateWatchlistProductListingError, UpdateWatchlistProductListingResult,
        UpdateWatchlistProductListingUseCase, WatchProductListingCommand, WatchProductListingError,
        WatchProductListingResult, WatchProductListingUseCase,
    };

    struct FakeAuthenticator(UserId);

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(TransportPrincipal::User {
                user_id: self.0,
                auth_method: AuthMethod::CognitoJwt,
                capabilities: Default::default(),
            })
        }
    }

    struct FakeList;
    #[async_trait::async_trait]
    impl ListWatchlistUseCase for FakeList {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ListWatchlistRequest,
        ) -> Result<ListWatchlistResult, ListWatchlistError> {
            Err(ListWatchlistError::TemporarilyUnavailable)
        }
    }

    struct FakeWatch;
    #[async_trait::async_trait]
    impl WatchProductListingUseCase for FakeWatch {
        async fn execute(
            &self,
            _: &OperationContext,
            _: WatchProductListingCommand,
        ) -> Result<WatchProductListingResult, WatchProductListingError> {
            Err(WatchProductListingError::ProductListingUnavailable)
        }
    }

    struct FakeUpdate;
    #[async_trait::async_trait]
    impl UpdateWatchlistProductListingUseCase for FakeUpdate {
        async fn execute(
            &self,
            _: &OperationContext,
            _: UpdateWatchlistProductListingCommand,
        ) -> Result<UpdateWatchlistProductListingResult, UpdateWatchlistProductListingError>
        {
            Err(UpdateWatchlistProductListingError::ProductListingNotFound)
        }
    }

    struct FakeUnwatch;
    #[async_trait::async_trait]
    impl UnwatchProductListingUseCase for FakeUnwatch {
        async fn execute(
            &self,
            _: &OperationContext,
            _: UnwatchProductListingCommand,
        ) -> Result<UnwatchProductListingResult, UnwatchProductListingError> {
            Err(UnwatchProductListingError::TemporarilyUnavailable {
                source: static_error("unused unwatch use case"),
            })
        }
    }

    fn state(user_id: UserId) -> WatchlistState {
        WatchlistState::new(
            Arc::new(FakeList),
            Arc::new(FakeWatch),
            Arc::new(FakeUpdate),
            Arc::new(FakeUnwatch),
            Arc::new(FakeAuthenticator(user_id)),
        )
    }

    async fn problem_error(response: axum::response::Response) -> Result<String, axum::Error> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default()["error"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    }

    #[tokio::test]
    async fn should_map_watch_product_listing_unavailable_from_controller()
    -> Result<(), Box<dyn std::error::Error>> {
        let app = Router::new()
            .route("/api/v1/me/watchlist", axum::routing::post(post_watchlist))
            .with_state(state(UserId::new()));

        let response = app
            .oneshot(
                Request::post("/api/v1/me/watchlist")
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"productListingId":"{}"}}"#,
                        ProductListingId::new()
                    )))?,
            )
            .await?;

        assert_eq!(StatusCode::CONFLICT, response.status());
        assert_eq!(
            "PRODUCT_LISTING_UNAVAILABLE",
            problem_error(response).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_update_watchlist_product_listing_not_found_from_controller()
    -> Result<(), Box<dyn std::error::Error>> {
        let product_listing_id = ProductListingId::new();
        let app = Router::new()
            .route(
                "/api/v1/me/watchlist/{product_listing_id}",
                axum::routing::patch(patch_watchlist),
            )
            .with_state(state(UserId::new()));

        let response = app
            .oneshot(
                Request::patch(format!("/api/v1/me/watchlist/{product_listing_id}"))
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"state":"ACTIVE"}"#))?,
            )
            .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert_eq!("PRODUCT_LISTING_NOT_FOUND", problem_error(response).await?);
        Ok(())
    }
}

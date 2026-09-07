use super::types::{
    CreateProductListingData, PartnerProductFailureData, parse_partner_product_batch,
};
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::PartnerProductListingsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use listing_source_core::ListingSourceId;

pub async fn create_products(
    State(state): State<PartnerProductListingsState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: String,
) -> Response {
    let listing_source_id = match parse_listing_source_id(&raw_listing_source_id) {
        Ok(listing_source_id) => listing_source_id,
        Err(error) => return error.into_response(),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let products: Vec<CreateProductListingData> = match parse_partner_product_batch(&body) {
        Ok(products) => products,
        Err(error) => return error.into_response(),
    };

    let mapped = match products
        .into_iter()
        .map(|product| {
            let raw_source_listing_id = product.source_listing_id.clone();
            product
                .into_command(listing_source_id)
                .map(|command| (raw_source_listing_id, command))
        })
        .collect::<Result<Vec<_>, ApiError>>()
    {
        Ok(mapped) => mapped,
        Err(error) => return error.into_response(),
    };

    let mut failures = Vec::new();
    let mut first_error = None;
    let mut successes = 0;
    for (source_listing_id, command) in mapped {
        match state.create.execute(&context, command).await {
            Ok(_) => successes += 1,
            Err(error) => {
                let error = ApiError::from(error);
                let error_code = error.code();
                first_error.get_or_insert(error);
                failures.push(PartnerProductFailureData::new(
                    listing_source_id,
                    source_listing_id,
                    error_code,
                ));
            }
        }
    }

    if successes == 0
        && let Some(error) = first_error
    {
        return error.into_response();
    }

    (StatusCode::OK, Json(failures)).into_response()
}

fn parse_listing_source_id(value: &str) -> Result<ListingSourceId, ApiError> {
    uuid::Uuid::parse_str(value)
        .map(ListingSourceId::from)
        .map_err(|_| {
            ApiError::bad_request(INVALID_UUID)
                .with_path_field("listingSourceId")
                .with_detail("Path parameter 'listingSourceId' must be a UUID.")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::partner_product_listings::types::MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE;
    use application::operation_context::{CredentialCapability, OperationContext};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, header};
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use product_listing_service::use_cases::{
        CreateProductListingCommand, CreateProductListingError, CreateProductListingResult,
        CreateProductListingUseCase, UpdateProductListingCommand, UpdateProductListingError,
        UpdateProductListingResult, UpdateProductListingUseCase, UpsertProductListingCommand,
        UpsertProductListingError, UpsertProductListingResult, UpsertProductListingUseCase,
        WithdrawProductListingError, WithdrawProductListingResult, WithdrawProductListingUseCase,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    mockall::mock! {
        CreateUseCase {}
        #[async_trait::async_trait]
        impl CreateProductListingUseCase for CreateUseCase {
            async fn execute(&self, context: &OperationContext, command: CreateProductListingCommand) -> Result<CreateProductListingResult, CreateProductListingError>;
        }
    }

    mockall::mock! {
        UpdateUseCase {}
        #[async_trait::async_trait]
        impl UpdateProductListingUseCase for UpdateUseCase {
            async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>;
            async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey, command: UpdateProductListingCommand) -> Result<UpdateProductListingResult, UpdateProductListingError>;
        }
    }

    mockall::mock! {
        UpsertUseCase {}
        #[async_trait::async_trait]
        impl UpsertProductListingUseCase for UpsertUseCase {
            async fn execute(&self, context: &OperationContext, command: UpsertProductListingCommand) -> Result<UpsertProductListingResult, UpsertProductListingError>;
        }
    }

    mockall::mock! {
        WithdrawUseCase {}
        #[async_trait::async_trait]
        impl WithdrawProductListingUseCase for WithdrawUseCase {
            async fn execute(&self, context: &OperationContext, product_listing_id: ProductListingId) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
            async fn execute_by_key(&self, context: &OperationContext, product_key: ProductListingKey) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
        }
    }

    mockall::mock! {
        Authenticator {}
        #[async_trait::async_trait]
        impl TokenAuthenticator for Authenticator {
            async fn authenticate(&self, bearer_token: &str, metadata: &RequestMetadata) -> Result<TransportPrincipal, AuthError>;
        }
    }

    #[tokio::test]
    async fn should_return_empty_failures_when_all_creates_succeed()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create
            .expect_execute()
            .times(2)
            .returning(|_, _| Ok(created()));
        let app = app(create);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{},{}]", product("first"), product("second")),
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_create_without_availability_assertion_when_it_is_omitted_or_null()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create
            .expect_execute()
            .times(2)
            .withf(|_, command| command.availability.is_none())
            .returning(|_, _| Ok(created()));
        let app = app(create);
        let listing_source_id = ListingSourceId::new();
        let omitted = product("omitted").replace(",\"availability\":\"AVAILABLE\"", "");
        let explicit_null = product("null").replace("\"AVAILABLE\"", "null");

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{omitted},{explicit_null}]"),
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(json!([]), body_json(response).await?);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_failed_key_when_create_batch_partially_succeeds()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create.expect_execute().times(2).returning(|_, command| {
            if command.source_listing_id.as_ref() == "failed" {
                Err(CreateProductListingError::SourceListingAlreadyExists)
            } else {
                Ok(created())
            }
        });
        let app = app(create);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{},{}]", product("created"), product("failed")),
            true,
        )
        .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            json!([{
                "listingSourceId": listing_source_id.to_string(),
                "sourceListingId": "failed",
                "error": "CONFLICT"
            }]),
            body_json(response).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_first_error_when_all_creates_fail()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create
            .expect_execute()
            .times(1)
            .returning(|_, _| Err(CreateProductListingError::SourceListingAlreadyExists));
        let app = app(create);
        let listing_source_id = ListingSourceId::new();

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{}]", product("duplicate")),
            true,
        )
        .await?;

        assert_eq!(StatusCode::CONFLICT, response.status());
        assert_eq!("CONFLICT", body_json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_batch_larger_than_limit_before_create_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create.expect_execute().never();
        let app = app(create);
        let listing_source_id = ListingSourceId::new();
        let body = format!(
            "[{}]",
            (0..=MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE)
                .map(|_| product("too-many"))
                .collect::<Vec<_>>()
                .join(",")
        );

        let response = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            body,
            true,
        )
        .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(json!("BAD_BODY_VALUE"), body_json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_auth_or_invalid_body_before_create_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut create = MockCreateUseCase::new();
        create.expect_execute().never();
        let app = app(create);
        let listing_source_id = ListingSourceId::new();

        let missing_auth = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            format!("[{}]", product("created")),
            false,
        )
        .await?;
        assert_eq!(StatusCode::UNAUTHORIZED, missing_auth.status());

        let invalid_body = request(
            &app,
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            "{".to_owned(),
            true,
        )
        .await?;
        assert_eq!(StatusCode::BAD_REQUEST, invalid_body.status());
        Ok(())
    }

    fn app(create: MockCreateUseCase) -> Router {
        let state = PartnerProductListingsState::new(
            Arc::new(create),
            Arc::new(MockUpdateUseCase::new()),
            Arc::new(MockUpsertUseCase::new()),
            Arc::new(MockWithdrawUseCase::new()),
            Arc::new(authenticator()),
        );
        Router::new()
            .route(
                "/api/v1/listing-sources/{listing_source_id}/product-listings",
                axum::routing::post(create_products),
            )
            .with_state(state)
    }

    fn authenticator() -> MockAuthenticator {
        let mut authenticator = MockAuthenticator::new();
        authenticator.expect_authenticate().returning(|_, _| {
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
            })
        });
        authenticator
    }

    fn created() -> CreateProductListingResult {
        CreateProductListingResult {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("created-product-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
        }
    }

    async fn request(
        app: &Router,
        uri: &str,
        body: String,
        authenticated: bool,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let mut builder = Request::builder().method("POST").uri(uri);
        if authenticated {
            builder = builder.header(header::AUTHORIZATION, "Bearer valid");
        }
        Ok(app.clone().oneshot(builder.body(Body::from(body))?).await?)
    }

    async fn body_json(response: Response) -> Result<Value, Box<dyn std::error::Error>> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn product(source_listing_id: &str) -> String {
        format!(
            r#"{{"sourceListingId":"{source_listing_id}","title":{{"text":"Cabinet","language":"en"}},"description":{{"text":"Old cabinet","language":"en"}},"availability":"AVAILABLE","url":"https://source.example/product-listings/{source_listing_id}","images":[]}}"#
        )
    }
}

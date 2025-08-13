use std::collections::HashMap;

use crate::repository::QueryItemRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmountOverflowError, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::domain::description::Description;
use item_core::item::domain::title::Title;
use item_core::item::domain::{Item, LocalizedItemView};
use tracing::error;

#[derive(thiserror::Error, Debug)]
pub enum GetItemError {
    #[error("Item with ShopId '{0}' and ShopsItemId '{1}' not found.")]
    ItemNotFound(ShopId, ShopsItemId),

    #[error("{0}")]
    MonetaryAmountOverflowError(#[from] MonetaryAmountOverflowError),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] Box<SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>>,
    ),
}

#[cfg(feature = "api")]
pub mod api {
    use crate::service::GetItemError;
    use common::api::error::ApiError;
    use common::api::error_code::{ITEM_NOT_FOUND, MONETARY_AMOUNT_OVERFLOW};

    impl From<GetItemError> for ApiError {
        fn from(err: GetItemError) -> Self {
            match err {
                GetItemError::ItemNotFound(_, _) => ApiError::not_found(ITEM_NOT_FOUND),
                GetItemError::MonetaryAmountOverflowError(_) => {
                    ApiError::internal_server_error(MONETARY_AMOUNT_OVERFLOW)
                }
                GetItemError::SdkGetItemError(_) => err.into(),
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait QueryItemService {
    async fn find_item(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Item, GetItemError>;

    async fn view_item(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<LocalizedItemView, GetItemError>;
}

pub struct QueryItemServiceImpl<'a> {
    repository: &'a (dyn QueryItemRepository + Sync),
}

impl<'a> QueryItemServiceImpl<'a> {
    pub fn new(repository: &'a (dyn QueryItemRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl<'a> QueryItemService for QueryItemServiceImpl<'a> {
    async fn find_item(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Item, GetItemError> {
        let item_record = self
            .repository
            .get_item_record(shop_id, shops_item_id)
            .await
            .map_err(Box::from)?
            .ok_or(GetItemError::ItemNotFound(
                shop_id.clone(),
                shops_item_id.clone(),
            ))?;

        Ok(item_record.into())
    }

    async fn view_item(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        preferred_languages: &[Language],
        currency: &Currency,
    ) -> Result<LocalizedItemView, GetItemError> {
        let item_record = self
            .repository
            .get_item_record(shop_id, shops_item_id)
            .await
            .map_err(Box::from)?
            .ok_or(GetItemError::ItemNotFound(
                shop_id.clone(),
                shops_item_id.clone(),
            ))?;

        let mut available_titles: HashMap<Language, Title> = HashMap::with_capacity(3);
        available_titles.insert(
            item_record.title_native.language.into(),
            item_record.title_native.text.into(),
        );
        if let Some(title_de) = item_record.title_de {
            available_titles.insert(Language::De, title_de.into());
        }
        if let Some(title_en) = item_record.title_en {
            available_titles.insert(Language::En, title_en.into());
        }

        let mut available_descriptions: HashMap<Language, Description> = HashMap::with_capacity(3);
        if let Some(description_native) = item_record.description_native {
            available_descriptions.insert(
                description_native.language.into(),
                description_native.text.into(),
            );
        }
        if let Some(description_de) = item_record.description_de {
            available_descriptions.insert(Language::De, description_de.into());
        }
        if let Some(description_en) = item_record.description_en {
            available_descriptions.insert(Language::En, description_en.into());
        }

        let title = Language::resolve(preferred_languages, available_titles).unwrap_or_else(|| {
            error!(
                shopId = %shop_id,
                shopsItemId = %shops_item_id,
                "Failed resolving title. This SHOULD be impossible because the native title always exists."
            );
            Localized::new(Language::En, "Unknown title".into())
        });
        let description = Language::resolve(preferred_languages, available_descriptions);

        let price = match currency {
            Currency::Eur => item_record
                .price_eur
                .map(|amount| Price::new(amount.into(), Currency::Eur)),
            Currency::Gbp => item_record
                .price_gbp
                .map(|amount| Price::new(amount.into(), Currency::Gbp)),
            Currency::Usd => item_record
                .price_usd
                .map(|amount| Price::new(amount.into(), Currency::Usd)),
            Currency::Aud => item_record
                .price_aud
                .map(|amount| Price::new(amount.into(), Currency::Aud)),
            Currency::Cad => item_record
                .price_cad
                .map(|amount| Price::new(amount.into(), Currency::Cad)),
            Currency::Nzd => item_record
                .price_nzd
                .map(|amount| Price::new(amount.into(), Currency::Nzd)),
        };

        let item_view = LocalizedItemView {
            item_id: item_record.item_id,
            event_id: item_record.event_id,
            shop_id: item_record.shop_id,
            shops_item_id: item_record.shops_item_id,
            shop_name: item_record.shop_name.into(),
            title,
            description,
            price,
            state: item_record.state.into(),
            url: item_record.url,
            images: item_record.images,
            hash: item_record.hash,
            created: item_record.created,
            updated: item_record.updated,
        };

        Ok(item_view)
    }
}

// #[cfg(test)]
// mod tests {
//     use common::shop_id::ShopId;

//     use crate::{
//         repository::MockQueryItemRepository,
//         service::{GetItemError, QueryItemService, QueryItemServiceImpl},
//     };

//     async fn should_return_item_not_found_err_when_item_does_not_exist() {
//         let shop_id = ShopId::new();
//         let shops_item_id = "non-existent".into();
//         let mut repository = &MockQueryItemRepository::default();
//         repository
//             .expect_get_item_record()
//             .return_once(|shop_id, shops_item_id| {});
//         let service = QueryItemServiceImpl { repository };
//         let actual = service.find_item(&shop_id, &shops_item_id).await;

//         assert!(actual.is_err());
//         match actual.unwrap_err() {
//             GetItemError::ItemNotFound(err_shop_id, err_shops_item_id) => {
//                 assert_eq!(err_shop_id, shop_id);
//                 assert_eq!(err_shops_item_id, shops_item_id);
//             }
//             _ => panic!("expected GetItemError::ItemNotFound"),
//         }
//     }
// }

use crate::repository::QueryItemRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::currency::domain::Currency;
use common::has::Has;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmountOverflowError, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::domain::{Item, LocalizedItemView};

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

#[async_trait]
impl<T: Has<aws_sdk_dynamodb::Client> + Sync> QueryItemService for T {
    async fn find_item(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Item, GetItemError> {
        let item_record = self
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
        languages: &[Language],
        currency: &Currency,
    ) -> Result<LocalizedItemView, GetItemError> {
        let item_record = self
            .get_item_record(shop_id, shops_item_id)
            .await
            .map_err(Box::from)?
            .ok_or(GetItemError::ItemNotFound(
                shop_id.clone(),
                shops_item_id.clone(),
            ))?;

        let mut available_languages = languages
            .iter()
            .filter(|x| x == &&Language::De || x == &&Language::En)
            .collect::<Vec<&Language>>();
        available_languages.push(&Language::De);
        available_languages.push(&Language::En);

        let title = match (
            available_languages.as_slice(),
            item_record.title_de,
            item_record.title_en,
        ) {
            ([Language::De, ..], Some(title_de), _) => {
                Localized::new(Language::De, title_de.into())
            }
            ([Language::En, ..], Some(title_en), _) => {
                Localized::new(Language::En, title_en.into())
            }
            _ => Localized::new(
                item_record.title_native.language.into(),
                item_record.title_native.text.into(),
            ),
        };

        let description = match (
            available_languages.as_slice(),
            item_record.description_de,
            item_record.description_en,
        ) {
            ([Language::De, ..], Some(description_de), _) => {
                Some(Localized::new(Language::De, description_de.into()))
            }
            ([Language::En, ..], Some(description_en), _) => {
                Some(Localized::new(Language::En, description_en.into()))
            }
            _ => item_record.description_native.map(|description_native| {
                Localized::new(
                    description_native.language.into(),
                    description_native.text.into(),
                )
            }),
        };

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

use crate::repository::ReadItemRecords;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::currency::domain::Currency;
use common::has::Has;
use common::price::domain::FixedFxRate;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::domain::Item;

#[derive(thiserror::Error, Debug)]
pub enum GetItemError {
    #[error("Item with ShopId '{0}' and ShopsItemId '{1}' not found.")]
    ItemNotFound(ShopId, ShopsItemId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] Box<SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>>,
    ),
}

#[async_trait]
pub trait ReadItem {
    async fn get_item_with_currency(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        currency: Currency,
    ) -> Result<Item, GetItemError>;
}

#[async_trait]
impl<T: Has<aws_sdk_dynamodb::Client> + Sync> ReadItem for T {
    async fn get_item_with_currency(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        currency: Currency,
    ) -> Result<Item, GetItemError> {
        let item_record = self
            .get_item_record(shop_id, shops_item_id)
            .await
            .map_err(Box::from)?
            .ok_or(GetItemError::ItemNotFound(*shop_id, shops_item_id.clone()))?;

        let mut item: Item = item_record.into();
        if let Some(price) = &mut item.price {
            // GitHub#2: Fetch current FxRate from DynamoDB instead
            price.exchanged(&FixedFxRate(), currency);
        }

        Ok(item)
    }
}

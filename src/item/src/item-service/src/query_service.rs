use std::collections::HashMap;

use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::localized::Localized;
use common::page::Page;
use common::price::domain::{MonetaryAmountOverflowError, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use common::sort::Sort;
use item_core::description::Description;
use item_core::item::{Item, LocalizedItemView, SortItemField};
use item_core::title::Title;
use item_dynamodb::repository::ItemDynamoDbRepository;
use search_filter_core::search_filter::SearchFilter;
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

pub mod api {
    use common::api::error::ApiError;
    use common::api::error_code::{ITEM_NOT_FOUND, MONETARY_AMOUNT_OVERFLOW};

    use crate::query_service::GetItemError;

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

#[derive(thiserror::Error, Debug)]
pub enum SearchItemsError {}

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

    async fn search_items(
        &self,
        search_filter: &SearchFilter,
        language: &Language,
        currency: &Currency,
        sort: &Option<Sort<SortItemField>>,
        page: &Option<Page>,
    ) -> Result<Vec<LocalizedItemView>, SearchItemsError>;
}

pub struct QueryItemServiceImpl<'a> {
    repository: &'a (dyn ItemDynamoDbRepository + Sync),
}

impl<'a> QueryItemServiceImpl<'a> {
    pub fn new(repository: &'a (dyn ItemDynamoDbRepository + Sync)) -> Self {
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

    async fn search_items(
        &self,
        _search_filter: &SearchFilter,
        _language: &Language,
        _currency: &Currency,
        _sort: &Option<Sort<SortItemField>>,
        _page: &Option<Page>,
    ) -> Result<Vec<LocalizedItemView>, SearchItemsError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {

    mod find_item {
        use crate::query_service::{GetItemError, QueryItemService, QueryItemServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{shop_id::ShopId, shops_item_id::ShopsItemId};
        use fake::{Fake, Faker};
        use item_dynamodb::repository::MockItemDynamoDbRepository;

        #[tokio::test]
        async fn should_return_item_when_exists() {
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual = service.find_item(&ShopId::new(), &ShopsItemId::new()).await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_item_not_found_err_when_item_does_not_exist() {
            let shop_id = ShopId::new();
            let shops_item_id = "non-existent".into();
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual = service.find_item(&shop_id, &shops_item_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetItemError::ItemNotFound(err_shop_id, err_shops_item_id) => {
                    assert_eq!(err_shop_id, shop_id);
                    assert_eq!(err_shops_item_id, shops_item_id);
                }
                _ => panic!("expected GetItemError::ItemNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let shop_id = ShopId::new();
            let shops_item_id = "non-existent".into();
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual = service.find_item(&shop_id, &shops_item_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetItemError::SdkGetItemError(_) => {}
                _ => panic!("expected GetItemError::ItemNotFound"),
            }
        }
    }

    mod view_item {
        use crate::query_service::{GetItemError, QueryItemService, QueryItemServiceImpl};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{
            currency::domain::Currency,
            language::{
                domain::Language,
                domain::Language::*,
                record::{LanguageRecord, TextRecord},
            },
            shop_id::ShopId,
            shops_item_id::ShopsItemId,
        };
        use fake::{Fake, Faker};
        use item_dynamodb::{item_record::ItemRecord, repository::MockItemDynamoDbRepository};

        #[tokio::test]
        async fn should_return_item_when_exists() {
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_item(&ShopId::new(), &ShopsItemId::new(), &[], &Currency::Eur)
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::eur(Currency::Eur, 2)]
        #[case::gbp(Currency::Gbp, 4)]
        #[case::usd(Currency::Usd, 10)]
        #[case::aud(Currency::Aud, 1000)]
        #[case::cad(Currency::Cad, 4000)]
        #[case::nzd(Currency::Nzd, 42)]
        async fn should_respect_currency(#[case] currency: Currency, #[case] expected_amount: u64) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.price_eur = Some(2);
            expected_record.price_gbp = Some(4);
            expected_record.price_usd = Some(10);
            expected_record.price_aud = Some(1000);
            expected_record.price_cad = Some(4000);
            expected_record.price_nzd = Some(42);
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual_price = service
                .view_item(&ShopId::new(), &ShopsItemId::new(), &[], &currency)
                .await
                .unwrap()
                .price
                .unwrap();
            assert_eq!(currency, actual_price.currency);
            assert_eq!(expected_amount, u64::from(actual_price.monetary_amount));
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], De, "German")]
        #[case(&[De], De, "German")]
        #[case(&[De, En], De, "German")]
        #[case(&[De, Fr], De, "German")]
        #[case(&[Fr, De, En, Es], De, "German")]
        #[case(&[En], En, "English")]
        #[case(&[En, De, Fr, Es], En, "English")]
        #[case(&[En, De, Es], En, "English")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        async fn should_respect_language_for_title(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_title: &str,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.title_native = TextRecord::new("Spanish", LanguageRecord::Es);
            expected_record.title_de = Some("German".to_string());
            expected_record.title_en = Some("English".to_string());
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual_title = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                )
                .await
                .unwrap()
                .title;
            assert_eq!(expected_language, actual_title.localization);
            assert_eq!(expected_title, actual_title.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], Es, "Spanish")]
        #[case(&[De], Es, "Spanish")]
        #[case(&[De, En], Es, "Spanish")]
        #[case(&[De, Fr], Es, "Spanish")]
        #[case(&[Fr, De, En, Es], Es, "Spanish")]
        #[case(&[En], Es, "Spanish")]
        #[case(&[En, De, Fr, Es], Es, "Spanish")]
        #[case(&[En, De, Es], Es, "Spanish")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        async fn should_fallback_to_native_when_only_native_exists_for_title(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_title: &str,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.title_native = TextRecord::new("Spanish", LanguageRecord::Es);
            expected_record.title_de = None;
            expected_record.title_en = None;
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual_title = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                )
                .await
                .unwrap()
                .title;
            assert_eq!(expected_language, actual_title.localization);
            assert_eq!(expected_title, actual_title.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], De, "German")]
        #[case(&[De], De, "German")]
        #[case(&[De, En], De, "German")]
        #[case(&[De, Fr], De, "German")]
        #[case(&[Fr, De, En, Es], De, "German")]
        #[case(&[En], En, "English")]
        #[case(&[En, De, Fr, Es], En, "English")]
        #[case(&[En, De, Es], En, "English")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        async fn should_respect_language_for_description(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_description: &str,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.description_native =
                Some(TextRecord::new("Spanish", LanguageRecord::Es));
            expected_record.description_de = Some("German".to_string());
            expected_record.description_en = Some("English".to_string());
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                )
                .await
                .unwrap()
                .description
                .unwrap();
            assert_eq!(expected_language, actual_description.localization);
            assert_eq!(expected_description, actual_description.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], Es, "Spanish")]
        #[case(&[De], Es, "Spanish")]
        #[case(&[De, En], Es, "Spanish")]
        #[case(&[De, Fr], Es, "Spanish")]
        #[case(&[Fr, De, En, Es], Es, "Spanish")]
        #[case(&[En], Es, "Spanish")]
        #[case(&[En, De, Fr, Es], Es, "Spanish")]
        #[case(&[En, De, Es], Es, "Spanish")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        async fn should_fallback_to_native_when_only_native_exists_for_description(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_description: &str,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.description_native =
                Some(TextRecord::new("Spanish", LanguageRecord::Es));
            expected_record.description_de = None;
            expected_record.description_en = None;
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                )
                .await
                .unwrap()
                .description
                .unwrap();
            assert_eq!(expected_language, actual_description.localization);
            assert_eq!(expected_description, actual_description.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[])]
        #[case(&[De])]
        #[case(&[De, En])]
        #[case(&[De, Fr])]
        #[case(&[Fr, De, En, Es])]
        #[case(&[En])]
        #[case(&[En, De, Fr, Es])]
        #[case(&[En, De, Es])]
        #[case(&[Es, De, En])]
        #[case(&[Es, En, De])]
        async fn should_return_item_without_description_when_none_exists(
            #[case] languages: &[Language],
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            let mut expected_record: ItemRecord = Faker.fake();
            expected_record.description_native = None;
            expected_record.description_de = None;
            expected_record.description_en = None;
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_item(
                    &ShopId::new(),
                    &ShopsItemId::new(),
                    languages,
                    &Currency::Gbp,
                )
                .await
                .unwrap()
                .description;
            assert!(actual_description.is_none());
        }

        #[tokio::test]
        async fn should_return_item_not_found_err_when_item_does_not_exist() {
            let shop_id = ShopId::new();
            let shops_item_id = "non-existent".into();
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_item(&shop_id, &shops_item_id, &[], &Currency::Eur)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetItemError::ItemNotFound(err_shop_id, err_shops_item_id) => {
                    assert_eq!(err_shop_id, shop_id);
                    assert_eq!(err_shops_item_id, shops_item_id);
                }
                _ => panic!("expected GetItemError::ItemNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let shop_id = ShopId::new();
            let shops_item_id = "non-existent".into();
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = QueryItemServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_item(&shop_id, &shops_item_id, &[], &Currency::Eur)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetItemError::SdkGetItemError(_) => {}
                _ => panic!("expected GetItemError::ItemNotFound"),
            }
        }
    }
}

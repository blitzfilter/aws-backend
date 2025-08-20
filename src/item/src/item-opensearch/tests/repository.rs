use common::currency::domain::Currency;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::item_state::domain::ItemState;
use common::language::domain::Language;
use common::page::Page;
use common::price::domain::MonetaryAmount;
use common::shops_item_id::ShopsItemId;
use common::sort::{Sort, SortOrder};
use fake::rand;
use item_core::sort_item_field::SortItemField;
use item_opensearch::item_document::ItemDocument;
use item_opensearch::item_state_document::ItemStateDocument;
use item_opensearch::item_update_document::ItemUpdateDocument;
use item_opensearch::repository::{ItemOpenSearchRepository, ItemOpenSearchRepositoryImpl};
use opensearch::http::Url;
use search_filter_core::array_query::AnyOfQuery;
use search_filter_core::range_query::RangeQuery;
use search_filter_core::search_filter::SearchFilter;
use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::vec;
use test_api::*;
use time::OffsetDateTime;
use time::macros::datetime;

#[localstack_test(services = [OpenSearch()])]
async fn should_create_item_document() {
    let item_id = ItemId::new();
    let expected = ItemDocument {
        item_id,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Listed,
        is_available: false,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(vec![expected.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;
    let actual = read_by_id("items", item_id).await;

    assert_eq!(expected, actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_create_item_documents() {
    let item_id1 = ItemId::new();
    let expected1 = ItemDocument {
        item_id: item_id1,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Listed,
        is_available: false,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let item_id2 = ItemId::new();
    let expected2 = ItemDocument {
        item_id: item_id2,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Listed,
        is_available: false,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(vec![expected1.clone(), expected2.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;
    let actual1 = read_by_id("items", item_id1).await;
    let actual2 = read_by_id("items", item_id2).await;

    assert_eq!(expected1, actual1);
    assert_eq!(expected2, actual2);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_update_item_document() {
    let item_id = ItemId::new();
    let initial = ItemDocument {
        item_id,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Listed,
        is_available: false,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let write_response = repository
        .create_item_documents(vec![initial.clone()])
        .await
        .unwrap();
    assert!(!write_response.errors);
    refresh_index("items").await;

    let updated_event_id = EventId::new();
    let updated_update_ts = OffsetDateTime::now_utc();
    let update = ItemUpdateDocument {
        event_id: updated_event_id,
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: Some(ItemStateDocument::Sold),
        is_available: None,
        updated: updated_update_ts,
    };
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let update_response = repository
        .update_item_documents(HashMap::from([(item_id, update)]))
        .await
        .unwrap();
    assert!(!update_response.errors);
    refresh_index("items").await;

    let mut expected = initial;
    expected.event_id = updated_event_id;
    expected.state = ItemStateDocument::Sold;
    expected.updated = updated_update_ts;

    let actual = read_by_id("items", item_id).await;

    assert_eq!(expected, actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_item_documents() {
    let expected = ItemDocument {
        item_id: Default::default(),
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Hallo Welt".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Available,
        is_available: false,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(vec![expected.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;

    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = SearchFilter {
        item_query: "Hallo Welt".into(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };
    let response = repository
        .search_item_documents(&search_filter, &Language::De, &Currency::Eur, &None, &None)
        .await
        .unwrap();

    assert_eq!(
        vec![expected],
        response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source)
            .collect::<Vec<_>>()
    )
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_item_documents_when_all_arguments_are_given() {
    let items = fake::vec![ItemDocument; 1000];
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = SearchFilter {
        item_query: "Lorem".into(),
        shop_name_query: Some("LLC".into()),
        price_query: Some(RangeQuery {
            min: Some(100u64.into()),
            max: Some(999999u64.into()),
        }),
        state_query: AnyOfQuery(HashSet::from_iter([
            ItemState::Available,
            ItemState::Listed,
        ])),
        created_query: Some(RangeQuery {
            min: Some(datetime!(1000-01-01 0:00 UTC)),
            max: Some(datetime!(3000-01-01 0:00 UTC)),
        }),
        updated_query: Some(RangeQuery {
            min: Some(datetime!(1000-01-01 0:00 UTC)),
            max: Some(datetime!(3000-01-01 0:00 UTC)),
        }),
    };
    let sort = Sort {
        sort: SortItemField::Price,
        order: SortOrder::Asc,
    };
    let page = Page { from: 0, size: 20 };
    let response = repository
        .search_item_documents(
            &search_filter,
            &Language::De,
            &Currency::Eur,
            &Some(sort),
            &Some(page),
        )
        .await;

    assert!(response.is_ok());
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(&[ItemState::Available])]
#[case(&[ItemState::Listed, ItemState::Available])]
#[case(&[ItemState::Reserved, ItemState::Listed, ItemState::Removed])]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_item_documents_when_states_are_given(#[case] states: &[ItemState]) {
    let items = fake::vec![ItemDocument; 3000]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("The same title".into());
            item
        })
        .collect::<Vec<_>>();
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = SearchFilter {
        item_query: "The same title".into(),
        shop_name_query: None,
        price_query: None,
        state_query: AnyOfQuery(HashSet::from_iter(states.iter().copied())),
        created_query: None,
        updated_query: None,
    };
    let response = repository
        .search_item_documents(&search_filter, &Language::De, &Currency::Eur, &None, &None)
        .await
        .unwrap();

    assert!(response.hits.total.value > 0);
    assert!(
        response
            .hits
            .hits
            .iter()
            .all(|hit| { states.contains(&ItemState::from(hit.source.state)) })
    );
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_item_documents_when_no_states_are_given() {
    let items = fake::vec![ItemDocument; 100]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("The same title".into());
            item
        })
        .collect::<Vec<_>>();
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = SearchFilter {
        item_query: "The same title".into(),
        shop_name_query: None,
        price_query: None,
        state_query: AnyOfQuery(HashSet::new()),
        created_query: None,
        updated_query: None,
    };
    let response = repository
        .search_item_documents(&search_filter, &Language::De, &Currency::Eur, &None, &None)
        .await
        .unwrap();

    assert_eq!(100u64, response.hits.total.value);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(RangeQuery { min: Some(0u64.into()), max: Some(999999u64.into()) }, SortOrder::Asc)]
#[case(RangeQuery { min: Some(0u64.into()), max: Some(999999u64.into()) }, SortOrder::Desc)]
#[case(RangeQuery { min: Some(300u64.into()), max: Some(5000u64.into()) }, SortOrder::Asc)]
#[case(RangeQuery { min: Some(500u64.into()), max: None }, SortOrder::Desc)]
#[case(RangeQuery { min: None, max: Some(8888u64.into()) }, SortOrder::Asc)]
#[case(RangeQuery { min: None, max: None }, SortOrder::Asc)]
#[case(RangeQuery { min: None, max: None }, SortOrder::Desc)]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_item_documents_respecting_order_when_price_range_is_given(
    #[case] price_query: RangeQuery<MonetaryAmount>,
    #[case] sort_direction: SortOrder,
) {
    let cheap_items = fake::vec![ItemDocument; 50]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("The same title".into());
            item.price_eur = Some(rand::random_range(150..=1000));
            item
        })
        .collect::<Vec<_>>();
    let expensive_items = fake::vec![ItemDocument; 50]
        .into_iter()
        .map(|mut item| {
            item.title_de = Some("The same title".into());
            item.price_eur = Some(rand::random_range(1500..=20000));
            item
        })
        .collect::<Vec<_>>();
    let items = [cheap_items, expensive_items].concat();
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = SearchFilter {
        item_query: "The same title".into(),
        shop_name_query: None,
        price_query: Some(price_query),
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };
    let response = repository
        .search_item_documents(
            &search_filter,
            &Language::De,
            &Currency::Eur,
            &Some(Sort {
                sort: SortItemField::Price,
                order: sort_direction,
            }),
            &Some(Page { from: 0, size: 100 }),
        )
        .await
        .unwrap();
    let actual_items = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();
    let sorter = |l: &ItemDocument, r: &ItemDocument| match l
        .price_eur
        .unwrap()
        .cmp(&r.price_eur.unwrap())
    {
        std::cmp::Ordering::Equal => l.item_id.to_string().cmp(&r.item_id.to_string()),
        ord => match sort_direction {
            SortOrder::Asc => ord,
            SortOrder::Desc => ord.reverse(),
        },
    };

    let mut expected_items = items
        .into_iter()
        .filter(|item| {
            let mut filter = true;
            if let Some(min) = price_query.min {
                filter = filter && item.price_eur.unwrap() >= *min;
            }
            if let Some(max) = price_query.max {
                filter = filter && item.price_eur.unwrap() <= *max;
            }
            filter
        })
        .collect::<Vec<_>>();
    expected_items.sort_by(sorter);

    assert_eq!(expected_items, actual_items);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(Page { from: 0, size: 10 })]
#[case(Page { from: 500, size: 10 })]
#[case(Page { from: 990, size: 10 })]
#[case(Page { from: 1000, size: 10 })]
#[case(Page { from: 995, size: 10 })]
#[case(Page { from: 0, size: 1000 })]
#[case(Page { from: 0, size: 2000 })]
#[case(Page { from: 5000, size: 10 })]
#[case(Page { from: 0, size: 1 })]
#[case(Page { from: 0, size: 0 })]
#[localstack_test(services = [OpenSearch()])]
async fn should_search_item_documents_respecting_paging_when_sorted_by_price(#[case] page: Page) {
    let items = fake::vec![ItemDocument; 1000]
        .into_iter()
        .map(|mut item| {
            item.title_en = Some("The same title".into());
            item.price_usd = Some(rand::random_range(1500..=20000));
            item
        })
        .collect::<Vec<_>>();
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(items.clone())
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;
    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = SearchFilter {
        item_query: "The same title".into(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };
    let response = repository
        .search_item_documents(
            &search_filter,
            &Language::En,
            &Currency::Usd,
            &Some(Sort {
                sort: SortItemField::Price,
                order: SortOrder::Asc,
            }),
            &Some(page),
        )
        .await
        .unwrap();
    let mut actual_items = response
        .hits
        .hits
        .into_iter()
        .map(|hit| hit.source)
        .collect::<Vec<_>>();
    let sorter = |l: &ItemDocument, r: &ItemDocument| match l
        .price_usd
        .unwrap()
        .cmp(&r.price_usd.unwrap())
    {
        std::cmp::Ordering::Equal => l.item_id.to_string().cmp(&r.item_id.to_string()),
        ord => ord,
    };
    actual_items.sort_by(sorter);

    let mut expected_items = items;
    expected_items.sort_by(sorter);
    let expected_items = expected_items
        .into_iter()
        .skip(page.from as usize)
        .take(page.size as usize)
        .collect::<Vec<_>>();

    assert_eq!(expected_items, actual_items);
}

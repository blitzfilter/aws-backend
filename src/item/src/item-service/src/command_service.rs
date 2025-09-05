use crate::item_command::{CreateItemCommand, UpdateItemCommand};
use async_trait::async_trait;
use common::batch::Batch;
use common::batch::dynamodb::handle_batch_output;
use common::has_key::HasKey;
use common::item_id::ItemKey;
use common::price::domain::FxRate;
use item_core::item::Item;
use item_core::item_event::ItemEvent;
use item_dynamodb::item_event_record::ItemEventRecord;
use item_dynamodb::item_record::ItemRecord;
use item_dynamodb::repository::ItemDynamoDbRepository;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use tracing::{error, info, warn};

/// Service handling inbound item-commands (towards persistence)
#[async_trait]
#[mockall::automock]
pub trait CommandItemService {
    async fn handle_create_items(
        &self,
        commands: Vec<CreateItemCommand>,
    ) -> Result<(), Vec<ItemKey>>;

    async fn handle_update_items(
        &self,
        commands: HashMap<ItemKey, UpdateItemCommand>,
    ) -> Result<(), Vec<ItemKey>>;
}

pub struct CommandItemServiceImpl<'a, T: FxRate + Sync> {
    dynamodb_repository: &'a (dyn ItemDynamoDbRepository + Sync),
    fx_rate: &'a T,
}

impl<'a, T: FxRate + Sync> CommandItemServiceImpl<'a, T> {
    pub fn new(
        dynamodb_repository: &'a (dyn ItemDynamoDbRepository + Sync),
        fx_rate: &'a T,
    ) -> Self {
        Self {
            dynamodb_repository,
            fx_rate,
        }
    }
}

#[async_trait]
impl<T: FxRate + Sync> CommandItemService for CommandItemServiceImpl<'_, T> {
    async fn handle_create_items(
        &self,
        commands: Vec<CreateItemCommand>,
    ) -> Result<(), Vec<ItemKey>> {
        let mut skipped_count = 0;
        let commands_len = commands.len();
        let mut failures = Vec::new();
        let create_chunks = commands
            .into_iter()
            .chunks(100)
            .into_iter()
            .map(|chunk| chunk.collect::<Vec<_>>())
            .collect::<Vec<_>>();

        for chunk in create_chunks {
            self.handle_create_chunk(chunk, &mut failures, &mut skipped_count)
                .await;
        }

        let failures_len = failures.len();
        info!(
            successful = commands_len - failures_len - skipped_count,
            failures = failures_len,
            skipped = skipped_count,
            "Handled multiple CreateItemCommands."
        );
        if failures_len == 0 {
            Ok(())
        } else {
            Err(failures)
        }
    }

    async fn handle_update_items(
        &self,
        commands: HashMap<ItemKey, UpdateItemCommand>,
    ) -> Result<(), Vec<ItemKey>> {
        let commands_len = commands.len();
        let mut skipped_count = 0;
        let mut failures: Vec<ItemKey> = Vec::new();
        let update_chunks = commands
            .into_iter()
            .chunks(100)
            .into_iter()
            .map(|chunk| chunk.collect::<HashMap<_, _>>())
            .collect::<Vec<_>>();
        for update_chunk in update_chunks {
            self.handle_update_chunk(update_chunk, &mut failures, &mut skipped_count)
                .await;
        }

        let failures_len = failures.len();
        info!(
            successful = commands_len - failures_len - skipped_count,
            failures = failures_len,
            skipped = skipped_count,
            "Handled multiple UpdateItemCommands."
        );
        if failures_len == 0 {
            Ok(())
        } else {
            Err(failures)
        }
    }
}

impl<T: FxRate + Sync> CommandItemServiceImpl<'_, T> {
    async fn handle_create_chunk(
        &self,
        create_chunk: Vec<CreateItemCommand>,
        failures: &mut Vec<ItemKey>,
        skipped_count: &mut usize,
    ) {
        let create_item_keys = Batch::try_from(
            create_chunk.iter()
                .map(CreateItemCommand::key)
                .collect::<Vec<_>>()
        ).expect("shouldn't fail creating Batch from Vec because by implementation itertools::chunks(100) produces Vec's of size no more than 100.");

        match self
            .dynamodb_repository
            .exist_item_records(&create_item_keys)
            .await
        {
            Ok(existing_item_keys) => {
                if let Some(unprocessed) = existing_item_keys.unprocessed {
                    warn!(
                        unprocessed = unprocessed.len(),
                        "Failed creating some items because the previously required BatchGetItem-Request contained unprocessed items."
                    );
                    failures.extend(unprocessed);
                }
                let existing_item_keys: HashSet<ItemKey> =
                    HashSet::from_iter(existing_item_keys.items);
                let events = create_chunk.into_iter().filter_map(|cmd| {
                    if existing_item_keys.contains(&cmd.key()) {
                        warn!(
                            shopId = &cmd.key().shop_id.to_string(),
                            shopsItemId = &cmd.key().shops_item_id.to_string(),
                            "Cannot create item because it already exists."
                        );
                        *skipped_count += 1;
                        None
                    } else {
                        let other_price = cmd
                            .price
                            .as_ref()
                            .and_then(|price| {
                                self.fx_rate
                                    .exchange_all(price.currency, price.monetary_amount)
                                    .ok()
                            })
                            .unwrap_or_default();
                        Some(Item::create(
                            cmd.shop_id,
                            cmd.shops_item_id,
                            cmd.shop_name,
                            cmd.native_title,
                            cmd.other_title,
                            cmd.native_description,
                            cmd.other_description,
                            cmd.price,
                            other_price,
                            cmd.state,
                            cmd.url,
                            cmd.images,
                        ))
                    }
                });
                let event_records = events.into_iter().filter_map(|event| {
                    let item_key = event.payload.key();
                    let record_res = ItemEventRecord::try_from(event);
                    match record_res {
                        Ok(record_event) => Some(record_event),
                        Err(err) => {
                            error!(error = %err, "Failed converting ItemEvent to ItemEventRecord.");
                            failures.push(item_key);
                            None
                        }
                    }
                });
                let batches = Batch::<_, 25>::chunked_from(event_records);

                for batch in batches {
                    let item_keys = batch.iter().map(ItemEventRecord::key).collect::<Vec<_>>();
                    let res = self.dynamodb_repository.put_item_event_records(batch).await;
                    match res {
                        Ok(output) => handle_batch_output::<ItemEventRecord>(output, failures),
                        Err(err) => {
                            error!(error = ?err, "Failed writing entire ItemEventRecord-Batch due to SdkError.");
                            failures.extend(item_keys);
                        }
                    }
                }
            }
            Err(err) => {
                error!(error = ?err, "Failed entire BatchGetItem-Operation due to SdkError.");
                failures.extend(create_item_keys);
            }
        }
    }

    async fn handle_update_chunk(
        &self,
        update_chunk: HashMap<ItemKey, UpdateItemCommand>,
        failures: &mut Vec<ItemKey>,
        skipped_count: &mut usize,
    ) {
        let update_item_keys = Batch::try_from(
            update_chunk
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        ).expect("shouldn't fail creating Batch from Vec because by implementation itertools::chunks(100) produces Vec's of size no more than 100.");

        match self
            .dynamodb_repository
            .get_item_records(&update_item_keys)
            .await
        {
            Ok(existing_item_records) => {
                if let Some(unprocessed) = existing_item_records.unprocessed {
                    warn!(
                        unprocessed = unprocessed.len(),
                        "Failed updating some items because the previously required BatchGetItem-Request contained unprocessed items."
                    );
                    failures.extend(unprocessed);
                }
                let events = self.determine_update_events(
                    update_chunk,
                    existing_item_records.items,
                    failures,
                    skipped_count,
                );
                let event_records = events.into_iter().filter_map(|event| {
                    let item_key = event.payload.key();
                    let record_res = ItemEventRecord::try_from(event);
                    match record_res {
                        Ok(record_event) => Some(record_event),
                        Err(err) => {
                            error!(error = %err, "Failed converting ItemEvent to ItemEventRecord.");
                            failures.push(item_key);
                            None
                        }
                    }
                });
                let batches = Batch::<_, 25>::chunked_from(event_records);

                for batch in batches {
                    let item_keys = batch.iter().map(ItemEventRecord::key).collect::<Vec<_>>();
                    let res = self.dynamodb_repository.put_item_event_records(batch).await;
                    match res {
                        Ok(output) => handle_batch_output::<ItemEventRecord>(output, failures),
                        Err(err) => {
                            error!(error = ?err, "Failed writing entire ItemEventRecord-Batch due to SdkError.");
                            failures.extend(item_keys);
                        }
                    }
                }
            }
            Err(err) => {
                error!(error = ?err, "Failed entire BatchGetItem-Operation due to SdkError.");
                failures.extend(update_item_keys);
            }
        }
    }

    fn determine_update_events(
        &self,
        update_chunk: HashMap<ItemKey, UpdateItemCommand>,
        existing_records: Vec<ItemRecord>,
        failures: &mut Vec<ItemKey>,
        skipped_count: &mut usize,
    ) -> Vec<ItemEvent> {
        let mut update_chunk = update_chunk;
        let mut events = Vec::with_capacity(existing_records.len());
        // consumes (remove) all existing items, leaving behind non-existent
        for mut existing_item in existing_records.into_iter().map(Item::from) {
            if let Some((item_key, update)) = update_chunk.remove_entry(&existing_item.key()) {
                let mut any_changes = false;
                if let Some(price_update) = update.price
                    && let Some(price_event) =
                        existing_item.change_price(price_update, self.fx_rate)
                {
                    events.push(price_event);
                    any_changes = true;
                }
                if let Some(state_update) = update.state
                    && let Some(state_event) = existing_item.change_state(state_update)
                {
                    events.push(state_event);
                    any_changes = true;
                }
                if !any_changes {
                    info!(
                        shopId = item_key.shop_id.to_string(),
                        shopsItemId = item_key.shops_item_id.to_string(),
                        "Received Update-Command for item that had no actual changes."
                    );
                    *skipped_count += 1;
                }
            }
        }

        for non_existent_key in update_chunk.into_keys() {
            warn!(
                shopId = non_existent_key.shop_id.to_string(),
                shopsItemId = non_existent_key.shops_item_id.to_string(),
                "Cannot update item because it doesn't exist."
            );
            failures.push(non_existent_key);
        }

        events
    }
}

#[cfg(test)]
pub mod tests {
    use crate::command_service::CommandItemService;
    use crate::item_command::UpdateItemCommand;
    use crate::{command_service::CommandItemServiceImpl, item_command::CreateItemCommand};
    use aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput;
    use common::item_id::ItemKey;
    use common::{batch::dynamodb::BatchGetItemResult, price::domain::FixedFxRate};
    use fake::Fake;
    use fake::Faker;
    use item_dynamodb::item_record::ItemRecord;
    use item_dynamodb::repository::MockItemDynamoDbRepository;
    use itertools::Itertools;

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(42)]
    #[case(85)]
    #[case(100)]
    #[case(169)]
    #[case(266)]
    #[case(491)]
    #[case(1048)]
    #[case(12569)]
    async fn should_handle_create_items(#[case] count: usize) {
        let commands = fake::vec![CreateItemCommand; count];
        let mut repository = MockItemDynamoDbRepository::default();
        repository
            .expect_exist_item_records()
            .returning(move |cmds| {
                let keys = cmds.clone().into();
                Box::pin(async move {
                    let res = BatchGetItemResult {
                        items: keys,
                        unprocessed: None,
                    };
                    Ok(res)
                })
            });
        repository
            .expect_put_item_event_records()
            .returning(|_| Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) }));
        let service = CommandItemServiceImpl::new(&repository, &FixedFxRate());

        let res = service.handle_create_items(commands).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(42)]
    #[case(85)]
    #[case(100)]
    #[case(169)]
    #[case(266)]
    #[case(491)]
    #[case(1048)]
    #[case(12569)]
    async fn should_handle_update_items(#[case] count: usize) {
        let commands = fake::vec![(ItemKey, UpdateItemCommand); count]
            .into_iter()
            .collect();
        let mut repository = MockItemDynamoDbRepository::default();
        repository
            .expect_get_item_records()
            .returning(move |given_keys| {
                let given_keys = given_keys.clone();
                Box::pin(async move {
                    let res = BatchGetItemResult {
                        items: given_keys
                            .iter()
                            .map(|key| {
                                let mut item_record: ItemRecord = Faker.fake();
                                item_record.shop_id = key.shop_id.clone();
                                item_record.shops_item_id = key.shops_item_id.clone();
                                item_record
                            })
                            .collect_vec(),
                        unprocessed: None,
                    };
                    Ok(res)
                })
            });
        repository
            .expect_put_item_event_records()
            .returning(|_| Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) }));
        let service = CommandItemServiceImpl::new(&repository, &FixedFxRate());

        let res = service.handle_update_items(commands).await;
        assert!(res.is_ok());
    }

    mod handle_create_chunk {
        use crate::command_service::CommandItemServiceImpl;
        use crate::item_command::CreateItemCommand;
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::batch_write_item::BatchWriteItemOutput,
        };
        use common::{
            batch::dynamodb::BatchGetItemResult, has_key::HasKey, price::domain::FixedFxRate,
        };
        use item_dynamodb::repository::MockItemDynamoDbRepository;
        use itertools::Itertools;

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"), 100)]
        #[case::timeout(SdkError::timeout_error("Something went wrong"), 78)]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())), 15)]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ), 1)]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ), 5)]
        async fn should_fail_all_for_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
            #[case] batch_size: usize,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_exist_item_records()
                .return_once(|_| Box::pin(async move { Err(expected) }));
            let service = CommandItemServiceImpl::new(&repository, &FixedFxRate());

            let mut failures = vec![];
            let mut skipped_count = 0;
            let _ = service
                .handle_create_chunk(
                    fake::vec![CreateItemCommand; batch_size],
                    &mut failures,
                    &mut skipped_count,
                )
                .await;

            assert_eq!(0, skipped_count);
            assert_eq!(batch_size, failures.len());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(70, 100)]
        #[case(10, 100)]
        #[case(100, 100)]
        #[case(15, 15)]
        #[case(0, 46)]
        #[case(0, 100)]
        #[case(0, 1)]
        #[case(1, 1)]
        async fn should_skip_commands_when_items_with_key_already_exist(
            #[case] existing_count: usize,
            #[case] batch_size: usize,
        ) {
            let existing = fake::vec![CreateItemCommand; existing_count];
            let non_existing = fake::vec![CreateItemCommand; batch_size - existing_count];
            let all = [existing.clone(), non_existing].concat();

            let mut repository = MockItemDynamoDbRepository::default();
            repository.expect_exist_item_records().return_once(|_| {
                Box::pin(async move {
                    let res = BatchGetItemResult {
                        items: existing.iter().map(CreateItemCommand::key).collect(),
                        unprocessed: None,
                    };
                    Ok(res)
                })
            });
            repository.expect_put_item_event_records().returning(|_| {
                Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
            });
            let service = CommandItemServiceImpl::new(&repository, &FixedFxRate());

            let mut failures = vec![];
            let mut skipped_count = 0;
            let _ = service
                .handle_create_chunk(all, &mut failures, &mut skipped_count)
                .await;

            assert_eq!(existing_count, skipped_count);
            assert!(failures.is_empty());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(70, 100)]
        #[case(10, 100)]
        #[case(100, 100)]
        #[case(15, 15)]
        #[case(0, 46)]
        #[case(0, 100)]
        #[case(0, 1)]
        #[case(1, 1)]
        async fn should_fail_unprocessed_commands_from_exists_check(
            #[case] unprocessed_count: usize,
            #[case] batch_size: usize,
        ) {
            let unprocessed = fake::vec![CreateItemCommand; unprocessed_count];
            let processed = fake::vec![CreateItemCommand; batch_size - unprocessed_count];
            let all = [unprocessed.clone(), processed.clone()].concat();

            let expected_failures = unprocessed.iter().map(CreateItemCommand::key).collect_vec();
            let mut repository = MockItemDynamoDbRepository::default();
            repository.expect_exist_item_records().return_once(|_| {
                Box::pin(async move {
                    let res = BatchGetItemResult {
                        items: vec![],
                        unprocessed: unprocessed
                            .iter()
                            .map(CreateItemCommand::key)
                            .collect_vec()
                            .try_into()
                            .ok(),
                    };
                    Ok(res)
                })
            });
            repository.expect_put_item_event_records().returning(|_| {
                Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
            });
            let service = CommandItemServiceImpl::new(&repository, &FixedFxRate());

            let mut failures = vec![];
            let mut skipped_count = 0;
            let _ = service
                .handle_create_chunk(all, &mut failures, &mut skipped_count)
                .await;

            assert_eq!(unprocessed_count, failures.len());
            assert!(expected_failures.iter().all(|key| failures.contains(key)));
            assert_eq!(0, skipped_count);
        }
    }

    mod handle_update_chunk {
        use crate::{command_service::CommandItemServiceImpl, item_command::UpdateItemCommand};
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::batch_write_item::BatchWriteItemOutput,
        };
        use common::item_id::ItemKey;
        use common::item_state::domain::ItemState;
        use common::{batch::dynamodb::BatchGetItemResult, price::domain::FixedFxRate};
        use fake::{Fake, Faker};
        use item_dynamodb::item_record::ItemRecord;
        use item_dynamodb::item_state_record::ItemStateRecord;
        use item_dynamodb::repository::MockItemDynamoDbRepository;
        use itertools::Itertools;

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"), 100)]
        #[case::timeout(SdkError::timeout_error("Something went wrong"), 78)]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())), 15)]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ), 1)]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ), 5)]
        async fn should_fail_all_for_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
            #[case] batch_size: usize,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_records()
                .return_once(|_| Box::pin(async move { Err(expected) }));
            let service = CommandItemServiceImpl::new(&repository, &FixedFxRate());

            let mut failures = vec![];
            let mut skipped_count = 0;
            let _ = service
                .handle_update_chunk(
                    fake::vec![(ItemKey, UpdateItemCommand); batch_size]
                        .into_iter()
                        .collect(),
                    &mut failures,
                    &mut skipped_count,
                )
                .await;

            assert_eq!(0, skipped_count);
            assert_eq!(batch_size, failures.len());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(70, 100)]
        #[case(10, 100)]
        #[case(100, 100)]
        #[case(15, 15)]
        #[case(0, 46)]
        #[case(0, 100)]
        #[case(0, 1)]
        #[case(1, 1)]
        async fn should_fail_commands_when_items_with_key_do_not_exist(
            #[case] existing_count: usize,
            #[case] batch_size: usize,
        ) {
            let mut repository = MockItemDynamoDbRepository::default();
            repository
                .expect_get_item_records()
                .return_once(move |given_keys| {
                    let given_keys = given_keys.clone();
                    Box::pin(async move {
                        let res = BatchGetItemResult {
                            items: given_keys
                                .iter()
                                .map(|key| {
                                    let mut item_record: ItemRecord = Faker.fake();
                                    item_record.shop_id = key.shop_id.clone();
                                    item_record.shops_item_id = key.shops_item_id.clone();
                                    item_record.state = ItemStateRecord::Reserved;
                                    item_record
                                })
                                .take(existing_count)
                                .collect_vec(),
                            unprocessed: None,
                        };
                        Ok(res)
                    })
                });
            repository.expect_put_item_event_records().returning(|_| {
                Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
            });
            let service = CommandItemServiceImpl::new(&repository, &FixedFxRate());

            let mut failures = vec![];
            let mut skipped_count = 0;
            let _ = service
                .handle_update_chunk(
                    fake::vec![(ItemKey, UpdateItemCommand); batch_size]
                        .into_iter()
                        .map(|(key, mut cmd)| {
                            cmd.state = Some(ItemState::Available);
                            (key, cmd)
                        })
                        .collect(),
                    &mut failures,
                    &mut skipped_count,
                )
                .await;

            assert_eq!(batch_size - existing_count, failures.len());
        }
    }

    mod find_update_events {
        use crate::command_service::CommandItemServiceImpl;
        use crate::item_command::UpdateItemCommand;
        use aws_sdk_dynamodb::{Client, Config};
        use common::currency::domain::Currency;
        use common::item_id::ItemKey;
        use common::item_state::domain::ItemState;
        use common::language::record::{LanguageRecord, TextRecord};
        use common::price::domain::{FixedFxRate, Price};
        use common::shops_item_id::ShopsItemId;
        use item_core::hash::ItemHash;
        use item_core::item_event::ItemCommonEventPayload;
        use item_dynamodb::item_record::ItemRecord;
        use item_dynamodb::item_state_record::ItemStateRecord;
        use item_dynamodb::repository::ItemDynamoDbRepositoryImpl;
        use std::collections::HashMap;
        use time::OffsetDateTime;
        use url::Url;

        #[test]
        fn should_determine_update_events() {
            let update_chunk = HashMap::from([
                (
                    ItemKey::new("123".into(), "abc".into()),
                    UpdateItemCommand {
                        price: None,
                        state: Some(ItemState::Sold),
                    },
                ),
                (
                    ItemKey::new("456".into(), "def".into()),
                    UpdateItemCommand {
                        price: Some(Price {
                            monetary_amount: 42u64.into(),
                            currency: Currency::Eur,
                        }),
                        state: Some(ItemState::Available),
                    },
                ),
            ]);
            let existing_records = vec![ItemRecord {
                pk: "".to_string(),
                sk: "".to_string(),
                gsi_1_pk: "".to_string(),
                gsi_1_sk: "".to_string(),
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: "123".into(),
                shops_item_id: "abc".into(),
                shop_name: "".to_string(),
                title_native: TextRecord::new("boop", LanguageRecord::De),
                title_de: None,
                title_en: None,
                description_native: None,
                description_de: None,
                description_en: None,
                price_native: None,
                price_eur: None,
                price_usd: None,
                price_gbp: None,
                price_aud: None,
                price_cad: None,
                price_nzd: None,
                state: ItemStateRecord::Listed,
                url: Url::parse("https://beep.bap").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &ItemState::Listed),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }];

            let mut failures: Vec<ItemKey> = vec![];
            let mut skipped_count = 0;
            let client = &Client::from_conf(Config::builder().behavior_version_latest().build());
            let service = CommandItemServiceImpl {
                dynamodb_repository: &ItemDynamoDbRepositoryImpl::new(client, "table_1"),
                fx_rate: &FixedFxRate::default(),
            };
            let actuals = service.determine_update_events(
                update_chunk,
                existing_records,
                &mut failures,
                &mut skipped_count,
            );

            assert_eq!(actuals.len(), 1);
            assert_eq!(
                actuals[0].payload.shops_item_id(),
                &ShopsItemId::from("abc")
            );
            assert_eq!(1, failures.len());
            assert_eq!(0, skipped_count);
        }

        #[test]
        fn should_determine_no_update_events_when_no_items_exist() {
            let update_chunk = HashMap::from([
                (
                    ItemKey::new("123".into(), "abc".into()),
                    UpdateItemCommand {
                        price: None,
                        state: Some(ItemState::Sold),
                    },
                ),
                (
                    ItemKey::new("456".into(), "def".into()),
                    UpdateItemCommand {
                        price: Some(Price {
                            monetary_amount: 42u64.into(),
                            currency: Currency::Eur,
                        }),
                        state: Some(ItemState::Available),
                    },
                ),
            ]);

            let mut failures: Vec<ItemKey> = vec![];
            let mut skipped_count = 0;
            let client = &Client::from_conf(Config::builder().behavior_version_latest().build());
            let service = CommandItemServiceImpl {
                dynamodb_repository: &ItemDynamoDbRepositoryImpl::new(client, "table_1"),
                fx_rate: &FixedFxRate::default(),
            };
            let actuals = service.determine_update_events(
                update_chunk,
                vec![],
                &mut failures,
                &mut skipped_count,
            );

            assert!(actuals.is_empty());
            assert_eq!(2, failures.len());
            assert_eq!(0, skipped_count);
        }

        #[test]
        fn should_find_no_update_events_when_no_actual_changes() {
            let update_chunk = HashMap::from([
                (
                    ItemKey::new("123".into(), "abc".into()),
                    UpdateItemCommand {
                        price: None,
                        state: Some(ItemState::Listed),
                    },
                ),
                (
                    ItemKey::new("456".into(), "def".into()),
                    UpdateItemCommand {
                        price: Some(Price {
                            monetary_amount: 42u64.into(),
                            currency: Currency::Eur,
                        }),
                        state: Some(ItemState::Available),
                    },
                ),
            ]);
            let existing_records = vec![ItemRecord {
                pk: "".to_string(),
                sk: "".to_string(),
                gsi_1_pk: "".to_string(),
                gsi_1_sk: "".to_string(),
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: "123".into(),
                shops_item_id: "abc".into(),
                shop_name: "".to_string(),
                title_native: TextRecord::new("boop", LanguageRecord::De),
                title_de: None,
                title_en: None,
                description_native: None,
                description_de: None,
                description_en: None,
                price_native: None,
                price_eur: None,
                price_usd: None,
                price_gbp: None,
                price_aud: None,
                price_cad: None,
                price_nzd: None,
                state: ItemStateRecord::Listed,
                url: Url::parse("https://beep.bap").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &ItemState::Listed),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }];

            let mut failures: Vec<ItemKey> = vec![];
            let mut skipped_count = 0;
            let client = &Client::from_conf(Config::builder().behavior_version_latest().build());
            let service = CommandItemServiceImpl {
                dynamodb_repository: &ItemDynamoDbRepositoryImpl::new(client, "table_1"),
                fx_rate: &FixedFxRate::default(),
            };
            let actuals = service.determine_update_events(
                update_chunk,
                existing_records,
                &mut failures,
                &mut skipped_count,
            );

            assert!(actuals.is_empty());
            assert_eq!(1, failures.len());
            assert_eq!(1, skipped_count);
        }
    }
}

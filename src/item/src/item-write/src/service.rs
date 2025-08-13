use crate::repository::PersistItemRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput;
use common::batch::Batch;
use common::has::HasKey;
use common::item_id::ItemKey;
use common::price::domain::FxRate;
use item_core::item::command::{CreateItemCommand, UpdateItemCommand};
use item_core::item::domain::Item;
use item_core::item::record::ItemRecord;
use item_core::item_event::domain::ItemEvent;
use item_core::item_event::record::ItemEventRecord;
use item_read::repository::QueryItemRepository;
use itertools::Itertools;
use serde::Deserialize;
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
    dynamodb_read_repository: &'a (dyn QueryItemRepository + Sync),
    dynamodb_write_repository: &'a (dyn PersistItemRepository + Sync),
    fx_rate: &'a T,
}

impl<'a, T: FxRate + Sync> CommandItemServiceImpl<'a, T> {
    pub fn new(
        dynamodb_read_repository: &'a (dyn QueryItemRepository + Sync),
        dynamodb_write_repository: &'a (dyn PersistItemRepository + Sync),
        fx_rate: &'a T,
    ) -> Self {
        Self {
            dynamodb_read_repository,
            dynamodb_write_repository,
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
            .dynamodb_read_repository
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
                    let res = self
                        .dynamodb_write_repository
                        .put_item_event_records(batch)
                        .await;
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
            .dynamodb_read_repository
            .get_item_records(&update_item_keys)
            .await
        {
            Ok(existing_item_keys) => {
                if let Some(unprocessed) = existing_item_keys.unprocessed {
                    warn!(
                        unprocessed = unprocessed.len(),
                        "Failed updating some items because the previously required BatchGetItem-Request contained unprocessed items."
                    );
                    failures.extend(unprocessed);
                }
                let events = self.find_update_events_with_existing_items(
                    update_chunk,
                    existing_item_keys.items,
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
                    let res = self
                        .dynamodb_write_repository
                        .put_item_event_records(batch)
                        .await;
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

    fn find_update_events_with_existing_items(
        &self,
        update_chunk: HashMap<ItemKey, UpdateItemCommand>,
        existing_records: Vec<ItemRecord>,
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

        for non_existent_key in update_chunk.keys() {
            warn!(
                shopId = non_existent_key.shop_id.to_string(),
                shopsItemId = non_existent_key.shops_item_id.to_string(),
                "Cannot update item because it doesn't exist."
            );
        }
        *skipped_count += update_chunk.len();

        events
    }
}

pub fn handle_batch_output<T>(output: BatchWriteItemOutput, failures: &mut Vec<T::Key>)
where
    T: HasKey + for<'de> Deserialize<'de>,
{
    let unprocessed = output
        .unprocessed_items
        .unwrap_or_default()
        .into_iter()
        .next()
        .map(|(_, unprocessed)| unprocessed)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|write_req| write_req.put_request)
        .map(|put_req| put_req.item)
        .filter_map(|ddb_item| {
            let record_res = serde_dynamo::from_item::<_, T>(ddb_item);
            match record_res {
                Ok(record_event) => Some(record_event),
                Err(err) => {
                    error!(
                        error = %err,
                        type = %std::any::type_name::<T>(),
                        "Failed converting DynamoDB-JSON to target-type from failed BatchWriteItemOutput."
                    );
                    None
                }
            }
        })
        .map(|t| t.key());

    failures.extend(unprocessed);
}

#[cfg(test)]
pub mod tests {
    use crate::repository::PersistItemRepositoryImpl;
    use crate::service::CommandItemServiceImpl;
    use aws_sdk_dynamodb::{Client, Config};
    use common::currency::domain::Currency;
    use common::item_id::ItemKey;
    use common::language::record::{LanguageRecord, TextRecord};
    use common::price::domain::{FixedFxRate, Price};
    use common::shops_item_id::ShopsItemId;
    use item_core::item::command::UpdateItemCommand;
    use item_core::item::hash::ItemHash;
    use item_core::item::record::ItemRecord;
    use item_core::item_event::domain::ItemCommonEventPayload;
    use item_core::item_state::domain::ItemState;
    use item_core::item_state::record::ItemStateRecord;
    use item_read::repository::QueryItemRepositoryImpl;
    use std::collections::HashMap;
    use time::OffsetDateTime;
    use url::Url;

    #[test]
    fn should_find_update_events_with_existing_items() {
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

        let mut skipped_count = 0;
        let client = &Client::from_conf(Config::builder().behavior_version_latest().build());
        let service = CommandItemServiceImpl {
            dynamodb_read_repository: &QueryItemRepositoryImpl::new(client),
            dynamodb_write_repository: &PersistItemRepositoryImpl::new(client),
            fx_rate: &FixedFxRate::default(),
        };
        let actuals = service.find_update_events_with_existing_items(
            update_chunk,
            existing_records,
            &mut skipped_count,
        );

        assert_eq!(actuals.len(), 1);
        assert_eq!(
            actuals[0].payload.shops_item_id(),
            &ShopsItemId::from("abc")
        );
        assert_eq!(1, skipped_count);
    }

    #[test]
    fn should_find_no_update_events_when_no_items_exist() {
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

        let mut skipped_count = 0;
        let client = &Client::from_conf(Config::builder().behavior_version_latest().build());
        let service = CommandItemServiceImpl {
            dynamodb_read_repository: &QueryItemRepositoryImpl::new(client),
            dynamodb_write_repository: &PersistItemRepositoryImpl::new(client),
            fx_rate: &FixedFxRate::default(),
        };
        let actuals = service.find_update_events_with_existing_items(
            update_chunk,
            vec![],
            &mut skipped_count,
        );

        assert!(actuals.is_empty());
        assert_eq!(2, skipped_count);
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

        let mut skipped_count = 0;
        let client = &Client::from_conf(Config::builder().behavior_version_latest().build());
        let service = CommandItemServiceImpl {
            dynamodb_read_repository: &QueryItemRepositoryImpl::new(client),
            dynamodb_write_repository: &PersistItemRepositoryImpl::new(client),
            fx_rate: &FixedFxRate::default(),
        };
        let actuals = service.find_update_events_with_existing_items(
            update_chunk,
            existing_records,
            &mut skipped_count,
        );

        assert!(actuals.is_empty());
        assert_eq!(2, skipped_count);
    }
}

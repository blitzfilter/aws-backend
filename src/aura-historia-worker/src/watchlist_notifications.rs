use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use product_listing_service::use_cases::{
    GenerateWatchlistNotificationsCommand, GenerateWatchlistNotificationsResult,
    GenerateWatchlistNotificationsUseCase,
};
use std::sync::Arc;

pub async fn consume_watchlist_notification_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    handler: Arc<dyn GenerateWatchlistNotificationsUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::WatchlistNotification, move |job| {
            generate_watchlist_notifications(handler.clone(), job)
        })
        .await;
}
async fn generate_watchlist_notifications(
    handler: Arc<dyn GenerateWatchlistNotificationsUseCase>,
    job: DomainJob,
) -> JobOutcome {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return JobOutcome::Invalid("unexpected_payload");
    };
    // The service loads the exact historical event and locks current lifecycle through commit.
    // No transport cache/current-event comparison may suppress a later historical notification.
    match handler
        .execute(GenerateWatchlistNotificationsCommand {
            event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        })
        .await
    {
        Ok(result) => watchlist_outcome(result),
        Err(_) => JobOutcome::DependencyUnavailable("watchlist_notification_unavailable"),
    }
}
fn watchlist_outcome(result: GenerateWatchlistNotificationsResult) -> JobOutcome {
    match result {
        GenerateWatchlistNotificationsResult::Applied {
            recipient_count,
            inserted_count,
            already_exists_count,
        } => {
            tracing::info!(
                recipient_count,
                inserted_count,
                already_exists_count,
                "historical watchlist notifications committed"
            );
            JobOutcome::Complete(if inserted_count == 0 && already_exists_count > 0 {
                "duplicate"
            } else {
                "applied"
            })
        }
        GenerateWatchlistNotificationsResult::SuppressedForMissingSource => {
            JobOutcome::Retry("missing_source")
        }
        GenerateWatchlistNotificationsResult::IgnoredEvent => JobOutcome::Complete("ignored_event"),
        GenerateWatchlistNotificationsResult::SuppressedForWithdrawnProductListing => {
            JobOutcome::Complete("withdrawn")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        QueueConfig,
        cdc::{IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue},
        in_memory_queue,
    };
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::ProductListingId;
    use std::sync::Mutex;
    struct Handler(Mutex<Vec<GenerateWatchlistNotificationsCommand>>);
    #[async_trait::async_trait]
    impl GenerateWatchlistNotificationsUseCase for Handler {
        async fn execute(
            &self,
            command: GenerateWatchlistNotificationsCommand,
        ) -> Result<
            GenerateWatchlistNotificationsResult,
            product_listing_service::use_cases::GenerateWatchlistNotificationsError,
        > {
            self.0.lock().unwrap().push(command);
            Ok(GenerateWatchlistNotificationsResult::IgnoredEvent)
        }
    }
    #[tokio::test]
    async fn should_map_product_event_job_to_watchlist_notification_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = in_memory_queue(QueueConfig::new(2))?;
        let event_id = EventId::new();
        let product_listing_id = ProductListingId::new();
        let job = DomainJob {
            target_queue: WorkerQueue::WatchlistNotification,
            idempotency_key: IdempotencyKey::new(format!("product-event:{event_id}")),
            ordering_key: OrderingKey::new(format!("product:{product_listing_id}")),
            payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id,
                product_listing_id,
            }),
        };
        sender.enqueue(job.clone()).await?;
        sender.enqueue(job).await?;
        drop(sender);
        let handler = Arc::new(Handler(Mutex::new(vec![])));
        consume_watchlist_notification_queue(receiver, handler.clone()).await;
        assert_eq!(
            vec![
                GenerateWatchlistNotificationsCommand {
                    event_id,
                    product_listing_id
                };
                2
            ],
            *handler.0.lock().unwrap()
        );
        Ok(())
    }
    #[test]
    fn should_retain_missing_source_and_complete_verified_historical_outcomes() {
        assert_eq!(
            JobOutcome::Retry("missing_source"),
            watchlist_outcome(GenerateWatchlistNotificationsResult::SuppressedForMissingSource)
        );
        for result in [
            GenerateWatchlistNotificationsResult::IgnoredEvent,
            GenerateWatchlistNotificationsResult::SuppressedForWithdrawnProductListing,
            GenerateWatchlistNotificationsResult::Applied {
                recipient_count: 1,
                inserted_count: 1,
                already_exists_count: 0,
            },
            GenerateWatchlistNotificationsResult::Applied {
                recipient_count: 1,
                inserted_count: 0,
                already_exists_count: 1,
            },
        ] {
            assert!(matches!(watchlist_outcome(result), JobOutcome::Complete(_)));
        }
    }
}

//! #1558: real executable + committed source -> Sequin -> child HTTP -> real SQS/Postgres.
//! Crash boundaries are external DB/HTTP barriers, never sleeps or production fault hooks.
mod process_support;

use process_support::*;
use test_api::{
    IntegrationTestService, Postgres, Sequin, aura_integration_test, get_postgres_client,
};

const POSTGRES: Postgres = Postgres::new("migrations");
const SEQUIN: Sequin = Sequin::worker_webhook_for_tables(&["public.product_listing_events"]);

#[aura_integration_test(services = [POSTGRES, WORKER_SQS, SEQUIN])]
async fn should_persist_accepted_work_after_process_dies_before_handler_commit_t07() {
    case(async {
        let pool = get_postgres_client().await;
        let observations = Observations::new();
        let sqs = Relay::sqs("primary", observations.clone(), false).await?;
        let address = unused_address()?;
        let webhook = Relay::webhook(address, observations.clone()).await?;
        let mut child = WorkerProcess::start(&pool, &sqs, address).await?;

        // This table isn't a source CDC table. Source commit and HTTP publication remain free,
        // while the real handler cannot read/write its target or finish its transaction.
        let mut barrier = pool.begin().await?;
        sqlx::query("LOCK TABLE product_listing_content_assessments IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *barrier)
            .await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let source = commit_source(&pool).await?;
        let (body, message_id) = observations.publication(source.event_id).await?;
        let original = observations.received(&message_id, 1).await?;
        assert_eq!(body, original.body);
        wait_for_blocked_handler(&pool, blocker_pid).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM product_listing_content_assessments")
                .fetch_one(&mut *barrier)
                .await?;
        assert_eq!(0, count, "202/publication must precede handler commit");
        let original_pid = child.id();
        child.kill()?;
        barrier.commit().await?;
        assert_eq!(None, assessment(&pool, source).await?);
        assert_queue_counts(1, 0).await?;

        let mut restarted = WorkerProcess::start(&pool, &sqs, address).await?;
        assert_ne!(original_pid, restarted.id());
        // Let the original production 60s visibility expire naturally: no republish/receive
        // by the test and no recreation of the source queue, DLQ, DB, or Sequin containers.
        let redelivery = observations.received(&message_id, 2).await?;
        assert_eq!(body, redelivery.body);
        assert!(
            original.handle != redelivery.handle,
            "restart must get a fresh real receipt"
        );
        persisted_assessment(&pool, source).await?;
        observations.deleted(&redelivery).await?;
        assert_eq!(1, assessment_count(&pool).await?);
        assert_queue_counts(0, 0).await?;
        restarted.kill()?;
        webhook.stop().await?;
        sqs.stop().await?;
        Ok(())
    })
    .await;
}

#[aura_integration_test(services = [POSTGRES, WORKER_SQS, SEQUIN])]
async fn should_preserve_committed_result_after_process_dies_before_sqs_delete_t08() {
    case(async {
        let pool = get_postgres_client().await;
        let observations = Observations::new();
        let sqs = Relay::sqs("primary", observations.clone(), true).await?;
        let address = unused_address()?;
        let webhook = Relay::webhook(address, observations.clone()).await?;
        let mut child = WorkerProcess::start(&pool, &sqs, address).await?;
        let source = commit_source(&pool).await?;
        let (body, message_id) = observations.publication(source.event_id).await?;
        let original = observations.received(&message_id, 1).await?;
        observations.delete_held(&original).await?;
        let committed = persisted_assessment(&pool, source).await?;
        let original_pid = child.id();
        child.kill()?;
        assert_queue_counts(1, 0).await?;
        assert_eq!(Some(committed.clone()), assessment(&pool, source).await?);

        sqs.allow_new_deletes();
        let mut restarted = WorkerProcess::start(&pool, &sqs, address).await?;
        assert_ne!(original_pid, restarted.id());
        let redelivery = observations.received(&message_id, 2).await?;
        assert_eq!(body, redelivery.body);
        assert!(original.handle != redelivery.handle);
        observations.deleted(&redelivery).await?;
        // Full target row, timestamps and xmin must survive: duplicate is a semantic no-op,
        // not an overwrite with equivalent values or an in-process deduplication cache.
        assert_eq!(Some(committed), assessment(&pool, source).await?);
        assert_eq!(1, assessment_count(&pool).await?);
        assert_queue_counts(0, 0).await?;
        restarted.kill()?;
        webhook.stop().await?;
        sqs.stop().await?;
        Ok(())
    })
    .await;
}

#[aura_integration_test(services = [POSTGRES, WORKER_SQS, SEQUIN])]
async fn should_preserve_one_result_when_two_os_consumers_receive_duplicate_work_t09() {
    case(async {
        let pool = get_postgres_client().await;
        let observations = Observations::new();
        let first_sqs = Relay::sqs("first", observations.clone(), false).await?;
        let address = unused_address()?;
        let webhook = Relay::webhook(address, observations.clone()).await?;
        let mut first = WorkerProcess::start(&pool, &first_sqs, address).await?;
        let mut barrier = pool.begin().await?;
        sqlx::query("LOCK TABLE product_listing_content_assessments IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *barrier)
            .await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let source = commit_source(&pool).await?;
        let (body, first_message_id) = observations.publication(source.event_id).await?;
        let first_receipt = observations.received(&first_message_id, 1).await?;
        let first_transactions = wait_for_blocked_handlers(&pool, blocker_pid, 1).await?;
        let (first_backend, first_xid) =
            first_transactions.first().ok_or("first handler missing")?;
        let first_xid = first_xid
            .as_ref()
            .ok_or("first writer has no transaction ID")?;

        // A has not committed. B receives a distinct publication of the exact Sequin batch
        // while A holds the product row and waits at the assessment write barrier.
        let second_sqs = Relay::sqs("second", observations.clone(), false).await?;
        let mut second = WorkerProcess::start(&pool, &second_sqs, unused_address()?).await?;
        assert_ne!(first.id(), second.id());
        let batch = observations.accepted_batch(source.event_id).await?;
        post_batch(address, &batch).await?;
        let publications = observations.publications(source.event_id, 2).await?;
        assert_eq!(2, publications.len());
        let (_, second_message_id) = publications
            .iter()
            .find(|(_, id)| id != &first_message_id)
            .ok_or("duplicate must have a distinct real SQS message ID")?;
        let second_receipt = observations.received(second_message_id, 1).await?;
        assert_eq!("first", first_receipt.consumer);
        assert_eq!("second", second_receipt.consumer);
        assert_eq!(body, second_receipt.body);
        let overlapping = wait_for_blocked_handlers(&pool, blocker_pid, 2).await?;
        assert!(overlapping.iter().any(|(pid, _)| pid == first_backend));
        assert_ne!(overlapping[0].0, overlapping[1].0);
        first.assert_running()?;
        second.assert_running()?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM product_listing_content_assessments")
                .fetch_one(&mut *barrier)
                .await?;
        assert_eq!(
            0, count,
            "both handlers must overlap before any output commits"
        );
        barrier.commit().await?;

        observations.deleted(&first_receipt).await?;
        observations.deleted(&second_receipt).await?;
        let committed = persisted_assessment(&pool, source).await?;
        assert_eq!(
            first_xid.as_str(),
            committed["tuple_version"],
            "B must not rewrite A's row"
        );
        assert_eq!(1, assessment_count(&pool).await?);
        assert_one_source_event(&pool, source).await?;
        assert_queue_counts(0, 0).await?;
        first.kill()?;
        second.kill()?;
        webhook.stop().await?;
        first_sqs.stop().await?;
        second_sqs.stop().await?;
        Ok(())
    })
    .await;
}

#[aura_integration_test(services = [POSTGRES, WORKER_SQS, SEQUIN])]
async fn should_keep_native_poison_dlq_across_os_restart_and_process_unrelated_work_t13() {
    case(async {
        let pool = get_postgres_client().await;
        let observations = Observations::new();
        let sqs = Relay::sqs("primary", observations.clone(), false).await?;
        let address = unused_address()?;
        let webhook = Relay::webhook(address, observations.clone()).await?;
        let mut child = WorkerProcess::start(&pool, &sqs, address).await?;
        assert_native_redrive_policy().await?;
        let event_id = uuid::Uuid::new_v4();
        let product_id = uuid::Uuid::new_v4();
        let poison = serde_json::json!({
            "schema_version": 999,
            "scope": "product-content-assessment",
            "idempotency_key": format!("product-event:{event_id}"),
            "ordering_key": format!("product:{product_id}"),
            "job_type": "PRODUCT_LISTING_EVENT",
            "payload": {"event_id": event_id, "product_listing_id": product_id}
        })
        .to_string();
        let poison_message_id = send(&poison).await?;
        let mut poison_receipts = Vec::new();
        for count in 1..=5 {
            let receipt = observations.received(&poison_message_id, count).await?;
            assert_eq!(poison, receipt.body);
            let retry_seconds = observations.retry_settled(&receipt).await?;
            assert!(
                retry_seconds >= 30 * (1 << (count - 1)),
                "production exponential retry unchanged"
            );
            // Accelerate only an already-confirmed real receipt after the worker has applied
            // its actual backoff. Native SQS retains ownership of receive history/redrive.
            make_visible(&receipt).await?;
            poison_receipts.push(receipt);
        }
        let dead = dlq_message().await?;
        assert_eq!(Some(poison.as_str()), dead.body());
        assert_eq!(Some(poison_message_id.as_str()), dead.message_id());
        observations.assert_never_deleted(&poison_receipts);
        assert_eq!(0, assessment_count(&pool).await?);
        assert_queue_counts(0, 1).await?;
        let original_pid = child.id();
        child.kill()?;

        let mut restarted = WorkerProcess::start(&pool, &sqs, address).await?;
        assert_ne!(original_pid, restarted.id());
        let still_dead = dlq_message().await?;
        assert_eq!(dead.body(), still_dead.body());
        assert_eq!(dead.message_id(), still_dead.message_id());
        assert_native_redrive_policy().await?;
        let healthy_source = commit_source(&pool).await?;
        let (_, healthy_message_id) = observations.publication(healthy_source.event_id).await?;
        persisted_assessment(&pool, healthy_source).await?;
        observations.completed(&healthy_message_id).await?;
        assert_eq!(1, assessment_count(&pool).await?);
        assert_queue_counts(0, 1).await?;
        let retained = dlq_message().await?;
        assert_eq!(Some(poison.as_str()), retained.body());
        assert_eq!(Some(poison_message_id.as_str()), retained.message_id());
        observations.assert_never_deleted(&poison_receipts);
        restarted.kill()?;
        webhook.stop().await?;
        sqs.stop().await?;
        Ok(())
    })
    .await;
}

#[aura_integration_test(services = [POSTGRES, WORKER_SQS, SEQUIN])]
async fn should_retry_same_sequin_batch_when_real_sqs_send_response_is_lost() {
    case(async {
        let pool = get_postgres_client().await;
        let observations = Observations::new();
        let sqs = Relay::sqs("primary", observations.clone(), false).await?;
        let loss = sqs.lose_response_once("SendMessage");
        let address = unused_address()?;
        let webhook = Relay::webhook(address, observations.clone()).await?;
        let mut child = WorkerProcess::start(&pool, &sqs, address).await?;
        let source = commit_source(&pool).await?;
        let invocation = observations.response_withheld("SendMessage").await?;
        // LocalStack really accepted and delivered the first publication even though the
        // HTTP ingress cannot confirm it. Observe its durable result before dropping the reply.
        let committed = persisted_assessment(&pool, source).await?;
        loss.release();
        let (rejected_batch, status) = observations.rejected_batch(source.event_id).await?;
        assert_eq!(reqwest::StatusCode::SERVICE_UNAVAILABLE, status);
        let accepted_batch = observations.accepted_batch(source.event_id).await?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rejected_batch)?,
            serde_json::from_str::<serde_json::Value>(&accepted_batch)?,
            "real Sequin must retry the same entire batch; test does not resubmit it",
        );
        let publications = observations.publications(source.event_id, 2).await?;
        assert_eq!(2, publications.len());
        assert_ne!(publications[0].1, publications[1].1);
        assert_eq!(publications[0].0, publications[1].0);
        for (body, message_id) in &publications {
            let receipt = observations.received(message_id, 1).await?;
            assert_eq!(body, &receipt.body);
            observations.deleted(&receipt).await?;
        }
        child.terminate()?;
        child.wait_for_clean_exit().await?;
        observations.assert_response_lost_once(&invocation);
        assert_eq!(Some(committed), assessment(&pool, source).await?);
        assert_eq!(1, assessment_count(&pool).await?);
        assert_one_source_event(&pool, source).await?;
        assert_queue_counts(0, 0).await?;
        webhook.stop().await?;
        sqs.stop().await?;
        Ok(())
    })
    .await;
}

#[aura_integration_test(services = [POSTGRES, WORKER_SQS, SEQUIN])]
async fn should_retry_ack_without_rerunning_handler_when_real_delete_response_is_lost() {
    case(async {
        let pool = get_postgres_client().await;
        let observations = Observations::new();
        let sqs = Relay::sqs("primary", observations.clone(), false).await?;
        let loss = sqs.lose_response_once("DeleteMessage");
        let address = unused_address()?;
        let webhook = Relay::webhook(address, observations.clone()).await?;
        let mut child = WorkerProcess::start(&pool, &sqs, address).await?;
        let source = commit_source(&pool).await?;
        let (_, message_id) = observations.publication(source.event_id).await?;
        let receipt = observations.received(&message_id, 1).await?;
        let lost_invocation = observations.response_withheld("DeleteMessage").await?;
        let committed = persisted_assessment(&pool, source).await?;
        assert_queue_counts(0, 0).await?;

        // Any handler re-execution would block at its first authoritative source read.
        // Keep this lock through successful acknowledgment retry AND clean process exit.
        let mut no_rerun = pool.begin().await?;
        sqlx::query("LOCK TABLE product_listings IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *no_rerun)
            .await?;
        loss.release();
        let replied_invocation = observations.delete_replied(&receipt).await?;
        child.terminate()?;
        child.wait_for_clean_exit().await?;
        observations.assert_response_lost_once(&lost_invocation);
        observations.assert_delete_retry_bounded(&receipt, &lost_invocation, &replied_invocation);
        assert_eq!(Some(committed), assessment(&pool, source).await?);
        assert_eq!(1, assessment_count(&pool).await?);
        assert_one_source_event(&pool, source).await?;
        no_rerun.rollback().await?;
        assert_queue_counts(0, 0).await?;
        webhook.stop().await?;
        sqs.stop().await?;
        Ok(())
    })
    .await;
}

#[aura_integration_test(services = [POSTGRES, WORKER_SQS, SEQUIN])]
async fn should_drain_blocked_work_on_sigterm_and_leave_queued_work_for_restart() {
    case(async {
        let pool = get_postgres_client().await;
        let observations = Observations::new();
        let sqs = Relay::sqs("primary", observations.clone(), false).await?;
        let address = unused_address()?;
        let webhook = Relay::webhook(address, observations.clone()).await?;
        let mut child = WorkerProcess::start(&pool, &sqs, address).await?;
        let mut barrier = pool.begin().await?;
        sqlx::query("LOCK TABLE product_listing_content_assessments IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *barrier)
            .await?;
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let source = commit_source(&pool).await?;
        let (body, message_id) = observations.publication(source.event_id).await?;
        let receipt = observations.received(&message_id, 1).await?;
        wait_for_blocked_handler(&pool, blocker_pid).await?;
        child.terminate()?;
        wait_for_http_shutdown(address).await?;
        child.assert_running()?;
        wait_for_blocked_handler(&pool, blocker_pid).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM product_listing_content_assessments")
                .fetch_one(&mut *barrier)
                .await?;
        assert_eq!(
            0, count,
            "SIGTERM must drain the owned attempt rather than cancel it"
        );
        let queued_message_id = send(&body).await?;
        barrier.commit().await?;
        let committed = persisted_assessment(&pool, source).await?;
        observations.deleted(&receipt).await?;
        child.wait_for_clean_exit().await?;
        assert_queue_counts(1, 0).await?;

        let mut restarted = WorkerProcess::start(&pool, &sqs, address).await?;
        let queued = observations.received(&queued_message_id, 1).await?;
        assert_eq!(body, queued.body);
        observations.deleted(&queued).await?;
        assert_eq!(Some(committed), assessment(&pool, source).await?);
        assert_eq!(1, assessment_count(&pool).await?);
        assert_one_source_event(&pool, source).await?;
        assert_queue_counts(0, 0).await?;
        restarted.kill()?;
        webhook.stop().await?;
        sqs.stop().await?;
        Ok(())
    })
    .await;
}

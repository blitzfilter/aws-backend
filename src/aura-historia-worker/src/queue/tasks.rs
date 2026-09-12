use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::task::JoinSet;

type CancelledJoin = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Keep aborted children joinable after their immediate owner is cancelled.
#[derive(Clone, Default)]
pub(super) struct CancelledTasks(Arc<Mutex<Vec<CancelledJoin>>>);

impl std::fmt::Debug for CancelledTasks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelledTasks").finish_non_exhaustive()
    }
}

impl CancelledTasks {
    pub(super) async fn join(&self) {
        loop {
            // Poison cannot justify losing custody of an already-aborted task.
            let task = self
                .0
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .pop();
            match task {
                Some(task) => task.await,
                None => return,
            }
        }
    }
}

pub(crate) struct OwnedTasks<T: Send + 'static> {
    tasks: JoinSet<T>,
    cancelled: CancelledTasks,
}

impl<T: Send + 'static> OwnedTasks<T> {
    pub(super) fn new(cancelled: CancelledTasks) -> Self {
        Self {
            tasks: JoinSet::new(),
            cancelled,
        }
    }
}

impl<T: Send + 'static> std::ops::Deref for OwnedTasks<T> {
    type Target = JoinSet<T>;
    fn deref(&self) -> &Self::Target {
        &self.tasks
    }
}
impl<T: Send + 'static> std::ops::DerefMut for OwnedTasks<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tasks
    }
}
impl<T: Send + 'static> Drop for OwnedTasks<T> {
    fn drop(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let mut tasks = std::mem::take(&mut self.tasks);
        tasks.abort_all();
        self.cancelled
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(Box::pin(async move {
                while tasks.join_next().await.is_some() {}
            }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn should_confirm_child_destruction_after_owner_abort_even_before_first_poll() {
        for started in [false, true] {
            let cancelled = CancelledTasks::default();
            let dropped = Arc::new(AtomicBool::new(false));
            let guard = Dropped(dropped.clone());
            let mut tasks = OwnedTasks::new(cancelled.clone());
            let (start, wait) = tokio::sync::oneshot::channel();
            tasks.spawn(async move {
                let _guard = guard;
                let _closed = start.send(());
                std::future::pending::<()>().await;
            });
            if started {
                wait.await.unwrap();
            }
            drop(tasks);
            cancelled.join().await;
            assert!(dropped.load(Ordering::SeqCst));
        }
    }
}

//! DBus/AT Command Serialization Module
//!
//! This module serializes D-Bus and AT operations per physical modem resource.
//! Commands targeting one modem must not overlap, while independent modem lines
//! must be able to progress concurrently.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

static RESOURCE_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

fn resource_locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    RESOURCE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn lock_for(resource_key: &str) -> Arc<Mutex<()>> {
    let mut locks = resource_locks().lock().await;
    Arc::clone(
        locks
            .entry(resource_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

/// Execute a future while holding the lock for one modem/QMI resource.
///
/// `resource_key` should normally be the selected ModemManager object path. A
/// dedicated process-global key may be used for genuinely global operations,
/// such as temporarily changing ModemManager's logging level.
pub async fn with_serial_for<T, F>(resource_key: &str, f: F) -> T
where
    F: Future<Output = T>,
{
    let key = resource_key.trim();
    assert!(!key.is_empty(), "serial resource key must not be empty");
    let lock = lock_for(key).await;
    let _guard = lock.lock().await;
    f.await
}

#[cfg(test)]
mod tests {
    use super::with_serial_for;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{oneshot, Notify};

    #[tokio::test]
    async fn same_resource_key_is_serialized() {
        let (first_acquired_tx, first_acquired_rx) = oneshot::channel();
        let (second_acquired_tx, mut second_acquired_rx) = oneshot::channel();
        let release = Arc::new(Notify::new());
        let first_release = Arc::clone(&release);

        let first = tokio::spawn(async move {
            with_serial_for("test-modem-same", async move {
                let _ = first_acquired_tx.send(());
                first_release.notified().await;
            })
            .await;
        });
        first_acquired_rx.await.expect("first task acquires lock");

        let second = tokio::spawn(async move {
            with_serial_for("test-modem-same", async move {
                let _ = second_acquired_tx.send(());
            })
            .await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(matches!(
            second_acquired_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        release.notify_one();
        first.await.expect("first task completes");
        second.await.expect("second task completes");
        second_acquired_rx
            .await
            .expect("second task acquires lock after first");
    }

    #[tokio::test]
    async fn different_resource_keys_do_not_block_each_other() {
        let (first_acquired_tx, first_acquired_rx) = oneshot::channel();
        let release = Arc::new(Notify::new());
        let first_release = Arc::clone(&release);

        let first = tokio::spawn(async move {
            with_serial_for("test-modem-a", async move {
                let _ = first_acquired_tx.send(());
                first_release.notified().await;
            })
            .await;
        });
        first_acquired_rx.await.expect("first task acquires lock");

        tokio::time::timeout(
            Duration::from_millis(100),
            with_serial_for("test-modem-b", async {}),
        )
        .await
        .expect("independent modem lock remains available");

        release.notify_one();
        first.await.expect("first task completes");
    }
}

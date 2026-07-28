//! DBus/AT Command Serialization Module
//!
//! This module provides a global lock to serialize all DBus and AT command operations
//! to prevent "org.ofono.Error.InProgress: Operation already in progress" errors.

use std::future::Future;
use tokio::sync::Mutex;

/// Global mutex to serialize DBus/AT operations
static DBUS_LOCK: Mutex<()> = Mutex::const_new(());

/// Execute a future while holding the global DBus lock
///
/// This ensures that only one DBus/AT operation can be in progress at a time,
/// preventing "Operation already in progress" errors from ofono.
///
/// # Example
/// ```rust
/// let result = with_serial(async {
///     send_at_command(&conn, "AT+CGSN").await
/// }).await;
/// ```
pub async fn with_serial<T, F>(f: F) -> T
where
    F: Future<Output = T>,
{
    let _guard = DBUS_LOCK.lock().await;
    f.await
}

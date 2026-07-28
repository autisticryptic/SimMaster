//! Notification domain: multi-channel push delivery and its send queue.
//!
//!   - `notification`: builds and dispatches notifications (SMS/call/DDNS/update)
//!     across the configured channels (Bark, Telegram, WeCom, etc.)
//!   - `notification_queue`: rate-limited, retrying background send queue

pub mod notification;
pub mod notification_queue;

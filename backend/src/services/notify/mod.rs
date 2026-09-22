//! Notification domain: multi-channel push delivery and its send queue.
//!
//!   - `notification`: builds and dispatches notifications (SMS/call/DDNS/update)
//!     across the configured channels (Bark, Telegram, WeCom, etc.)
//!   - `notification_queue`: rate-limited, retrying background send queue
//!   - `telegram_endpoint`: Bot API endpoint resolution for direct or
//!     reverse-proxy access, with HTTPS/SSRF validation and token redaction

pub mod notification;
pub mod notification_queue;
pub mod telegram_endpoint;

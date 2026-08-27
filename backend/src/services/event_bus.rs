//! Unified application event bus.
//!
//! The database is the durable outbox and owns the in-process broadcast
//! channel.  This small facade gives producers and consumers one named API so
//! the browser SSE stream, notification workers, and future integrations do
//! not need to know how events are stored.

use std::sync::Arc;

use anyhow::Result;
use serde_json::json;
use tokio::sync::broadcast;

use crate::platform::db::{AppEventEntry, Database};
use crate::services::system::diagnostic_log::{self, DiagnosticLogSink};
use crate::services::system::system_event::SystemEvent;

#[derive(Clone)]
pub struct AppEventBus {
    database: Arc<Database>,
    /// On-disk mirror of every published event, with the raw error string kept
    /// whole. Optional so tests and tools can build a bus without a writer.
    diagnostic_log: Option<Arc<DiagnosticLogSink>>,
}

impl AppEventBus {
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            database,
            diagnostic_log: None,
        }
    }

    /// Mirror published events into the on-disk diagnostic log.
    pub fn with_diagnostic_log(mut self, sink: Arc<DiagnosticLogSink>) -> Self {
        self.diagnostic_log = Some(sink);
        self
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEventEntry> {
        self.database.subscribe_app_events()
    }

    pub fn history_after(
        &self,
        after_id: i64,
        line_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AppEventEntry>> {
        Ok(self
            .database
            .get_app_events_after(after_id, line_id, limit)?)
    }

    pub fn recent(&self, line_id: Option<&str>, limit: i64) -> Result<Vec<AppEventEntry>> {
        Ok(self.database.get_recent_app_events(line_id, limit)?)
    }

    /// Publish an event to the durable outbox, the in-process broadcast channel,
    /// and the on-disk diagnostic log.
    ///
    /// The diagnostic record is built here rather than in a subscriber task
    /// because the main/UE-worker attribution comes from a task-local: a
    /// downstream subscriber runs in its own task and would label every record
    /// `main`, however the event was produced.
    pub fn publish(
        &self,
        event_type: &str,
        line_id: Option<&str>,
        transport: Option<&str>,
        payload: serde_json::Value,
    ) -> Result<i64> {
        if let Some(sink) = &self.diagnostic_log {
            sink.record(diagnostic_log::record_for_app_event(
                event_type, line_id, transport, &payload,
            ));
        }
        Ok(self.database.insert_app_event(
            event_type,
            line_id,
            transport,
            &payload.to_string(),
            &chrono::Utc::now().to_rfc3339(),
        )?)
    }

    pub fn publish_system_event(&self, event: &SystemEvent) -> Result<i64> {
        self.publish(
            &format!("system.{}", event.event_code),
            None,
            Some("system"),
            json!(event),
        )
    }
}

use std::{collections::VecDeque, convert::Infallible, sync::Arc, time::Duration};

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::{stream, Stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::{
    platform::{db::AppEventEntry, shutdown::ShutdownSignal},
    services::event_bus::AppEventBus,
};

#[derive(Debug, Default, Deserialize)]
pub struct EventStreamQuery {
    pub line_id: Option<String>,
    pub after_id: Option<i64>,
}

#[derive(Serialize)]
struct AppEventPayload {
    id: i64,
    device_id: String,
    event_type: String,
    line_id: Option<String>,
    transport: Option<String>,
    payload: Value,
    created_at: String,
}

impl From<AppEventEntry> for AppEventPayload {
    fn from(event: AppEventEntry) -> Self {
        let payload = serde_json::from_str(&event.payload_json)
            .unwrap_or_else(|_| Value::String(event.payload_json));
        Self {
            id: event.id,
            device_id: event.device_id,
            event_type: event.event_type,
            line_id: event.line_id,
            transport: event.transport,
            payload,
            created_at: event.created_at,
        }
    }
}

struct EventStreamState {
    pending: VecDeque<AppEventEntry>,
    receiver: broadcast::Receiver<AppEventEntry>,
    event_bus: Arc<AppEventBus>,
    line_id: Option<String>,
    last_id: i64,
    replaying: bool,
    /// Ends the stream when the process is going down. Without this the
    /// response never completes, and `with_graceful_shutdown` waits on it until
    /// the force-exit watchdog fires -- which skips every teardown path.
    shutdown: ShutdownSignal,
}

fn event_matches_line(event: &AppEventEntry, line_id: Option<&str>) -> bool {
    line_id.is_none() || event.line_id.as_deref() == line_id
}

fn sse_event(event: AppEventEntry) -> Event {
    let id = event.id;
    let data =
        serde_json::to_string(&AppEventPayload::from(event)).unwrap_or_else(|_| "{}".to_string());
    Event::default()
        .id(id.to_string())
        .event("app_event")
        .data(data)
}

pub async fn stream_app_events(
    State(event_bus): State<Arc<AppEventBus>>,
    State(shutdown): State<ShutdownSignal>,
    Query(query): Query<EventStreamQuery>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe before reading history so an event committed during the query
    // is either in the history page or waiting in the receiver. Duplicate IDs
    // are discarded by `last_id` below.
    let receiver = event_bus.subscribe();
    let line_id = query
        .line_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let header_after_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());
    let requested_after_id = query.after_id.or(header_after_id);
    let initial = match requested_after_id {
        Some(after_id) => event_bus.history_after(after_id.max(0), line_id.as_deref(), 500),
        None => event_bus.recent(line_id.as_deref(), 100),
    }
    .unwrap_or_else(|error| {
        tracing::warn!(error = %error, "Failed to load application event history for SSE");
        Vec::new()
    });
    let last_id = requested_after_id.unwrap_or(0).max(0);

    let state = EventStreamState {
        pending: initial.into(),
        receiver,
        event_bus,
        line_id,
        last_id,
        replaying: requested_after_id.is_some(),
        shutdown,
    };
    let stream = stream::unfold(state, |mut state| async move {
        loop {
            // Drain what is already buffered even while going down, so a client
            // that reconnects does not lose events, then end the response.
            if state.pending.is_empty() && state.shutdown.is_shutting_down() {
                return None;
            }

            if let Some(event) = state.pending.pop_front() {
                if event.id <= state.last_id {
                    continue;
                }
                state.last_id = event.id;
                return Some((Ok(sse_event(event)), state));
            }

            if state.replaying {
                match state
                    .event_bus
                    .history_after(state.last_id, state.line_id.as_deref(), 500)
                {
                    Ok(events) if !events.is_empty() => {
                        state.pending.extend(events);
                        continue;
                    }
                    Ok(_) => state.replaying = false,
                    Err(error) => {
                        state.replaying = false;
                        tracing::warn!(error = %error, "Failed to continue SSE history replay");
                    }
                }
            }

            // `recv` on an idle bus parks indefinitely, so the shutdown signal
            // has to be raced against it rather than only polled around it.
            let received = tokio::select! {
                biased;
                _ = state.shutdown.wait() => return None,
                received = state.receiver.recv() => received,
            };

            match received {
                Ok(event) => {
                    if event.id <= state.last_id
                        || !event_matches_line(&event, state.line_id.as_deref())
                    {
                        continue;
                    }
                    state.last_id = event.id;
                    return Some((Ok(sse_event(event)), state));
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        skipped,
                        "SSE application event receiver lagged; replaying durable history"
                    );
                    match state.event_bus.history_after(
                        state.last_id,
                        state.line_id.as_deref(),
                        500,
                    ) {
                        Ok(events) => {
                            state.pending.extend(events);
                            state.replaying = true;
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, "Failed to replay application event history");
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

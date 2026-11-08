//! Server-sent events (`GET /api/event`).
//!
//! Each frame is a single line of compact JSON prefixed with `data: ` because
//! some clients (notably the Termux File Manager app) parse the stream line by
//! line instead of buffering until the blank separator.

use crate::state::AppState;
use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::Stream;
use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

pub async fn subscribe(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let filter = params
        .get("location[directory]")
        .or_else(|| params.get("directory"))
        .cloned();
    let mut receiver = state.bus().subscribe();
    let connected = serde_json::to_string(&state.bus().connected()).unwrap_or_default();
    let stream = async_stream::stream! {
        yield Ok(SseEvent::default().data(connected));
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if let Some(directory) = &filter {
                        let matches = event
                            .location
                            .as_ref()
                            .map(|location| &location.directory == directory)
                            .unwrap_or(false);
                        if !matches {
                            continue;
                        }
                    }
                    let payload = match serde_json::to_string(&event) {
                        Ok(payload) => payload,
                        Err(_) => continue,
                    };
                    yield Ok(SseEvent::default().data(payload));
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

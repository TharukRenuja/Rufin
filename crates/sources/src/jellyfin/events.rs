//! Jellyfin's concrete library-change feed.
//!
//! The socket carries only hints. HTTP resolution in `refresh` produces the
//! finite canonical update; disconnected or folder-wide intervals widen to a
//! complete source read owned by Rufin.

use base64::{Engine as _, engine::general_purpose};
use futures_util::{SinkExt, StreamExt};
use getrandom::fill;
use reqwest::StatusCode;
use serde::Deserialize;
use tokio::time::{Duration, interval, sleep};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};
use tracing::warn;

use super::*;
use crate::SourceLibraryChange;

const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
const JELLYFIN_WEBSOCKET_KEY_BYTES: usize = 16;
const FEED_RETRY_MIN: Duration = Duration::from_secs(5);
const FEED_RETRY_MAX: Duration = Duration::from_secs(60);
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(500);

impl JellyfinSource {
    async fn connect_library_socket(&self) -> SourceResult<WebSocketStream<reqwest::Upgraded>> {
        let key = websocket_key()?;
        let response = self
            .client
            .get(endpoint(&self.base_url, "socket")?)
            .version(reqwest::Version::HTTP_11)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", key)
            .send()
            .await
            .map_err(|error| SourceError::Network(error.to_string()))?;
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(SourceError::Server {
                status: response.status().as_u16(),
                message: "Jellyfin WebSocket upgrade was rejected".to_string(),
            });
        }
        let upgraded = response
            .upgrade()
            .await
            .map_err(|error| SourceError::Network(error.to_string()))?;
        Ok(WebSocketStream::from_raw_socket(upgraded, Role::Client, None).await)
    }

    pub(crate) async fn listen_library_changes(
        &self,
        on_ready: &mut (dyn FnMut(bool) -> bool + Send),
        on_change: &mut (dyn FnMut(SourceLibraryChange) -> bool + Send),
        should_stop: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<()> {
        let mut delay = FEED_RETRY_MIN;
        let mut reconnecting = false;
        while !should_stop() {
            let result = self
                .listen_library_changes_once(reconnecting, on_ready, on_change, should_stop)
                .await;
            if should_stop() {
                return Ok(());
            }
            if let Err(error) = result {
                warn!(%error, "Jellyfin library change feed disconnected");
            }
            reconnecting = true;
            if !wait_before_retry(delay, should_stop).await {
                return Ok(());
            }
            delay = delay.saturating_mul(2).min(FEED_RETRY_MAX);
        }
        Ok(())
    }

    async fn listen_library_changes_once(
        &self,
        reconnecting: bool,
        on_ready: &mut (dyn FnMut(bool) -> bool + Send),
        on_change: &mut (dyn FnMut(SourceLibraryChange) -> bool + Send),
        should_stop: &(dyn Fn() -> bool + Send + Sync),
    ) -> SourceResult<()> {
        let mut socket = self.connect_library_socket().await?;
        if !on_ready(reconnecting) {
            return Ok(());
        }
        let mut keep_alive = interval(KEEP_ALIVE_INTERVAL);
        let mut stop_poll = interval(STOP_POLL_INTERVAL);
        loop {
            if should_stop() {
                return Ok(());
            }
            tokio::select! {
                _ = stop_poll.tick() => {
                    if should_stop() {
                        return Ok(());
                    }
                }
                _ = keep_alive.tick() => {
                    send_keep_alive(&mut socket).await?;
                }
                message = socket.next() => {
                    let Some(message) = message else {
                        return Ok(());
                    };
                    match message.map_err(websocket_error)? {
                        Message::Text(text) => match library_socket_message(&text)? {
                            JellyfinSocketMessage::Change(change) => {
                                if !on_change(change) {
                                    return Ok(());
                                }
                            }
                            JellyfinSocketMessage::ForceKeepAlive => {
                                send_keep_alive(&mut socket).await?;
                            }
                            JellyfinSocketMessage::Other => {}
                        },
                        Message::Close(_) => return Ok(()),
                        Message::Ping(payload) => socket
                            .send(Message::Pong(payload))
                            .await
                            .map_err(websocket_error)?,
                        Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
                    }
                }
            }
        }
    }
}

async fn wait_before_retry(
    delay: Duration,
    should_stop: &(dyn Fn() -> bool + Send + Sync),
) -> bool {
    let mut remaining = delay;
    while !remaining.is_zero() {
        let step = remaining.min(STOP_POLL_INTERVAL);
        sleep(step).await;
        if should_stop() {
            return false;
        }
        remaining = remaining.saturating_sub(step);
    }
    true
}

#[derive(Deserialize)]
struct SocketMessage {
    #[serde(rename = "MessageType")]
    message_type: String,
    #[serde(rename = "Data", default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Eq, PartialEq)]
enum JellyfinSocketMessage {
    Change(SourceLibraryChange),
    ForceKeepAlive,
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LibraryChangedData {
    #[serde(default)]
    items_added: Vec<String>,
    #[serde(default)]
    items_updated: Vec<String>,
    #[serde(default)]
    items_removed: Vec<String>,
    #[serde(default)]
    folders_added_to: Vec<String>,
    #[serde(default)]
    folders_removed_from: Vec<String>,
    #[serde(default)]
    collection_folders: Vec<String>,
}

fn library_socket_message(text: &str) -> SourceResult<JellyfinSocketMessage> {
    let message = serde_json::from_str::<SocketMessage>(text)
        .map_err(|error| SourceError::Other(error.to_string()))?;
    match message.message_type.as_str() {
        "LibraryChanged" => {
            let Some(data) = message.data else {
                return Ok(JellyfinSocketMessage::Other);
            };
            let data = serde_json::from_value::<LibraryChangedData>(data)
                .map_err(|error| SourceError::Other(error.to_string()))?;
            if !data.folders_added_to.is_empty()
                || !data.folders_removed_from.is_empty()
                || !data.collection_folders.is_empty()
            {
                return Ok(JellyfinSocketMessage::Change(SourceLibraryChange::full()));
            }
            let items = data
                .items_added
                .into_iter()
                .chain(data.items_updated)
                .chain(data.items_removed)
                .collect::<std::collections::BTreeSet<_>>();
            if items.is_empty() {
                Ok(JellyfinSocketMessage::Other)
            } else {
                Ok(JellyfinSocketMessage::Change(
                    SourceLibraryChange::jellyfin_items(items),
                ))
            }
        }
        "ForceKeepAlive" => Ok(JellyfinSocketMessage::ForceKeepAlive),
        _ => Ok(JellyfinSocketMessage::Other),
    }
}

async fn send_keep_alive(socket: &mut WebSocketStream<reqwest::Upgraded>) -> SourceResult<()> {
    socket
        .send(Message::Text(
            r#"{"MessageType":"KeepAlive"}"#.to_string().into(),
        ))
        .await
        .map_err(websocket_error)
}

fn websocket_key() -> SourceResult<String> {
    let mut bytes = [0_u8; JELLYFIN_WEBSOCKET_KEY_BYTES];
    fill(&mut bytes).map_err(|error| SourceError::Other(error.to_string()))?;
    Ok(general_purpose::STANDARD.encode(bytes))
}

fn websocket_error(error: tokio_tungstenite::tungstenite::Error) -> SourceError {
    SourceError::Network(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_context_widens_item_changes_to_full() {
        let message = library_socket_message(
            r#"{"MessageType":"LibraryChanged","Data":{"ItemsAdded":["item-one"],"ItemsUpdated":["item-two","item-one"],"ItemsRemoved":["item-three"],"FoldersAddedTo":["folder-one"]}}"#,
        )
        .expect("parse message");

        assert_eq!(
            message,
            JellyfinSocketMessage::Change(SourceLibraryChange::full())
        );
    }

    #[test]
    fn library_update_unions_item_ids_without_folder_context() {
        let message = library_socket_message(
            r#"{"MessageType":"LibraryChanged","Data":{"ItemsAdded":["item-one"],"ItemsUpdated":["item-two","item-one"],"ItemsRemoved":["item-three"]}}"#,
        )
        .expect("parse message");

        assert_eq!(
            message,
            JellyfinSocketMessage::Change(SourceLibraryChange::jellyfin_items([
                "item-one".to_string(),
                "item-two".to_string(),
                "item-three".to_string(),
            ]))
        );
    }

    #[test]
    fn empty_library_update_emits_no_change() {
        let message = library_socket_message(
            r#"{"MessageType":"LibraryChanged","Data":{"ItemsAdded":[],"ItemsUpdated":[],"ItemsRemoved":[]}}"#,
        )
        .expect("parse message");

        assert_eq!(message, JellyfinSocketMessage::Other);
    }
}

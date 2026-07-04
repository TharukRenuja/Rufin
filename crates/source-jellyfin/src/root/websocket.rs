use super::*;
use base64::{Engine as _, engine::general_purpose};
use futures_util::{SinkExt, StreamExt};
use getrandom::fill;
use reqwest::StatusCode;
use tokio::time::{Duration, interval};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
const JELLYFIN_WEBSOCKET_KEY_BYTES: usize = 16;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JellyfinLibraryChange {
    pub items_added: Vec<String>,
    pub items_updated: Vec<String>,
    pub items_removed: Vec<String>,
}

impl JellyfinLibraryChange {
    pub fn is_empty(&self) -> bool {
        self.items_added.is_empty()
            && self.items_updated.is_empty()
            && self.items_removed.is_empty()
    }

    pub fn merge(&mut self, other: Self) {
        push_unique_raw_item_ids(&mut self.items_added, &other.items_added);
        push_unique_raw_item_ids(&mut self.items_updated, &other.items_updated);
        push_unique_raw_item_ids(&mut self.items_removed, &other.items_removed);
    }

    pub fn fetch_item_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        push_unique_raw_item_ids(&mut ids, &self.items_added);
        push_unique_raw_item_ids(&mut ids, &self.items_updated);
        ids
    }

    pub fn removed_track_ids(&self) -> Vec<TrackId> {
        let mut ids = Vec::new();
        for raw_id in &self.items_removed {
            let raw_id = raw_id.trim();
            if raw_id.is_empty() {
                continue;
            }
            let track_id = TrackId::new(jellyfin_id("track", raw_id));
            if !ids.contains(&track_id) {
                ids.push(track_id);
            }
        }
        ids
    }
}

impl JellyfinSource {
    pub async fn listen_library_changes(
        &self,
        mut on_change: impl FnMut(JellyfinLibraryChange) -> bool,
        should_stop: impl Fn() -> bool,
    ) -> SourceResult<()> {
        let mut socket = self.connect_library_socket().await?;
        let mut keep_alive = interval(KEEP_ALIVE_INTERVAL);
        loop {
            if should_stop() {
                return Ok(());
            }
            tokio::select! {
                _ = keep_alive.tick() => {
                    send_keep_alive(&mut socket).await?;
                }
                message = socket.next() => {
                    let Some(message) = message else {
                        return Ok(());
                    };
                    match message.map_err(websocket_error)? {
                        Message::Text(text) => {
                            match library_socket_message(&text)? {
                                JellyfinSocketMessage::LibraryChanged(change) if !change.is_empty() => {
                                    if !on_change(change) {
                                        return Ok(());
                                    }
                                }
                                JellyfinSocketMessage::LibraryChanged(_) => {}
                                JellyfinSocketMessage::ForceKeepAlive => {
                                    send_keep_alive(&mut socket).await?;
                                }
                                JellyfinSocketMessage::Other => {}
                            }
                        }
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

    async fn connect_library_socket(&self) -> SourceResult<WebSocketStream<reqwest::Upgraded>> {
        let key = websocket_key()?;
        let config = JellyfinClientConfig::new(
            self.identity.base_url.clone(),
            false,
            Some(self.device_id.to_string()),
        );
        let response = self
            .client
            .get(endpoint(&self.base_url, "socket")?)
            .version(reqwest::Version::HTTP_11)
            .header(
                header::AUTHORIZATION,
                auth_header(&config, Some(&self.access_token)),
            )
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", key)
            .send()
            .await
            .map_err(|error| SourceError::Other(error.to_string()))?;
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(SourceError::Other(format!(
                "Jellyfin WebSocket upgrade returned {}",
                response.status()
            )));
        }
        let upgraded = response
            .upgrade()
            .await
            .map_err(|error| SourceError::Other(error.to_string()))?;
        Ok(WebSocketStream::from_raw_socket(upgraded, Role::Client, None).await)
    }
}

#[derive(Deserialize)]
struct SocketMessage {
    #[serde(rename = "MessageType")]
    message_type: String,
    #[serde(rename = "Data", default)]
    data: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct LibraryUpdateInfo {
    #[serde(rename = "ItemsAdded", default)]
    items_added: Vec<String>,
    #[serde(rename = "ItemsUpdated", default)]
    items_updated: Vec<String>,
    #[serde(rename = "ItemsRemoved", default)]
    items_removed: Vec<String>,
}

enum JellyfinSocketMessage {
    LibraryChanged(JellyfinLibraryChange),
    ForceKeepAlive,
    Other,
}

fn library_socket_message(text: &str) -> SourceResult<JellyfinSocketMessage> {
    let message = serde_json::from_str::<SocketMessage>(text)
        .map_err(|error| SourceError::Other(error.to_string()))?;
    match message.message_type.as_str() {
        "LibraryChanged" => {
            let Some(data) = message.data else {
                return Ok(JellyfinSocketMessage::Other);
            };
            let info = serde_json::from_value::<LibraryUpdateInfo>(data)
                .map_err(|error| SourceError::Other(error.to_string()))?;
            Ok(JellyfinSocketMessage::LibraryChanged(
                JellyfinLibraryChange {
                    items_added: clean_raw_item_ids(info.items_added),
                    items_updated: clean_raw_item_ids(info.items_updated),
                    items_removed: clean_raw_item_ids(info.items_removed),
                },
            ))
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
    SourceError::Other(error.to_string())
}

fn clean_raw_item_ids(ids: Vec<String>) -> Vec<String> {
    let mut cleaned = Vec::new();
    push_unique_raw_item_ids(&mut cleaned, &ids);
    cleaned
}

fn push_unique_raw_item_ids(target: &mut Vec<String>, ids: &[String]) {
    for id in ids {
        let id = id.trim();
        if !id.is_empty() && !target.iter().any(|existing| existing == id) {
            target.push(id.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_changed_message_extracts_item_ids() {
        let message = library_socket_message(
            r#"{"MessageType":"LibraryChanged","Data":{"ItemsAdded":[" one ","one"],"ItemsUpdated":["two"],"ItemsRemoved":["three"]}}"#,
        )
        .expect("parse message");

        let JellyfinSocketMessage::LibraryChanged(change) = message else {
            panic!("expected library change");
        };
        assert_eq!(change.items_added, vec!["one"]);
        assert_eq!(change.items_updated, vec!["two"]);
        assert_eq!(change.items_removed, vec!["three"]);
        assert_eq!(change.fetch_item_ids(), vec!["one", "two"]);
        assert_eq!(
            change.removed_track_ids(),
            vec![TrackId::new("jellyfin:track:three")]
        );
    }
}

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

impl JellyfinSource {
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

#[async_trait(?Send)]
impl LibraryChangeFeed for JellyfinSource {
    async fn listen(
        &self,
        on_ready: &mut dyn FnMut() -> bool,
        on_change: &mut dyn FnMut(LibraryChange) -> bool,
        should_stop: &dyn Fn() -> bool,
    ) -> SourceResult<()> {
        let mut socket = self.connect_library_socket().await?;
        if !on_ready() {
            return Ok(());
        }
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
                                JellyfinSocketMessage::Change(change) => {
                                    if !on_change(change) {
                                        return Ok(());
                                    }
                                }
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
    Change(LibraryChange),
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
            let changes = SourceObjectChanges::new(
                data.items_added
                    .into_iter()
                    .chain(data.items_updated)
                    .chain(data.items_removed),
            );
            if !data.folders_added_to.is_empty()
                || !data.folders_removed_from.is_empty()
                || !data.collection_folders.is_empty()
            {
                return Ok(JellyfinSocketMessage::Change(LibraryChange::Full));
            }
            if !changes.is_empty() {
                return Ok(JellyfinSocketMessage::Change(LibraryChange::Objects(
                    changes,
                )));
            }
            Ok(JellyfinSocketMessage::Other)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_context_widens_item_changes_to_full() {
        let message = library_socket_message(
            r#"{"MessageType":"LibraryChanged","Data":{"ItemsAdded":["item-one"],"ItemsUpdated":["item-two","item-one"],"ItemsRemoved":["item-three"],"FoldersAddedTo":["folder-one"]}}"#,
        )
        .expect("parse message");

        assert_eq!(message, JellyfinSocketMessage::Change(LibraryChange::Full));
    }

    #[test]
    fn library_update_unions_item_ids_without_folder_context() {
        let message = library_socket_message(
            r#"{"MessageType":"LibraryChanged","Data":{"ItemsAdded":["item-one"],"ItemsUpdated":["item-two","item-one"],"ItemsRemoved":["item-three"]}}"#,
        )
        .expect("parse message");

        assert_eq!(
            message,
            JellyfinSocketMessage::Change(LibraryChange::Objects(SourceObjectChanges::new([
                "item-one".to_string(),
                "item-two".to_string(),
                "item-three".to_string(),
            ])))
        );
    }

    #[test]
    fn folder_only_library_update_requires_full_resolution() {
        let message = library_socket_message(
            r#"{"MessageType":"LibraryChanged","Data":{"ItemsAdded":[],"ItemsUpdated":[],"ItemsRemoved":[],"CollectionFolders":["music-one"]}}"#,
        )
        .expect("parse message");

        assert_eq!(message, JellyfinSocketMessage::Change(LibraryChange::Full));
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

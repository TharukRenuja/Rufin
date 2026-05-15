use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::{Client, StatusCode, Url, header};
use rufin_core::{
    Album, AlbumId, Artist, ArtistId, Genre, GenreId, HomeSection, HomeSectionKind, Playlist,
    PlaylistId, ServerId, ServerIdentity, Track, TrackId,
};
use rufin_provider::{
    AlbumDetail, ImageKind, ImageMetadata, LoginRequest, MusicProvider, PagedRequest,
    PagedResponse, ProviderCapabilities, ProviderError, ProviderIdentity, ProviderResult,
    ProviderSession, SavedProviderSession, SearchResults, StreamDescriptor,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tracing::instrument;

const CLIENT_NAME: &str = "Rufin";
const DEVICE_NAME: &str = "Linux Desktop";
const DEVICE_ID: &str = "rufin-native";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JellyfinClientConfig {
    pub base_url: String,
    pub trust_invalid_cert: bool,
    pub device_id: String,
    pub device_name: String,
    pub client_name: String,
    pub client_version: String,
}

impl JellyfinClientConfig {
    pub fn new(base_url: impl Into<String>, trust_invalid_cert: bool) -> Self {
        Self {
            base_url: base_url.into(),
            trust_invalid_cert,
            device_id: DEVICE_ID.to_string(),
            device_name: DEVICE_NAME.to_string(),
            client_name: CLIENT_NAME.to_string(),
            client_version: CLIENT_VERSION.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct JellyfinProvider {
    client: Client,
    base_url: Url,
    user_id: String,
    access_token: Arc<str>,
    identity: ProviderIdentity,
    capabilities: ProviderCapabilities,
}

impl JellyfinProvider {
    #[instrument(skip(request), fields(base_url = %request.base_url, username = %request.username, trust_invalid_cert = request.trust_invalid_cert))]
    pub async fn login(request: LoginRequest) -> ProviderResult<ProviderSession> {
        let config = JellyfinClientConfig::new(&request.base_url, request.trust_invalid_cert);
        let base_url = normalize_base_url(&config.base_url)?;
        let client = build_client(config.trust_invalid_cert)?;

        let body = AuthenticateByNameRequest {
            username: request.username.clone(),
            password: request.password,
        };
        let auth_url = endpoint(&base_url, "Users/AuthenticateByName")?;
        let response = send_json::<AuthenticationResult>(
            client
                .post(auth_url)
                .header(header::AUTHORIZATION, auth_header(&config, None))
                .json(&body),
        )
        .await?;

        let server_name = public_server_name(&client, &base_url, &config)
            .await
            .unwrap_or_else(|| "Jellyfin".to_string());
        let server_id = response
            .server_id
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| stable_server_id(base_url.as_str()));

        Ok(ProviderSession {
            server: ServerIdentity {
                id: ServerId::new(format!("jellyfin:server:{server_id}")),
                provider: "jellyfin".to_string(),
                name: server_name,
                base_url: base_url.as_str().trim_end_matches('/').to_string(),
            },
            user_id: response.user.id,
            username: response.user.name,
            access_token: response.access_token,
        })
    }

    pub fn from_saved_session(session: SavedProviderSession) -> ProviderResult<Self> {
        let config =
            JellyfinClientConfig::new(&session.server.base_url, session.trust_invalid_cert);
        let base_url = normalize_base_url(&config.base_url)?;
        let client = build_client(config.trust_invalid_cert)?;
        Ok(Self {
            client,
            base_url,
            user_id: session.user_id,
            access_token: Arc::from(session.access_token),
            identity: ProviderIdentity {
                server: session.server,
            },
            capabilities: ProviderCapabilities::default(),
        })
    }

    pub fn image_url(
        &self,
        item_id: &str,
        kind: ImageKind,
        tag: Option<&str>,
    ) -> ProviderResult<String> {
        let image_kind = match kind {
            ImageKind::Primary => "Primary",
            ImageKind::Backdrop => "Backdrop",
        };
        let mut url = endpoint(
            &self.base_url,
            &format!("Items/{}/Images/{image_kind}", raw_item_id(item_id)),
        )?;
        if let Some(tag) = tag.filter(|tag| !tag.is_empty()) {
            url.query_pairs_mut().append_pair("tag", tag);
        }
        Ok(url.to_string())
    }

    async fn item_page(
        &self,
        include_types: &str,
        request: PagedRequest,
    ) -> ProviderResult<PagedResponse<JellyfinItem>> {
        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", include_types)
            .append_pair("StartIndex", &request.offset.to_string())
            .append_pair("Limit", &request.limit.to_string())
            .append_pair("Fields", "Genres,DateCreated,PremiereDate,ProductionYear,RunTimeTicks,ParentId,AlbumId,ArtistItems,UserData,ImageTags,ChildCount,AlbumCount,SongCount")
            .append_pair("SortBy", "SortName");

        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(PagedResponse::new(
            response.items,
            response.total_record_count.unwrap_or(0),
        ))
    }

    async fn people_page(
        &self,
        path: &str,
        request: PagedRequest,
    ) -> ProviderResult<PagedResponse<JellyfinItem>> {
        let mut url = endpoint(&self.base_url, path)?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("StartIndex", &request.offset.to_string())
            .append_pair("Limit", &request.limit.to_string())
            .append_pair(
                "Fields",
                "UserData,ItemCounts,ChildCount,AlbumCount,SongCount",
            );

        let response = self.get_json::<ItemQueryResult>(url).await?;
        Ok(PagedResponse::new(
            response.items,
            response.total_record_count.unwrap_or(0),
        ))
    }

    async fn get_json<T: DeserializeOwned>(&self, url: Url) -> ProviderResult<T> {
        let config = JellyfinClientConfig::new(self.identity.server.base_url.clone(), false);
        send_json(self.client.get(url).header(
            header::AUTHORIZATION,
            auth_header(&config, Some(&self.access_token)),
        ))
        .await
    }
}

#[async_trait(?Send)]
impl MusicProvider for JellyfinProvider {
    fn identity(&self) -> &ProviderIdentity {
        &self.identity
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    async fn home_sections(&self) -> ProviderResult<Vec<HomeSection>> {
        let albums = self.albums(PagedRequest::new(0, 48)).await?.items;
        let sections = [
            (HomeSectionKind::Explore, 0_usize),
            (HomeSectionKind::MostPlayed, 6),
            (HomeSectionKind::NewlyAdded, 12),
            (HomeSectionKind::RecentlyPlayed, 18),
            (HomeSectionKind::RecentlyReleased, 24),
        ]
        .into_iter()
        .map(|(kind, offset)| HomeSection {
            kind,
            albums: albums.iter().skip(offset).take(8).cloned().collect(),
        })
        .filter(|section| !section.albums.is_empty())
        .collect();
        Ok(sections)
    }

    async fn albums(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Album>> {
        let response = self.item_page("MusicAlbum", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(album_from_item).collect(),
            response.total,
        ))
    }

    async fn album_detail(&self, album_id: &AlbumId) -> ProviderResult<AlbumDetail> {
        let raw_album_id = raw_item_id(album_id.as_str());
        let mut album_url = endpoint(&self.base_url, &format!("Items/{raw_album_id}"))?;
        album_url
            .query_pairs_mut()
            .append_pair("UserId", &self.user_id);
        let album = album_from_item(self.get_json::<JellyfinItem>(album_url).await?);

        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("ParentId", raw_album_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Audio")
            .append_pair("SortBy", "ParentIndexNumber,IndexNumber,SortName")
            .append_pair(
                "Fields",
                "Genres,ProductionYear,RunTimeTicks,ParentId,UserData,ImageTags",
            )
            .append_pair("StartIndex", "0")
            .append_pair("Limit", "500");
        let response = self.get_json::<ItemQueryResult>(url).await?;
        let tracks = response.items.into_iter().map(track_from_item).collect();

        Ok(AlbumDetail { album, tracks })
    }

    async fn tracks(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Track>> {
        let response = self.item_page("Audio", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(track_from_item).collect(),
            response.total,
        ))
    }

    async fn artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>> {
        let response = self.people_page("Artists", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(artist_from_item).collect(),
            response.total,
        ))
    }

    async fn album_artists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Artist>> {
        let response = self.people_page("Artists/AlbumArtists", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(artist_from_item).collect(),
            response.total,
        ))
    }

    async fn genres(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Genre>> {
        let response = self.people_page("Genres", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(genre_from_item).collect(),
            response.total,
        ))
    }

    async fn playlists(&self, request: PagedRequest) -> ProviderResult<PagedResponse<Playlist>> {
        let response = self.item_page("Playlist", request).await?;
        Ok(PagedResponse::new(
            response.items.into_iter().map(playlist_from_item).collect(),
            response.total,
        ))
    }

    async fn track(&self, track_id: &TrackId) -> ProviderResult<Track> {
        let mut url = endpoint(
            &self.base_url,
            &format!("Items/{}", raw_item_id(track_id.as_str())),
        )?;
        url.query_pairs_mut().append_pair("UserId", &self.user_id);
        self.get_json::<JellyfinItem>(url)
            .await
            .map(track_from_item)
    }

    async fn stream(&self, track_id: &TrackId) -> ProviderResult<StreamDescriptor> {
        let raw_track_id = raw_item_id(track_id.as_str());
        let mut url = endpoint(&self.base_url, &format!("Audio/{raw_track_id}/stream"))?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("DeviceId", DEVICE_ID)
            .append_pair("Static", "true")
            .append_pair("api_key", &self.access_token);
        let mut redacted_url = url.clone();
        redacted_url
            .query_pairs_mut()
            .clear()
            .append_pair("UserId", &self.user_id)
            .append_pair("DeviceId", DEVICE_ID)
            .append_pair("Static", "true")
            .append_pair("api_key", "<redacted>");
        Ok(StreamDescriptor::with_redacted(
            url.to_string(),
            redacted_url.to_string(),
        ))
    }

    async fn search(&self, query: &str) -> ProviderResult<SearchResults> {
        if query.trim().is_empty() {
            return Ok(SearchResults::default());
        }

        let mut url = endpoint(&self.base_url, "Items")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("SearchTerm", query)
            .append_pair("IncludeItemTypes", "Audio,MusicAlbum,MusicArtist,Playlist")
            .append_pair("Limit", "100")
            .append_pair(
                "Fields",
                "Genres,ProductionYear,RunTimeTicks,ParentId,UserData,ImageTags",
            );
        let response = self.get_json::<ItemQueryResult>(url).await?;
        let mut results = SearchResults::default();
        for item in response.items {
            match item.item_type.as_deref() {
                Some("Audio") => results.tracks.push(track_from_item(item)),
                Some("MusicAlbum") => results.albums.push(album_from_item(item)),
                Some("MusicArtist") | Some("Artist") => {
                    results.artists.push(artist_from_item(item))
                }
                Some("Playlist") => results.playlists.push(playlist_from_item(item)),
                _ => {}
            }
        }
        Ok(results)
    }

    async fn image_metadata(
        &self,
        item_id: &str,
        kind: ImageKind,
    ) -> ProviderResult<ImageMetadata> {
        Ok(ImageMetadata {
            item_id: raw_item_id(item_id).to_string(),
            kind,
            tag: None,
            url: self.image_url(item_id, kind, None)?,
        })
    }
}

async fn public_server_name(
    client: &Client,
    base_url: &Url,
    config: &JellyfinClientConfig,
) -> Option<String> {
    let url = endpoint(base_url, "System/Info/Public").ok()?;
    let response = send_json::<PublicSystemInfo>(
        client
            .get(url)
            .header(header::AUTHORIZATION, auth_header(config, None)),
    )
    .await
    .ok()?;
    response.server_name.or(response.local_address)
}

async fn send_json<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> ProviderResult<T> {
    let response = request.send().await.map_err(map_reqwest_error)?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ProviderError::Auth(format!(
            "Jellyfin returned {}",
            status.as_u16()
        )));
    }
    if status == StatusCode::NOT_FOUND {
        return Err(ProviderError::NotFound);
    }
    if status.is_client_error() || status.is_server_error() {
        let message = response.text().await.unwrap_or_else(|_| status.to_string());
        return Err(ProviderError::Server {
            status: status.as_u16(),
            message,
        });
    }

    response.json::<T>().await.map_err(map_reqwest_error)
}

fn build_client(trust_invalid_cert: bool) -> ProviderResult<Client> {
    Client::builder()
        .danger_accept_invalid_certs(trust_invalid_cert)
        .build()
        .map_err(map_reqwest_error)
}

fn normalize_base_url(raw: &str) -> ProviderResult<Url> {
    let trimmed = raw.trim().trim_end_matches('/');
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let mut url =
        Url::parse(&candidate).map_err(|error| ProviderError::Other(error.to_string()))?;
    let path = url.path().trim_end_matches('/').to_string();
    let normalized_path = if path.is_empty() {
        "/".to_string()
    } else {
        format!("{path}/")
    };
    url.set_path(&normalized_path);
    Ok(url)
}

fn endpoint(base_url: &Url, path: &str) -> ProviderResult<Url> {
    let mut url = base_url.clone();
    let base_path = base_url.path().trim_end_matches('/');
    let path = path.trim_start_matches('/');
    let full_path = if base_path.is_empty() {
        format!("/{path}")
    } else {
        format!("{base_path}/{path}")
    };
    url.set_path(&full_path);
    url.set_query(None);
    Ok(url)
}

fn auth_header(config: &JellyfinClientConfig, token: Option<&str>) -> String {
    let mut value = format!(
        "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
        config.client_name, config.device_name, config.device_id, config.client_version
    );
    if let Some(token) = token {
        value.push_str(&format!(", Token=\"{token}\""));
    }
    value
}

fn map_reqwest_error(error: reqwest::Error) -> ProviderError {
    let message = error.to_string();
    if message.to_lowercase().contains("certificate") || message.to_lowercase().contains("tls") {
        ProviderError::Tls(message)
    } else if error.is_connect() || error.is_request() || error.is_timeout() {
        ProviderError::Network(message)
    } else if let Some(status) = error.status() {
        ProviderError::Server {
            status: status.as_u16(),
            message,
        }
    } else {
        ProviderError::Other(message)
    }
}

fn raw_item_id(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

fn jellyfin_id(kind: &str, id: &str) -> String {
    format!("jellyfin:{kind}:{id}")
}

fn stable_server_id(input: &str) -> String {
    format!("{:016x}", stable_hash(input))
}

fn stable_hash(input: &str) -> u64 {
    input.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn color_seed(id: &str) -> u32 {
    (stable_hash(id) & 0xffff_ffff) as u32
}

fn duration_seconds(ticks: Option<i64>) -> u32 {
    ticks
        .map(|value| (value.max(0) / 10_000_000) as u32)
        .unwrap_or(0)
}

fn u16_from_option(value: Option<i32>) -> u16 {
    value.unwrap_or_default().clamp(0, i32::from(u16::MAX)) as u16
}

fn u32_from_option(value: Option<i32>) -> u32 {
    value.unwrap_or_default().max(0) as u32
}

fn favorite(user_data: &Option<UserData>) -> bool {
    user_data
        .as_ref()
        .and_then(|data| data.is_favorite)
        .unwrap_or(false)
}

fn album_from_item(item: JellyfinItem) -> Album {
    let artist_id = item
        .artist_items
        .as_ref()
        .and_then(|items| items.first())
        .map(|artist| ArtistId::new(jellyfin_id("artist", &artist.id)));
    let artist = item
        .album_artist
        .clone()
        .or_else(|| {
            item.artists
                .as_ref()
                .and_then(|artists| artists.first().cloned())
        })
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let item_id = item.id.clone();
    Album {
        id: AlbumId::new(jellyfin_id("album", &item.id)),
        title: item.name.unwrap_or_else(|| "Untitled Album".to_string()),
        artist,
        artist_id,
        year: u16_from_option(item.production_year),
        track_count: u16_from_option(item.child_count),
        duration_seconds: duration_seconds(item.run_time_ticks),
        favorite: favorite(&item.user_data),
        color_seed: color_seed(&item_id),
    }
}

fn track_from_item(item: JellyfinItem) -> Track {
    let artist_id = item
        .artist_items
        .as_ref()
        .and_then(|items| items.first())
        .map(|artist| ArtistId::new(jellyfin_id("artist", &artist.id)));
    let album_id = item
        .album_id
        .as_deref()
        .or(item.parent_id.as_deref())
        .unwrap_or(&item.id);
    Track {
        id: TrackId::new(jellyfin_id("track", &item.id)),
        album_id: AlbumId::new(jellyfin_id("album", album_id)),
        title: item.name.unwrap_or_else(|| "Untitled Track".to_string()),
        artist: item
            .artists
            .as_ref()
            .and_then(|artists| artists.first().cloned())
            .unwrap_or_else(|| {
                item.album_artist
                    .unwrap_or_else(|| "Unknown Artist".to_string())
            }),
        artist_id,
        album: item.album.unwrap_or_else(|| "Unknown Album".to_string()),
        year: u16_from_option(item.production_year),
        duration_seconds: duration_seconds(item.run_time_ticks),
        favorite: favorite(&item.user_data),
        disc_number: u16_from_option(item.parent_index_number),
        track_number: u16_from_option(item.index_number),
    }
}

fn artist_from_item(item: JellyfinItem) -> Artist {
    Artist {
        id: ArtistId::new(jellyfin_id("artist", &item.id)),
        name: item.name.unwrap_or_else(|| "Unknown Artist".to_string()),
        album_count: u32_from_option(
            item.album_count
                .or_else(|| {
                    item.item_counts
                        .as_ref()
                        .and_then(|counts| counts.album_count)
                })
                .or(item.child_count),
        ),
        track_count: u32_from_option(item.song_count.or_else(|| {
            item.item_counts
                .as_ref()
                .and_then(|counts| counts.song_count)
        })),
        favorite: favorite(&item.user_data),
    }
}

fn genre_from_item(item: JellyfinItem) -> Genre {
    Genre {
        id: GenreId::new(jellyfin_id("genre", &item.id)),
        name: item.name.unwrap_or_else(|| "Unknown Genre".to_string()),
        album_count: u32_from_option(
            item.album_count
                .or_else(|| {
                    item.item_counts
                        .as_ref()
                        .and_then(|counts| counts.album_count)
                })
                .or(item.child_count),
        ),
        track_count: u32_from_option(item.song_count.or_else(|| {
            item.item_counts
                .as_ref()
                .and_then(|counts| counts.song_count)
        })),
    }
}

fn playlist_from_item(item: JellyfinItem) -> Playlist {
    Playlist {
        id: PlaylistId::new(jellyfin_id("playlist", &item.id)),
        name: item.name.unwrap_or_else(|| "Untitled Playlist".to_string()),
        track_count: u32_from_option(item.child_count),
        duration_seconds: duration_seconds(item.run_time_ticks),
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    server_name: Option<String>,
    local_address: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticateByNameRequest {
    username: String,
    #[serde(rename = "Pw")]
    password: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticationResult {
    access_token: String,
    server_id: Option<String>,
    user: JellyfinUser,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinUser {
    id: String,
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ItemQueryResult {
    #[serde(default)]
    items: Vec<JellyfinItem>,
    total_record_count: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinItem {
    id: String,
    name: Option<String>,
    #[serde(rename = "Type")]
    item_type: Option<String>,
    album_artist: Option<String>,
    artists: Option<Vec<String>>,
    artist_items: Option<Vec<NameIdPair>>,
    album: Option<String>,
    album_id: Option<String>,
    parent_id: Option<String>,
    production_year: Option<i32>,
    run_time_ticks: Option<i64>,
    child_count: Option<i32>,
    album_count: Option<i32>,
    song_count: Option<i32>,
    item_counts: Option<JellyfinItemCounts>,
    index_number: Option<i32>,
    parent_index_number: Option<i32>,
    user_data: Option<UserData>,
    #[allow(dead_code)]
    image_tags: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinItemCounts {
    album_count: Option<i32>,
    song_count: Option<i32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NameIdPair {
    #[allow(dead_code)]
    name: Option<String>,
    id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserData {
    is_favorite: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rufin_provider::MusicProvider;
    use wiremock::matchers::{header_regex, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn login_posts_credentials_and_maps_session() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Users/AuthenticateByName"))
            .and(header_regex(
                "authorization",
                "MediaBrowser Client=\"Rufin\"",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "AccessToken": "secret-token",
                "ServerId": "server-one",
                "User": { "Id": "user-one", "Name": "demo" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/System/Info/Public"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ServerName": "Music Box"
            })))
            .mount(&server)
            .await;

        let session = JellyfinProvider::login(LoginRequest {
            base_url: server.uri(),
            username: "demo".to_string(),
            password: "pw".to_string(),
            trust_invalid_cert: false,
        })
        .await
        .expect("login");

        assert_eq!(session.server.id.as_str(), "jellyfin:server:server-one");
        assert_eq!(session.server.name, "Music Box");
        assert_eq!(session.username, "demo");
        assert_eq!(session.access_token, "secret-token");
    }

    #[test]
    fn bare_server_addresses_default_to_http() {
        let url = normalize_base_url("music.local:8096").expect("normalized url");

        assert_eq!(url.as_str(), "http://music.local:8096/");
    }

    #[tokio::test]
    async fn album_reads_send_auth_header_and_map_pages() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("IncludeItemTypes", "MusicAlbum"))
            .and(query_param("StartIndex", "5"))
            .and(query_param("Limit", "2"))
            .and(header_regex("authorization", "Token=\"token-one\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 20,
                "Items": [{
                    "Id": "album-one",
                    "Name": "Blue Rooms",
                    "Type": "MusicAlbum",
                    "AlbumArtist": "Astral Kin",
                    "ProductionYear": 2024,
                    "ChildCount": 9,
                    "RunTimeTicks": 1800000000i64,
                    "UserData": { "IsFavorite": true }
                }]
            })))
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        let page = provider
            .albums(PagedRequest::new(5, 2))
            .await
            .expect("albums");

        assert_eq!(page.total, 20);
        assert_eq!(page.items[0].id.as_str(), "jellyfin:album:album-one");
        assert_eq!(page.items[0].title, "Blue Rooms");
        assert!(page.items[0].favorite);
    }

    #[tokio::test]
    async fn album_detail_loads_album_and_matching_tracks() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items/album-one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Id": "album-one",
                "Name": "Blue Rooms",
                "Type": "MusicAlbum",
                "AlbumArtist": "Astral Kin",
                "ChildCount": 1
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("ParentId", "album-one"))
            .and(query_param("IncludeItemTypes", "Audio"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 1,
                "Items": [{
                    "Id": "track-one",
                    "Name": "First Motion",
                    "Type": "Audio",
                    "AlbumId": "album-one",
                    "Album": "Blue Rooms",
                    "Artists": ["Astral Kin"],
                    "IndexNumber": 1,
                    "RunTimeTicks": 2100000000i64
                }]
            })))
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        let detail = provider
            .album_detail(&AlbumId::new("jellyfin:album:album-one"))
            .await
            .expect("detail");

        assert_eq!(detail.album.id.as_str(), "jellyfin:album:album-one");
        assert_eq!(
            detail.tracks[0].album_id.as_str(),
            "jellyfin:album:album-one"
        );
        assert_eq!(detail.tracks[0].duration_seconds, 210);
    }

    #[tokio::test]
    async fn artists_playlists_and_favorites_map_common_counts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Artists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "TotalRecordCount": 1,
                "Items": [{
                    "Id": "artist-one",
                    "Name": "Astral Kin",
                    "ItemCounts": {
                        "AlbumCount": 4,
                        "SongCount": 30
                    },
                    "UserData": { "IsFavorite": true }
                }]
            })))
            .mount(&server)
            .await;
        let provider = provider(&server, "token-one");

        let artists = provider
            .artists(PagedRequest::new(0, 50))
            .await
            .expect("artists");

        assert_eq!(artists.items[0].id.as_str(), "jellyfin:artist:artist-one");
        assert_eq!(artists.items[0].album_count, 4);
        assert_eq!(artists.items[0].track_count, 30);
        assert!(artists.items[0].favorite);
    }

    #[tokio::test]
    async fn auth_and_server_errors_are_distinct() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad token"))
            .mount(&server)
            .await;
        let provider = provider(&server, "bad-token");

        let error = provider
            .albums(PagedRequest::new(0, 1))
            .await
            .expect_err("auth error");

        assert!(matches!(error, ProviderError::Auth(_)));
    }

    #[tokio::test]
    async fn stream_url_uses_direct_audio_endpoint_and_redacts_token() {
        let server = MockServer::start().await;
        let provider = provider(&server, "secret-token");

        let stream = provider
            .stream(&TrackId::new("jellyfin:track:track-one"))
            .await
            .expect("stream");

        assert!(
            stream
                .uri()
                .starts_with(&format!("{}/Audio/track-one/stream?", server.uri()))
        );
        assert!(stream.uri().contains("api_key=secret-token"));
        assert!(stream.redacted_uri().contains("api_key=%3Credacted%3E"));
        assert!(!format!("{stream:?}").contains("secret-token"));
    }

    fn provider(server: &MockServer, token: &str) -> JellyfinProvider {
        JellyfinProvider::from_saved_session(SavedProviderSession {
            server: ServerIdentity {
                id: ServerId::new("jellyfin:server:test"),
                provider: "jellyfin".to_string(),
                name: "Test".to_string(),
                base_url: server.uri(),
            },
            user_id: "user-one".to_string(),
            username: "demo".to_string(),
            trust_invalid_cert: false,
            access_token: token.to_string(),
        })
        .expect("provider")
    }
}

use source::{
    MusicSource, SavedSourceSession, SourceResult, SourceSession, StreamDescriptor, StreamRequest,
};
use source_jellyfin::JellyfinSource;
pub use source_jellyfin::{
    DiscoveredJellyfinServer, JellyfinLibraryChange, JellyfinLyricsSearch,
    discover_jellyfin_servers,
};
use source_local::{LOCAL_SOURCE_ID, LocalSource};
use source_subsonic::{SubsonicFlavor, SubsonicLoginRequest, SubsonicSource};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamingSource {
    Jellyfin,
    Navidrome,
    Subsonic,
    Local,
}

impl StreamingSource {
    pub const ALL: [Self; 4] = [Self::Jellyfin, Self::Navidrome, Self::Subsonic, Self::Local];

    pub fn from_source_id(source_id: &str) -> Option<Self> {
        match source_id {
            "jellyfin" => Some(Self::Jellyfin),
            "navidrome" => Some(Self::Navidrome),
            "subsonic" | "opensubsonic" => Some(Self::Subsonic),
            LOCAL_SOURCE_ID => Some(Self::Local),
            _ => None,
        }
    }

    pub fn source_id(self) -> &'static str {
        match self {
            Self::Jellyfin => "jellyfin",
            Self::Navidrome => "navidrome",
            Self::Subsonic => "subsonic",
            Self::Local => LOCAL_SOURCE_ID,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Jellyfin => "Jellyfin",
            Self::Navidrome => "Navidrome",
            Self::Subsonic => "Subsonic / OpenSubsonic",
            Self::Local => "Local",
        }
    }

    fn subsonic_flavor(self) -> Option<SubsonicFlavor> {
        match self {
            Self::Jellyfin => None,
            Self::Navidrome => Some(SubsonicFlavor::Navidrome),
            Self::Subsonic => Some(SubsonicFlavor::Subsonic),
            Self::Local => None,
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub enum LoadedSource {
    Jellyfin(JellyfinSource),
    Local(LocalSource),
    Subsonic(SubsonicSource),
}

impl LoadedSource {
    pub fn as_music_source(&self) -> &dyn MusicSource {
        match self {
            Self::Jellyfin(source) => source,
            Self::Local(source) => source,
            Self::Subsonic(source) => source,
        }
    }

    pub async fn lyrics_with_search(
        &self,
        track_id: &domain::TrackId,
        search: JellyfinLyricsSearch,
    ) -> SourceResult<Option<source::Lyrics>> {
        match self {
            Self::Jellyfin(source) => source.lyrics_with_search(track_id, search).await,
            Self::Local(_) => Ok(None),
            Self::Subsonic(source) => {
                let allow_remote = matches!(
                    search,
                    JellyfinLyricsSearch::ServerThenRemote | JellyfinLyricsSearch::RemoteThenServer
                );
                source.lyrics(track_id, allow_remote).await
            }
        }
    }
}

pub async fn login_source(
    source: StreamingSource,
    base_url: String,
    username: String,
    password: String,
    trust_invalid_cert: bool,
    device_id: Option<String>,
) -> SourceResult<SourceSession> {
    if source == StreamingSource::Local {
        return Err(source::SourceError::Unsupported("local login"));
    }
    if let Some(flavor) = source.subsonic_flavor() {
        return SubsonicSource::login(SubsonicLoginRequest {
            base_url,
            username,
            password,
            trust_invalid_cert,
            flavor,
        })
        .await;
    }

    JellyfinSource::login(source::LoginRequest {
        base_url,
        username,
        password,
        trust_invalid_cert,
        device_id,
    })
    .await
}

pub fn source_from_saved(session: SavedSourceSession) -> SourceResult<LoadedSource> {
    match StreamingSource::from_source_id(&session.server.provider) {
        Some(StreamingSource::Jellyfin) => {
            JellyfinSource::from_saved_session(session).map(LoadedSource::Jellyfin)
        }
        Some(StreamingSource::Local) => {
            LocalSource::from_server(session.server).map(LoadedSource::Local)
        }
        Some(StreamingSource::Navidrome | StreamingSource::Subsonic) => {
            SubsonicSource::from_saved_session(session).map(LoadedSource::Subsonic)
        }
        None => Err(source::SourceError::Unsupported("saved source type")),
    }
}

pub fn jellyfin_stream_descriptor_from_saved_session(
    session: &SavedSourceSession,
    request: &StreamRequest,
) -> SourceResult<StreamDescriptor> {
    JellyfinSource::stream_descriptor_from_saved_session(session, request)
}

pub fn source_display_name(source_id: &str) -> &'static str {
    StreamingSource::from_source_id(source_id)
        .map(StreamingSource::title)
        .unwrap_or("Music Server")
}

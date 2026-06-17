use source::{
    MusicProvider, ProviderResult, ProviderSession, SavedProviderSession, StreamDescriptor,
    StreamRequest,
};
use source_jellyfin::JellyfinProvider;
pub use source_jellyfin::{
    DiscoveredJellyfinServer, JellyfinLyricsSearch, discover_jellyfin_servers,
};
use source_local::{LOCAL_PROVIDER_ID, LocalProvider};
use source_subsonic::{SubsonicFlavor, SubsonicLoginRequest, SubsonicProvider};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamingProvider {
    Jellyfin,
    Navidrome,
    Subsonic,
    Local,
}

impl StreamingProvider {
    pub const ALL: [Self; 4] = [Self::Jellyfin, Self::Navidrome, Self::Subsonic, Self::Local];

    pub fn from_index(index: u32) -> Self {
        Self::ALL
            .get(index as usize)
            .copied()
            .unwrap_or(Self::Jellyfin)
    }

    pub fn from_provider_id(provider: &str) -> Option<Self> {
        match provider {
            "jellyfin" => Some(Self::Jellyfin),
            "navidrome" => Some(Self::Navidrome),
            "subsonic" | "opensubsonic" => Some(Self::Subsonic),
            LOCAL_PROVIDER_ID => Some(Self::Local),
            _ => None,
        }
    }

    pub fn provider_id(self) -> &'static str {
        match self {
            Self::Jellyfin => "jellyfin",
            Self::Navidrome => "navidrome",
            Self::Subsonic => "subsonic",
            Self::Local => LOCAL_PROVIDER_ID,
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
pub enum LoadedProvider {
    Jellyfin(JellyfinProvider),
    Local(LocalProvider),
    Subsonic(SubsonicProvider),
}

impl LoadedProvider {
    pub fn as_music_provider(&self) -> &dyn MusicProvider {
        match self {
            Self::Jellyfin(provider) => provider,
            Self::Local(provider) => provider,
            Self::Subsonic(provider) => provider,
        }
    }

    pub async fn lyrics_with_search(
        &self,
        track_id: &domain::TrackId,
        search: JellyfinLyricsSearch,
    ) -> ProviderResult<Option<source::Lyrics>> {
        if !self.as_music_provider().capabilities().lyrics {
            return Ok(None);
        }
        match self {
            Self::Jellyfin(provider) => provider.lyrics_with_search(track_id, search).await,
            Self::Local(provider) => {
                let allow_remote = matches!(
                    search,
                    JellyfinLyricsSearch::ServerThenRemote | JellyfinLyricsSearch::RemoteThenServer
                );
                provider.lyrics(track_id, allow_remote).await
            }
            Self::Subsonic(provider) => {
                let allow_remote = matches!(
                    search,
                    JellyfinLyricsSearch::ServerThenRemote | JellyfinLyricsSearch::RemoteThenServer
                );
                provider.lyrics(track_id, allow_remote).await
            }
        }
    }
}

pub async fn login_provider(
    provider: StreamingProvider,
    base_url: String,
    username: String,
    password: String,
    trust_invalid_cert: bool,
    device_id: Option<String>,
) -> ProviderResult<ProviderSession> {
    if provider == StreamingProvider::Local {
        return Err(source::ProviderError::Unsupported("local login"));
    }
    if let Some(flavor) = provider.subsonic_flavor() {
        return SubsonicProvider::login(SubsonicLoginRequest {
            base_url,
            username,
            password,
            trust_invalid_cert,
            flavor,
        })
        .await;
    }

    JellyfinProvider::login(source::LoginRequest {
        base_url,
        username,
        password,
        trust_invalid_cert,
        device_id,
    })
    .await
}

pub fn provider_from_saved(session: SavedProviderSession) -> ProviderResult<LoadedProvider> {
    match StreamingProvider::from_provider_id(&session.server.provider) {
        Some(StreamingProvider::Jellyfin) => {
            JellyfinProvider::from_saved_session(session).map(LoadedProvider::Jellyfin)
        }
        Some(StreamingProvider::Local) => {
            LocalProvider::from_server(session.server).map(LoadedProvider::Local)
        }
        Some(StreamingProvider::Navidrome | StreamingProvider::Subsonic) => {
            SubsonicProvider::from_saved_session(session).map(LoadedProvider::Subsonic)
        }
        None => Err(source::ProviderError::Unsupported("saved provider type")),
    }
}

pub fn jellyfin_stream_descriptor_from_saved_session(
    session: &SavedProviderSession,
    request: &StreamRequest,
) -> ProviderResult<StreamDescriptor> {
    JellyfinProvider::stream_descriptor_from_saved_session(session, request)
}

pub fn provider_display_name(provider_id: &str) -> &'static str {
    StreamingProvider::from_provider_id(provider_id)
        .map(StreamingProvider::title)
        .unwrap_or("Music Server")
}

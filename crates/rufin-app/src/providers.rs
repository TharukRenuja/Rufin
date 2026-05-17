use rufin_provider::{MusicProvider, ProviderResult, ProviderSession, SavedProviderSession};
use rufin_provider_jellyfin::JellyfinProvider;
pub use rufin_provider_jellyfin::{
    DiscoveredJellyfinServer, JellyfinLyricsSearch, discover_jellyfin_servers,
};
use rufin_provider_subsonic::{SubsonicFlavor, SubsonicLoginRequest, SubsonicProvider};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamingProvider {
    Jellyfin,
    Navidrome,
    Subsonic,
}

impl StreamingProvider {
    pub const ALL: [Self; 3] = [Self::Jellyfin, Self::Navidrome, Self::Subsonic];

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
            _ => None,
        }
    }

    pub fn provider_id(self) -> &'static str {
        match self {
            Self::Jellyfin => "jellyfin",
            Self::Navidrome => "navidrome",
            Self::Subsonic => "subsonic",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Jellyfin => "Jellyfin",
            Self::Navidrome => "Navidrome",
            Self::Subsonic => "Subsonic / OpenSubsonic",
        }
    }

    fn subsonic_flavor(self) -> Option<SubsonicFlavor> {
        match self {
            Self::Jellyfin => None,
            Self::Navidrome => Some(SubsonicFlavor::Navidrome),
            Self::Subsonic => Some(SubsonicFlavor::Subsonic),
        }
    }
}

pub enum LoadedProvider {
    Jellyfin(JellyfinProvider),
    Subsonic(SubsonicProvider),
}

impl LoadedProvider {
    pub fn as_music_provider(&self) -> &dyn MusicProvider {
        match self {
            Self::Jellyfin(provider) => provider,
            Self::Subsonic(provider) => provider,
        }
    }

    pub async fn lyrics_with_search(
        &self,
        track_id: &rufin_core::TrackId,
        search: JellyfinLyricsSearch,
    ) -> ProviderResult<Option<rufin_provider::Lyrics>> {
        match self {
            Self::Jellyfin(provider) => provider.lyrics_with_search(track_id, search).await,
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
) -> ProviderResult<ProviderSession> {
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

    JellyfinProvider::login(rufin_provider::LoginRequest {
        base_url,
        username,
        password,
        trust_invalid_cert,
    })
    .await
}

pub fn provider_from_saved(session: SavedProviderSession) -> ProviderResult<LoadedProvider> {
    match StreamingProvider::from_provider_id(&session.server.provider) {
        Some(StreamingProvider::Jellyfin) => {
            JellyfinProvider::from_saved_session(session).map(LoadedProvider::Jellyfin)
        }
        Some(StreamingProvider::Navidrome | StreamingProvider::Subsonic) => {
            SubsonicProvider::from_saved_session(session).map(LoadedProvider::Subsonic)
        }
        None => Err(rufin_provider::ProviderError::Unsupported(
            "saved provider type",
        )),
    }
}

pub fn provider_display_name(provider_id: &str) -> &'static str {
    StreamingProvider::from_provider_id(provider_id)
        .map(StreamingProvider::title)
        .unwrap_or("Music Server")
}

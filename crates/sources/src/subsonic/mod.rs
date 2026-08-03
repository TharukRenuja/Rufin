use crate::config::{decode_provider_payload, require_payload_version};
use crate::policy::{raw_item_id, stable_hash, u16_from_option};
use crate::{
    ConnectedSource, CredentialHostInput, CredentialSettingsInput, GeneratedTracksRequest,
    ImageBytes, LyricsSearch, NativeLyricAgent, NativeLyricAgentRole, NativeLyricCue,
    NativeLyricCueLine, NativeLyricLine, NativeLyrics, NativeLyricsDocument, NativeLyricsOrigin,
    NativeLyricsRole, PlaybackReport, PlaybackReportKind, PlayedFilter, RandomTrackRequest,
    SourceConfiguration, SourceEditResult, SourceError, SourceResult,
};
use library::{
    Album, AlbumId, AlbumRelations, Artist, ArtistCredit, ArtistId, FavoriteItemId, Folder,
    FolderId, Genre, GenreCredit, GenreId, HomeItemId, ImageRef, MoodCredit, MoodId, MusicFolder,
    MusicFolderId, Playlist, PlaylistEntry, PlaylistId, PlaylistSnapshot, ResolvedStream,
    SourceHomeSection, SourceHomeSectionKind, SourceId, StreamRequest, Track, TrackData, TrackId,
    TrackRelations, normalize_release_types,
};
use reqwest::{Client, Url};
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::instrument;

mod client;
mod item;
mod navidrome;
mod refresh;

use client::*;
use item::*;

#[cfg(test)]
mod tests;

const SOURCE_CONFIG_VERSION: u32 = 1;
const NAVIDROME_LIBRARY_VERSION: u32 = 1;

#[derive(Deserialize)]
struct SubsonicSourcePayload {
    version: u32,
    base_url: String,
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    username: String,
    trust_invalid_cert: bool,
    #[serde(default)]
    navidrome_library_version: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubsonicSourceConfig {
    pub(crate) base_url: String,
    pub(crate) username: String,
    pub(crate) trust_invalid_cert: bool,
    pub(crate) navidrome_library_version: u32,
}

impl SubsonicSourceConfig {
    pub fn from_configuration(stored: &crate::SourceConfiguration) -> SourceResult<Self> {
        if !matches!(stored.kind.as_str(), "navidrome" | "subsonic") {
            return Err(SourceError::InvalidConfig(format!(
                "expected a Subsonic source, found {}",
                stored.kind
            )));
        }
        let payload: SubsonicSourcePayload = decode_provider_payload(stored)?;
        require_payload_version(payload.version, SOURCE_CONFIG_VERSION)?;
        let username = if payload.username.trim().is_empty() {
            payload.user_id
        } else {
            payload.username
        };
        if username.trim().is_empty() {
            return Err(SourceError::InvalidConfig(
                "the saved OpenSubsonic username is missing".to_string(),
            ));
        }
        Ok(Self {
            base_url: payload.base_url,
            username,
            trust_invalid_cert: payload.trust_invalid_cert,
            navidrome_library_version: payload.navidrome_library_version,
        })
    }

    pub(crate) fn into_payload(self) -> serde_json::Value {
        serde_json::json!({
            "version": SOURCE_CONFIG_VERSION,
            "base_url": self.base_url,
            "username": self.username,
            "trust_invalid_cert": self.trust_invalid_cert,
            "navidrome_library_version": self.navidrome_library_version,
        })
    }

    pub(crate) fn same_account(&self, other: &Self) -> SourceResult<bool> {
        let current = rest_endpoint_identity(&normalize_base_url(&self.base_url)?);
        let next = rest_endpoint_identity(&normalize_base_url(&other.base_url)?);
        Ok(self.username == other.username && current == next)
    }
}

struct AuthenticatedSubsonic {
    configuration: SourceConfiguration,
    source: SubsonicSource,
    credential: String,
}

impl AuthenticatedSubsonic {
    fn connected(mut self, source_id: Option<SourceId>) -> ConnectedSource {
        if let Some(source_id) = source_id {
            self.configuration.source_id = source_id;
        }
        ConnectedSource::subsonic(self.configuration, self.source, Some(self.credential))
    }
}

pub(crate) async fn connect(
    flavor: SubsonicFlavor,
    credentials: CredentialHostInput,
) -> SourceResult<ConnectedSource> {
    SubsonicSource::authenticate(flavor, credentials)
        .await
        .map(|authenticated| authenticated.connected(None))
}

pub(crate) fn open(
    configuration: &SourceConfiguration,
    credential: Option<String>,
) -> SourceResult<SubsonicSource> {
    let config = SubsonicSourceConfig::from_configuration(configuration)?;
    let credential = credential.ok_or_else(|| {
        SourceError::InvalidConfig("saved OpenSubsonic credentials are missing".to_string())
    })?;
    SubsonicSource::open(
        SubsonicFlavor::from_source_id(&configuration.kind)?,
        config,
        credential,
    )
}

pub(crate) async fn edit(
    current: SourceConfiguration,
    current_credential: Option<String>,
    credentials: CredentialSettingsInput,
) -> SourceResult<SourceEditResult> {
    let flavor = SubsonicFlavor::from_source_id(&current.kind)?;
    crate::source::require_source_edit(&current, flavor.source_id())?;
    let saved = SubsonicSourceConfig::from_configuration(&current)?;
    let name = crate::source::edited_source_name(&credentials.name, &current.name);
    let address_changed = crate::source::comparable_address(&credentials.base_url)
        != crate::source::comparable_address(&saved.base_url);
    let username_changed = credentials.username.trim() != saved.username;
    let has_password = !credentials.password.is_empty();

    if (address_changed || username_changed) && !has_password {
        return Err(SourceError::Other(
            "Enter the server password to save address or username changes.".to_string(),
        ));
    }

    if has_password {
        let authenticated = SubsonicSource::authenticate(
            flavor,
            CredentialHostInput {
                server_name: Some(name),
                server_url: credentials.base_url,
                username: credentials.username,
                password: credentials.password,
                trust_invalid_cert: credentials.trust_invalid_cert,
            },
        )
        .await?;
        let next = SubsonicSourceConfig::from_configuration(&authenticated.configuration)?;
        let source_id = if saved.same_account(&next)? {
            Some(current.source_id)
        } else {
            None
        };
        return Ok(SourceEditResult::Connected(Box::new(
            authenticated.connected(source_id),
        )));
    }

    let reopen = credentials.trust_invalid_cert != saved.trust_invalid_cert;
    let configuration = crate::config::encode_provider_payload(
        current.source_id.clone(),
        flavor.source_id(),
        name,
        SubsonicSourceConfig {
            base_url: saved.base_url,
            username: saved.username,
            trust_invalid_cert: credentials.trust_invalid_cert,
            navidrome_library_version: saved.navidrome_library_version,
        }
        .into_payload(),
    );
    if configuration == current {
        return Ok(SourceEditResult::Unchanged);
    }
    if !reopen {
        return Ok(SourceEditResult::ConfigurationOnly(configuration));
    }
    let source = open(&configuration, current_credential)?;
    Ok(SourceEditResult::Connected(Box::new(
        ConnectedSource::subsonic(configuration, source, None),
    )))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubsonicFlavor {
    Navidrome,
    Subsonic,
}
impl SubsonicFlavor {
    pub fn source_id(self) -> &'static str {
        match self {
            Self::Navidrome => "navidrome",
            Self::Subsonic => "subsonic",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Navidrome => "Navidrome",
            Self::Subsonic => "OpenSubsonic",
        }
    }

    pub(crate) fn from_source_id(value: &str) -> SourceResult<Self> {
        match value {
            "navidrome" => Ok(Self::Navidrome),
            "subsonic" => Ok(Self::Subsonic),
            _ => Err(SourceError::InvalidConfig(format!(
                "unknown OpenSubsonic source kind {value}"
            ))),
        }
    }
}
#[derive(Debug)]
pub struct SubsonicSource {
    client: Client,
    base_url: Url,
    username: String,
    credential: Arc<SubsonicCredential>,
    navidrome_session: navidrome::NavidromeSession,
    navidrome_library_version: u32,
    flavor: SubsonicFlavor,
    trust_invalid_cert: bool,
    metadata_editing: AtomicBool,
}
impl SubsonicSource {
    fn open(
        flavor: SubsonicFlavor,
        config: SubsonicSourceConfig,
        credential: String,
    ) -> SourceResult<Self> {
        let base_url = normalize_base_url(&config.base_url)?;
        let client = build_client(config.trust_invalid_cert)?;
        let credential = SubsonicCredential::parse(&credential)?;
        if config.navidrome_library_version > NAVIDROME_LIBRARY_VERSION {
            return Err(SourceError::InvalidConfig(format!(
                "Navidrome library version {} is not supported.",
                config.navidrome_library_version
            )));
        }
        if config.navidrome_library_version > 0
            && (flavor != SubsonicFlavor::Navidrome || credential.navidrome_password().is_none())
        {
            return Err(SourceError::InvalidConfig(
                "Saved Navidrome access needs the server password.".to_string(),
            ));
        }
        Ok(Self {
            client,
            base_url,
            username: config.username,
            credential: Arc::new(credential),
            navidrome_session: navidrome::NavidromeSession::default(),
            navidrome_library_version: config.navidrome_library_version,
            flavor,
            trust_invalid_cert: config.trust_invalid_cert,
            metadata_editing: AtomicBool::new(false),
        })
    }

    #[instrument(skip(credentials), fields(base_url = %credentials.server_url, username = %credentials.username, source_kind = flavor.source_id(), trust_invalid_cert = credentials.trust_invalid_cert))]
    async fn authenticate(
        flavor: SubsonicFlavor,
        credentials: CredentialHostInput,
    ) -> SourceResult<AuthenticatedSubsonic> {
        let CredentialHostInput {
            server_name: submitted_name,
            server_url,
            username,
            password,
            trust_invalid_cert,
        } = credentials;
        let base_url = normalize_base_url(&server_url)?;
        let client = build_client(trust_invalid_cert)?;
        let credential = if flavor == SubsonicFlavor::Navidrome {
            SubsonicCredential::from_navidrome_password(&password)
        } else {
            SubsonicCredential::from_password(&password)
        };
        let mut auth_url = endpoint(&base_url, "getUser")?;
        auth_url
            .query_pairs_mut()
            .extend_pairs(credential.common_query(&username, &[("username", &username)]));
        let response = subsonic_json::<AuthenticateBody>(client.get(auth_url)).await?;
        let body = response.body;
        if body.user.username.trim().is_empty() {
            return Err(SourceError::Auth(
                "OpenSubsonic returned an empty canonical username".to_string(),
            ));
        }
        let canonical_username = body.user.username;
        let provider_name = response
            .server_type
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| flavor.display_name().to_string());
        let metadata_editing = body.user.admin_role;
        let source_kind = flavor.source_id();
        let rest_endpoint = rest_endpoint_identity(&base_url);
        let source_hash = stable_source_id(source_kind, &rest_endpoint, &canonical_username);
        let serialized_credential = credential.serialize();
        let configuration = crate::config::encode_provider_payload(
            SourceId::new(format!("{source_kind}:server:{source_hash}")),
            source_kind,
            crate::source::configured_source_name(submitted_name, provider_name),
            SubsonicSourceConfig {
                base_url: base_url.as_str().trim_end_matches('/').to_string(),
                username: canonical_username.clone(),
                trust_invalid_cert,
                navidrome_library_version: if flavor == SubsonicFlavor::Navidrome {
                    NAVIDROME_LIBRARY_VERSION
                } else {
                    0
                },
            }
            .into_payload(),
        );
        let source = Self {
            client,
            base_url,
            username: canonical_username,
            credential: Arc::new(credential),
            navidrome_session: navidrome::NavidromeSession::default(),
            navidrome_library_version: if flavor == SubsonicFlavor::Navidrome {
                NAVIDROME_LIBRARY_VERSION
            } else {
                0
            },
            flavor,
            trust_invalid_cert,
            metadata_editing: AtomicBool::new(metadata_editing),
        };
        Ok(AuthenticatedSubsonic {
            configuration,
            source,
            credential: serialized_credential,
        })
    }

    pub(crate) fn metadata_editing_available(&self) -> bool {
        self.metadata_editing.load(Ordering::Acquire)
    }

    pub(crate) async fn refresh_metadata_editing(&self) {
        let available = self
            .get_json::<AuthenticateBody>("getUser", &[("username", self.username.clone())])
            .await
            .is_ok_and(|body| body.user.admin_role);
        self.metadata_editing.store(available, Ordering::Release);
    }
}

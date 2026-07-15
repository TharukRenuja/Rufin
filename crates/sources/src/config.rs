use std::path::PathBuf;

use library::{SourceId, StoredSource};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{SourceError, SourceResult, subsonic::SubsonicFlavor};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceIdentity {
    pub id: SourceId,
    pub kind: String,
    pub name: String,
    pub base_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialSourceConfig {
    pub source: SourceIdentity,
    pub user_id: String,
    pub username: String,
    pub trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LibrarySourceSelection {
    Local,
    #[serde(alias = "Server")]
    Source(SourceId),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalLibraryFolder {
    pub path: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LibrarySourceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<LibrarySourceSelection>,
    #[serde(default)]
    pub local_folders: Vec<LocalLibraryFolder>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialHostInput {
    pub server_name: Option<String>,
    pub server_url: String,
    pub username: String,
    pub password: String,
    pub trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialHostPreset {
    pub server_name: String,
    pub server_url: String,
    pub username: String,
    pub trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JellyfinSetupInput {
    pub credentials: CredentialHostInput,
    pub use_instant_mix: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalFolderHostInput {
    pub roots: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialSettingsInput {
    pub source_id: SourceId,
    pub name: String,
    pub base_url: String,
    pub username: String,
    pub password: String,
    pub trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JellyfinSettingsInput {
    pub credentials: CredentialSettingsInput,
    pub use_instant_mix: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceSetupInput {
    Jellyfin(JellyfinSetupInput),
    Subsonic {
        flavor: SubsonicFlavor,
        credentials: CredentialHostInput,
    },
    Local(LocalFolderHostInput),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceSettingsInput {
    Jellyfin(JellyfinSettingsInput),
    Subsonic {
        flavor: SubsonicFlavor,
        credentials: CredentialSettingsInput,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditableSource {
    pub source_id: SourceId,
    pub kind: String,
    pub credentials: CredentialHostPreset,
    pub jellyfin_use_instant_mix: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocalAccessInput {
    pub source_id: SourceId,
    pub root_path: PathBuf,
    pub server_prefix: Option<String>,
    pub local_prefix: Option<String>,
}

impl LibrarySourceSettings {
    pub fn sanitize(&mut self) {
        let mut seen = Vec::<String>::new();
        self.local_folders.retain_mut(|folder| {
            folder.path = folder.path.trim().to_string();
            if folder.path.is_empty() || seen.iter().any(|path| path == &folder.path) {
                return false;
            }
            seen.push(folder.path.clone());
            true
        });
    }
}

impl CredentialSourceConfig {
    pub(crate) fn from_stored_fields(
        stored: &StoredSource,
        base_url: String,
        user_id: String,
        username: String,
        trust_invalid_cert: bool,
    ) -> Self {
        Self {
            source: SourceIdentity {
                id: stored.source_id.clone(),
                kind: stored.kind.clone(),
                name: stored.name.clone(),
                base_url,
            },
            user_id,
            username,
            trust_invalid_cert,
        }
    }
}

pub(crate) fn decode_provider_payload<T: DeserializeOwned>(
    stored: &StoredSource,
) -> SourceResult<T> {
    serde_json::from_str(&stored.provider_payload)
        .map_err(|error| SourceError::InvalidConfig(error.to_string()))
}

pub(crate) fn require_payload_version(actual: u32, expected: u32) -> SourceResult<()> {
    if actual != expected {
        return Err(SourceError::InvalidConfig(format!(
            "unsupported payload version {actual}"
        )));
    }
    Ok(())
}

pub(crate) fn encode_provider_payload(
    source: SourceIdentity,
    provider_payload: serde_json::Value,
) -> StoredSource {
    StoredSource {
        source_id: source.id,
        kind: source.kind,
        name: source.name,
        provider_payload: provider_payload.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use library::{SourceId, StoredSource};

    use super::*;
    use crate::jellyfin::JellyfinSourceConfig;
    use crate::local::LocalSourceConfig;
    use crate::subsonic::SubsonicSourceConfig;

    fn migrated_source(kind: &str, base_url: &str) -> StoredSource {
        StoredSource {
            source_id: SourceId::new(format!("{kind}:server:test")),
            kind: kind.to_string(),
            name: "Test Source".to_string(),
            provider_payload: serde_json::json!({
                "version": 1,
                "base_url": base_url,
                "user_id": "account-id",
                "username": "listener",
                "trust_invalid_cert": true,
                "use_jellyfin_instant_mix": true,
            })
            .to_string(),
        }
    }

    #[test]
    fn migrated_payload_decodes_and_round_trips_for_current_providers() {
        let jellyfin = JellyfinSourceConfig::from_stored(&migrated_source(
            "jellyfin",
            "https://jellyfin.example",
        ))
        .expect("Jellyfin migration payload");
        assert_eq!(jellyfin.credentials.user_id, "account-id");
        assert_eq!(jellyfin.credentials.username, "listener");
        assert!(jellyfin.credentials.trust_invalid_cert);
        assert!(jellyfin.use_instant_mix);
        assert_eq!(
            JellyfinSourceConfig::from_stored(&jellyfin.clone().into_stored())
                .expect("round-trip Jellyfin config"),
            jellyfin
        );

        let subsonic = SubsonicSourceConfig::from_stored(&migrated_source(
            "subsonic",
            "https://subsonic.example",
        ))
        .expect("Subsonic migration payload");
        assert_eq!(subsonic.credentials.user_id, "account-id");
        assert_eq!(subsonic.credentials.username, "listener");
        assert!(subsonic.credentials.trust_invalid_cert);
        assert_eq!(
            SubsonicSourceConfig::from_stored(&subsonic.clone().into_stored())
                .expect("round-trip Subsonic config"),
            subsonic
        );

        let local = LocalSourceConfig::from_stored(&migrated_source("local", "/music"))
            .expect("Local migration payload");
        assert_eq!(local.source.base_url, "/music");
        assert_eq!(
            LocalSourceConfig::from_stored(&local.clone().into_stored())
                .expect("round-trip Local config"),
            local
        );
    }

    #[test]
    fn unsupported_provider_payload_version_is_rejected() {
        let mut stored = migrated_source("jellyfin", "https://music.example");
        stored.provider_payload = serde_json::json!({
            "version": 2,
            "base_url": "https://music.example",
            "user_id": "account-id",
            "username": "listener",
            "trust_invalid_cert": false,
            "use_jellyfin_instant_mix": false,
        })
        .to_string();

        let error =
            JellyfinSourceConfig::from_stored(&stored).expect_err("unsupported payload version");
        assert!(matches!(error, SourceError::InvalidConfig(_)));
    }

    #[test]
    fn library_source_settings_sanitize_folders() {
        let mut settings = LibrarySourceSettings {
            selected: None,
            local_folders: vec![
                LocalLibraryFolder {
                    path: " /music ".to_string(),
                },
                LocalLibraryFolder {
                    path: "/music".to_string(),
                },
                LocalLibraryFolder {
                    path: " ".to_string(),
                },
                LocalLibraryFolder {
                    path: "/archive".to_string(),
                },
            ],
        };

        settings.sanitize();

        assert_eq!(
            settings.local_folders,
            vec![
                LocalLibraryFolder {
                    path: "/music".to_string()
                },
                LocalLibraryFolder {
                    path: "/archive".to_string()
                }
            ]
        );
    }

    #[test]
    fn library_source_settings_preserve_stored_shape_and_server_alias() {
        let settings = serde_json::from_value::<LibrarySourceSettings>(serde_json::json!({
            "selected": { "Server": "remote-source" },
            "local_folders": [{ "path": "/music" }]
        }))
        .expect("deserialize legacy source selection");
        assert_eq!(
            settings.selected,
            Some(LibrarySourceSelection::Source(SourceId::new(
                "remote-source"
            )))
        );
        assert_eq!(
            serde_json::to_value(settings).expect("serialize source settings"),
            serde_json::json!({
                "selected": { "Source": "remote-source" },
                "local_folders": [{ "path": "/music" }]
            })
        );
        assert_eq!(
            serde_json::to_value(LibrarySourceSettings::default())
                .expect("serialize default source settings"),
            serde_json::json!({ "local_folders": [] })
        );
    }
}

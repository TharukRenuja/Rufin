mod active;
pub(crate) use active::*;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use library::StoredSource;
use library::{FolderDetail, FolderId, ImageRef, MusicFolderId, SourceId};
use secrets::SecretStore;
use sources::jellyfin::{
    JELLYFIN_SOURCE_ID, JellyfinConfiguredSession, JellyfinLoginRequest, JellyfinLoginSession,
    JellyfinSource, JellyfinSourceConfig,
};
use sources::local::{
    LOCAL_SOURCE_ID, LocalChangeFeed, LocalScanProgress, LocalSource, LocalSourceConfig,
};
use sources::subsonic::{
    SubsonicConfiguredSession, SubsonicFlavor, SubsonicLoginRequest, SubsonicLoginSession,
    SubsonicSource, SubsonicSourceConfig,
};
use sources::{
    CredentialHostInput, CredentialHostPreset, CredentialSettingsInput, EditableSource,
    JellyfinSettingsInput, JellyfinSetupInput, LocalFolderHostInput, SourceSettingsInput,
    SourceSetupInput,
};
use sources::{
    GeneratedTrackSeedKind, GeneratedTrackStrategy, ImageBytes, LibrarySourceSelection,
    PlayedFilter, RandomTrackDomain, SourceIdentity, StreamDescriptor, StreamRequest,
};
use tokio::runtime::Runtime;

use crate::controller::{SourceCommands, StoreHandle};

use sources::{CredentialSourceConfig, FolderBrowser, ImageProvider, MusicSource, StreamResolver};

pub(crate) type LocalLoader = Arc<
    dyn Fn(
            &mut dyn FnMut(LocalScanProgress),
            &library_sync::CancellationToken,
        ) -> sources::SourceResult<LocalSource>
        + Send
        + Sync,
>;
pub(crate) type LocalRootsLoader = Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>;

const PLAYED_ALL: &[PlayedFilter] = &[PlayedFilter::All];
const PLAYED_ALL_FILTERS: &[PlayedFilter] = &[
    PlayedFilter::All,
    PlayedFilter::Unplayed,
    PlayedFilter::Played,
];
const LOCAL_RADIO_SEEDS: &[GeneratedTrackSeedKind] = &[
    GeneratedTrackSeedKind::Track,
    GeneratedTrackSeedKind::Album,
    GeneratedTrackSeedKind::Artist,
    GeneratedTrackSeedKind::Genre,
];
const REMOTE_RADIO_SEEDS: &[GeneratedTrackSeedKind] = &[
    GeneratedTrackSeedKind::Track,
    GeneratedTrackSeedKind::Album,
    GeneratedTrackSeedKind::Artist,
    GeneratedTrackSeedKind::Genre,
    GeneratedTrackSeedKind::Playlist,
];
pub(crate) struct AuthenticatedSource {
    pub(crate) saved: StoredSource,
    pub(crate) credential: String,
    pub(crate) active: Arc<ActiveSource>,
    pub(crate) authenticated_source_id: SourceId,
}

pub(crate) fn source_identity_changed(
    previous: &StoredSource,
    next: &StoredSource,
    authenticated_source_id: &SourceId,
) -> bool {
    previous.kind != next.kind
        || previous.source_id != *authenticated_source_id
        || source_account_id(previous) != source_account_id(next)
}

pub(crate) struct PreparedSourceSettingsUpdate {
    pub(crate) previous: StoredSource,
    pub(crate) saved: StoredSource,
    pub(crate) active: Arc<ActiveSource>,
    pub(crate) identity_changed: bool,
    pub(crate) credential: Option<String>,
}

struct CredentialSettingsPreparation {
    previous: StoredSource,
    next: StoredSource,
    reauth: Option<CredentialHostInput>,
    common_changed: bool,
}

type ActivateConfigured =
    fn(&StoreHandle, &Arc<dyn SecretStore>, &StoredSource) -> Result<Arc<ActiveSource>, String>;
type NeedsAuth = fn(&Arc<dyn SecretStore>, &StoredSource) -> Result<bool, String>;
type ConfiguredForSync = fn(&StoreHandle, &StoredSource) -> bool;
type SourceSelection = fn(&StoredSource) -> LibrarySourceSelection;
type DecodeIdentity = fn(&StoredSource) -> Result<SourceIdentity, String>;
type DecodeCredentials = fn(&StoredSource) -> Result<CredentialSourceConfig, String>;
type EncodeCredentials = fn(&StoredSource, CredentialSourceConfig) -> Result<StoredSource, String>;

#[derive(Clone, Copy)]
struct CredentialConfigCodec {
    decode: DecodeCredentials,
    encode: EncodeCredentials,
}

/// Saved-configuration and executable-operation laws for one source type.
struct SourceOperations {
    canonical_kind: &'static str,
    activate: ActivateConfigured,
    needs_auth: NeedsAuth,
    configured_for_sync: ConfiguredForSync,
    selection: SourceSelection,
    identity: DecodeIdentity,
    credentials: Option<CredentialConfigCodec>,
}

static LOCAL: SourceOperations = SourceOperations {
    canonical_kind: LOCAL_SOURCE_ID,
    activate: activate_local_registration,
    needs_auth: local_needs_auth,
    configured_for_sync: local_configured_for_sync,
    selection: local_selection,
    identity: decode_local_identity,
    credentials: None,
};
static JELLYFIN: SourceOperations = SourceOperations {
    canonical_kind: JELLYFIN_SOURCE_ID,
    activate: activate_jellyfin_registration,
    needs_auth: credential_needs_auth,
    configured_for_sync: always_configured_for_sync,
    selection: source_selection,
    identity: decode_jellyfin_identity,
    credentials: Some(CredentialConfigCodec {
        decode: decode_jellyfin_credentials,
        encode: encode_jellyfin_credentials,
    }),
};
static NAVIDROME: SourceOperations = SourceOperations {
    canonical_kind: "navidrome",
    activate: activate_subsonic_registration,
    needs_auth: credential_needs_auth,
    configured_for_sync: always_configured_for_sync,
    selection: source_selection,
    identity: decode_subsonic_identity,
    credentials: Some(CredentialConfigCodec {
        decode: decode_subsonic_credentials,
        encode: encode_subsonic_credentials,
    }),
};
static SUBSONIC: SourceOperations = SourceOperations {
    canonical_kind: "subsonic",
    activate: activate_subsonic_registration,
    needs_auth: credential_needs_auth,
    configured_for_sync: always_configured_for_sync,
    selection: source_selection,
    identity: decode_subsonic_identity,
    credentials: Some(CredentialConfigCodec {
        decode: decode_subsonic_credentials,
        encode: encode_subsonic_credentials,
    }),
};

static SOURCE_OPERATIONS: [&SourceOperations; 4] = [&JELLYFIN, &NAVIDROME, &SUBSONIC, &LOCAL];

fn source_operations(kind: &str) -> Option<&'static SourceOperations> {
    SOURCE_OPERATIONS
        .iter()
        .copied()
        .find(|operations| operations.canonical_kind == kind)
}

pub(crate) fn configured_source_supported(kind: &str) -> bool {
    source_operations(kind).is_some()
}

pub(crate) fn configured_source_ready_for_sync(store: &StoreHandle, saved: &StoredSource) -> bool {
    source_operations(&saved.kind)
        .is_some_and(|operations| (operations.configured_for_sync)(store, saved))
}

fn decode_jellyfin_credentials(saved: &StoredSource) -> Result<CredentialSourceConfig, String> {
    JellyfinSourceConfig::from_stored(saved)
        .map(|config| config.credentials)
        .map_err(|error| error.to_string())
}

fn decode_jellyfin_identity(saved: &StoredSource) -> Result<SourceIdentity, String> {
    decode_jellyfin_credentials(saved).map(|config| config.source)
}

fn encode_jellyfin_credentials(
    saved: &StoredSource,
    credentials: CredentialSourceConfig,
) -> Result<StoredSource, String> {
    let mut config = JellyfinSourceConfig::from_stored(saved).map_err(|error| error.to_string())?;
    config.credentials = credentials;
    Ok(config.into_stored())
}

fn decode_subsonic_credentials(saved: &StoredSource) -> Result<CredentialSourceConfig, String> {
    SubsonicSourceConfig::from_stored(saved)
        .map(|config| config.credentials)
        .map_err(|error| error.to_string())
}

fn decode_subsonic_identity(saved: &StoredSource) -> Result<SourceIdentity, String> {
    decode_subsonic_credentials(saved).map(|config| config.source)
}

fn decode_local_identity(saved: &StoredSource) -> Result<SourceIdentity, String> {
    LocalSourceConfig::from_stored(saved)
        .map(|config| config.source)
        .map_err(|error| error.to_string())
}

fn encode_subsonic_credentials(
    _saved: &StoredSource,
    credentials: CredentialSourceConfig,
) -> Result<StoredSource, String> {
    Ok(SubsonicSourceConfig { credentials }.into_stored())
}

fn credential_config(saved: &StoredSource) -> Result<CredentialSourceConfig, String> {
    let operations = source_operations(&saved.kind)
        .ok_or_else(|| "Saved source type is no longer supported.".to_string())?;
    let codec = operations
        .credentials
        .ok_or_else(|| "Saved source does not use credential settings.".to_string())?;
    (codec.decode)(saved)
}

fn replace_credential_config(
    operations: &SourceOperations,
    saved: &StoredSource,
    credentials: CredentialSourceConfig,
) -> Result<StoredSource, String> {
    let codec = operations
        .credentials
        .ok_or_else(|| "Saved source does not use credential settings.".to_string())?;
    (codec.encode)(saved, credentials)
}

fn source_account_id(saved: &StoredSource) -> Option<String> {
    source_operations(&saved.kind)
        .and_then(|operations| operations.credentials)
        .and_then(|codec| (codec.decode)(saved).ok())
        .map(|config| config.user_id)
}

pub(crate) fn editable_configured_source(saved: &StoredSource) -> Result<EditableSource, String> {
    Ok(EditableSource {
        source_id: saved.source_id.clone(),
        kind: saved.kind.clone(),
        credentials: credential_host_preset(saved)?,
        jellyfin_use_instant_mix: if saved.kind == JELLYFIN_SOURCE_ID {
            Some(
                JellyfinSourceConfig::from_stored(saved)
                    .map_err(|error| error.to_string())?
                    .use_instant_mix,
            )
        } else {
            None
        },
    })
}

pub(crate) fn configured_source_identity(saved: &StoredSource) -> Result<SourceIdentity, String> {
    let operations = source_operations(&saved.kind)
        .ok_or_else(|| "Saved source type is no longer supported.".to_string())?;
    (operations.identity)(saved)
}

pub(crate) fn local_configured_source() -> StoredSource {
    LocalSourceConfig {
        source: local_source_identity(),
    }
    .into_stored()
}

pub(crate) fn local_source_identity() -> SourceIdentity {
    SourceIdentity {
        id: SourceId::new(crate::controller::LOCAL_SOURCE_IDENTITY_ID),
        kind: LOCAL_SOURCE_ID.to_string(),
        name: "Local".to_string(),
        base_url: String::new(),
    }
}

pub(crate) fn ensure_local_configured_source(store: &StoreHandle) -> Result<StoredSource, String> {
    let saved = local_configured_source_for_store(store)?;
    store.with_store(|store| store.save_source(&saved))?;
    Ok(saved)
}

pub(crate) fn local_configured_source_for_store(
    store: &StoreHandle,
) -> Result<StoredSource, String> {
    if !crate::controller::load_settings_from_store(store)
        .sources
        .local_folders
        .is_empty()
    {
        return Ok(local_configured_source());
    }
    let active = store.with_store(|store| store.active_source())?;
    if let Some(saved) = active
        && LOCAL.canonical_kind == saved.kind
    {
        return Ok(saved);
    }
    let saved_sources = store.with_store(|store| store.list_sources())?;
    Ok(saved_sources
        .into_iter()
        .find(|saved| {
            LOCAL.canonical_kind == saved.kind
                && saved.source_id.as_str() != crate::controller::LOCAL_SOURCE_IDENTITY_ID
        })
        .unwrap_or_else(local_configured_source))
}

pub(crate) fn activate_configured_source(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> Result<Arc<ActiveSource>, String> {
    let operations = source_operations(&saved.kind)
        .ok_or_else(|| "Saved source type is no longer supported.".to_string())?;
    (operations.activate)(store, secrets, saved)
}
pub(crate) fn configured_source_needs_auth(
    secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> Result<bool, String> {
    let operations = source_operations(&saved.kind)
        .ok_or_else(|| "Saved source type is no longer supported.".to_string())?;
    (operations.needs_auth)(secrets, saved)
}

pub(crate) fn configured_source_selection(saved: &StoredSource) -> LibrarySourceSelection {
    source_operations(&saved.kind).map_or_else(
        || LibrarySourceSelection::Source(saved.source_id.clone()),
        |operations| (operations.selection)(saved),
    )
}

pub(crate) fn configure_source(controller: &SourceCommands, input: SourceSetupInput) {
    match input {
        SourceSetupInput::Jellyfin(input) => {
            configure_jellyfin_source(controller, "Jellyfin", input);
        }
        SourceSetupInput::Subsonic {
            flavor,
            credentials,
        } => configure_subsonic_source(flavor, controller, flavor.display_name(), credentials),
        SourceSetupInput::Local(input) => configure_local_source(controller, input),
    }
}

pub(crate) fn update_source(controller: &SourceCommands, input: SourceSettingsInput) {
    match input {
        SourceSettingsInput::Jellyfin(input) => {
            update_jellyfin_settings(controller, "Jellyfin", input);
        }
        SourceSettingsInput::Subsonic {
            flavor,
            credentials,
        } => update_subsonic_settings(flavor, controller, flavor.display_name(), credentials),
    }
}

fn configure_local_source(controller: &SourceCommands, input: LocalFolderHostInput) {
    controller.add_library_folders(input.roots);
}

fn activate_local_registration(
    store: &StoreHandle,
    _secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> Result<Arc<ActiveSource>, String> {
    activate_local_saved(store, saved)
}

fn local_needs_auth(
    _secrets: &Arc<dyn SecretStore>,
    _saved: &StoredSource,
) -> Result<bool, String> {
    Ok(false)
}

fn local_selection(_saved: &StoredSource) -> LibrarySourceSelection {
    LibrarySourceSelection::Local
}

fn local_configured_for_sync(store: &StoreHandle, _saved: &StoredSource) -> bool {
    !crate::controller::load_settings_from_store(store)
        .sources
        .local_folders
        .is_empty()
}

fn configure_jellyfin_source(
    controller: &SourceCommands,
    source_name: &'static str,
    input: JellyfinSetupInput,
) {
    controller.configure_authenticated_source(source_name, move |runtime, store| {
        authenticate_jellyfin(runtime, store, input)
    });
}

fn authenticate_jellyfin(
    runtime: &Runtime,
    store: &StoreHandle,
    input: JellyfinSetupInput,
) -> Result<AuthenticatedSource, String> {
    let session = login_jellyfin(runtime, store, &input.credentials)?;
    let mut source = session.source.clone();
    if let Some(name) = input
        .credentials
        .server_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        source.name = name.to_string();
    }
    let saved = JellyfinSourceConfig {
        credentials: CredentialSourceConfig {
            source,
            user_id: session.user_id,
            username: session.username,
            trust_invalid_cert: input.credentials.trust_invalid_cert,
        },
        use_instant_mix: input.use_instant_mix,
    }
    .into_stored();
    authenticated_jellyfin(
        saved,
        session.source.id,
        session.access_token,
        session.device_id,
    )
}

fn update_jellyfin_settings(
    controller: &SourceCommands,
    source_name: &'static str,
    input: JellyfinSettingsInput,
) {
    let source_id = input.credentials.source_id.clone();
    controller.update_source_settings(
        source_id,
        source_name,
        move |runtime, store, secrets, saved, authentication_started| {
            prepare_jellyfin_settings_update(
                runtime,
                store,
                secrets,
                saved,
                input,
                authentication_started,
            )
        },
    );
}

fn prepare_jellyfin_settings_update(
    runtime: &Runtime,
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    saved: StoredSource,
    input: JellyfinSettingsInput,
    authentication_started: &dyn Fn(),
) -> Result<Option<PreparedSourceSettingsUpdate>, String> {
    prepare_jellyfin_settings_update_with_authentication(
        store,
        secrets,
        saved,
        input,
        authentication_started,
        |saved, request| reauthenticate_jellyfin(runtime, store, saved, request),
    )
}

fn prepare_jellyfin_settings_update_with_authentication(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    saved: StoredSource,
    input: JellyfinSettingsInput,
    authentication_started: &dyn Fn(),
    login: impl FnOnce(StoredSource, CredentialHostInput) -> Result<AuthenticatedSource, String>,
) -> Result<Option<PreparedSourceSettingsUpdate>, String> {
    if JELLYFIN.canonical_kind != saved.kind {
        return Err("Saved server source is no longer supported.".to_string());
    }
    let previous_use_instant_mix = JellyfinSourceConfig::from_stored(&saved)
        .map_err(|error| error.to_string())?
        .use_instant_mix;
    let mut prepared = prepare_credential_settings(&JELLYFIN, saved, input.credentials)?;
    let mut next_config =
        JellyfinSourceConfig::from_stored(&prepared.next).map_err(|error| error.to_string())?;
    next_config.use_instant_mix = input.use_instant_mix;
    prepared.next = next_config.into_stored();
    let changed = prepared.common_changed || previous_use_instant_mix != input.use_instant_mix;
    finish_settings_update(
        &JELLYFIN,
        store,
        secrets,
        prepared,
        changed,
        authentication_started,
        login,
    )
}

fn reauthenticate_jellyfin(
    runtime: &Runtime,
    store: &StoreHandle,
    saved: StoredSource,
    input: CredentialHostInput,
) -> Result<AuthenticatedSource, String> {
    let session = login_jellyfin(runtime, store, &input)?;
    let authenticated_source_id = session.source.id.clone();
    let mut config =
        JellyfinSourceConfig::from_stored(&saved).map_err(|error| error.to_string())?;
    config.credentials.source.base_url = session.source.base_url;
    config.credentials.user_id = session.user_id;
    config.credentials.username = session.username;
    let saved = config.into_stored();
    authenticated_jellyfin(
        saved,
        authenticated_source_id,
        session.access_token,
        session.device_id,
    )
}

fn activate_jellyfin_registration(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> Result<Arc<ActiveSource>, String> {
    activate_jellyfin_configured(store, saved, saved_credential(secrets, &saved.source_id)?)
}

fn configure_subsonic_source(
    flavor: SubsonicFlavor,
    controller: &SourceCommands,
    source_name: &'static str,
    input: CredentialHostInput,
) {
    controller.configure_authenticated_source(source_name, move |runtime, _store| {
        authenticate_new_subsonic(runtime, input, flavor)
    });
}

fn update_subsonic_settings(
    flavor: SubsonicFlavor,
    controller: &SourceCommands,
    source_name: &'static str,
    input: CredentialSettingsInput,
) {
    let operations = source_operations(flavor.source_id())
        .expect("configured Subsonic flavor must have source operations");
    let source_id = input.source_id.clone();
    controller.update_source_settings(
        source_id,
        source_name,
        move |runtime, store, secrets, saved, authentication_started| {
            if operations.canonical_kind != saved.kind {
                return Err("Saved server source is no longer supported.".to_string());
            }
            let prepared = prepare_credential_settings(operations, saved, input)?;
            let changed = prepared.common_changed;
            finish_settings_update(
                operations,
                store,
                secrets,
                prepared,
                changed,
                authentication_started,
                |saved, request| reauthenticate_subsonic(runtime, saved, request, flavor),
            )
        },
    );
}

fn activate_subsonic_registration(
    _store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> Result<Arc<ActiveSource>, String> {
    activate_subsonic_configured(saved, saved_credential(secrets, &saved.source_id)?)
}

fn credential_needs_auth(
    secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> Result<bool, String> {
    saved_credential_missing(secrets, &saved.source_id)
}

fn always_configured_for_sync(_store: &StoreHandle, _saved: &StoredSource) -> bool {
    true
}

fn source_selection(saved: &StoredSource) -> LibrarySourceSelection {
    LibrarySourceSelection::Source(saved.source_id.clone())
}

pub(crate) fn credential_host_preset(saved: &StoredSource) -> Result<CredentialHostPreset, String> {
    let config = credential_config(saved)?;
    Ok(CredentialHostPreset {
        server_name: saved.name.clone(),
        server_url: config.source.base_url,
        username: config.username,
        trust_invalid_cert: config.trust_invalid_cert,
    })
}

fn prepare_credential_settings(
    operations: &SourceOperations,
    saved: StoredSource,
    input: CredentialSettingsInput,
) -> Result<CredentialSettingsPreparation, String> {
    let mut config = credential_config(&saved)?;
    let next_name = input.name.trim().to_string();
    let next_base_url = input.base_url.trim().to_string();
    let next_username = input.username.trim().to_string();
    if next_base_url.is_empty() {
        return Err("Enter a server address.".to_string());
    }
    if next_username.is_empty() {
        return Err("Enter a username.".to_string());
    }

    let password_entered = !input.password.is_empty();
    let auth_sensitive = config.source.base_url != next_base_url
        || config.username != next_username
        || password_entered;
    if auth_sensitive && input.password.is_empty() {
        return Err("Enter the server password to save address or username changes.".to_string());
    }
    let common_changed = saved.name != next_name
        || config.source.base_url != next_base_url
        || config.username != next_username
        || config.trust_invalid_cert != input.trust_invalid_cert
        || password_entered;
    let reauth = auth_sensitive.then(|| CredentialHostInput {
        server_name: None,
        server_url: next_base_url.clone(),
        username: next_username.clone(),
        password: input.password,
        trust_invalid_cert: input.trust_invalid_cert,
    });
    let previous = saved;
    config.source.name = next_name;
    config.source.base_url = next_base_url;
    config.username = next_username;
    config.trust_invalid_cert = input.trust_invalid_cert;
    let next = replace_credential_config(operations, &previous, config)?;
    Ok(CredentialSettingsPreparation {
        previous,
        next,
        reauth,
        common_changed,
    })
}

fn finish_settings_update(
    operations: &SourceOperations,
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    prepared: CredentialSettingsPreparation,
    changed: bool,
    authentication_started: &dyn Fn(),
    login: impl FnOnce(StoredSource, CredentialHostInput) -> Result<AuthenticatedSource, String>,
) -> Result<Option<PreparedSourceSettingsUpdate>, String> {
    if !changed {
        return Ok(None);
    }
    let CredentialSettingsPreparation {
        previous,
        next,
        reauth,
        common_changed: _,
    } = prepared;
    let Some(request) = reauth else {
        let active = (operations.activate)(store, secrets, &next)?;
        return Ok(Some(PreparedSourceSettingsUpdate {
            previous,
            saved: next,
            active,
            identity_changed: false,
            credential: None,
        }));
    };
    authentication_started();
    let authenticated = login(next, request)?;
    if operations.canonical_kind != authenticated.saved.kind {
        return Err("Authenticated source did not match the saved server.".to_string());
    }
    let identity_changed = source_identity_changed(
        &previous,
        &authenticated.saved,
        &authenticated.authenticated_source_id,
    );
    Ok(Some(PreparedSourceSettingsUpdate {
        previous,
        saved: authenticated.saved,
        active: authenticated.active,
        identity_changed,
        credential: Some(authenticated.credential),
    }))
}

fn login_jellyfin(
    runtime: &Runtime,
    store: &StoreHandle,
    input: &CredentialHostInput,
) -> Result<JellyfinLoginSession, String> {
    runtime
        .block_on(JellyfinSource::login(JellyfinLoginRequest {
            base_url: input.server_url.clone(),
            username: input.username.clone(),
            password: input.password.clone(),
            trust_invalid_cert: input.trust_invalid_cert,
            device_id: ensure_jellyfin_device_id(store)?,
        }))
        .map_err(|error| error.to_string())
}

fn authenticate_new_subsonic(
    runtime: &Runtime,
    input: CredentialHostInput,
    flavor: SubsonicFlavor,
) -> Result<AuthenticatedSource, String> {
    let server_name = input.server_name.clone();
    let trust_invalid_cert = input.trust_invalid_cert;
    let session = login_subsonic(runtime, input, flavor)?;
    let mut source = session.source.clone();
    if let Some(name) = server_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        source.name = name.to_string();
    }
    let saved = SubsonicSourceConfig {
        credentials: CredentialSourceConfig {
            source,
            user_id: session.username.clone(),
            username: session.username,
            trust_invalid_cert,
        },
    }
    .into_stored();
    authenticated_subsonic(saved, session.source.id, session.credential)
}

fn reauthenticate_subsonic(
    runtime: &Runtime,
    saved: StoredSource,
    input: CredentialHostInput,
    flavor: SubsonicFlavor,
) -> Result<AuthenticatedSource, String> {
    let session = login_subsonic(runtime, input, flavor)?;
    let authenticated_source_id = session.source.id.clone();
    let mut config =
        SubsonicSourceConfig::from_stored(&saved).map_err(|error| error.to_string())?;
    config.credentials.source.base_url = session.source.base_url;
    config.credentials.user_id = session.username.clone();
    config.credentials.username = session.username;
    let saved = config.into_stored();
    authenticated_subsonic(saved, authenticated_source_id, session.credential)
}

fn login_subsonic(
    runtime: &Runtime,
    input: CredentialHostInput,
    flavor: SubsonicFlavor,
) -> Result<SubsonicLoginSession, String> {
    runtime
        .block_on(SubsonicSource::login(SubsonicLoginRequest {
            base_url: input.server_url,
            username: input.username,
            password: input.password,
            trust_invalid_cert: input.trust_invalid_cert,
            flavor,
        }))
        .map_err(|error| error.to_string())
}

fn authenticated_jellyfin(
    saved: StoredSource,
    authenticated_source_id: SourceId,
    credential: String,
    device_id: String,
) -> Result<AuthenticatedSource, String> {
    let active = activate_jellyfin_session(&saved, credential.clone(), device_id)?;
    Ok(AuthenticatedSource {
        saved,
        credential,
        active,
        authenticated_source_id,
    })
}

fn authenticated_subsonic(
    saved: StoredSource,
    authenticated_source_id: SourceId,
    credential: String,
) -> Result<AuthenticatedSource, String> {
    let active = activate_subsonic_configured(&saved, credential.clone())?;
    Ok(AuthenticatedSource {
        saved,
        credential,
        active,
        authenticated_source_id,
    })
}

struct ConfiguredLocalSource {
    load: LocalLoader,
}

impl ConfiguredLocalSource {
    fn source(&self) -> sources::SourceResult<LocalSource> {
        (self.load)(&mut |_| {}, &library_sync::CancellationToken::new())
    }
}

struct ConfiguredLocalStreams;

#[async_trait(?Send)]
impl StreamResolver for ConfiguredLocalStreams {
    async fn resolve_stream(
        &self,
        request: &StreamRequest,
    ) -> sources::SourceResult<StreamDescriptor> {
        Err(sources::SourceError::Other(format!(
            "Cached local source is missing for track {}. Resync the local library.",
            request.track_id.as_str()
        )))
    }
}

struct ConfiguredLocalImages {
    roots: LocalRootsLoader,
}

#[async_trait(?Send)]
impl ImageProvider for ConfiguredLocalImages {
    async fn image_bytes(
        &self,
        image_ref: &ImageRef,
        _size: u32,
    ) -> sources::SourceResult<ImageBytes> {
        LocalSource::cover_item_bytes(&image_ref.item_id, (self.roots)())
    }
}

#[async_trait(?Send)]
impl FolderBrowser for ConfiguredLocalSource {
    async fn folder(
        &self,
        folder_id: Option<&FolderId>,
        music_folder_id: Option<&MusicFolderId>,
    ) -> sources::SourceResult<FolderDetail> {
        FolderBrowser::folder(&self.source()?, folder_id, music_folder_id).await
    }
}

fn build_local_active_source(
    identity: SourceIdentity,
    load: LocalLoader,
    roots: LocalRootsLoader,
) -> Arc<ActiveSource> {
    let source = Arc::new(ConfiguredLocalSource {
        load: Arc::clone(&load),
    });
    let streams = Arc::new(ConfiguredLocalStreams);
    let images = Arc::new(ConfiguredLocalImages {
        roots: Arc::clone(&roots),
    });
    let generated_tracks = crate::controller::cached_generated_track_executor(identity.id.clone());
    let auto_dj = crate::controller::cached_auto_dj_operation(
        identity.id.clone(),
        Arc::clone(&generated_tracks),
    );
    let local_audio: AudioFileLookup = Arc::new(cached_local_audio_path);
    let sync = crate::controller::local_sync_operation(
        identity.id.clone(),
        identity.clone(),
        Arc::clone(&load),
        Arc::clone(&roots),
    );
    let freshness = library_sync::Freshness::Events(Arc::new(LocalChangeFeed::new(roots)));
    let home_section = cached_home_section_loader(identity.id.clone());
    Arc::new(ActiveSource {
        identity,
        sync,
        freshness: Some(freshness),
        home_section,
        playback_file: Arc::clone(&local_audio),
        sidecar_file: local_audio,
        streams,
        images,
        favorites: OperationOwner::Store,
        playlist_creation: OperationOwner::Store,
        playlist_rows: PlaylistRowOperations::default(),
        random_tracks: RandomTrackOperation {
            domain: RandomTrackDomain::new(PLAYED_ALL, true, true),
            executor: OperationOwner::Store,
        },
        manual_radio: ManualRadioOperation {
            seed_domain: LOCAL_RADIO_SEEDS,
            executor: generated_tracks,
        },
        auto_dj,
        folders: Some(source),
        lyrics: None,
        reporter: None,
    })
}

fn build_jellyfin_active_source(
    source: JellyfinSource,
    use_instant_mix: bool,
) -> Arc<ActiveSource> {
    let identity = source.identity().clone();
    let source = Arc::new(source);
    let strategy = if use_instant_mix {
        GeneratedTrackStrategy::MixOnly
    } else {
        GeneratedTrackStrategy::SourceDefault
    };
    let generated_tracks =
        crate::controller::native_generated_track_executor(source.clone(), strategy);
    let random_tracks = RandomTrackOperation {
        domain: RandomTrackDomain::new(PLAYED_ALL_FILTERS, true, true),
        executor: OperationOwner::Native(source.clone()),
    };
    let auto_dj = crate::controller::native_auto_dj_operation(
        Arc::clone(&generated_tracks),
        random_tracks.clone(),
    );
    let sync = crate::controller::remote_sync_operation(source.clone(), Some(source.clone()));
    let freshness = library_sync::Freshness::Events(source.clone());
    let home_section = native_home_section_loader(source.clone());
    Arc::new(ActiveSource {
        identity,
        sync,
        freshness: Some(freshness),
        home_section,
        playback_file: Arc::new(matched_remote_audio_path),
        sidecar_file: Arc::new(accessible_remote_audio_path),
        streams: source.clone(),
        images: source.clone(),
        favorites: OperationOwner::Native(source.clone()),
        playlist_creation: OperationOwner::Native(source.clone()),
        playlist_rows: PlaylistRowOperations {
            rename: Some(source.clone()),
            delete: Some(source.clone()),
            add_tracks: Some(PlaylistMutationOperation {
                executor: source.clone(),
                readback: source.clone(),
            }),
            remove_entries: Some(PlaylistMutationOperation {
                executor: source.clone(),
                readback: source.clone(),
            }),
            move_entry: Some(PlaylistMutationOperation {
                executor: source.clone(),
                readback: source.clone(),
            }),
        },
        random_tracks,
        manual_radio: ManualRadioOperation {
            seed_domain: REMOTE_RADIO_SEEDS,
            executor: generated_tracks,
        },
        auto_dj,
        folders: Some(source.clone()),
        lyrics: Some(source.clone()),
        reporter: Some(source),
    })
}

fn build_subsonic_active_source(source: SubsonicSource) -> Arc<ActiveSource> {
    let identity = source.identity().clone();
    let source = Arc::new(source);
    let generated_tracks = crate::controller::native_generated_track_executor(
        source.clone(),
        GeneratedTrackStrategy::SourceDefault,
    );
    let random_tracks = RandomTrackOperation {
        domain: RandomTrackDomain::new(PLAYED_ALL, true, true),
        executor: OperationOwner::Native(source.clone()),
    };
    let auto_dj = crate::controller::native_auto_dj_operation(
        Arc::clone(&generated_tracks),
        random_tracks.clone(),
    );
    let sync = crate::controller::remote_sync_operation(source.clone(), None);
    let freshness = library_sync::Freshness::Probe {
        interval: Duration::from_secs(5 * 60),
        probe: source.clone(),
    };
    let home_section = native_home_section_loader(source.clone());
    Arc::new(ActiveSource {
        identity,
        sync,
        freshness: Some(freshness),
        home_section,
        playback_file: Arc::new(matched_remote_audio_path),
        sidecar_file: Arc::new(accessible_remote_audio_path),
        streams: source.clone(),
        images: source.clone(),
        favorites: OperationOwner::Native(source.clone()),
        playlist_creation: OperationOwner::Native(source.clone()),
        playlist_rows: PlaylistRowOperations {
            rename: Some(source.clone()),
            delete: Some(source.clone()),
            add_tracks: Some(PlaylistMutationOperation {
                executor: source.clone(),
                readback: source.clone(),
            }),
            remove_entries: Some(PlaylistMutationOperation {
                executor: source.clone(),
                readback: source.clone(),
            }),
            move_entry: Some(PlaylistMutationOperation {
                executor: source.clone(),
                readback: source.clone(),
            }),
        },
        random_tracks,
        manual_radio: ManualRadioOperation {
            seed_domain: REMOTE_RADIO_SEEDS,
            executor: generated_tracks,
        },
        auto_dj,
        folders: Some(source.clone()),
        lyrics: Some(source.clone()),
        reporter: Some(source),
    })
}

fn activate_jellyfin_configured(
    store: &StoreHandle,
    saved: &StoredSource,
    credential: String,
) -> Result<Arc<ActiveSource>, String> {
    activate_jellyfin_session(saved, credential, ensure_jellyfin_device_id(store)?)
}

fn activate_jellyfin_session(
    saved: &StoredSource,
    credential: String,
    device_id: String,
) -> Result<Arc<ActiveSource>, String> {
    let config = JellyfinSourceConfig::from_stored(saved).map_err(|error| error.to_string())?;
    JellyfinSource::from_configured_session(JellyfinConfiguredSession {
        source: config.credentials.source,
        user_id: config.credentials.user_id,
        trust_invalid_cert: config.credentials.trust_invalid_cert,
        access_token: credential,
        device_id,
    })
    .map(|source| build_jellyfin_active_source(source, config.use_instant_mix))
    .map_err(|error| error.to_string())
}

fn activate_subsonic_configured(
    saved: &StoredSource,
    credential: String,
) -> Result<Arc<ActiveSource>, String> {
    let config = SubsonicSourceConfig::from_stored(saved).map_err(|error| error.to_string())?;
    SubsonicSource::from_configured_session(SubsonicConfiguredSession {
        source: config.credentials.source,
        username: config.credentials.username,
        trust_invalid_cert: config.credentials.trust_invalid_cert,
        credential,
    })
    .map(build_subsonic_active_source)
    .map_err(|error| error.to_string())
}

fn activate_local_saved(
    store: &StoreHandle,
    saved: &StoredSource,
) -> Result<Arc<ActiveSource>, String> {
    let identity = LocalSourceConfig::from_stored(saved)
        .map_err(|error| error.to_string())?
        .source;
    let roots_store = store.clone();
    let roots_identity = identity.clone();
    let roots: LocalRootsLoader = Arc::new(move || {
        if roots_identity.id.as_str() == crate::controller::LOCAL_SOURCE_IDENTITY_ID {
            crate::controller::load_settings_from_store(&roots_store)
                .sources
                .local_folders
                .iter()
                .map(|folder| PathBuf::from(&folder.path))
                .collect()
        } else {
            vec![PathBuf::from(&roots_identity.base_url)]
        }
    });
    let load_store = store.clone();
    let load_identity = identity.clone();
    let load_roots = Arc::clone(&roots);
    let load: LocalLoader = Arc::new(move |progress, cancellation| {
        let manifest = load_store
            .with_store(|store| store.load_local_manifest(&load_identity.id))
            .map_err(sources::SourceError::Other)?;
        LocalSource::from_roots_with_manifest_scan(
            load_roots(),
            load_identity.clone(),
            manifest,
            progress,
            || cancellation.is_cancelled(),
        )
    });
    Ok(build_local_active_source(identity, load, roots))
}

fn saved_credential(
    secrets: &Arc<dyn SecretStore>,
    source_id: &SourceId,
) -> Result<String, String> {
    secrets
        .load_token(source_id.as_str())
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "No saved token found for the active server.".to_string())
}

fn saved_credential_missing(
    secrets: &Arc<dyn SecretStore>,
    source_id: &SourceId,
) -> Result<bool, String> {
    secrets
        .load_token(source_id.as_str())
        .map(|credential| credential.is_none())
        .map_err(|error| error.to_string())
}

pub(crate) fn ensure_jellyfin_device_id(store: &StoreHandle) -> Result<String, String> {
    if let Some(device_id) = normalized_device_id(&store.load_settings().jellyfin_device_id) {
        return Ok(device_id);
    }
    store.update_settings(|settings| {
        if let Some(device_id) = normalized_device_id(&settings.jellyfin_device_id) {
            return Ok(device_id);
        }

        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|error| format!("failed to generate Jellyfin device id: {error}"))?;
        let mut device_id = String::from("rufin-");
        for byte in bytes {
            use std::fmt::Write as _;
            write!(&mut device_id, "{byte:02x}")
                .map_err(|error| format!("failed to format Jellyfin device id: {error}"))?;
        }
        settings.jellyfin_device_id = device_id.clone();
        settings.migrate_defaults();
        Ok(device_id)
    })
}

fn normalized_device_id(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
pub(crate) fn prepare_jellyfin_settings_update_with_login(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    saved: StoredSource,
    input: JellyfinSettingsInput,
    login: impl FnOnce(StoredSource, CredentialHostInput) -> Result<AuthenticatedSource, String>,
) -> Result<Option<PreparedSourceSettingsUpdate>, String> {
    prepare_jellyfin_settings_update_with_authentication(
        store,
        secrets,
        saved,
        input,
        &|| {},
        login,
    )
}

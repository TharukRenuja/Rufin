mod active;
pub(crate) use active::*;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use domain::LibrarySourceSelection;
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
    GeneratedTrackSeedKind, GeneratedTrackStrategy, ImageBytes, PlayedFilter, SourceIdentity,
    StreamDescriptor, StreamRequest,
};
use tokio::runtime::Runtime;

use crate::controller::{AppController, StoreHandle};
use crate::i18n::{msgid, tr};
use crate::ui::{Shell, SourceSetupFlow};

use sources::{CredentialSourceConfig, FolderBrowser, ImageProvider, MusicSource, StreamResolver};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourcePickerPresentation {
    pub(crate) title: &'static str,
    pub(crate) icon_name: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceEntityKind {
    Album,
    Artist,
}

impl SourceEntityKind {
    fn id_prefix(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Artist => "artist",
        }
    }
}

pub(crate) struct SourceEntityLink {
    pub(crate) label: &'static str,
    pub(crate) icon_name: &'static str,
    pub(crate) url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CredentialHostInput {
    pub(crate) server_name: Option<String>,
    pub(crate) server_url: String,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CredentialHostPreset {
    pub(crate) server_name: String,
    pub(crate) server_url: String,
    pub(crate) username: String,
    pub(crate) trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JellyfinSetupInput {
    pub(crate) credentials: CredentialHostInput,
    pub(crate) use_instant_mix: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalFolderHostInput {
    pub(crate) roots: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CredentialSettingsInput {
    pub(crate) source_id: SourceId,
    pub(crate) name: String,
    pub(crate) base_url: String,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) trust_invalid_cert: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JellyfinSettingsInput {
    pub(crate) credentials: CredentialSettingsInput,
    pub(crate) use_instant_mix: bool,
}

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

type SetupFlowFactory = fn(&Rc<Shell>) -> Rc<dyn SourceSetupFlow>;
type SettingsGroupFactory = fn(&Rc<Shell>, &StoredSource) -> Result<gtk::Widget, String>;
type ActivateConfigured =
    fn(&StoreHandle, &Arc<dyn SecretStore>, &StoredSource) -> Result<Arc<ActiveSource>, String>;
type NeedsAuth = fn(&Arc<dyn SecretStore>, &StoredSource) -> Result<bool, String>;
type ConfiguredForSync = fn(&StoreHandle, &StoredSource) -> bool;
type SourceSelection = fn(&StoredSource) -> LibrarySourceSelection;
type EntityLink = fn(&SourceIdentity, SourceEntityKind, &str) -> Option<SourceEntityLink>;
type DecodeIdentity = fn(&StoredSource) -> Result<SourceIdentity, String>;
type DecodeCredentials = fn(&StoredSource) -> Result<CredentialSourceConfig, String>;
type EncodeCredentials = fn(&StoredSource, CredentialSourceConfig) -> Result<StoredSource, String>;

#[derive(Clone, Copy)]
struct CredentialConfigCodec {
    decode: DecodeCredentials,
    encode: EncodeCredentials,
}

/// Everything needed to add, reconnect, load and show one saved source type
pub(crate) struct SourceRegistration {
    pub(crate) canonical_kind: &'static str,
    pub(crate) picker: SourcePickerPresentation,
    pub(crate) new_setup_flow: SetupFlowFactory,
    pub(crate) settings_group: Option<SettingsGroupFactory>,
    pub(crate) activate: ActivateConfigured,
    pub(crate) needs_auth: NeedsAuth,
    pub(crate) configured_for_sync: ConfiguredForSync,
    pub(crate) selection: SourceSelection,
    pub(crate) entity_link: Option<EntityLink>,
    identity: DecodeIdentity,
    credentials: Option<CredentialConfigCodec>,
}

static LOCAL: SourceRegistration = SourceRegistration {
    canonical_kind: LOCAL_SOURCE_ID,
    picker: SourcePickerPresentation {
        title: msgid("Local"),
        icon_name: "rufin-route-folders-symbolic",
    },
    new_setup_flow: local_setup_flow,
    settings_group: None,
    activate: activate_local_registration,
    needs_auth: local_needs_auth,
    configured_for_sync: local_configured_for_sync,
    selection: local_selection,
    entity_link: None,
    identity: decode_local_identity,
    credentials: None,
};
static JELLYFIN: SourceRegistration = SourceRegistration {
    canonical_kind: JELLYFIN_SOURCE_ID,
    picker: SourcePickerPresentation {
        title: msgid("Jellyfin"),
        icon_name: "io.github.screwys.Rufin.source.jellyfin",
    },
    new_setup_flow: jellyfin_setup_flow,
    settings_group: Some(jellyfin_settings_group),
    activate: activate_jellyfin_registration,
    needs_auth: credential_needs_auth,
    configured_for_sync: always_configured_for_sync,
    selection: source_selection,
    entity_link: Some(jellyfin_entity_link),
    identity: decode_jellyfin_identity,
    credentials: Some(CredentialConfigCodec {
        decode: decode_jellyfin_credentials,
        encode: encode_jellyfin_credentials,
    }),
};
static NAVIDROME: SourceRegistration = SourceRegistration {
    canonical_kind: "navidrome",
    picker: SourcePickerPresentation {
        title: msgid("Navidrome"),
        icon_name: "io.github.screwys.Rufin.source.navidrome",
    },
    new_setup_flow: navidrome_setup_flow,
    settings_group: Some(navidrome_settings_group),
    activate: activate_subsonic_registration,
    needs_auth: credential_needs_auth,
    configured_for_sync: always_configured_for_sync,
    selection: source_selection,
    entity_link: Some(navidrome_entity_link),
    identity: decode_subsonic_identity,
    credentials: Some(CredentialConfigCodec {
        decode: decode_subsonic_credentials,
        encode: encode_subsonic_credentials,
    }),
};
static SUBSONIC: SourceRegistration = SourceRegistration {
    canonical_kind: "subsonic",
    picker: SourcePickerPresentation {
        title: msgid("OpenSubsonic"),
        icon_name: "io.github.screwys.Rufin.source.opensubsonic",
    },
    new_setup_flow: subsonic_setup_flow,
    settings_group: Some(subsonic_settings_group),
    activate: activate_subsonic_registration,
    needs_auth: credential_needs_auth,
    configured_for_sync: always_configured_for_sync,
    selection: source_selection,
    entity_link: None,
    identity: decode_subsonic_identity,
    credentials: Some(CredentialConfigCodec {
        decode: decode_subsonic_credentials,
        encode: encode_subsonic_credentials,
    }),
};

static REGISTRATIONS: [&SourceRegistration; 4] = [&JELLYFIN, &NAVIDROME, &SUBSONIC, &LOCAL];

pub(crate) fn source_registrations() -> &'static [&'static SourceRegistration] {
    &REGISTRATIONS
}

pub(crate) fn default_source_registration() -> &'static SourceRegistration {
    &JELLYFIN
}

pub(crate) fn resolve_source_registration(kind: &str) -> Option<&'static SourceRegistration> {
    source_registrations()
        .iter()
        .copied()
        .find(|registration| registration.canonical_kind == kind)
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
    let registration = resolve_source_registration(&saved.kind)
        .ok_or_else(|| "Saved source type is no longer supported.".to_string())?;
    let codec = registration
        .credentials
        .ok_or_else(|| "Saved source does not use credential settings.".to_string())?;
    (codec.decode)(saved)
}

fn replace_credential_config(
    registration: &SourceRegistration,
    saved: &StoredSource,
    credentials: CredentialSourceConfig,
) -> Result<StoredSource, String> {
    let codec = registration
        .credentials
        .ok_or_else(|| "Saved source does not use credential settings.".to_string())?;
    (codec.encode)(saved, credentials)
}

fn source_account_id(saved: &StoredSource) -> Option<String> {
    resolve_source_registration(&saved.kind)
        .and_then(|registration| registration.credentials)
        .and_then(|codec| (codec.decode)(saved).ok())
        .map(|config| config.user_id)
}

pub(crate) fn configured_source_username(saved: &StoredSource) -> Option<String> {
    credential_config(saved).ok().map(|config| config.username)
}

pub(crate) fn configured_source_identity(saved: &StoredSource) -> Result<SourceIdentity, String> {
    let registration = resolve_source_registration(&saved.kind)
        .ok_or_else(|| "Saved source type is no longer supported.".to_string())?;
    (registration.identity)(saved)
}

pub(crate) fn local_configured_source() -> StoredSource {
    LocalSourceConfig {
        source: SourceIdentity {
            id: SourceId::new(crate::controller::LOCAL_SOURCE_IDENTITY_ID),
            kind: LOCAL_SOURCE_ID.to_string(),
            name: "Local".to_string(),
            base_url: String::new(),
        },
    }
    .into_stored()
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
    let registration = resolve_source_registration(&saved.kind)
        .ok_or_else(|| "Saved source type is no longer supported.".to_string())?;
    (registration.activate)(store, secrets, saved)
}
pub(crate) fn configured_source_needs_auth(
    secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> Result<bool, String> {
    let registration = resolve_source_registration(&saved.kind)
        .ok_or_else(|| "Saved source type is no longer supported.".to_string())?;
    (registration.needs_auth)(secrets, saved)
}

pub(crate) fn configured_source_selection(saved: &StoredSource) -> LibrarySourceSelection {
    resolve_source_registration(&saved.kind).map_or_else(
        || LibrarySourceSelection::Source(saved.source_id.clone()),
        |registration| (registration.selection)(saved),
    )
}

fn configure_local(controller: &AppController, input: LocalFolderHostInput) {
    controller.add_library_folders(input.roots);
}

fn local_setup_flow(shell: &Rc<Shell>) -> Rc<dyn SourceSetupFlow> {
    crate::ui::new_local_source_setup_flow(shell, &LOCAL, move |controller, roots| {
        configure_local(controller, LocalFolderHostInput { roots });
    })
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

fn configure_jellyfin(controller: &AppController, input: JellyfinSetupInput) {
    controller.configure_authenticated_source(JELLYFIN.picker.title, move |runtime, store| {
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

fn update_jellyfin_settings_input(controller: &AppController, input: JellyfinSettingsInput) {
    let source_id = input.credentials.source_id.clone();
    controller.update_source_settings(
        source_id,
        JELLYFIN.picker.title,
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

fn jellyfin_entity_link(
    source: &SourceIdentity,
    kind: SourceEntityKind,
    entity_id: &str,
) -> Option<SourceEntityLink> {
    let base_url = clean_source_base_url(&source.base_url)?;
    let item_id = raw_source_entity_id(entity_id, JELLYFIN.canonical_kind, kind)?;
    Some(SourceEntityLink {
        label: msgid("Open on Jellyfin"),
        icon_name: JELLYFIN.picker.icon_name,
        url: format!("{base_url}/web/index.html#!/details?id={item_id}"),
    })
}

fn jellyfin_setup_flow(shell: &Rc<Shell>) -> Rc<dyn SourceSetupFlow> {
    let saved = shell.reconnect_saved_source(&JELLYFIN);
    crate::ui::new_jellyfin_source_setup_flow(
        shell,
        &JELLYFIN,
        saved
            .as_ref()
            .and_then(|saved| credential_host_preset(saved).ok()),
        saved
            .as_ref()
            .and_then(|saved| JellyfinSourceConfig::from_stored(saved).ok())
            .is_some_and(|config| config.use_instant_mix),
        move |controller, credentials, use_instant_mix| {
            configure_jellyfin(
                controller,
                JellyfinSetupInput {
                    credentials,
                    use_instant_mix,
                },
            );
        },
    )
}

fn jellyfin_settings_group(shell: &Rc<Shell>, saved: &StoredSource) -> Result<gtk::Widget, String> {
    let config = JellyfinSourceConfig::from_stored(saved).map_err(|error| error.to_string())?;
    let instant_mix = adw::SwitchRow::builder()
        .title(tr("Use Jellyfin Instant Mix for recommendations"))
        .subtitle(tr("This uses Jellyfin API for play radio, necessary if you want recommendation plugins to work."))
        .active(config.use_instant_mix)
        .build();
    let instant_mix_for_submit = instant_mix.clone();
    Ok(crate::ui::credential_source_settings_group(
        shell,
        saved.source_id.clone(),
        credential_host_preset(saved)?,
        JELLYFIN.picker.title,
        Some(instant_mix),
        move |controller, credentials| {
            update_jellyfin_settings_input(
                controller,
                JellyfinSettingsInput {
                    credentials,
                    use_instant_mix: instant_mix_for_submit.is_active(),
                },
            );
        },
    ))
}

fn activate_jellyfin_registration(
    store: &StoreHandle,
    secrets: &Arc<dyn SecretStore>,
    saved: &StoredSource,
) -> Result<Arc<ActiveSource>, String> {
    activate_jellyfin_configured(store, saved, saved_credential(secrets, &saved.source_id)?)
}

fn configure_subsonic(
    registration: &'static SourceRegistration,
    flavor: SubsonicFlavor,
    controller: &AppController,
    input: CredentialHostInput,
) {
    controller.configure_authenticated_source(registration.picker.title, move |runtime, _store| {
        authenticate_new_subsonic(runtime, input, flavor)
    });
}

fn update_subsonic_settings(
    registration: &'static SourceRegistration,
    flavor: SubsonicFlavor,
    controller: &AppController,
    input: CredentialSettingsInput,
) {
    let source_id = input.source_id.clone();
    controller.update_source_settings(
        source_id,
        registration.picker.title,
        move |runtime, store, secrets, saved, authentication_started| {
            if registration.canonical_kind != saved.kind {
                return Err("Saved server source is no longer supported.".to_string());
            }
            let prepared = prepare_credential_settings(registration, saved, input)?;
            let changed = prepared.common_changed;
            finish_settings_update(
                registration,
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

fn subsonic_setup_flow_for(
    shell: &Rc<Shell>,
    registration: &'static SourceRegistration,
    flavor: SubsonicFlavor,
) -> Rc<dyn SourceSetupFlow> {
    crate::ui::new_credential_source_setup_flow(
        shell,
        registration,
        shell
            .reconnect_saved_source(registration)
            .as_ref()
            .and_then(|saved| credential_host_preset(saved).ok()),
        move |controller, input| configure_subsonic(registration, flavor, controller, input),
    )
}

fn navidrome_setup_flow(shell: &Rc<Shell>) -> Rc<dyn SourceSetupFlow> {
    subsonic_setup_flow_for(shell, &NAVIDROME, SubsonicFlavor::Navidrome)
}

fn subsonic_setup_flow(shell: &Rc<Shell>) -> Rc<dyn SourceSetupFlow> {
    subsonic_setup_flow_for(shell, &SUBSONIC, SubsonicFlavor::Subsonic)
}

fn subsonic_settings_group_for(
    shell: &Rc<Shell>,
    saved: &StoredSource,
    registration: &'static SourceRegistration,
    flavor: SubsonicFlavor,
) -> Result<gtk::Widget, String> {
    Ok(crate::ui::credential_source_settings_group(
        shell,
        saved.source_id.clone(),
        credential_host_preset(saved)?,
        registration.picker.title,
        None,
        move |controller, input| update_subsonic_settings(registration, flavor, controller, input),
    ))
}

fn navidrome_settings_group(
    shell: &Rc<Shell>,
    saved: &StoredSource,
) -> Result<gtk::Widget, String> {
    subsonic_settings_group_for(shell, saved, &NAVIDROME, SubsonicFlavor::Navidrome)
}

fn subsonic_settings_group(shell: &Rc<Shell>, saved: &StoredSource) -> Result<gtk::Widget, String> {
    subsonic_settings_group_for(shell, saved, &SUBSONIC, SubsonicFlavor::Subsonic)
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

fn credential_host_preset(saved: &StoredSource) -> Result<CredentialHostPreset, String> {
    let config = credential_config(saved)?;
    Ok(CredentialHostPreset {
        server_name: saved.name.clone(),
        server_url: config.source.base_url,
        username: config.username,
        trust_invalid_cert: config.trust_invalid_cert,
    })
}

fn prepare_credential_settings(
    registration: &SourceRegistration,
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
    let next = replace_credential_config(registration, &previous, config)?;
    Ok(CredentialSettingsPreparation {
        previous,
        next,
        reauth,
        common_changed,
    })
}

fn finish_settings_update(
    registration: &SourceRegistration,
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
        let active = (registration.activate)(store, secrets, &next)?;
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
    if registration.canonical_kind != authenticated.saved.kind {
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

fn navidrome_entity_link(
    source: &SourceIdentity,
    kind: SourceEntityKind,
    entity_id: &str,
) -> Option<SourceEntityLink> {
    let base_url = clean_source_base_url(&source.base_url)?;
    let item_id = raw_source_entity_id(entity_id, "navidrome", kind)?;
    Some(SourceEntityLink {
        label: msgid("Open on Navidrome"),
        icon_name: NAVIDROME.picker.icon_name,
        url: format!(
            "{base_url}/app/#/{}/{}/show",
            kind.id_prefix(),
            percent_encode_path_segment(item_id)
        ),
    })
}

fn raw_source_entity_id<'a>(
    entity_id: &'a str,
    source_kind: &str,
    kind: SourceEntityKind,
) -> Option<&'a str> {
    let raw_id = entity_id.strip_prefix(&format!("{source_kind}:{}:", kind.id_prefix()))?;
    let raw_id = raw_id.trim();
    (!raw_id.is_empty()).then_some(raw_id)
}

fn clean_source_base_url(base_url: &str) -> Option<&str> {
    let base_url = base_url.trim().trim_end_matches('/');
    (!base_url.is_empty()).then_some(base_url)
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{byte:02X}"));
            }
        }
    }
    encoded
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

#[cfg(test)]
pub(crate) fn update_jellyfin_settings(controller: &AppController, input: JellyfinSettingsInput) {
    update_jellyfin_settings_input(controller, input);
}

#[cfg(test)]
pub(crate) fn configure_local_source(controller: &AppController, input: LocalFolderHostInput) {
    configure_local(controller, input);
}

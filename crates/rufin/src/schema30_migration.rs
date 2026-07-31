//! Installs the released schema-30 data into the final Settings and Store.
//!
//! Settings and Library keep ownership of their own formats. This module only
//! composes the one cross-file transition before either ordinary owner opens.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use library::{
    Schema30AcceptedSource, Schema30Configuration, Schema30Migration, Schema30MigrationReport,
    Schema30Repeat, SourceId,
};
use playback::RepeatMode;
use serde::Deserialize;
use sources::{LOCAL_LIBRARY_SOURCE_ID, LOCAL_SOURCE_ID, SourceConfiguration};
use tracing::{info, warn};

use crate::settings::{
    ConfiguredLocalAccess, ConfiguredSource, CredentialRef, StoredSettings, read_startup_settings,
    write_settings,
};

#[derive(Default, Deserialize)]
struct ReleasedSettingsProjection {
    #[serde(default)]
    sources: ReleasedSourceSettings,
}

#[derive(Default, Deserialize)]
struct ReleasedSourceSettings {
    #[serde(default)]
    selected: Option<ReleasedSelection>,
    #[serde(default)]
    local_folders: Vec<ReleasedLocalFolder>,
}

#[derive(Deserialize)]
enum ReleasedSelection {
    Local,
    #[serde(alias = "Server")]
    Source(SourceId),
}

#[derive(Deserialize)]
struct ReleasedLocalFolder {
    path: String,
}

struct SettingsInput {
    stored: StoredSettings,
    current_sources_are_authority: bool,
    shuffle_setting_is_authority: bool,
    repeat_setting_is_authority: bool,
    released_sources: ReleasedSourceSettings,
}

pub(crate) fn install_if_needed(
    settings_path: &Path,
    released_store_path: &Path,
    final_store_path: &Path,
) -> Result<Option<Schema30MigrationReport>, String> {
    let input = read_settings_input(settings_path)?;
    if final_store_path.exists() {
        install_settings_without_released_data(settings_path, input)?;
        return Ok(None);
    }
    if !released_store_path.exists() {
        install_settings_without_released_data(settings_path, input)?;
        return Ok(None);
    }

    let migration = match Schema30Migration::open(released_store_path) {
        Ok(migration) => migration,
        Err(error) => {
            install_settings_without_released_data(settings_path, input)?;
            warn!(
                %error,
                path = %released_store_path.display(),
                "could not import the released Store; files were preserved"
            );
            return Ok(None);
        }
    };
    let import_shuffle = !input.shuffle_setting_is_authority;
    let import_repeat = !input.repeat_setting_is_authority;
    let mut next = merge_settings(input, migration.configuration())?;
    if let Some(modes) = next
        .sources
        .selected_source_id
        .as_ref()
        .and_then(|source_id| migration.playback_modes(source_id))
    {
        if import_shuffle {
            next.ui.shuffle_enabled = modes.shuffle_enabled;
        }
        if import_repeat {
            next.ui.repeat_mode = match modes.repeat {
                Schema30Repeat::Off => RepeatMode::Off,
                Schema30Repeat::One => RepeatMode::One,
                Schema30Repeat::All => RepeatMode::All,
            };
        }
    }
    let accepted = next
        .sources
        .configured
        .iter()
        .map(|source| Schema30AcceptedSource {
            source_id: source.configuration.source_id.clone(),
            local: source.configuration.kind == LOCAL_SOURCE_ID,
        })
        .collect::<Vec<_>>();

    let prepared = prepared_store_path(final_store_path);
    remove_prepared_store(&prepared)?;
    let report = match migration.prepare_store(&prepared, &accepted) {
        Ok(report) => report,
        Err(error) => {
            remove_prepared_store(&prepared)?;
            write_settings(settings_path, &next)?;
            warn!(
                %error,
                path = %released_store_path.display(),
                "could not import released user data; files were preserved"
            );
            return Ok(None);
        }
    };

    write_settings(settings_path, &next)?;
    fs::rename(&prepared, final_store_path).map_err(|error| error.to_string())?;
    sync_parent(final_store_path)?;
    report_migration(&report);
    Ok(Some(report))
}

fn install_settings_without_released_data(
    settings_path: &Path,
    input: SettingsInput,
) -> Result<(), String> {
    let previous = input.stored.clone();
    let next = merge_settings(input, &Schema30Configuration::default())?;
    if next != previous {
        write_settings(settings_path, &next)?;
    }
    Ok(())
}

fn read_settings_input(path: &Path) -> Result<SettingsInput, String> {
    let startup = read_startup_settings(path)?;
    let stored = startup.stored;
    let raw_value = startup.raw;
    let released = raw_value
        .as_ref()
        .and_then(|value| {
            match serde_json::from_value::<ReleasedSettingsProjection>(value.clone()) {
                Ok(released) => Some(released),
                Err(error) => {
                    warn!(%error, "ignored unreadable legacy source settings");
                    None
                }
            }
        })
        .unwrap_or_default();
    let current_sources_are_authority = raw_value
        .as_ref()
        .and_then(|value| value.get("sources")?.get("configured")?.as_array())
        .is_some_and(|configured| !configured.is_empty());
    let shuffle_setting_is_authority = raw_value
        .as_ref()
        .is_some_and(|value| value.get("shuffle_enabled").is_some());
    let repeat_setting_is_authority = raw_value
        .as_ref()
        .is_some_and(|value| value.get("repeat_mode").is_some());
    Ok(SettingsInput {
        stored,
        current_sources_are_authority,
        shuffle_setting_is_authority,
        repeat_setting_is_authority,
        released_sources: released.sources,
    })
}

fn merge_settings(
    input: SettingsInput,
    released: &Schema30Configuration,
) -> Result<StoredSettings, String> {
    let SettingsInput {
        mut stored,
        current_sources_are_authority,
        shuffle_setting_is_authority: _,
        repeat_setting_is_authority: _,
        released_sources,
    } = input;
    let facts = released
        .sources
        .iter()
        .map(|source| (source.source_id.clone(), source))
        .collect::<HashMap<_, _>>();

    if current_sources_are_authority {
        for configured in &mut stored.sources.configured {
            apply_released_source_choices(
                configured,
                facts.get(&configured.configuration.source_id).copied(),
            );
        }
        stored.migrate_defaults();
        return Ok(stored);
    }

    let roots = released_local_roots(&released_sources);
    let mut configured = Vec::new();
    for source in &released.sources {
        let configuration = if source.kind == LOCAL_SOURCE_ID {
            if roots.is_empty() {
                warn!(
                    source_id = %source.source_id,
                    "ignored a released Local source without configured folders"
                );
                continue;
            }
            match SourceConfiguration::local(
                source.source_id.clone(),
                source.name.clone(),
                roots.clone(),
            ) {
                Ok(configuration) => configuration,
                Err(error) => {
                    warn!(
                        source_id = %source.source_id,
                        %error,
                        "ignored invalid released Local folders"
                    );
                    continue;
                }
            }
        } else {
            SourceConfiguration {
                source_id: source.source_id.clone(),
                kind: source.kind.clone(),
                name: source.name.clone(),
                provider_payload: source.provider_payload.clone(),
            }
        };
        let mut migrated = ConfiguredSource {
            credential_ref: (configuration.kind != LOCAL_SOURCE_ID)
                .then(|| CredentialRef::new(configuration.source_id.as_str())),
            configuration,
            music_folder_id: None,
            local_access: None,
        };
        apply_released_source_choices(&mut migrated, Some(source));
        configured.push(migrated);
    }

    if !roots.is_empty()
        && !configured
            .iter()
            .any(|source| source.configuration.kind == LOCAL_SOURCE_ID)
    {
        configured.push(ConfiguredSource {
            configuration: SourceConfiguration::local(
                SourceId::new(LOCAL_LIBRARY_SOURCE_ID),
                "Local",
                roots,
            )
            .map_err(string_error)?,
            credential_ref: None,
            music_folder_id: None,
            local_access: None,
        });
    }
    configured.sort_by(|left, right| {
        left.configuration
            .name
            .to_ascii_lowercase()
            .cmp(&right.configuration.name.to_ascii_lowercase())
            .then_with(|| {
                left.configuration
                    .source_id
                    .cmp(&right.configuration.source_id)
            })
    });

    let selected = released_selection(&released_sources, &configured)
        .or_else(|| valid_selection(released.active_source_id.clone(), &configured));
    stored.sources.configured = configured;
    stored.sources.selected_source_id = selected;
    stored.migrate_defaults();
    Ok(stored)
}

fn apply_released_source_choices(
    configured: &mut ConfiguredSource,
    released: Option<&library::Schema30Source>,
) {
    let Some(released) = released else {
        return;
    };
    if configured.music_folder_id.is_none() {
        configured
            .music_folder_id
            .clone_from(&released.music_folder_id);
    }
    if configured.local_access.is_none() {
        configured.local_access =
            released
                .local_access
                .as_ref()
                .map(|access| ConfiguredLocalAccess {
                    root_path: PathBuf::from(&access.root_path),
                    server_prefix: access.server_prefix.clone(),
                    local_prefix: access.local_prefix.clone(),
                });
    }
}

fn released_local_roots(settings: &ReleasedSourceSettings) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for folder in &settings.local_folders {
        let path = folder.path.trim();
        if path.is_empty() {
            continue;
        }
        let path = PathBuf::from(path);
        if !roots.iter().any(|existing| existing == &path) {
            roots.push(path);
        }
    }
    roots
}

fn released_selection(
    settings: &ReleasedSourceSettings,
    configured: &[ConfiguredSource],
) -> Option<SourceId> {
    match settings.selected.as_ref()? {
        ReleasedSelection::Local => configured
            .iter()
            .find(|source| source.configuration.source_id.as_str() == LOCAL_LIBRARY_SOURCE_ID)
            .or_else(|| {
                configured
                    .iter()
                    .find(|source| source.configuration.kind == LOCAL_SOURCE_ID)
            })
            .map(|source| source.configuration.source_id.clone()),
        ReleasedSelection::Source(source_id) => {
            valid_selection(Some(source_id.clone()), configured)
        }
    }
}

fn valid_selection(
    selected: Option<SourceId>,
    configured: &[ConfiguredSource],
) -> Option<SourceId> {
    selected.filter(|selected| {
        configured
            .iter()
            .any(|source| &source.configuration.source_id == selected)
    })
}

fn prepared_store_path(final_store: &Path) -> PathBuf {
    final_store.with_extension("sqlite.preparing")
}

fn remove_prepared_store(path: &Path) -> Result<(), String> {
    for owned in [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-shm"),
    ] {
        match fs::remove_file(&owned) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn sync_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

fn report_migration(report: &Schema30MigrationReport) {
    info!(
        playback_checkpoints = report.playback_checkpoints,
        local_favorites = report.local_favorites,
        local_playlists = report.local_playlists,
        smart_playlists = report.smart_playlists,
        activity_rows = report.activity_rows,
        "migrated released Rufin data"
    );
    let skipped = report.skipped_playback_checkpoints
        + report.skipped_local_favorites
        + report.skipped_local_playlists
        + report.skipped_smart_playlists
        + report.skipped_activity_rows;
    if skipped > 0 {
        warn!(
            playback_checkpoints = report.skipped_playback_checkpoints,
            local_favorites = report.skipped_local_favorites,
            local_playlists = report.skipped_local_playlists,
            smart_playlists = report.skipped_smart_playlists,
            activity_rows = report.skipped_activity_rows,
            "some unreadable optional released rows were not migrated"
        );
    }
}

fn string_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
#[path = "schema30_migration_tests.rs"]
mod tests;

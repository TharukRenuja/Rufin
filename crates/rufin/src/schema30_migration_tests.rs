use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use library::{
    ActivityItemId, ActivityPeriod, AlbumId, ArtistCredit, ArtistId, CandidateBatch,
    CandidateFinish, CandidateHeader, CueSegment, HomeFacts, Library, PlaybackLoad,
    PlaybackProvenance, PlaylistId, SmartPlaylistBuiltin, SmartPlaylistRule,
    SmartPlaylistRuleField, SmartPlaylistRuleOperator, SmartPlaylistRuleValue, SourceId, Track,
    TrackData, TrackId, TrackRelations,
};
use rusqlite::{Connection, params};
use secrets::{ConfigSecretStore, SecretStore as _, SwitchableSecretStore};
use sources::{EditableSource, SourceConfiguration};

use super::*;
use crate::settings::{load_provider_secret, read_settings};

const LOCAL_ID: &str = "local:server:library";
const REMOTE_ID: &str = "jellyfin:server:released";
const NAVIDROME_ID: &str = "navidrome:server:released";
const LOCAL_TRACK_ID: &str = "local:track:cue-one";
const LOCAL_ALBUM_ID: &str = "local:album:cue-one";
const LOCAL_ARTIST_ID: &str = "local:artist:cue-one";
const LOCAL_PLAYLIST_ID: &str = "local:playlist:duplicates";

#[test]
fn released_data_migrates_once_and_reattaches_after_source_rebuild() {
    let directory = tempfile::tempdir().expect("temporary migration directory");
    let settings_path = directory.path().join("settings.json");
    let secrets_path = directory.path().join("secrets.json");
    let released_store = directory.path().join("rufin-cache.sqlite");
    let final_store = directory.path().join("rufin-store.sqlite");
    write_released_settings(&settings_path);
    write_released_store(&released_store);

    let secrets = ConfigSecretStore::new(secrets_path.clone());
    secrets
        .save_token(REMOTE_ID, "released-jellyfin-token")
        .expect("save released Jellyfin token");
    secrets
        .save_token(NAVIDROME_ID, "released-navidrome-token")
        .expect("save released Navidrome token");

    let first = install_if_needed(&settings_path, &released_store, &final_store)
        .expect("migrate released data")
        .expect("migration report");
    assert_eq!(first.playback_checkpoints, 3);
    assert_eq!(first.local_favorites, 3);
    assert_eq!(first.local_playlists, 1);
    assert_eq!(first.smart_playlists, 10);
    assert_eq!(first.activity_rows, 2);
    assert_eq!(
        first.skipped_playback_checkpoints
            + first.skipped_local_favorites
            + first.skipped_local_playlists
            + first.skipped_smart_playlists
            + first.skipped_activity_rows,
        0
    );

    let installed_settings = fs::read(&settings_path).expect("read migrated Settings");
    fs::remove_file(&final_store).expect("simulate interruption before Store publication");
    let second = install_if_needed(&settings_path, &released_store, &final_store)
        .expect("repeat migration after Settings installation")
        .expect("repeated migration report");
    assert_eq!(second, first);
    assert_eq!(
        fs::read(&settings_path).expect("read repeated Settings"),
        installed_settings
    );
    let final_store_before = fs::read(&final_store).expect("read installed Store");
    assert!(
        install_if_needed(&settings_path, &released_store, &final_store)
            .expect("skip completed migration")
            .is_none()
    );
    assert_eq!(
        fs::read(&settings_path).expect("read Settings after completed migration"),
        installed_settings
    );
    assert_eq!(
        fs::read(&final_store).expect("read Store after completed migration"),
        final_store_before
    );

    let stored = read_settings(&settings_path).expect("read final Settings");
    assert!(stored.ui.private_mode);
    assert!(!stored.ui.lyrics.external_lyrics_enabled);
    assert!(!stored.ui.rich_presence.enabled);
    assert_eq!(stored.jellyfin_device_id, "released-device");
    assert!(stored.secret_scope_id.is_empty());
    assert_eq!(stored.sources.configured.len(), 3);
    assert_eq!(
        stored.sources.selected_source_id,
        Some(SourceId::new(REMOTE_ID))
    );
    assert!(stored.ui.auto_dj_enabled);
    assert_eq!(stored.ui.repeat_mode, playback::RepeatMode::One);
    assert!(stored.ui.shuffle_enabled);

    let local = configured_source(&stored, LOCAL_ID);
    assert!(local.credential_ref.is_none());
    match local
        .configuration
        .editable()
        .expect("decode migrated Local configuration")
    {
        EditableSource::Local { roots, .. } => {
            assert_eq!(
                roots,
                vec![PathBuf::from("/music"), PathBuf::from("/archive")]
            );
        }
        other => panic!("expected Local configuration, found {other:?}"),
    }

    let remote = configured_source(&stored, REMOTE_ID);
    assert_eq!(
        remote
            .credential_ref
            .as_ref()
            .expect("migrated credential reference")
            .as_str(),
        REMOTE_ID
    );
    assert_eq!(
        remote
            .music_folder_id
            .as_ref()
            .expect("migrated music folder")
            .as_str(),
        "folder:released"
    );
    let local_access = remote
        .local_access
        .as_ref()
        .expect("migrated remote Local access");
    assert_eq!(local_access.root_path, PathBuf::from("/mnt/music"));
    assert_eq!(local_access.server_prefix.as_deref(), Some("/srv/music"));
    assert_eq!(local_access.local_prefix.as_deref(), Some("/mnt/music"));
    match remote
        .configuration
        .editable()
        .expect("decode migrated remote configuration")
    {
        EditableSource::Credentials {
            credentials,
            jellyfin_use_instant_mix,
            ..
        } => {
            assert_eq!(credentials.server_url, "https://music.example");
            assert_eq!(credentials.username, "listener");
            assert!(credentials.trust_invalid_cert);
            assert_eq!(jellyfin_use_instant_mix, Some(true));
        }
        other => panic!("expected credential source, found {other:?}"),
    }

    let secret_store = Arc::new(SwitchableSecretStore::new(Arc::new(
        ConfigSecretStore::with_scope(secrets_path, stored.secret_scope_id.clone()),
    )));
    assert_eq!(
        load_provider_secret(
            &secret_store,
            remote
                .credential_ref
                .as_ref()
                .expect("remote credential reference"),
        )
        .expect("load migrated provider token")
        .as_deref(),
        Some("released-jellyfin-token")
    );
    let navidrome = configured_source(&stored, NAVIDROME_ID);
    assert_eq!(
        navidrome
            .credential_ref
            .as_ref()
            .expect("Navidrome credential reference")
            .as_str(),
        NAVIDROME_ID
    );
    match navidrome
        .configuration
        .editable()
        .expect("decode migrated Navidrome configuration")
    {
        EditableSource::Credentials {
            credentials,
            jellyfin_use_instant_mix,
            ..
        } => {
            assert_eq!(credentials.server_url, "https://navidrome.example");
            assert_eq!(credentials.username, "nav-listener");
            assert!(!credentials.trust_invalid_cert);
            assert_eq!(jellyfin_use_instant_mix, None);
        }
        other => panic!("expected Navidrome credentials, found {other:?}"),
    }
    assert_eq!(
        load_provider_secret(
            &secret_store,
            navidrome
                .credential_ref
                .as_ref()
                .expect("Navidrome credential reference"),
        )
        .expect("load migrated Navidrome token")
        .as_deref(),
        Some("released-navidrome-token")
    );

    let library = Library::open(&final_store).expect("open migrated Library");
    let local_id = SourceId::new(LOCAL_ID);
    let remote_id = SourceId::new(REMOTE_ID);
    let navidrome_id = SourceId::new(NAVIDROME_ID);
    assert!(
        library
            .load_source(&local_id)
            .expect("load absent Local facts")
            .is_none()
    );
    assert!(
        library
            .load_source(&remote_id)
            .expect("load absent remote facts")
            .is_none()
    );
    assert!(
        library
            .load_source(&navidrome_id)
            .expect("load absent Navidrome facts")
            .is_none()
    );

    let local_checkpoint = ready_checkpoint(&library, &local_id);
    assert_eq!(local_checkpoint.revision, 1);
    assert_eq!(local_checkpoint.state.progress_millis, 17_000);
    assert_eq!(
        local_checkpoint.queue.occurrences[0].provenance,
        PlaybackProvenance::Context {
            context_id: "released-local-context".to_string(),
            source_rank: 4,
        }
    );
    let restored = playback::restore_checkpoint(
        &local_checkpoint,
        None,
        stored.ui.repeat_mode,
        stored.ui.shuffle_enabled,
        11,
    )
    .expect("restore released Local queue");
    assert_eq!(restored.repeat_mode(), playback::RepeatMode::One);
    assert!(restored.shuffle_enabled());
    let restored_track = &restored.entries()[0].track;
    assert_eq!(
        restored_track.source_path.as_deref(),
        Some("/music/Album/disc.flac")
    );
    assert_eq!(
        restored_track.cue,
        Some(CueSegment {
            cue_path: "/music/Album/disc.cue".to_string(),
            start_millis: 1_000,
            end_millis: 241_000,
        })
    );

    let remote_checkpoint = ready_checkpoint(&library, &remote_id);
    assert_eq!(remote_checkpoint.revision, 7);
    assert_eq!(remote_checkpoint.state.progress_millis, 23_000);
    assert_eq!(remote_checkpoint.queue.occurrences.len(), 2);
    assert_eq!(
        remote_checkpoint
            .queue
            .traversal
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["remote-occurrence-b", "remote-occurrence-a"]
    );
    let restored_remote = playback::restore_checkpoint(
        &remote_checkpoint,
        None,
        stored.ui.repeat_mode,
        stored.ui.shuffle_enabled,
        12,
    )
    .expect("restore released v1 remote queue");
    assert_eq!(restored_remote.repeat_mode(), playback::RepeatMode::One);
    assert!(restored_remote.shuffle_enabled());
    let navidrome_checkpoint = ready_checkpoint(&library, &navidrome_id);
    assert_eq!(navidrome_checkpoint.queue.occurrences.len(), 100);
    assert!(navidrome_checkpoint.queue.traversal.is_empty());
    let restored_navidrome = playback::restore_checkpoint(
        &navidrome_checkpoint,
        None,
        stored.ui.repeat_mode,
        stored.ui.shuffle_enabled,
        13,
    )
    .expect("restore released Navidrome queue");
    assert_eq!(restored_navidrome.repeat_mode(), playback::RepeatMode::One);
    assert!(restored_navidrome.shuffle_enabled());

    let lifetime = library
        .activity_summary(&local_id, ActivityPeriod::Lifetime)
        .expect("read migrated lifetime activity");
    let lifetime_track = lifetime
        .tracks
        .iter()
        .find(|item| item.id == ActivityItemId::Track(TrackId::new(LOCAL_TRACK_ID)))
        .expect("migrated lifetime Track");
    assert_eq!(lifetime_track.play_count, 5);
    assert_eq!(lifetime_track.skip_count, Some(3));
    assert!(lifetime_track.last_played_at.is_some());
    let month = library
        .activity_summary(&local_id, ActivityPeriod::Month("2026-07".to_string()))
        .expect("read migrated monthly activity");
    assert_eq!(month.tracks.len(), 1);
    assert_eq!(month.tracks[0].play_count, 3);
    assert_eq!(month.tracks[0].skip_count, None);

    let mut candidate = library
        .begin_source_candidate(CandidateHeader {
            source_id: local_id.clone(),
            input_version: 1,
            input_digest: [7; 32],
        })
        .expect("begin rebuilt Local candidate");
    candidate
        .write(CandidateBatch::Tracks(vec![local_track()]))
        .expect("write rebuilt Local Track");
    let commit = candidate
        .finish(
            CandidateFinish {
                freshness: None,
                home: HomeFacts::RufinDefined,
                accepted_at: 1,
            },
            None,
        )
        .and_then(|prepared| prepared.accept())
        .expect("accept rebuilt Local candidate");
    assert_reattached_user_data(&commit.loaded);

    drop(commit);
    drop(library);
    let reopened = Library::open(&final_store)
        .expect("reopen migrated Library")
        .load_source(&local_id)
        .expect("load rebuilt Local source")
        .expect("rebuilt Local source");
    assert_reattached_user_data(&reopened);
}

#[test]
fn albumless_legacy_queue_migrates_without_rebuildable_track_cache() {
    const SOURCE_ID: &str = "jellyfin:server:albumless-queue";
    const TRACK_ID: &str = "jellyfin:track:albumless";

    let directory = tempfile::tempdir().expect("temporary migration directory");
    let settings_path = directory.path().join("settings.json");
    let released_store = directory.path().join("rufin-cache.sqlite");
    let final_store = directory.path().join("rufin-store.sqlite");
    write_released_settings_without_selection(&settings_path);

    let connection = Connection::open(&released_store).expect("open released Store");
    connection
        .execute_batch(RELEASED_SCHEMA)
        .expect("create released Store");
    connection
        .execute(
            "INSERT INTO sources(source_id, kind, name, provider_payload)
             VALUES (?1, 'jellyfin', 'Albumless Queue', ?2)",
            params![
                SOURCE_ID,
                serde_json::json!({
                    "version": 1,
                    "base_url": "https://music.example",
                    "user_id": "account-id",
                    "username": "listener",
                    "trust_invalid_cert": false
                })
                .to_string()
            ],
        )
        .expect("insert released source");
    connection
        .execute(
            "INSERT INTO active_source(singleton, source_id) VALUES (1, ?1)",
            [SOURCE_ID],
        )
        .expect("insert released selection");
    let queue = serde_json::json!({
        "server_id": SOURCE_ID,
        "entries": [{
            "id": "albumless-occurrence",
            "track_id": TRACK_ID,
            "album_id": null,
            "title": "Albumless Track",
            "artist": "Standalone Artist",
            "artist_id": null,
            "album": "",
            "year": 2026,
            "duration_seconds": 180,
            "favorite": false,
            "image_ref": null,
            "local_path": null,
            "source_format": "mp3",
            "origin": {"Manual": {}}
        }],
        "current_index": 0,
        "repeat_mode": "Off",
        "shuffle": {"enabled": false},
        "progress_seconds": 12
    });
    connection
        .execute(
            "INSERT INTO playback_checkpoints(
                 source_id, revision, selected_occurrence_id, progress_millis,
                 repeat_mode, shuffle_enabled, payload
             ) VALUES (?1, 0, 'albumless-occurrence', 12000, 'Off', 0, ?2)",
            params![SOURCE_ID, queue.to_string()],
        )
        .expect("insert released queue");
    drop(connection);

    let report = install_if_needed(&settings_path, &released_store, &final_store)
        .expect("migrate released data")
        .expect("migration report");
    assert_eq!(report.playback_checkpoints, 1);
    assert_eq!(report.skipped_playback_checkpoints, 0);

    let library = Library::open(&final_store).expect("open migrated Library");
    let checkpoint = ready_checkpoint(&library, &SourceId::new(SOURCE_ID));
    let restored =
        playback::restore_checkpoint(&checkpoint, None, playback::RepeatMode::Off, false, 0)
            .expect("restore cache-independent albumless queue");
    assert_eq!(restored.entries().len(), 1);
    assert_eq!(restored.entries()[0].track.id, TrackId::new(TRACK_ID));
    assert!(restored.entries()[0].track.album_id.is_none());
    assert_eq!(checkpoint.state.progress_millis, 12_000);
}

#[test]
fn unsupported_released_store_recovers_local_settings_and_allows_a_fresh_store() {
    let directory = tempfile::tempdir().expect("temporary migration directory");
    let settings_path = directory.path().join("settings.json");
    let released_store = directory.path().join("rufin-cache.sqlite");
    let final_store = directory.path().join("rufin-store.sqlite");
    write_released_settings_value(&settings_path, Some(serde_json::json!("Local")));
    let connection = Connection::open(&released_store).expect("open unsupported Store");
    connection
        .execute_batch(
            "PRAGMA user_version = 13;
             CREATE TABLE sources (
                 source_id TEXT PRIMARY KEY,
                 kind TEXT NOT NULL,
                 name TEXT NOT NULL,
                 provider_payload TEXT NOT NULL
             );",
        )
        .expect("create unsupported Store");
    drop(connection);
    let store_before = fs::read(&released_store).expect("read Store before failure");

    assert!(
        install_if_needed(&settings_path, &released_store, &final_store)
            .expect("skip unsupported released data")
            .is_none()
    );

    assert_eq!(
        fs::read(&released_store).expect("read Store after failure"),
        store_before,
        "unsupported released data remains available without blocking startup"
    );
    assert!(!final_store.exists());

    let stored = read_settings(&settings_path).expect("read recovered Settings");
    assert_eq!(stored.sources.configured.len(), 1);
    let local = &stored.sources.configured[0];
    assert_eq!(
        stored.sources.selected_source_id.as_ref(),
        Some(&local.configuration.source_id)
    );
    match local
        .configuration
        .editable()
        .expect("decode recovered Local configuration")
    {
        EditableSource::Local { roots, .. } => assert_eq!(
            roots,
            vec![PathBuf::from("/music"), PathBuf::from("/archive")]
        ),
        other => panic!("expected Local configuration, found {other:?}"),
    }

    let (library, repair) = Library::open_with_repair(&final_store)
        .expect("open a fresh Store after skipping released data");
    assert!(repair.is_none());
    assert!(
        library
            .load_source(&local.configuration.source_id)
            .expect("read fresh Store")
            .is_none()
    );
}

#[test]
fn legacy_local_settings_are_recovered_independently_of_store_migration() {
    let directory = tempfile::tempdir().expect("temporary migration directory");
    let settings_path = directory.path().join("settings.json");
    let released_store = directory.path().join("rufin-cache.sqlite");
    let final_store = directory.path().join("rufin-store.sqlite");
    write_released_settings_value(&settings_path, Some(serde_json::json!("Local")));
    drop(Library::open(&final_store).expect("create an existing final Store"));

    assert!(
        install_if_needed(&settings_path, &released_store, &final_store)
            .expect("recover settings independently of Store migration")
            .is_none()
    );

    let stored = read_settings(&settings_path).expect("read recovered Settings");
    assert_eq!(stored.sources.configured.len(), 1);
    assert_eq!(
        stored.sources.selected_source_id,
        Some(stored.sources.configured[0].configuration.source_id.clone())
    );
}

#[test]
fn released_active_source_is_only_the_fallback_when_settings_has_no_selection() {
    let directory = tempfile::tempdir().expect("temporary migration directory");
    let settings_path = directory.path().join("settings.json");
    let released_store = directory.path().join("rufin-cache.sqlite");
    let final_store = directory.path().join("rufin-store.sqlite");
    write_released_settings_without_selection(&settings_path);
    write_released_store(&released_store);

    install_if_needed(&settings_path, &released_store, &final_store)
        .expect("migrate released selection fallback")
        .expect("migration report");

    assert_eq!(
        read_settings(&settings_path)
            .expect("read migrated Settings")
            .sources
            .selected_source_id,
        Some(SourceId::new(LOCAL_ID))
    );
}

#[test]
fn current_configured_sources_remain_the_complete_authority() {
    let directory = tempfile::tempdir().expect("temporary migration directory");
    let settings_path = directory.path().join("settings.json");
    let released_store = directory.path().join("rufin-cache.sqlite");
    let final_store = directory.path().join("rufin-store.sqlite");
    write_released_store(&released_store);

    let current_only_id = SourceId::new("subsonic:server:current-only");
    let current_only = ConfiguredSource {
        configuration: SourceConfiguration {
            source_id: current_only_id.clone(),
            kind: "subsonic".to_string(),
            name: "Current only".to_string(),
            provider_payload: serde_json::json!({
                "version": 1,
                "base_url": "https://current-only.example",
                "user_id": "current-account",
                "username": "current-listener",
                "trust_invalid_cert": false
            })
            .to_string(),
        },
        credential_ref: Some(CredentialRef::new("current-only-secret")),
        music_folder_id: None,
        local_access: None,
    };
    let current_jellyfin_payload = serde_json::json!({
        "version": 1,
        "base_url": "https://current.example",
        "user_id": "current-jellyfin-account",
        "username": "current-jellyfin-listener",
        "trust_invalid_cert": false,
        "use_jellyfin_instant_mix": false
    })
    .to_string();
    let current_jellyfin = ConfiguredSource {
        configuration: SourceConfiguration {
            source_id: SourceId::new(REMOTE_ID),
            kind: "jellyfin".to_string(),
            name: "Current Jellyfin".to_string(),
            provider_payload: current_jellyfin_payload.clone(),
        },
        credential_ref: Some(CredentialRef::new("current-jellyfin-secret")),
        music_folder_id: None,
        local_access: None,
    };
    let mut current = StoredSettings::default();
    current.sources.configured = vec![current_only, current_jellyfin];
    current.sources.selected_source_id = None;
    write_settings(&settings_path, &current).expect("write current Settings");

    install_if_needed(&settings_path, &released_store, &final_store)
        .expect("migrate with current Settings authority")
        .expect("migration report");

    let migrated = read_settings(&settings_path).expect("read migrated current Settings");
    assert_eq!(
        migrated
            .sources
            .configured
            .iter()
            .map(|source| source.configuration.source_id.clone())
            .collect::<Vec<_>>(),
        vec![current_only_id, SourceId::new(REMOTE_ID)]
    );
    assert!(migrated.sources.selected_source_id.is_none());
    let jellyfin = configured_source(&migrated, REMOTE_ID);
    assert_eq!(jellyfin.configuration.name, "Current Jellyfin");
    assert_eq!(
        jellyfin.configuration.provider_payload,
        current_jellyfin_payload
    );
    assert_eq!(
        jellyfin
            .credential_ref
            .as_ref()
            .expect("current credential reference")
            .as_str(),
        "current-jellyfin-secret"
    );
    assert_eq!(
        jellyfin
            .music_folder_id
            .as_ref()
            .expect("released folder enriches matching current source")
            .as_str(),
        "folder:released"
    );
    assert!(
        jellyfin.local_access.is_some(),
        "released Local access enriches the matching current source"
    );
    assert!(migrated.sources.configured.iter().all(|source| !matches!(
        source.configuration.source_id.as_str(),
        LOCAL_ID | NAVIDROME_ID
    )));
}

#[test]
fn invalid_remote_payload_does_not_forget_the_configured_source() {
    let broken_id = SourceId::new("jellyfin:server:broken");
    let released = Schema30Configuration {
        sources: vec![library::Schema30Source {
            source_id: broken_id.clone(),
            kind: "jellyfin".to_string(),
            name: "Needs repair".to_string(),
            provider_payload: "{not-json".to_string(),
            music_folder_id: None,
            local_access: None,
        }],
        active_source_id: Some(broken_id.clone()),
        skipped_sources: 0,
    };

    let migrated = merge_settings(
        SettingsInput {
            stored: StoredSettings::default(),
            current_sources_are_authority: false,
            shuffle_setting_is_authority: false,
            repeat_setting_is_authority: false,
            released_sources: ReleasedSourceSettings::default(),
        },
        &released,
    )
    .expect("preserve configured source with invalid payload");

    let configured = configured_source(&migrated, broken_id.as_str());
    assert_eq!(configured.configuration.provider_payload, "{not-json");
    assert_eq!(
        configured
            .credential_ref
            .as_ref()
            .expect("preserved credential reference")
            .as_str(),
        broken_id.as_str()
    );
    assert!(configured.configuration.input_identity().is_err());
    assert_eq!(migrated.sources.selected_source_id, Some(broken_id));
}

fn configured_source<'a>(stored: &'a StoredSettings, source_id: &str) -> &'a ConfiguredSource {
    stored
        .sources
        .configured
        .iter()
        .find(|source| source.configuration.source_id.as_str() == source_id)
        .unwrap_or_else(|| panic!("configured source {source_id}"))
}

fn ready_checkpoint(library: &Library, source_id: &SourceId) -> library::PlaybackCheckpoint {
    match library
        .load_playback(source_id)
        .expect("load migrated Playback")
    {
        PlaybackLoad::Ready(checkpoint) => checkpoint,
        other => panic!("expected ready Playback, found {other:?}"),
    }
}

fn assert_reattached_user_data(loaded: &Arc<library::LoadedLibrary>) {
    let track = loaded
        .track(&TrackId::new(LOCAL_TRACK_ID))
        .expect("read rebuilt Track")
        .expect("rebuilt Track");
    assert!(track.favorite);
    assert!(
        loaded
            .album(&AlbumId::new(LOCAL_ALBUM_ID))
            .expect("read rebuilt Album")
            .expect("rebuilt Album")
            .favorite
    );
    assert!(
        loaded
            .artist(&ArtistId::new(LOCAL_ARTIST_ID))
            .expect("read rebuilt Artist")
            .expect("rebuilt Artist")
            .favorite
    );

    let playlist = loaded
        .playlist_detail(&PlaylistId::new(LOCAL_PLAYLIST_ID))
        .expect("read migrated Local Playlist")
        .expect("migrated Local Playlist");
    assert_eq!(playlist.entries.len(), 2);
    let first = playlist
        .entries
        .entry(0)
        .expect("read first migrated Playlist entry")
        .expect("first migrated Playlist entry");
    let second = playlist
        .entries
        .entry(1)
        .expect("read second migrated Playlist entry")
        .expect("second migrated Playlist entry");
    assert_eq!(first.occurrence_id, "first");
    assert_eq!(second.occurrence_id, "second");
    assert_eq!(first.track.id, second.track.id);

    let smart = loaded
        .smart_playlists(None)
        .expect("read migrated smart Playlists");
    assert_eq!(smart.len(), 4);
    assert_eq!(
        smart
            .iter()
            .map(|summary| summary.smart_playlist.builtin)
            .collect::<Vec<_>>(),
        vec![
            Some(SmartPlaylistBuiltin::NeverPlayed),
            None,
            Some(SmartPlaylistBuiltin::MostPlayed),
            Some(SmartPlaylistBuiltin::MostSkipped),
        ]
    );
    assert_eq!(
        smart
            .iter()
            .map(|summary| summary.smart_playlist.position)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    let either_artist = smart
        .iter()
        .find(|summary| summary.smart_playlist.id.as_str() == "local:smart:artists")
        .expect("read migrated Any smart Playlist");
    assert!(either_artist.smart_playlist.definition.match_all.is_empty());
    assert_eq!(
        either_artist.smart_playlist.definition.match_any,
        [
            SmartPlaylistRule {
                field: SmartPlaylistRuleField::Artist,
                operator: SmartPlaylistRuleOperator::Equals,
                value: Some(SmartPlaylistRuleValue::Text("Cannons".to_string())),
            },
            SmartPlaylistRule {
                field: SmartPlaylistRuleField::Artist,
                operator: SmartPlaylistRuleOperator::Equals,
                value: Some(SmartPlaylistRuleValue::Text("Night Tapes".to_string())),
            },
        ]
    );
}

fn write_released_settings(path: &Path) {
    write_released_settings_value(path, Some(serde_json::json!({"Source": REMOTE_ID})));
}

fn write_released_settings_without_selection(path: &Path) {
    write_released_settings_value(path, None);
}

fn write_released_settings_value(path: &Path, selection: Option<serde_json::Value>) {
    let mut sources = serde_json::json!({
        "local_folders": [
            {"path": " /music "},
            {"path": "/music"},
            {"path": "/archive"}
        ]
    });
    if let Some(selection) = selection {
        sources["selected"] = selection;
    }
    let value = serde_json::json!({
        "theme_preference": "System",
        "private_mode": true,
        "auto_dj_enabled": true,
        "notifications_enabled": false,
        "external_lyrics_enabled": false,
        "discord_presence_enabled": false,
        "secret_storage_mode": "config-file",
        "jellyfin_device_id": "released-device",
        "sources": sources
    });
    fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&value).expect("serialize released Settings")
        ),
    )
    .expect("write released Settings");
}

fn write_released_store(path: &Path) {
    let connection = Connection::open(path).expect("open released Store fixture");
    connection
        .execute_batch(RELEASED_SCHEMA)
        .expect("create released Store fixture");
    connection
        .execute(
            "INSERT INTO sources(source_id, kind, name, provider_payload)
             VALUES (?1, 'local', 'Local', ?2)",
            params![
                LOCAL_ID,
                serde_json::json!({
                    "version": 1,
                    "base_url": "/obsolete-local-root",
                    "user_id": "",
                    "username": "",
                    "trust_invalid_cert": false
                })
                .to_string()
            ],
        )
        .expect("insert released Local source");
    connection
        .execute(
            "INSERT INTO sources(source_id, kind, name, provider_payload)
             VALUES (?1, 'jellyfin', 'Released Server', ?2)",
            params![
                REMOTE_ID,
                serde_json::json!({
                    "version": 1,
                    "base_url": "https://music.example",
                    "user_id": "account-id",
                    "username": "listener",
                    "trust_invalid_cert": true,
                    "use_jellyfin_instant_mix": true
                })
                .to_string()
            ],
        )
        .expect("insert released remote source");
    connection
        .execute(
            "INSERT INTO sources(source_id, kind, name, provider_payload)
             VALUES (?1, 'navidrome', 'Navidrome', ?2)",
            params![
                NAVIDROME_ID,
                serde_json::json!({
                    "version": 1,
                    "base_url": "https://navidrome.example",
                    "user_id": "nav-account",
                    "username": "nav-listener",
                    "trust_invalid_cert": false
                })
                .to_string()
            ],
        )
        .expect("insert released Navidrome source");
    connection
        .execute(
            "INSERT INTO active_source(singleton, source_id) VALUES (1, ?1)",
            [LOCAL_ID],
        )
        .expect("insert released active source");
    connection
        .execute(
            "INSERT INTO source_library_preferences(source_id, selected_music_folder_id)
             VALUES (?1, 'folder:released')",
            [REMOTE_ID],
        )
        .expect("insert released music folder");
    connection
        .execute(
            "INSERT INTO source_local_access(
                 source_id, root_path, path_replace_from, path_replace_to
             ) VALUES (?1, '/mnt/music', '/srv/music', '/mnt/music')",
            [REMOTE_ID],
        )
        .expect("insert released Local access");

    connection
        .execute(
            "INSERT INTO tracks(
                 source_id, track_id, album_id, title, artist, artist_id, album,
                 year, duration_seconds, favorite, disc_number, track_number,
                 image_item_id, image_tag, local_path, source_format, sync_generation
             ) VALUES (
                 ?1, ?2, ?3, 'Cue Track', 'Cue Artist', ?4, 'Cue Album',
                 2024, 240, 0, 1, 1, NULL, NULL,
                 '/music/Album/disc.flac', 'flac', 1
             )",
            params![LOCAL_ID, LOCAL_TRACK_ID, LOCAL_ALBUM_ID, LOCAL_ARTIST_ID],
        )
        .expect("insert released Local Track");
    connection
        .execute(
            "INSERT INTO source_objects(
                 source_id, source_object_id, entity_kind, entity_id,
                 source_object_kind, source_path, parent_source_object_id,
                 cue_path, cue_revision, cue_track_index,
                 segment_start_ms, segment_end_ms, metadata_json, sync_generation
             ) VALUES (
                 ?1, 'cue-track:one', 'track', ?2, 'cue_track',
                 '/music/Album/disc.flac', 'local-file:disc',
                 '/music/Album/disc.cue', 'cue-revision', 1,
                 1000, 241000, '{}', 1
             )",
            params![LOCAL_ID, LOCAL_TRACK_ID],
        )
        .expect("insert released CUE relationship");

    let legacy_queue = serde_json::json!({
        "server_id": LOCAL_ID,
        "entries": [{
            "id": "local-occurrence",
            "track_id": LOCAL_TRACK_ID,
            "album_id": LOCAL_ALBUM_ID,
            "title": "Cue Track",
            "artist": "Cue Artist",
            "artist_id": LOCAL_ARTIST_ID,
            "album": "Cue Album",
            "year": 2024,
            "duration_seconds": 240,
            "favorite": false,
            "image_ref": null,
            "local_path": "/obsolete/fallback.flac",
            "source_format": "flac",
            "origin": {
                "Source": {
                    "shuffle_key": concat!(
                        "source-shuffle|source=released-local-context",
                        "|source-index=4|track=local:track:cue-one"
                    )
                }
            }
        }],
        "current_index": 0,
        "repeat_mode": "All",
        "shuffle": { "enabled": false },
        "progress_seconds": 17
    });
    connection
        .execute(
            "INSERT INTO playback_checkpoints(
                 source_id, revision, selected_occurrence_id, progress_millis,
                 repeat_mode, shuffle_enabled, payload
             ) VALUES (?1, 0, 'local-occurrence', 17000, 'All', 0, ?2)",
            params![LOCAL_ID, legacy_queue.to_string()],
        )
        .expect("insert released legacy queue");

    let remote_track = serde_json::json!({
        "id": "remote:track:one",
        "album_id": "remote:album:one",
        "title": "Remote Track",
        "artist": "Remote Artist",
        "artist_id": "remote:artist:one",
        "artist_credits": [],
        "album_artist_credits": [],
        "album": "Remote Album",
        "year": 2025,
        "duration_seconds": 200,
        "favorite": true,
        "disc_number": 1,
        "track_number": 2,
        "image_ref": null,
        "local_path": null,
        "source_format": "mp3",
        "musicbrainz_recording_id": null
    });
    let remote_queue = serde_json::json!({
        "version": 1,
        "entries": [
            {
                "occurrence": "remote-occurrence-a",
                "track": remote_track.clone(),
                "provenance": "Manual"
            },
            {
                "occurrence": "remote-occurrence-b",
                "track": remote_track,
                "provenance": "Radio"
            }
        ],
        "traversal": ["remote-occurrence-b", "remote-occurrence-a"]
    });
    connection
        .execute(
            "INSERT INTO playback_checkpoints(
                 source_id, revision, selected_occurrence_id, progress_millis,
                 repeat_mode, shuffle_enabled, payload
             ) VALUES (?1, 7, 'remote-occurrence-a', 23000, 'One', 1, ?2)",
            params![REMOTE_ID, remote_queue.to_string()],
        )
        .expect("insert released v1 queue");
    let navidrome_track = serde_json::json!({
        "id": "navidrome:track:one",
        "album_id": "navidrome:album:one",
        "title": "Navidrome Track",
        "artist": "Navidrome Artist",
        "artist_id": "navidrome:artist:one",
        "artist_credits": [],
        "album_artist_credits": [],
        "album": "Navidrome Album",
        "year": 2025,
        "duration_seconds": 210,
        "favorite": false,
        "disc_number": 1,
        "track_number": 1,
        "image_ref": null,
        "local_path": null,
        "source_format": "flac",
        "musicbrainz_recording_id": null
    });
    let navidrome_entries = (0..100)
        .map(|index| {
            serde_json::json!({
                "occurrence": format!("navidrome-occurrence-{index}"),
                "track": navidrome_track.clone(),
                "provenance": "Manual"
            })
        })
        .collect::<Vec<_>>();
    let navidrome_queue = serde_json::json!({
        "version": 1,
        "entries": navidrome_entries,
        "traversal": []
    });
    connection
        .execute(
            "INSERT INTO playback_checkpoints(
                 source_id, revision, selected_occurrence_id, progress_millis,
                 repeat_mode, shuffle_enabled, payload
             ) VALUES (
                 ?1, 12, 'navidrome-occurrence-42', 31000, 'Off', 0, ?2
             )",
            params![NAVIDROME_ID, navidrome_queue.to_string()],
        )
        .expect("insert released Navidrome queue");

    for (kind, item_id) in [
        ("track", LOCAL_TRACK_ID),
        ("album", LOCAL_ALBUM_ID),
        ("artist", LOCAL_ARTIST_ID),
    ] {
        connection
            .execute(
                "INSERT INTO item_favorite_overrides(
                     source_id, item_kind, item_id, favorite
                 ) VALUES (?1, ?2, ?3, 1)",
                params![LOCAL_ID, kind, item_id],
            )
            .expect("insert released Local favorite");
    }
    connection
        .execute(
            "INSERT INTO item_favorite_overrides(
                 source_id, item_kind, item_id, favorite
             ) VALUES (?1, 'track', 'remote:track:one', 1)",
            [REMOTE_ID],
        )
        .expect("insert remote favorite that must remain source-owned");

    connection
        .execute(
            "INSERT INTO playlists(
                 source_id, playlist_id, name, track_count, duration_seconds,
                 owner, sync_generation
             ) VALUES (?1, ?2, 'Duplicates', 2, 480, 'store', 1)",
            params![LOCAL_ID, LOCAL_PLAYLIST_ID],
        )
        .expect("insert released Local Playlist");
    for (position, occurrence) in [(1, "second"), (0, "first")] {
        connection
            .execute(
                "INSERT INTO playlist_tracks(
                     source_id, playlist_id, entry_id, track_id, position,
                     sync_generation
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                params![
                    LOCAL_ID,
                    LOCAL_PLAYLIST_ID,
                    occurrence,
                    LOCAL_TRACK_ID,
                    position
                ],
            )
            .expect("insert released Playlist occurrence");
    }

    for source_id in [LOCAL_ID, REMOTE_ID, NAVIDROME_ID] {
        for (key, builtin, name, position) in [
            (
                "most_played",
                SmartPlaylistBuiltin::MostPlayed,
                "Most played",
                7,
            ),
            (
                "never_played",
                SmartPlaylistBuiltin::NeverPlayed,
                "Never played",
                2,
            ),
            (
                "most_skipped",
                SmartPlaylistBuiltin::MostSkipped,
                "Most skipped",
                9,
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO smart_playlists(
                         source_id, smart_playlist_id, name, builtin_key,
                         definition_json, position
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        source_id,
                        format!("{source_id}:smart:{key}"),
                        name,
                        key,
                        released_builtin_smart_playlist_definition(builtin),
                        position
                    ],
                )
                .expect("insert released built-in smart Playlist");
        }
    }
    connection
        .execute(
            "INSERT INTO smart_playlists(
                 source_id, smart_playlist_id, name, builtin_key,
                 definition_json, position
             ) VALUES (?1, 'local:smart:artists', 'Either Artist', NULL, ?2, 5)",
            params![
                LOCAL_ID,
                serde_json::json!({
                    "root": {
                        "mode": "Any",
                        "rules": [
                            {
                                "Rule": {
                                    "field": "Artist",
                                    "operator": "Equals",
                                    "value": {"Text": "Cannons"}
                                }
                            },
                            {
                                "Rule": {
                                    "field": "Artist",
                                    "operator": "Equals",
                                    "value": {"Text": "Night Tapes"}
                                }
                            }
                        ]
                    },
                    "sort_field": "Title",
                    "descending": false
                })
                .to_string()
            ],
        )
        .expect("insert released Any smart Playlist");
    for (period, plays, skips, last_played) in [
        ("legacy", 2, 1, "2026-06-01T10:00:00Z"),
        ("2026-07", 3, 2, "2026-07-24T10:00:00Z"),
    ] {
        connection
            .execute(
                "INSERT INTO track_activity_period(
                     source_id, period, track_id, qualified_plays, skips,
                     last_played_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![LOCAL_ID, period, LOCAL_TRACK_ID, plays, skips, last_played],
            )
            .expect("insert released activity");
    }
}

fn released_builtin_smart_playlist_definition(builtin: SmartPlaylistBuiltin) -> String {
    let (field, operator, value, sort_field, descending) = match builtin {
        SmartPlaylistBuiltin::MostPlayed => (
            "Played",
            "Is",
            serde_json::json!({"Bool": true}),
            "PlayCount",
            true,
        ),
        SmartPlaylistBuiltin::NeverPlayed => (
            "Played",
            "Is",
            serde_json::json!({"Bool": false}),
            "Title",
            false,
        ),
        SmartPlaylistBuiltin::MostSkipped => (
            "SkipCount",
            "Above",
            serde_json::json!({"Number": 0}),
            "SkipCount",
            true,
        ),
    };
    serde_json::json!({
        "root": {
            "mode": "All",
            "rules": [{
                "Rule": {
                    "field": field,
                    "operator": operator,
                    "value": value
                }
            }]
        },
        "sort_field": sort_field,
        "descending": descending
    })
    .to_string()
}

fn local_track() -> Track {
    let artist = ArtistCredit {
        id: ArtistId::new(LOCAL_ARTIST_ID),
        name: "Cue Artist".to_string(),
        musicbrainz_artist_id: None,
    };
    Track::new(TrackData {
        id: TrackId::new(LOCAL_TRACK_ID),
        album_id: Some(AlbumId::new(LOCAL_ALBUM_ID)),
        title: "Cue Track".to_string(),
        artist: "Cue Artist".to_string(),
        album: "Cue Album".to_string(),
        album_artwork: None,
        year: 2024,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: 240,
        favorite: false,
        disc_number: 1,
        track_number: 1,
        image_ref: None,
        local_artwork: None,
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        source_path: Some("/music/Album/disc.flac".to_string()),
        cue: Some(CueSegment {
            cue_path: "/music/Album/disc.cue".to_string(),
            start_millis: 1_000,
            end_millis: 241_000,
        }),
        source_format: Some("flac".to_string()),
        comment: None,
        skip_count: None,
        bpm: None,
        relations: TrackRelations {
            artists: vec![artist.clone()],
            album_artists: vec![artist],
            genres: Vec::new(),
            moods: Vec::new(),
            music_folders: Vec::new(),
        },
    })
}

const RELEASED_SCHEMA: &str = r#"
PRAGMA user_version = 30;
CREATE TABLE sources (
    source_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    provider_payload TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE source_local_access (
    source_id TEXT PRIMARY KEY REFERENCES sources(source_id) ON DELETE CASCADE,
    root_path TEXT NOT NULL,
    path_replace_from TEXT,
    path_replace_to TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE source_library_preferences (
    source_id TEXT PRIMARY KEY REFERENCES sources(source_id) ON DELETE CASCADE,
    selected_music_folder_id TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE active_source (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE
);
CREATE TABLE tracks (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    track_id TEXT NOT NULL,
    album_id TEXT NOT NULL,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    artist_id TEXT,
    album TEXT NOT NULL,
    year INTEGER NOT NULL,
    release_date TEXT,
    date_added TEXT,
    last_played TEXT,
    play_count INTEGER,
    user_rating INTEGER,
    duration_seconds INTEGER NOT NULL,
    favorite INTEGER NOT NULL,
    disc_number INTEGER NOT NULL,
    track_number INTEGER NOT NULL,
    image_item_id TEXT,
    image_tag TEXT,
    local_path TEXT,
    source_format TEXT,
    comment TEXT,
    skip_count INTEGER,
    bpm INTEGER,
    sync_generation INTEGER NOT NULL,
    PRIMARY KEY (source_id, track_id)
);
CREATE TABLE source_objects (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    source_object_id TEXT NOT NULL,
    entity_kind TEXT NOT NULL DEFAULT '',
    entity_id TEXT,
    source_object_kind TEXT NOT NULL,
    source_path TEXT,
    parent_source_object_id TEXT,
    cue_path TEXT,
    cue_revision TEXT,
    cue_track_index INTEGER,
    segment_start_ms INTEGER,
    segment_end_ms INTEGER,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    sync_generation INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (source_id, source_object_id, entity_kind)
);
CREATE TABLE playback_checkpoints (
    source_id TEXT PRIMARY KEY REFERENCES sources(source_id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    selected_occurrence_id TEXT,
    progress_millis INTEGER NOT NULL,
    repeat_mode TEXT NOT NULL,
    shuffle_enabled INTEGER NOT NULL,
    payload TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE item_favorite_overrides (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    item_kind TEXT NOT NULL CHECK (
        item_kind IN ('album', 'track', 'artist', 'album_artist')
    ),
    item_id TEXT NOT NULL,
    favorite INTEGER NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (source_id, item_kind, item_id)
);
CREATE TABLE playlists (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    playlist_id TEXT NOT NULL,
    name TEXT NOT NULL,
    track_count INTEGER NOT NULL,
    duration_seconds INTEGER NOT NULL,
    top_genres_json TEXT NOT NULL DEFAULT '[]',
    image_item_id TEXT,
    image_tag TEXT,
    owner TEXT NOT NULL DEFAULT 'native' CHECK (owner IN ('native', 'store')),
    sync_generation INTEGER NOT NULL,
    PRIMARY KEY (source_id, playlist_id)
);
CREATE TABLE playlist_tracks (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    playlist_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    sync_generation INTEGER NOT NULL,
    PRIMARY KEY (source_id, playlist_id, entry_id)
);
CREATE TABLE smart_playlists (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    smart_playlist_id TEXT NOT NULL,
    name TEXT NOT NULL,
    builtin_key TEXT,
    definition_json TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (source_id, smart_playlist_id)
);
CREATE TABLE track_activity_period (
    source_id TEXT NOT NULL REFERENCES sources(source_id) ON DELETE CASCADE,
    period TEXT NOT NULL,
    track_id TEXT NOT NULL,
    qualified_plays INTEGER NOT NULL DEFAULT 0,
    skips INTEGER NOT NULL DEFAULT 0,
    last_played_at TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (source_id, period, track_id)
);
"#;

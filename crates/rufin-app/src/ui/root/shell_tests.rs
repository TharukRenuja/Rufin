use super::cover::{collection_cover_decode_extent, cover_group_collage_ready};
use super::grouped_detail_view::GROUPED_DETAIL_COVER_FETCH_SIZE;
use super::lyrics_playback_state::allow_loaded_lyrics_cache_revisit;
use super::responsive_layout_state::startup_loading_screen_active;
use super::right_panel::{
    clamp_queue_lyrics_position, queue_lyrics_default_position, queue_lyrics_initial_position,
    queue_lyrics_position_from_ratio, queue_lyrics_position_ratio,
};
use super::startup_reveal::{
    StartupRevealAction, connection_progress_status_label, cover_warm_delay,
    startup_loading_status_label, startup_loading_status_parts, startup_prime_action,
    startup_route_reveal_action, take_pending_warm,
};
use super::{
    AutoLyricsRequest, LocalSourceCacheGateAction, LocalSourceCacheGateInput,
    PlaylistEntryListState, PlaylistEntrySort, SnapshotRenderDecision, album_play_activation,
    auto_lyrics_request_for_settings, auto_lyrics_skip_action_enabled,
    cover::record_cover_path_lookup_request, current_playback_track_id,
    home_visible_sections::changed_visible_home_section_kinds, local_source_cache_gate_action,
    local_source_snapshot_is_syncing, lyrics_result_subtitle, lyrics_result_subtitle_markup,
    lyrics_result_title_markup, lyrics_search_response_matches_query,
    playlist_detail_compact_for_width, playlist_drop_index, playlist_entries_for_state,
    playlist_entry_play_activation, playlist_route_margin, playlist_sort_width,
    preferences_login_status_toast_message, queue_source_waits_for_snapshot,
    seekbar_target_seconds, snapshot_event_outcome,
};
use crate::controller::{
    LibraryCounts, LibraryHomeUpdate, LibrarySyncStatus, LyricsSearchResult, NormalizedPlayTarget,
    PlayAnchor, PlayTarget, normalize_loaded_source_activation,
};
use gdk_pixbuf::{Colorspace, Pixbuf};
use rufin_core::{
    Album, AlbumId, AppSettings, ArtistId, Genre, GenreId, HomeBlockKind, HomeSection,
    HomeSectionKind, ImageRef, LibraryLayout, LibrarySourceSelection, Playlist, PlaylistId,
    QueueAnchor, QueueEntry, QueueEntryId, QueueSnapshot, RepeatMode, Route, SearchKind, ServerId,
    ServerIdentity, ShuffleState, SidebarRouteItem, SmartPlaylist, SmartPlaylistDefinition,
    SmartPlaylistId, SmartPlaylistMatchMode, SmartPlaylistRuleGroup, SmartPlaylistSortField, Track,
    TrackId, TrackSortKey, TrackTableSettings,
};
use rufin_provider::{LyricLine, Lyrics, LyricsSource, PlaylistEntry, SearchResults};
use rufin_store::LibraryDelta;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn shell_hide_available() {
    let mut settings = AppSettings::default();

    assert!(!super::tray::exit_tray_hide(&settings, true));

    settings.exit_to_tray = true;

    assert!(!super::tray::exit_tray_hide(&settings, true));

    settings.tray_enabled = true;

    assert!(super::tray::exit_tray_hide(&settings, true));
    assert!(!super::tray::exit_tray_hide(&settings, false));
}

#[test]
fn shell_start_available() {
    let mut settings = AppSettings::default();

    assert!(!super::tray::should_start_minimized(&settings, true));

    settings.start_minimized = true;

    assert!(!super::tray::should_start_minimized(&settings, true));

    settings.tray_enabled = true;

    assert!(super::tray::should_start_minimized(&settings, true));
    assert!(!super::tray::should_start_minimized(&settings, false));
}

#[test]
pub(in crate::ui) fn shell_reuse_cover() {
    let candidates = super::decoded_cover_candidate_sizes(super::DETAIL_COVER_SIZE);

    assert!(candidates.contains(&super::DETAIL_COVER_SIZE));
    assert!(candidates.contains(&super::GRID_COVER_SIZE));
    assert!(
        candidates
            .iter()
            .position(|size| *size == super::DETAIL_COVER_SIZE)
            < candidates
                .iter()
                .position(|size| *size == super::GRID_COVER_SIZE)
    );
}
#[test]
pub(in crate::ui) fn shell_use_thumbnail() {
    let server_id = ServerId::new("server:one");
    let image_ref = ImageRef::new("jellyfin:album:one", Some("tag-one".to_string()));
    let grid_key = rufin_store::image_cache_key(
        &server_id,
        &image_ref.item_id,
        image_ref.tag.as_deref().expect("tag"),
        super::GRID_COVER_SIZE,
    );
    let grid_path = PathBuf::from("/tmp/rufin-grid-cover.jpg");

    let artwork = super::playback_artwork_path_from_lookup(
        &server_id,
        &image_ref,
        super::THUMB_COVER_SIZE,
        |key| (key == grid_key).then(|| grid_path.clone()),
    )
    .expect("playback artwork path");

    assert_eq!(artwork.key, grid_key);
    assert_eq!(artwork.path, grid_path);
}

#[test]
pub(in crate::ui) fn shell_accept_size() {
    let server_id = ServerId::new("server:one");
    let image_ref = ImageRef::new("jellyfin:album:one", Some("tag-one".to_string()));
    let grid_key = rufin_store::image_cache_key(
        &server_id,
        &image_ref.item_id,
        image_ref.tag.as_deref().expect("tag"),
        super::GRID_COVER_SIZE,
    );
    let other_key = rufin_store::image_cache_key(
        &ServerId::new("server:two"),
        &image_ref.item_id,
        image_ref.tag.as_deref().expect("tag"),
        super::GRID_COVER_SIZE,
    );

    assert!(super::playback_artwork_key_matches(
        &server_id,
        &image_ref,
        super::THUMB_COVER_SIZE,
        &grid_key,
    ));
    assert!(!super::playback_artwork_key_matches(
        &server_id,
        &image_ref,
        super::THUMB_COVER_SIZE,
        &other_key,
    ));
}

#[test]
pub(in crate::ui) fn shell_playback_portals() {
    let cover = Pixbuf::new(Colorspace::Rgb, false, 8, 320, 180).expect("cover pixbuf");
    cover.fill(0x336699ff);

    let bytes = super::notification_icon_pixbuf(&cover).expect("notification bytes");
    let icon = Pixbuf::from_read(Cursor::new(bytes)).expect("notification pixbuf");

    assert_eq!(icon.width(), super::THUMB_COVER_SIZE as i32);
    assert_eq!(icon.height(), super::THUMB_COVER_SIZE as i32);
}

#[test]
pub(in crate::ui) fn shell_warm_lookup() {
    let mut lookups = HashMap::new();

    assert!(record_cover_path_lookup_request(
        &mut lookups,
        "album-art".to_string(),
        super::CoverPathLookupIntent::Warm,
    ));
    assert!(!record_cover_path_lookup_request(
        &mut lookups,
        "album-art".to_string(),
        super::CoverPathLookupIntent::Visible,
    ));
    assert_eq!(
        lookups.get("album-art"),
        Some(&super::CoverPathLookupIntent::Visible)
    );

    assert!(record_cover_path_lookup_request(
        &mut lookups,
        "now-playing".to_string(),
        super::CoverPathLookupIntent::Visible,
    ));
    assert!(!record_cover_path_lookup_request(
        &mut lookups,
        "now-playing".to_string(),
        super::CoverPathLookupIntent::Warm,
    ));
    assert_eq!(
        lookups.get("now-playing"),
        Some(&super::CoverPathLookupIntent::Visible)
    );
}
#[test]
pub(in crate::ui) fn shell_home_sections() {
    let explore = test_home_album_section(HomeSectionKind::Explore, 1);
    let most_played = test_home_album_section(HomeSectionKind::MostPlayed, 2);
    let previous = vec![explore.clone(), most_played.clone()];
    let mut changed_explore = explore.clone();
    changed_explore.albums[0].title = "Different explore album".to_string();
    let mut changed_most_played = most_played.clone();
    changed_most_played.albums[0].title = "Different most played album".to_string();
    let sections = vec![changed_explore, changed_most_played];
    let visible = vec![
        HomeSectionKind::Explore,
        HomeSectionKind::MostPlayed,
        HomeSectionKind::RecentlyPlayed,
    ];

    assert_eq!(
        changed_visible_home_section_kinds(visible.clone(), &previous, &sections, false),
        vec![HomeSectionKind::MostPlayed]
    );
    assert_eq!(
        changed_visible_home_section_kinds(visible, &previous, &sections, true),
        vec![HomeSectionKind::Explore, HomeSectionKind::MostPlayed]
    );
}
#[test]
pub(in crate::ui) fn shell_prioritize_completion() {
    let previous_source = None;
    let next_source = Some(LibrarySourceSelection::Local);

    let outcome = snapshot_event_outcome(true, false, &previous_source, &next_source, true, true);

    assert_eq!(outcome.render, SnapshotRenderDecision::FirstRunFinished);
    assert!(!outcome.entered_first_run);
}
#[test]
pub(in crate::ui) fn shell_change_source() {
    let previous_source = None;
    let next_source = Some(LibrarySourceSelection::Local);

    let outcome =
        snapshot_event_outcome(false, false, &previous_source, &next_source, false, false);

    assert_eq!(outcome.render, SnapshotRenderDecision::SourceChanged);
    assert!(!outcome.entered_first_run);
}
#[test]
pub(in crate::ui) fn shell_preserve_source() {
    let source = Some(LibrarySourceSelection::Local);

    let outcome = snapshot_event_outcome(false, false, &source, &source, false, false);

    assert_eq!(outcome.render, SnapshotRenderDecision::PreserveScroll);
    assert!(!outcome.entered_first_run);
}
#[test]
pub(in crate::ui) fn shell_apply_sync_status() {
    let mut library = test_library_snapshot();
    let server = test_server("active");
    library.server = Some(server.clone());
    library.selected_source = Some(LibrarySourceSelection::Server(server.id.clone()));
    library.cached_track_count = 2;

    let applied = super::apply_library_sync_status(
        &mut library,
        LibrarySyncStatus {
            server_id: server.id.clone(),
            sync_status: "Cached library ready".to_string(),
            last_error: None,
            counts: LibraryCounts {
                tracks: 2,
                ..LibraryCounts::default()
            },
            home: None,
            delta: LibraryDelta::default(),
        },
    );

    assert!(applied);
    assert_eq!(library.sync_status, "Cached library ready");
    assert_eq!(library.cached_track_count, 2);
}
#[test]
pub(in crate::ui) fn shell_apply_sync_delta_invalidates_loaded_pages() {
    let mut library = test_library_snapshot();
    let server = test_server("active");
    let section = HomeSection {
        kind: HomeSectionKind::Explore,
        albums: Vec::new(),
        tracks: Vec::new(),
    };
    let track = test_track("Track", Some(ArtistId::fake(1)));
    library.server = Some(server.clone());
    library.selected_source = Some(LibrarySourceSelection::Server(server.id.clone()));
    library.tracks = vec![track.clone()];
    library.favorites = vec![track.clone()];
    library.search = SearchResults {
        tracks: vec![track.clone()],
        ..SearchResults::default()
    };

    let applied = super::apply_library_sync_status(
        &mut library,
        LibrarySyncStatus {
            server_id: server.id.clone(),
            sync_status: "Cached library ready".to_string(),
            last_error: None,
            counts: LibraryCounts {
                tracks: 30_000,
                playlists: 40,
                ..LibraryCounts::default()
            },
            home: Some(LibraryHomeUpdate {
                sections: vec![section.clone()],
                prefetched_explore: None,
            }),
            delta: LibraryDelta {
                tracks: rufin_store::TrackDelta {
                    fields: vec![track.id.clone()],
                    ..Default::default()
                },
                home_changed: true,
                ..LibraryDelta::default()
            },
        },
    );

    assert!(applied);
    assert!(library.tracks.is_empty());
    assert!(library.favorites.is_empty());
    assert!(library.search.tracks.is_empty());
    assert_eq!(library.cached_track_count, 30_000);
    assert_eq!(library.cached_playlist_count, 40);
    assert_eq!(library.home_sections, vec![section]);
}

#[test]
pub(in crate::ui) fn shell_apply_sync_playlist_entries_keep_playlist_page() {
    let mut library = test_library_snapshot();
    let server = test_server("active");
    let playlist = test_playlist("Regular", test_image_ref("playlist"));
    library.server = Some(server.clone());
    library.selected_source = Some(LibrarySourceSelection::Server(server.id.clone()));
    library.playlists = vec![playlist.clone()];

    let applied = super::apply_library_sync_status(
        &mut library,
        LibrarySyncStatus {
            server_id: server.id.clone(),
            sync_status: "Cached library ready".to_string(),
            last_error: None,
            counts: LibraryCounts {
                playlists: 1,
                ..LibraryCounts::default()
            },
            home: None,
            delta: LibraryDelta {
                playlists: rufin_store::PlaylistDelta {
                    entries: vec![playlist.id.clone()],
                    ..Default::default()
                },
                ..LibraryDelta::default()
            },
        },
    );

    assert!(applied);
    assert_eq!(library.playlists, vec![playlist]);
    assert_eq!(library.cached_playlist_count, 1);
}
#[test]
pub(in crate::ui) fn shell_ignore_stale_sync_status() {
    let mut library = test_library_snapshot();
    let server = test_server("active");
    library.server = Some(server.clone());
    library.selected_source = Some(LibrarySourceSelection::Server(server.id.clone()));
    library.sync_status = "Cached library ready".to_string();

    let applied = super::apply_library_sync_status(
        &mut library,
        LibrarySyncStatus {
            server_id: ServerId::new("server:stale"),
            sync_status: "Sync needs attention".to_string(),
            last_error: Some("sync failed".to_string()),
            counts: LibraryCounts {
                tracks: 10,
                ..LibraryCounts::default()
            },
            home: None,
            delta: LibraryDelta {
                home_changed: true,
                ..LibraryDelta::default()
            },
        },
    );

    assert!(!applied);
    assert_eq!(library.sync_status, "Cached library ready");
    assert_eq!(library.last_error, None);
}
#[test]
pub(in crate::ui) fn shell_snapshot_entry() {
    let source = None::<LibrarySourceSelection>;

    let outcome = snapshot_event_outcome(false, true, &source, &source, false, false);

    assert_eq!(outcome.render, SnapshotRenderDecision::PreserveScroll);
    assert!(outcome.entered_first_run);
}
#[test]
pub(in crate::ui) fn shell_map_states() {
    let cases = [
        (
            "cached library stays visible",
            LocalCacheGateInput::cached(false, false, "Cached library ready"),
            LocalSourceCacheGateAction::None,
        ),
        (
            "folder change waits behind gate",
            LocalCacheGateInput::cached_startup(true, false, "Cached library ready"),
            LocalSourceCacheGateAction::Enter,
        ),
        (
            "revealed cached folder change stays visible",
            LocalCacheGateInput::cached(true, false, "Cached library ready"),
            LocalSourceCacheGateAction::None,
        ),
        (
            "empty folder selection stays visible",
            LocalCacheGateInput {
                local_folders_changed: true,
                has_local_folders: false,
                has_cached_library: false,
                startup_route_revealed: true,
                preparing: false,
                sync_seen: false,
                sync_status: "Cached library ready",
            },
            LocalSourceCacheGateAction::None,
        ),
        (
            "uncached local sync waits behind gate",
            LocalCacheGateInput::uncached(false, false, "Syncing library…"),
            LocalSourceCacheGateAction::Enter,
        ),
        (
            "cached sync stays visible",
            LocalCacheGateInput::cached(false, false, "Syncing library…"),
            LocalSourceCacheGateAction::None,
        ),
        (
            "preparing waits for first snapshot",
            LocalCacheGateInput::uncached(false, true, "Cached library ready"),
            LocalSourceCacheGateAction::Wait,
        ),
        (
            "preparing waits while snapshot is syncing",
            LocalCacheGateInput::uncached(true, true, "Syncing library…"),
            LocalSourceCacheGateAction::Wait,
        ),
        (
            "preparing reveals after synced snapshot",
            LocalCacheGateInput::uncached(true, true, "Cached library ready"),
            LocalSourceCacheGateAction::Reveal,
        ),
    ];

    assert!(local_source_snapshot_is_syncing("Syncing library…"));
    for (name, input, expected) in cases {
        assert_eq!(local_cache_gate_action(input), expected, "{name}");
    }
}

#[test]
pub(in crate::ui) fn shell_source_local() {
    let source = Some(LibrarySourceSelection::Server(rufin_core::ServerId::new(
        "jellyfin:server:test",
    )));

    assert_eq!(
        local_source_cache_gate_action(LocalSourceCacheGateInput {
            local_folders_changed: false,
            next_source: &source,
            has_local_folders: true,
            has_cached_library: true,
            startup_route_revealed: true,
            preparing: true,
            sync_seen: true,
            sync_status: "Cached library ready",
        }),
        LocalSourceCacheGateAction::Cancel
    );
}
#[test]
pub(in crate::ui) fn shell_match_snapshot() {
    let old_source = ServerId::new("jellyfin:server:old");
    let next_source = ServerId::new("local:source");
    let queue = QueueSnapshot {
        server_id: next_source.clone(),
        entries: Vec::new(),
        current_index: None,
        repeat_mode: RepeatMode::All,
        shuffle: Default::default(),
        shuffle_order: Vec::new(),
        progress_seconds: 0,
    };

    assert!(queue_source_waits_for_snapshot(
        Some(&queue),
        Some(&old_source)
    ));
    assert!(!queue_source_waits_for_snapshot(
        Some(&queue),
        Some(&next_source)
    ));
    assert!(!queue_source_waits_for_snapshot(None, Some(&old_source)));
}
#[test]
pub(in crate::ui) fn shell_use_reveal() {
    assert!(startup_loading_screen_active(false, false));
    assert!(!startup_loading_screen_active(true, false));
    assert!(!startup_loading_screen_active(false, true));
}
#[test]
pub(in crate::ui) fn startup_route_reveal() {
    assert_eq!(
        startup_route_reveal_action(
            true,
            4,
            Duration::from_millis(super::STARTUP_ROUTE_REVEAL_MAX_MS)
        ),
        StartupRevealAction::RevealExpired
    );
    assert_eq!(
        startup_route_reveal_action(
            false,
            4,
            Duration::from_millis(super::STARTUP_ROUTE_REVEAL_MAX_MS)
        ),
        StartupRevealAction::RevealExpired
    );
    assert_eq!(
        startup_route_reveal_action(
            true,
            0,
            Duration::from_millis(super::STARTUP_ROUTE_REVEAL_MIN_MS)
        ),
        StartupRevealAction::RevealReady
    );
}
#[test]
pub(in crate::ui) fn run_cover_prime() {
    assert_eq!(
        startup_prime_action(3, Duration::from_millis(super::PRIME_TIMEOUT_MS)),
        StartupRevealAction::RevealExpired
    );
}
#[test]
pub(in crate::ui) fn shell_hide_status() {
    assert_eq!(startup_loading_status_label(""), None);
    assert_eq!(startup_loading_status_label("Cached library ready"), None);
    assert_eq!(
        startup_loading_status_label("Syncing Local library…"),
        Some("Syncing Local library…".to_string())
    );
}
#[test]
pub(in crate::ui) fn shell_split_status_detail() {
    assert_eq!(
        startup_loading_status_parts(
            "Caching local library… This may take some time. Reading track metadata for Local, 25/2,567 tracks processed (12s)"
        ),
        (
            "Caching local library… This may take some time.".to_string(),
            Some("Reading track metadata for Local, 25/2,567 tracks processed (12s)".to_string())
        )
    );
}
#[test]
pub(in crate::ui) fn shell_connection_progress_detail() {
    assert_eq!(
        connection_progress_status_label(
            "Caching library… This may take some time. Fetching music folders for Desktop (Jellyfin) (20s elapsed)"
        ),
        Some("Fetching music folders for Desktop (Jellyfin) (20s elapsed)".to_string())
    );
    assert_eq!(
        connection_progress_status_label(
            "Library cache ready for Desktop (Jellyfin) in 44s elapsed"
        ),
        Some("Preparing library…".to_string())
    );
}
#[test]
pub(in crate::ui) fn shell_group_cover_waits_for_multiple_ready_slots() {
    assert!(!cover_group_collage_ready(0, 0));
    assert!(!cover_group_collage_ready(1, 1));
    assert!(!cover_group_collage_ready(2, 0));
    assert!(!cover_group_collage_ready(2, 1));
    assert!(cover_group_collage_ready(2, 2));
    assert!(cover_group_collage_ready(4, 2));
}
#[test]
pub(in crate::ui) fn shell_collection_cover_uses_thumbnail_extent() {
    assert_eq!(
        collection_cover_decode_extent(super::THUMB_COVER_SIZE, 180),
        96
    );
    assert_eq!(
        collection_cover_decode_extent(super::GRID_COVER_SIZE, 180),
        180
    );
}

#[test]
pub(in crate::ui) fn shell_grouped_detail_uses_grid_collection_art() {
    assert_eq!(GROUPED_DETAIL_COVER_FETCH_SIZE, super::GRID_COVER_SIZE);
}
#[test]
pub(in crate::ui) fn shell_stay_cover() {
    let mut library = test_library_snapshot();
    let home_ref = test_image_ref("home");
    let mut home_album = test_album("Home Artist", Some(ArtistId::fake(90)));
    home_album.image_ref = Some(home_ref.clone());
    library.home_sections = vec![HomeSection {
        kind: HomeSectionKind::Explore,
        albums: vec![home_album],
        tracks: Vec::new(),
    }];

    let first_track_ref = test_image_ref("track-a");
    let mut first_track = test_track("Route Artist", Some(ArtistId::fake(1)));
    first_track.title = "A route track".to_string();
    first_track.image_ref = Some(first_track_ref.clone());
    let mut second_track = test_track("Route Artist", Some(ArtistId::fake(1)));
    second_track.id = TrackId::fake(2);
    second_track.title = "B route track".to_string();
    second_track.image_ref = Some(test_image_ref("track-b"));
    library.tracks = vec![second_track, first_track];

    let first_album_ref = test_image_ref("album-a");
    let mut first_album = test_album("Route Artist", Some(ArtistId::fake(2)));
    first_album.title = "A route album".to_string();
    first_album.image_ref = Some(first_album_ref.clone());
    let mut second_album = test_album("Route Artist", Some(ArtistId::fake(2)));
    second_album.id = AlbumId::fake(2);
    second_album.title = "B route album".to_string();
    second_album.image_ref = Some(test_image_ref("album-b"));
    library.albums = vec![second_album, first_album];

    let settings = AppSettings {
        home_blocks: vec![HomeBlockKind::Explore],
        ..Default::default()
    };
    let targets = super::startup_cover_targets(&library, &settings, 0);
    let target_refs = targets
        .iter()
        .map(|target| target.image_ref.item_id.as_str())
        .collect::<Vec<_>>();

    assert!(target_refs.contains(&home_ref.item_id.as_str()));
    assert!(!target_refs.contains(&first_track_ref.item_id.as_str()));
    assert!(!target_refs.contains(&first_album_ref.item_id.as_str()));

    let home_targets = super::startup_prime_targets(&library, &settings, 0);
    let home_target_refs = home_targets
        .iter()
        .map(|target| target.image_ref.item_id.as_str())
        .collect::<Vec<_>>();
    assert!(home_target_refs.contains(&home_ref.item_id.as_str()));
    assert!(!home_target_refs.contains(&first_track_ref.item_id.as_str()));
    assert!(!home_target_refs.contains(&first_album_ref.item_id.as_str()));
}

#[test]
pub(in crate::ui) fn shell_startup_playback_cover_target() {
    let playback_ref = test_image_ref("playback");
    let mut targets = Vec::new();
    let player = super::PlaybackSnapshot {
        current: Some(test_queue_entry("Now", playback_ref.clone())),
        ..super::PlaybackSnapshot::default()
    };

    super::push_startup_playback_targets(&mut targets, &player);

    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].image_ref, playback_ref);
    assert_eq!(targets[0].fetch_size, super::THUMB_COVER_SIZE);
    assert_eq!(targets[0].size, super::player::BOTTOM_PLAYER_COVER_SIZE);
}

#[test]
pub(in crate::ui) fn shell_startup_queue_cover_targets() {
    let first_ref = test_image_ref("queue-first");
    let second_ref = test_image_ref("queue-second");
    let queue = QueueSnapshot {
        server_id: ServerId::new("server:active"),
        entries: vec![
            test_queue_entry("Visible Song", first_ref.clone()),
            test_queue_entry("Hidden Song", second_ref.clone()),
        ],
        current_index: Some(0),
        repeat_mode: RepeatMode::Off,
        shuffle: ShuffleState::default(),
        shuffle_order: Vec::new(),
        progress_seconds: 0,
    };
    let mut targets = Vec::new();

    super::push_startup_queue_targets(
        &mut targets,
        Some(&queue),
        "visible",
        true,
        false,
        720,
        Some(&ServerId::new("server:active")),
    );

    let target_refs = targets
        .iter()
        .map(|target| target.image_ref.item_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(target_refs, vec![first_ref.item_id.as_str()]);

    targets.clear();
    super::push_startup_queue_targets(
        &mut targets,
        Some(&queue),
        "",
        false,
        false,
        720,
        Some(&ServerId::new("server:active")),
    );
    assert!(targets.is_empty());

    super::push_startup_queue_targets(
        &mut targets,
        Some(&queue),
        "",
        true,
        false,
        720,
        Some(&ServerId::new("server:stale")),
    );
    assert!(targets.is_empty());
}

#[test]
pub(in crate::ui) fn shell_warm_matrix() {
    let mut library = test_library_snapshot();
    library.server = Some(test_server("source"));
    let first_track_ref = test_image_ref("track-a");
    let mut first_track = test_track("Route Artist", Some(ArtistId::fake(1)));
    first_track.title = "A route track".to_string();
    first_track.image_ref = Some(first_track_ref.clone());
    let mut second_track = test_track("Route Artist", Some(ArtistId::fake(1)));
    second_track.id = TrackId::fake(2);
    second_track.title = "B route track".to_string();
    second_track.image_ref = Some(test_image_ref("track-b"));
    library.tracks = vec![second_track, first_track];

    let first_album_ref = test_image_ref("album-a");
    let mut first_album = test_album("Route Artist", Some(ArtistId::fake(2)));
    first_album.title = "A route album".to_string();
    first_album.image_ref = Some(first_album_ref.clone());
    let mut second_album = test_album("Route Artist", Some(ArtistId::fake(2)));
    second_album.id = AlbumId::fake(2);
    second_album.title = "B route album".to_string();
    second_album.image_ref = Some(test_image_ref("album-b"));
    library.albums = vec![second_album, first_album];

    let settings = AppSettings::default();
    let targets =
        super::source_warm_targets(&library, &[], &settings, test_initial_route_metrics());
    let target_refs = targets
        .iter()
        .map(|target| target.image_ref.item_id.as_str())
        .collect::<Vec<_>>();

    assert!(target_refs.contains(&first_track_ref.item_id.as_str()));
    assert!(target_refs.contains(&first_album_ref.item_id.as_str()));
    assert_eq!(
        target_refs
            .iter()
            .filter(|item_id| **item_id == first_track_ref.item_id)
            .count(),
        1
    );
    assert!(
        target_refs
            .iter()
            .position(|item_id| *item_id == first_track_ref.item_id)
            < target_refs
                .iter()
                .position(|item_id| *item_id == first_album_ref.item_id)
    );
}

#[test]
pub(in crate::ui) fn shell_include_refs() {
    let shared = test_image_ref("shared-art");
    let genre_only = test_image_ref("genre-only");
    let mut library = test_library_snapshot();
    library.server = Some(test_server("source"));
    let mut track = test_track("Route Artist", Some(ArtistId::fake(1)));
    track.image_ref = Some(shared.clone());
    library.tracks = vec![track];
    library.genres = vec![Genre {
        id: GenreId::fake(1),
        name: "Genre".to_string(),
        album_count: 1,
        track_count: 1,
        image_refs: vec![shared.clone(), genre_only.clone()],
        image_ref: Some(shared.clone()),
    }];

    let settings = AppSettings::default();
    let targets =
        super::source_warm_targets(&library, &[], &settings, test_initial_route_metrics());

    assert_eq!(
        targets
            .iter()
            .filter(|target| target.image_ref.item_id == shared.item_id)
            .count(),
        1
    );
    assert!(
        targets
            .iter()
            .any(|target| target.image_ref.item_id == genre_only.item_id)
    );
}

#[test]
pub(in crate::ui) fn shell_include_playlist() {
    let playlist_ref = test_image_ref("playlist-group");
    let smart_ref = test_image_ref("smart-group");
    let mut library = test_library_snapshot();
    library.server = Some(test_server("source"));
    library.playlists = vec![test_playlist("Regular", playlist_ref.clone())];
    let smart_playlists = vec![test_smart_playlist("Smart", smart_ref.clone())];

    let settings = AppSettings::default();
    let targets = super::source_warm_targets(
        &library,
        &smart_playlists,
        &settings,
        test_initial_route_metrics(),
    );
    let target_refs = targets
        .iter()
        .map(|target| target.image_ref.item_id.as_str())
        .collect::<Vec<_>>();

    assert!(target_refs.contains(&playlist_ref.item_id.as_str()));
    assert!(target_refs.contains(&smart_ref.item_id.as_str()));
}

#[test]
pub(in crate::ui) fn shell_warm_background_refs() {
    let mut library = test_library_snapshot();
    library.server = Some(test_server("source"));
    let background_ref = test_image_ref("background-album");
    library.albums = (0..24)
        .map(|index| {
            let mut album = test_album("Route Artist", Some(ArtistId::fake(index + 1)));
            album.id = AlbumId::fake(index + 1);
            album.title = format!("Album {index:02}");
            album.image_ref = Some(if index == 23 {
                background_ref.clone()
            } else {
                test_image_ref(&format!("album-{index:02}"))
            });
            album
        })
        .collect();

    let targets = super::source_warm_targets(
        &library,
        &[],
        &AppSettings::default(),
        test_initial_route_metrics(),
    );

    assert!(
        targets
            .iter()
            .any(|target| target.image_ref.item_id == background_ref.item_id)
    );
}

#[test]
pub(in crate::ui) fn shell_warm_route() {
    let genre_ref = test_image_ref("hidden-genre");
    let playlist_ref = test_image_ref("hidden-playlist");
    let mut library = test_library_snapshot();
    library.server = Some(test_server("source"));
    library.genres = vec![Genre {
        id: GenreId::fake(1),
        name: "Genre".to_string(),
        album_count: 1,
        track_count: 1,
        image_refs: vec![genre_ref.clone()],
        image_ref: None,
    }];
    library.playlists = vec![test_playlist("Regular", playlist_ref.clone())];
    let mut settings = AppSettings::default();
    for entry in &mut settings.sidebar.route_items {
        if matches!(
            entry.item,
            SidebarRouteItem::Genres | SidebarRouteItem::Playlists
        ) {
            entry.visible = false;
        }
    }

    let targets =
        super::source_warm_targets(&library, &[], &settings, test_initial_route_metrics());
    let target_refs = targets
        .iter()
        .map(|target| target.image_ref.item_id.as_str())
        .collect::<Vec<_>>();

    assert!(!target_refs.contains(&genre_ref.item_id.as_str()));
    assert!(!target_refs.contains(&playlist_ref.item_id.as_str()));
}

#[test]
pub(in crate::ui) fn shell_warm_settlement() {
    assert!(cover_warm_delay() > super::RESPONSIVE_RENDER_DELAY_MS * 4);
}

#[test]
pub(in crate::ui) fn shell_clear_pending() {
    let first = ServerId::new("source:first");
    let second = ServerId::new("source:second");
    let mut pending = Some((second.clone(), 2));

    assert!(!take_pending_warm(&mut pending, &first, 1));
    assert_eq!(pending, Some((second, 2)));

    let mut pending = Some((first.clone(), 3));
    assert!(!take_pending_warm(&mut pending, &first, 1));
    assert_eq!(pending, Some((first.clone(), 3)));
    assert!(take_pending_warm(&mut pending, &first, 3));
    assert_eq!(pending, None);
}

#[test]
pub(in crate::ui) fn shell_cover_rules() {
    let first = test_image_ref("first");
    let second = test_image_ref("second");
    let duplicate = first.clone();

    let slots = super::cover_group_slots(&[first.clone(), second.clone(), duplicate]);
    let slot_refs = slots
        .iter()
        .map(|image_ref| image_ref.item_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        slot_refs,
        vec![
            first.item_id.as_str(),
            second.item_id.as_str(),
            first.item_id.as_str(),
            first.item_id.as_str(),
        ]
    );
}
#[test]
pub(in crate::ui) fn row_bottom_clamp() {
    let (visible_start, visible_end) = super::visible_index_range_from_metrics(
        100,
        LibraryLayout::Row,
        5_000.0,
        500.0,
        50,
        4,
        160,
    );

    assert_eq!((visible_start, visible_end), (90, 100));
}
#[test]
pub(in crate::ui) fn shell_use_geometry() {
    assert_eq!(
        super::initial_visible_count_from_metrics(LibraryLayout::Row, 900, 720, 4, 160,),
        17
    );
    assert_eq!(
        super::initial_visible_count_from_metrics(LibraryLayout::Grid, 900, 720, 4, 160,),
        20
    );
}
#[test]
pub(in crate::ui) fn grid_bottom_clamp() {
    let (visible_start, visible_end) = super::visible_index_range_from_metrics(
        100,
        LibraryLayout::Grid,
        6_000.0,
        744.0,
        50,
        4,
        160,
    );

    assert_eq!((visible_start, visible_end), (84, 100));
}
#[test]
pub(in crate::ui) fn shell_clamp_height() {
    assert_eq!(clamp_queue_lyrics_position(800, 1701), 500);
    assert_eq!(clamp_queue_lyrics_position(800, 10), 120);
    assert_eq!(clamp_queue_lyrics_position(200, 1701), 120);
    assert_eq!(queue_lyrics_default_position(700), 400);
    assert_eq!(queue_lyrics_default_position(1400), 1000);
    assert_eq!(queue_lyrics_initial_position(700, None, None), 400);
    assert_eq!(queue_lyrics_initial_position(700, None, Some(0.5)), 350);
    assert_eq!(queue_lyrics_initial_position(700, None, Some(2.0)), 400);
    assert_eq!(
        queue_lyrics_initial_position(700, None, Some(f64::NAN)),
        400
    );
    assert_eq!(
        queue_lyrics_initial_position(700, Some(380), Some(0.5)),
        380
    );
    assert_eq!(queue_lyrics_position_from_ratio(700, 0.5), 350);
    assert_eq!(queue_lyrics_position_ratio(700, 350), 0.5);
    let saved_default_ratio = queue_lyrics_position_ratio(700, 400);
    assert_eq!(
        queue_lyrics_initial_position(1400, None, Some(saved_default_ratio)),
        800
    );
}
#[test]
pub(in crate::ui) fn shell_use_entry() {
    let track_id = TrackId::fake(7);
    let snapshot = super::PlaybackSnapshot {
        current: Some(QueueEntry {
            id: QueueEntryId::new("queue-7"),
            track_id: track_id.clone(),
            album_id: None,
            title: "Restored".to_string(),
            artist: "Artist".to_string(),
            artist_id: None,
            album: "Album".to_string(),
            year: 2026,
            duration_seconds: 180,
            favorite: false,
            image_ref: None,
            local_path: None,
            source_format: None,
            origin: None,
        }),
        ..super::PlaybackSnapshot::default()
    };

    assert_eq!(current_playback_track_id(&snapshot), Some(track_id));
    assert_eq!(
        current_playback_track_id(&super::PlaybackSnapshot::default()),
        None
    );
}
#[test]
pub(in crate::ui) fn shell_track_field() {
    let mut first = test_track("Artist B", None);
    first.title = "Alpha".to_string();
    first.album = "Plain Album".to_string();
    first.duration_seconds = 240;
    let mut second = test_track("Artist A", None);
    second.id = TrackId::fake(2);
    second.title = "Beta".to_string();
    second.album = "Needle Album".to_string();
    second.duration_seconds = 120;
    let entries = vec![
        PlaylistEntry {
            entry_id: "entry-alpha".to_string(),
            track: first,
        },
        PlaylistEntry {
            entry_id: "entry-beta".to_string(),
            track: second,
        },
    ];

    let filtered = playlist_entries_for_state(
        &entries,
        &PlaylistEntryListState {
            query: "needle".to_string(),
            sort: PlaylistEntrySort::Order,
            descending: false,
        },
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].1.entry_id, "entry-beta");

    let sorted = playlist_entries_for_state(
        &entries,
        &PlaylistEntryListState {
            query: String::new(),
            sort: PlaylistEntrySort::Album,
            descending: true,
        },
    );
    assert_eq!(sorted[0].1.entry_id, "entry-alpha");
    assert_eq!(sorted[1].1.entry_id, "entry-beta");
}
#[test]
pub(in crate::ui) fn shell_playlist_panes() {
    assert!(playlist_detail_compact_for_width(550));
    assert_eq!(playlist_route_margin(550), 16);
    assert!(!playlist_detail_compact_for_width(760));
    assert_eq!(playlist_route_margin(760), 24);
    assert_eq!(playlist_sort_width(360), 120);
    assert_eq!(playlist_sort_width(550), 150);
    assert_eq!(playlist_sort_width(760), 170);
}
#[test]
pub(in crate::ui) fn shell_track_occurrences() {
    let mut duplicate = test_track("Artist", None);
    duplicate.id = TrackId::fake(7);
    let entries = [
        PlaylistEntry {
            entry_id: "entry-a".to_string(),
            track: duplicate.clone(),
        },
        PlaylistEntry {
            entry_id: "entry-b".to_string(),
            track: duplicate,
        },
    ];
    let activation = playlist_entry_play_activation(
        PlaylistId::fake(3),
        &entries[1],
        1,
        &PlaylistEntryListState::default(),
    )
    .expect("playlist activation should be available");

    let PlayTarget::StoreBackedSource { anchor, .. } = activation.target else {
        panic!("playlist activation should use the store-backed source");
    };
    assert!(matches!(
        anchor,
        PlayAnchor {
            source_index: 1,
            source_item_id: Some(ref id),
            ..
        } if id == "entry-b"
    ));
}
#[test]
pub(in crate::ui) fn shell_track_anchor() {
    let tracks = (1..=3)
        .map(|number| {
            let mut track = test_track("Artist", None);
            track.id = TrackId::fake(number);
            track
        })
        .collect::<Vec<_>>();
    let activation =
        album_play_activation(AlbumId::fake(1), tracks, 1, None).expect("album activation");
    let normalized =
        normalize_loaded_source_activation(activation).expect("album activation should normalize");

    let NormalizedPlayTarget::Replacement(replacement) = normalized.target else {
        panic!("album activation should replace the queue");
    };
    assert_eq!(replacement.items.len(), 3);
    assert_eq!(
        replacement.anchor,
        QueueAnchor::SourcePosition {
            position: 1,
            track_id: TrackId::fake(2),
        }
    );
}
#[test]
pub(in crate::ui) fn shell_drop_source() {
    let entries = ["a", "b", "c"]
        .into_iter()
        .enumerate()
        .map(|(index, entry_id)| {
            let mut track = test_track("Artist", None);
            track.id = TrackId::fake(index + 1);
            PlaylistEntry {
                entry_id: entry_id.to_string(),
                track,
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(playlist_drop_index(&entries, "a", 2, false), Some(1));
    assert_eq!(playlist_drop_index(&entries, "a", 2, true), Some(2));
    assert_eq!(playlist_drop_index(&entries, "c", 0, false), Some(0));
    assert_eq!(playlist_drop_index(&entries, "b", 1, false), None);
}
#[test]
pub(in crate::ui) fn track_artist_route() {
    let track = test_track("Track Artist", Some(ArtistId::fake(3)));
    assert_eq!(
        super::track_artist_route(&track),
        Some(Route::ArtistDetail(ArtistId::fake(3)))
    );

    let track = test_track("Loose Artist", None);
    assert_eq!(
        super::track_artist_route(&track),
        Some(Route::Search {
            query: "Loose Artist".to_string(),
            kind: SearchKind::Artists,
        })
    );

    assert_eq!(super::track_artist_route(&test_track("   ", None)), None);
}
#[test]
pub(in crate::ui) fn album_artist_route() {
    let album = test_album("Album Artist", Some(ArtistId::fake(5)));
    assert_eq!(
        super::album_artist_route(&album),
        Some(Route::ArtistDetail(ArtistId::fake(5)))
    );

    let album = test_album("Compilation Artist", None);
    assert_eq!(
        super::album_artist_route(&album),
        Some(Route::Search {
            query: "Compilation Artist".to_string(),
            kind: SearchKind::Artists,
        })
    );

    assert_eq!(super::album_artist_route(&test_album("", None)), None);
}
#[test]
pub(in crate::ui) fn shell_track_option() {
    assert_eq!(
        sorted_artist_track_titles(true),
        vec!["Bravo".to_string(), "Zulu".to_string(), "Alpha".to_string()]
    );
    assert_eq!(
        sorted_artist_track_titles(false),
        vec!["Alpha".to_string(), "Bravo".to_string(), "Zulu".to_string()]
    );
}
#[test]
pub(in crate::ui) fn shell_use_clamped() {
    assert_eq!(seekbar_target_seconds(42.4, 180), 42);
    assert_eq!(seekbar_target_seconds(42.5, 180), 43);
    assert_eq!(seekbar_target_seconds(-10.0, 180), 0);
    assert_eq!(seekbar_target_seconds(220.0, 180), 180);
    assert_eq!(seekbar_target_seconds(f64::NAN, 180), 0);
}
fn sorted_artist_track_titles(favorite_first: bool) -> Vec<String> {
    let mut favorite_late = test_track("Artist", Some(ArtistId::fake(1)));
    favorite_late.id = TrackId::fake(1);
    favorite_late.title = "Zulu".to_string();
    favorite_late.favorite = true;
    let mut ordinary_first = test_track("Artist", Some(ArtistId::fake(1)));
    ordinary_first.id = TrackId::fake(2);
    ordinary_first.title = "Alpha".to_string();
    let mut favorite_early = test_track("Artist", Some(ArtistId::fake(1)));
    favorite_early.id = TrackId::fake(3);
    favorite_early.title = "Bravo".to_string();
    favorite_early.favorite = true;

    let mut tracks = vec![favorite_late, ordinary_first, favorite_early];
    let settings = TrackTableSettings {
        sort_key: TrackSortKey::Title,
        ..TrackTableSettings::default()
    };

    super::sort_tracks_with_options(&mut tracks, &settings, favorite_first);

    tracks.into_iter().map(|track| track.title).collect()
}
#[test]
pub(in crate::ui) fn shell_track_external() {
    let track_id = TrackId::fake(11);
    let mut settings = AppSettings {
        external_lyrics_enabled: true,
        ..AppSettings::default()
    };
    let remote_lyrics = Lyrics {
        track_id: track_id.clone(),
        source: LyricsSource::Remote,
        lines: vec![LyricLine {
            text: "remote line".to_string(),
            start_millis: None,
        }],
    };

    assert!(auto_lyrics_skip_action_enabled(
        &settings,
        Some(&track_id),
        Some(&remote_lyrics)
    ));

    settings
        .suppressed_auto_lyrics_track_ids
        .push(track_id.as_str().to_string());
    assert!(!auto_lyrics_skip_action_enabled(
        &settings,
        Some(&track_id),
        Some(&remote_lyrics)
    ));

    settings.suppressed_auto_lyrics_track_ids.clear();
    settings.external_lyrics_enabled = false;
    assert!(!auto_lyrics_skip_action_enabled(
        &settings,
        Some(&track_id),
        Some(&remote_lyrics)
    ));

    settings.external_lyrics_enabled = true;
    settings.private_mode = true;
    assert!(!auto_lyrics_skip_action_enabled(
        &settings,
        Some(&track_id),
        Some(&remote_lyrics)
    ));
    assert!(!auto_lyrics_skip_action_enabled(&settings, None, None));
    assert!(!auto_lyrics_skip_action_enabled(
        &settings,
        Some(&track_id),
        None
    ));
}
#[test]
pub(in crate::ui) fn shell_auto_lyrics() {
    let track_id = TrackId::fake(13);
    let settings = AppSettings {
        external_lyrics_enabled: true,
        ..AppSettings::default()
    };
    let server_lyrics = Lyrics {
        track_id: track_id.clone(),
        source: LyricsSource::Server,
        lines: vec![LyricLine {
            text: "server line".to_string(),
            start_millis: None,
        }],
    };
    let remote_lyrics = Lyrics {
        track_id: track_id.clone(),
        source: LyricsSource::Remote,
        lines: vec![LyricLine {
            text: "remote line".to_string(),
            start_millis: None,
        }],
    };

    assert!(!auto_lyrics_skip_action_enabled(
        &settings,
        Some(&track_id),
        Some(&server_lyrics)
    ));
    assert!(auto_lyrics_skip_action_enabled(
        &settings,
        Some(&track_id),
        Some(&remote_lyrics)
    ));
}
#[test]
pub(in crate::ui) fn shell_keep_suppressed() {
    let track_id = TrackId::fake(12);
    let mut settings = AppSettings {
        external_lyrics_enabled: true,
        ..AppSettings::default()
    };

    assert_eq!(
        auto_lyrics_request_for_settings(&settings, &track_id, true),
        Some(AutoLyricsRequest::Default)
    );

    settings
        .suppressed_auto_lyrics_track_ids
        .push(track_id.as_str().to_string());
    assert_eq!(
        auto_lyrics_request_for_settings(&settings, &track_id, true),
        Some(AutoLyricsRequest::ServerOnly)
    );

    settings.suppressed_auto_lyrics_track_ids.clear();
    settings.external_lyrics_enabled = false;
    assert_eq!(
        auto_lyrics_request_for_settings(&settings, &track_id, true),
        Some(AutoLyricsRequest::ServerOnly)
    );

    settings.external_lyrics_enabled = true;
    settings.private_mode = true;
    assert_eq!(
        auto_lyrics_request_for_settings(&settings, &track_id, true),
        Some(AutoLyricsRequest::ServerOnly)
    );

    settings.private_mode = false;
    settings.lyrics_panel_visible = false;
    assert_eq!(
        auto_lyrics_request_for_settings(&settings, &track_id, false),
        None
    );
    assert_eq!(
        auto_lyrics_request_for_settings(&settings, &track_id, true),
        Some(AutoLyricsRequest::Default)
    );
}
#[test]
pub(in crate::ui) fn shell_allow_cache() {
    let track_id = TrackId::fake(13);
    let previous_failed_track_id = TrackId::fake(14);
    let mut attempted = HashSet::from([track_id.clone(), previous_failed_track_id.clone()]);
    let lyrics = Lyrics {
        track_id: track_id.clone(),
        source: LyricsSource::Remote,
        lines: vec![LyricLine {
            text: "line one".to_string(),
            start_millis: Some(1_000),
        }],
    };

    allow_loaded_lyrics_cache_revisit(&mut attempted, Some(&lyrics));

    assert!(!attempted.contains(&track_id));
    assert!(attempted.contains(&previous_failed_track_id));
    allow_loaded_lyrics_cache_revisit(&mut attempted, None);
    assert!(attempted.contains(&previous_failed_track_id));
}
#[test]
pub(in crate::ui) fn shell_use_statuses() {
    assert_eq!(
        preferences_login_status_toast_message("Checking Jellyfin server…"),
        Some("Checking Jellyfin server…")
    );
    assert_eq!(
        preferences_login_status_toast_message("Server settings saved."),
        Some("Server settings saved.")
    );
    assert_eq!(
        preferences_login_status_toast_message("Server settings saved. Resyncing library…"),
        Some("Server settings saved. Resyncing library…")
    );
    assert_eq!(
        preferences_login_status_toast_message("No changes to save."),
        Some("No changes to save.")
    );
    assert_eq!(
        preferences_login_status_toast_message("Library sync complete"),
        None
    );
}
#[test]
pub(in crate::ui) fn shell_ignore_field() {
    assert!(lyrics_search_response_matches_query(
        "", "Opening", "", "Opening",
    ));
    assert!(lyrics_search_response_matches_query(
        "ATARASHII GAKKO",
        "Freaks",
        "atarashii gakko",
        "freaks",
    ));
    assert!(!lyrics_search_response_matches_query(
        "Earlier Artist",
        "Opening",
        "",
        "Opening",
    ));
    assert!(!lyrics_search_response_matches_query(
        "",
        "Opening Theme",
        "",
        "Opening",
    ));
    assert!(!lyrics_search_response_matches_query(
        "Earlier Artist",
        "Long Song Title",
        "",
        "Song",
    ));
}
#[test]
pub(in crate::ui) fn shell_lyrics_exist() {
    let result = LyricsSearchResult {
        id: 12,
        track_name: "Example Track".to_string(),
        artist_name: "Example Artist".to_string(),
        album_name: "Example Album".to_string(),
        duration_seconds: 95,
        synced_lyrics: Some("[00:01.00]line".to_string()),
        plain_lyrics: Some("line".to_string()),
    };

    assert_eq!(
        lyrics_result_subtitle(&result),
        "Example Album - 1:35 - Synced lyrics"
    );
}
#[test]
pub(in crate::ui) fn shell_lyrics_text() {
    let result = LyricsSearchResult {
        id: 13,
        track_name: "Poker Face (Piano & Voice Version) [Live]".to_string(),
        artist_name: "Lady Gaga".to_string(),
        album_name: "Hits & Rarities".to_string(),
        duration_seconds: 95,
        synced_lyrics: Some("[00:01.00]line".to_string()),
        plain_lyrics: None,
    };

    assert_eq!(
        lyrics_result_title_markup(&result).as_str(),
        "Lady Gaga - Poker Face (Piano &amp; Voice Version) [Live]"
    );
    assert_eq!(
        lyrics_result_subtitle_markup(&result).as_str(),
        "Hits &amp; Rarities - 1:35 - Synced lyrics"
    );
}
#[test]
pub(in crate::ui) fn portrait_cover_crop() {
    let rect = super::cover_draw_rect(100, 200, 34, 34);
    assert!((rect.scale - 0.34).abs() < f64::EPSILON);
    assert!((rect.x - 0.0).abs() < f64::EPSILON);
    assert!((rect.y + 17.0).abs() < f64::EPSILON);
}
#[test]
pub(in crate::ui) fn landscape_cover_crop() {
    let rect = super::cover_draw_rect(200, 100, 44, 44);
    assert!((rect.scale - 0.44).abs() < f64::EPSILON);
    assert!((rect.x + 22.0).abs() < f64::EPSILON);
    assert!((rect.y - 0.0).abs() < f64::EPSILON);
}
#[derive(Clone, Copy)]
struct LocalCacheGateInput<'a> {
    local_folders_changed: bool,
    has_local_folders: bool,
    has_cached_library: bool,
    startup_route_revealed: bool,
    preparing: bool,
    sync_seen: bool,
    sync_status: &'a str,
}

impl<'a> LocalCacheGateInput<'a> {
    fn cached(local_folders_changed: bool, preparing: bool, sync_status: &'a str) -> Self {
        Self::cached_inner(local_folders_changed, true, preparing, sync_status)
    }

    fn cached_startup(local_folders_changed: bool, preparing: bool, sync_status: &'a str) -> Self {
        Self::cached_inner(local_folders_changed, false, preparing, sync_status)
    }

    fn cached_inner(
        local_folders_changed: bool,
        startup_route_revealed: bool,
        preparing: bool,
        sync_status: &'a str,
    ) -> Self {
        Self {
            local_folders_changed,
            has_local_folders: true,
            has_cached_library: true,
            startup_route_revealed,
            preparing,
            sync_seen: false,
            sync_status,
        }
    }

    fn uncached(sync_seen: bool, preparing: bool, sync_status: &'a str) -> Self {
        Self {
            local_folders_changed: false,
            has_local_folders: true,
            has_cached_library: false,
            startup_route_revealed: true,
            preparing,
            sync_seen,
            sync_status,
        }
    }
}

fn local_cache_gate_action(input: LocalCacheGateInput<'_>) -> LocalSourceCacheGateAction {
    let source = Some(LibrarySourceSelection::Local);
    local_source_cache_gate_action(LocalSourceCacheGateInput {
        local_folders_changed: input.local_folders_changed,
        next_source: &source,
        has_local_folders: input.has_local_folders,
        has_cached_library: input.has_cached_library,
        startup_route_revealed: input.startup_route_revealed,
        preparing: input.preparing,
        sync_seen: input.sync_seen,
        sync_status: input.sync_status,
    })
}

pub(in crate::ui) fn test_library_snapshot() -> crate::controller::LibrarySnapshot {
    crate::controller::LibrarySnapshot {
        server: None,
        servers: Vec::new(),
        selected_source: None,
        local_folders: Vec::new(),
        server_local_access: Vec::new(),
        local_access: None,
        local_access_status: crate::controller::LocalAccessStatus::default(),
        music_folders: Vec::new(),
        selected_music_folder_id: None,
        username: None,
        first_run: false,
        sync_status: String::new(),
        last_error: None,
        cached_album_count: 0,
        cached_track_count: 0,
        cached_artist_count: 0,
        cached_album_artist_count: 0,
        cached_genre_count: 0,
        cached_playlist_count: 0,
        home_sections: Vec::new(),
        prefetched_explore: None,
        albums: Vec::new(),
        tracks: Vec::new(),
        artists: Vec::new(),
        album_artists: Vec::new(),
        genres: Vec::new(),
        playlists: Vec::new(),
        favorites: Vec::new(),
        search: SearchResults::default(),
    }
}
fn test_server(suffix: &str) -> ServerIdentity {
    ServerIdentity {
        id: ServerId::new(format!("server:{suffix}")),
        provider: "test".to_string(),
        name: format!("Server {suffix}"),
        base_url: "http://localhost".to_string(),
    }
}
fn test_initial_route_metrics() -> super::InitialRouteCoverMetrics {
    super::InitialRouteCoverMetrics {
        route_height: 720,
        app_height: 720,
        grid_columns: 4,
        grid_card_size: 160,
        home_showcase_seed: 0,
    }
}
pub(in crate::ui) fn test_image_ref(suffix: &str) -> ImageRef {
    ImageRef::new(format!("local:cover:file%3A%2F%2F{suffix}"), None)
}
fn test_playlist(name: &str, image_ref: ImageRef) -> Playlist {
    Playlist {
        id: PlaylistId::fake(1),
        name: name.to_string(),
        track_count: 1,
        duration_seconds: 180,
        image_refs: vec![image_ref],
        image_ref: None,
    }
}
fn test_smart_playlist(name: &str, image_ref: ImageRef) -> SmartPlaylist {
    SmartPlaylist {
        id: SmartPlaylistId::fake(1),
        name: name.to_string(),
        position: 0,
        builtin: None,
        definition: SmartPlaylistDefinition {
            root: SmartPlaylistRuleGroup {
                mode: SmartPlaylistMatchMode::All,
                rules: Vec::new(),
            },
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: None,
        },
        track_count: 1,
        duration_seconds: 180,
        image_refs: vec![image_ref],
        image_ref: None,
    }
}
fn test_queue_entry(title: &str, image_ref: ImageRef) -> QueueEntry {
    QueueEntry {
        id: QueueEntryId::new(format!("queue:{title}")),
        track_id: TrackId::fake(1),
        album_id: None,
        title: title.to_string(),
        artist: "Artist".to_string(),
        artist_id: None,
        album: "Album".to_string(),
        year: 2026,
        duration_seconds: 180,
        favorite: false,
        image_ref: Some(image_ref),
        local_path: None,
        source_format: None,
        origin: None,
    }
}
pub(in crate::ui) fn test_album(artist: &str, artist_id: Option<ArtistId>) -> Album {
    Album {
        id: AlbumId::fake(1),
        title: "Album".to_string(),
        artist: artist.to_string(),
        artist_id,
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        track_count: 1,
        duration_seconds: 180,
        favorite: false,
        color_seed: 1,
        image_ref: None,
        genres: Vec::new(),
    }
}
pub(in crate::ui) fn test_home_album_section(kind: HomeSectionKind, album_id: u32) -> HomeSection {
    let mut album = test_album("Album Artist", Some(ArtistId::fake(album_id)));
    album.id = AlbumId::fake(album_id);
    HomeSection {
        kind,
        albums: vec![album],
        tracks: Vec::new(),
    }
}
pub(in crate::ui) fn test_track(artist: &str, artist_id: Option<ArtistId>) -> Track {
    Track {
        id: TrackId::fake(1),
        album_id: AlbumId::fake(1),
        title: "Track".to_string(),
        artist: artist.to_string(),
        artist_id,
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: "Album".to_string(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: 180,
        favorite: false,
        disc_number: 1,
        track_number: 1,
        image_ref: None,
        genres: Vec::new(),
        local_path: None,
        source_format: None,
        comment: None,
        skip_count: None,
    }
}

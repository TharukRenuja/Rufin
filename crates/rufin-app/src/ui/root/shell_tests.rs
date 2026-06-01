use super::responsive_layout_state::startup_loading_screen_active;
use super::right_panel::{
    clamp_queue_lyrics_position, queue_lyrics_default_position, queue_lyrics_initial_position,
    queue_lyrics_position_from_ratio, queue_lyrics_position_ratio,
};
use super::startup_reveal::{
    StartupRevealAction, first_run_cover_prime_reveal_action, startup_loading_status_label,
    startup_route_reveal_action,
};
use super::{
    AutoLyricsRequest, GRID_COVER_SIZE, LocalSourceCacheGateAction, PLAYLIST_ENTRY_SORTS,
    PlaylistEntryListState, PlaylistEntrySort, SnapshotRenderDecision,
    auto_lyrics_request_for_settings, auto_lyrics_skip_action_enabled,
    cover::record_cover_path_lookup_request, current_playback_track_id,
    home_visible_sections::changed_visible_home_section_kinds, local_source_cache_gate_action,
    local_source_snapshot_is_syncing, lyrics_result_subtitle, lyrics_search_response_matches_query,
    playlist_detail_compact_for_width, playlist_detail_cover_decode_size_for_width,
    playlist_detail_cover_fetch_size, playlist_detail_cover_size_for_width,
    playlist_detail_header_orientation_for_width, playlist_detail_route_margin_for_width,
    playlist_detail_sort_width_for_width, playlist_detail_toolbar_orientation_for_width,
    playlist_drop_index, playlist_entries_for_state, playlist_entry_play_count_text,
    playlist_tracks_starting_at, preferences_login_status_toast_message,
    queue_source_waits_for_snapshot, seekbar_target_seconds, snapshot_event_outcome,
    snapshot_local_source_cache_gate_action,
};
use crate::controller::{LyricsSearchResult, PlaybackPerfEvent};
use gdk_pixbuf::{Colorspace, Pixbuf};
use rufin_core::{
    Album, AlbumId, AppSettings, ArtistId, HomeBlockKind, HomeSection, HomeSectionKind, ImageRef,
    LibraryLayout, LibrarySourceSelection, PlaylistId, QueueEntry, QueueEntryId, QueueSnapshot,
    RepeatMode, Route, SearchKind, ServerId, Track, TrackId, TrackSortKey, TrackTableSettings,
};
use rufin_provider::{LyricLine, Lyrics, LyricsSource, PlaylistEntry, SearchResults};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[test]
fn close_request_hides_only_when_exit_to_tray_is_enabled_and_available() {
    let mut settings = AppSettings::default();

    assert!(!super::tray::should_hide_on_close_for_exit_to_tray(
        &settings, true
    ));

    settings.exit_to_tray = true;

    assert!(!super::tray::should_hide_on_close_for_exit_to_tray(
        &settings, true
    ));

    settings.tray_enabled = true;

    assert!(super::tray::should_hide_on_close_for_exit_to_tray(
        &settings, true
    ));
    assert!(!super::tray::should_hide_on_close_for_exit_to_tray(
        &settings, false
    ));
}

#[test]
fn startup_hides_only_when_start_minimized_is_enabled_and_tray_is_available() {
    let mut settings = AppSettings::default();

    assert!(!super::tray::should_start_minimized(&settings, true));

    settings.start_minimized = true;

    assert!(!super::tray::should_start_minimized(&settings, true));

    settings.tray_enabled = true;

    assert!(super::tray::should_start_minimized(&settings, true));
    assert!(!super::tray::should_start_minimized(&settings, false));
}

fn ui_perf_test_options(strict_contracts: bool) -> super::UiPerfOptions {
    super::UiPerfOptions {
        max_gap_ms: 120,
        route_ms: 650,
        route_ready_ms: 250,
        drag_ms: 900,
        duration_ms: 15_000,
        asset_ms: 300,
        require_assets: false,
        terminal_events: false,
        observe_scroll: false,
        strict_contracts,
        launch_started_at: Instant::now(),
        output: None,
    }
}

fn route_visible_contract(phase: &'static str, route: &str) -> super::UiPerfRouteVisibleContract {
    super::UiPerfRouteVisibleContract {
        phase,
        route: route.to_string(),
        layout: "row",
        visible_start: 0,
        visible_end: 12,
        expected_visible: 12,
        ready: 12,
        final_missing: 0,
        pending: 0,
        rendered_expected: 12,
        rendered_ready: 12,
        rendered_final_missing: 0,
        rendered_fallback: 0,
        fallback_after_reveal: 0,
        pending_assets: 0,
        active_decodes: 0,
        queued_decodes: 0,
        path_lookups: 0,
        pending_samples: Vec::new(),
    }
}

#[test]
pub(in crate::ui) fn detail_cover_lookup_can_reuse_prefetched_grid_cover() {
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
pub(in crate::ui) fn playback_artwork_path_uses_prefetched_grid_cover_for_thumbnail() {
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
pub(in crate::ui) fn playback_artwork_key_match_accepts_candidate_cover_sizes() {
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
pub(in crate::ui) fn playback_notification_icon_bytes_are_square_for_portals() {
    let cover = Pixbuf::new(Colorspace::Rgb, false, 8, 320, 180).expect("cover pixbuf");
    cover.fill(0x336699ff);

    let bytes =
        super::playback_notification_icon_bytes_from_pixbuf(&cover).expect("notification bytes");
    let icon = Pixbuf::from_read(Cursor::new(bytes)).expect("notification pixbuf");

    assert_eq!(icon.width(), super::THUMB_COVER_SIZE as i32);
    assert_eq!(icon.height(), super::THUMB_COVER_SIZE as i32);
}

#[test]
pub(in crate::ui) fn visible_cover_lookup_reuses_and_upgrades_warm_lookup() {
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
pub(in crate::ui) fn home_section_pages_reset_for_new_home_data() {
    let mut states = HashMap::from([(
        HomeSectionKind::Explore,
        super::HomeSectionState {
            page_start: 6,
            page_size: 3,
        },
    )]);

    super::reset_home_section_pages(&mut states);

    assert!(states.is_empty());
}
#[test]
pub(in crate::ui) fn home_refresh_targets_only_changed_visible_sections() {
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
pub(in crate::ui) fn snapshot_event_outcome_prioritizes_first_run_completion() {
    let previous_source = None;
    let next_source = Some(LibrarySourceSelection::Local);

    let outcome = snapshot_event_outcome(true, false, &previous_source, &next_source, true, true);

    assert_eq!(outcome.render, SnapshotRenderDecision::FirstRunFinished);
    assert!(!outcome.entered_first_run);
}
#[test]
pub(in crate::ui) fn snapshot_event_outcome_navigates_when_source_changes() {
    let previous_source = None;
    let next_source = Some(LibrarySourceSelection::Local);

    let outcome =
        snapshot_event_outcome(false, false, &previous_source, &next_source, false, false);

    assert_eq!(outcome.render, SnapshotRenderDecision::SourceChanged);
    assert!(!outcome.entered_first_run);
}
#[test]
pub(in crate::ui) fn snapshot_event_outcome_preserves_scroll_for_stable_source() {
    let source = Some(LibrarySourceSelection::Local);

    let outcome = snapshot_event_outcome(false, false, &source, &source, false, false);

    assert_eq!(outcome.render, SnapshotRenderDecision::PreserveScroll);
    assert!(!outcome.entered_first_run);
}
#[test]
pub(in crate::ui) fn snapshot_event_outcome_marks_first_run_entry() {
    let source = None::<LibrarySourceSelection>;

    let outcome = snapshot_event_outcome(false, true, &source, &source, false, false);

    assert_eq!(outcome.render, SnapshotRenderDecision::PreserveScroll);
    assert!(outcome.entered_first_run);
}
#[test]
pub(in crate::ui) fn local_source_cache_gate_ignores_cached_source_change() {
    let source = Some(LibrarySourceSelection::Local);

    assert_eq!(
        local_source_cache_gate_action(false, &source, true, false, false, "Cached library ready"),
        LocalSourceCacheGateAction::None
    );
    assert_eq!(
        local_source_cache_gate_action(true, &source, false, false, false, "Cached library ready"),
        LocalSourceCacheGateAction::None
    );
    assert_eq!(
        snapshot_local_source_cache_gate_action(
            SnapshotRenderDecision::SourceChanged,
            false,
            &source,
            true,
            false,
            false,
            "Cached library ready",
        ),
        LocalSourceCacheGateAction::None
    );
}
#[test]
pub(in crate::ui) fn local_source_cache_gate_enters_for_folder_change() {
    let source = Some(LibrarySourceSelection::Local);

    assert_eq!(
        local_source_cache_gate_action(false, &source, true, false, false, "Cached library ready"),
        LocalSourceCacheGateAction::None
    );
    assert_eq!(
        local_source_cache_gate_action(true, &source, true, false, false, "Cached library ready"),
        LocalSourceCacheGateAction::Enter
    );
}
#[test]
pub(in crate::ui) fn local_source_cache_gate_enters_for_same_source_local_sync() {
    let source = Some(LibrarySourceSelection::Local);

    assert_eq!(
        local_source_cache_gate_action(false, &source, true, false, false, "Syncing library..."),
        LocalSourceCacheGateAction::Enter
    );
    assert_eq!(
        local_source_cache_gate_action(false, &source, true, false, false, "Cached library ready"),
        LocalSourceCacheGateAction::None
    );
}
#[test]
pub(in crate::ui) fn local_source_cache_gate_waits_until_sync_snapshot_finishes() {
    let source = Some(LibrarySourceSelection::Local);

    assert_eq!(
        local_source_cache_gate_action(false, &source, true, true, false, "Cached library ready"),
        LocalSourceCacheGateAction::Wait
    );
    assert!(local_source_snapshot_is_syncing("Syncing library..."));
    assert_eq!(
        local_source_cache_gate_action(false, &source, true, true, true, "Syncing library..."),
        LocalSourceCacheGateAction::Wait
    );
    assert_eq!(
        local_source_cache_gate_action(false, &source, true, true, true, "Cached library ready"),
        LocalSourceCacheGateAction::Reveal
    );
}
#[test]
pub(in crate::ui) fn local_source_cache_gate_cancels_when_source_leaves_local() {
    let source = Some(LibrarySourceSelection::Server(rufin_core::ServerId::new(
        "jellyfin:server:test",
    )));

    assert_eq!(
        local_source_cache_gate_action(false, &source, true, true, true, "Cached library ready"),
        LocalSourceCacheGateAction::Cancel
    );
}
#[test]
pub(in crate::ui) fn queue_source_waits_until_library_snapshot_matches() {
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
pub(in crate::ui) fn first_run_completion_suppresses_local_source_cache_gate() {
    let source = Some(LibrarySourceSelection::Local);

    assert_eq!(
        snapshot_local_source_cache_gate_action(
            SnapshotRenderDecision::FirstRunFinished,
            false,
            &source,
            true,
            true,
            true,
            "Cached library ready",
        ),
        LocalSourceCacheGateAction::None
    );
}
#[test]
pub(in crate::ui) fn startup_loading_uses_root_stack_until_route_reveal() {
    assert!(startup_loading_screen_active(false, false));
    assert!(!startup_loading_screen_active(true, false));
    assert!(!startup_loading_screen_active(false, true));
}
#[test]
pub(in crate::ui) fn startup_route_reveal_expires_pending_cover_prime() {
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
}
#[test]
pub(in crate::ui) fn first_run_cover_prime_expires_pending_cover_prime() {
    assert_eq!(
        first_run_cover_prime_reveal_action(
            3,
            Duration::from_millis(super::FIRST_RUN_COVER_PRIME_TIMEOUT_MS)
        ),
        StartupRevealAction::RevealExpired
    );
}
#[test]
pub(in crate::ui) fn startup_loading_status_hides_idle_cache_status() {
    assert_eq!(startup_loading_status_label(""), None);
    assert_eq!(startup_loading_status_label("Cached library ready"), None);
    assert_eq!(
        startup_loading_status_label("Syncing Local library..."),
        Some("Syncing Local library...".to_string())
    );
}
#[test]
pub(in crate::ui) fn startup_prime_targets_stay_to_home_visible_covers() {
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
    let targets = super::startup_cover_prime_targets_from_snapshot(&library, &settings, 0);
    let target_refs = targets
        .iter()
        .map(|target| target.image_ref.item_id.as_str())
        .collect::<Vec<_>>();

    assert!(target_refs.contains(&home_ref.item_id.as_str()));
    assert!(!target_refs.contains(&first_track_ref.item_id.as_str()));
    assert!(!target_refs.contains(&first_album_ref.item_id.as_str()));

    let home_targets =
        super::startup_home_cover_prime_targets_from_snapshot(&library, &settings, 0);
    let home_target_refs = home_targets
        .iter()
        .map(|target| target.image_ref.item_id.as_str())
        .collect::<Vec<_>>();
    assert!(home_target_refs.contains(&home_ref.item_id.as_str()));
    assert!(!home_target_refs.contains(&first_track_ref.item_id.as_str()));
    assert!(!home_target_refs.contains(&first_album_ref.item_id.as_str()));
}

#[test]
pub(in crate::ui) fn library_route_prime_targets_include_full_visible_image_routes() {
    let mut library = test_library_snapshot();
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
    let targets = super::library_route_cover_prime_targets_from_snapshot(&library, &settings);
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
pub(in crate::ui) fn cover_group_slots_repeat_ordered_refs_without_unique_collage_rules() {
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
pub(in crate::ui) fn manual_ui_perf_observer_records_scrolls_by_route() {
    let mut options = ui_perf_test_options(false);
    options.observe_scroll = true;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_manual_scroll_step("Tracks", 10.0, 100.0);
    monitor.record_manual_scroll_step("Tracks", 40.0, 100.0);
    monitor.record_manual_scroll_step("Albums", 5.0, 50.0);
    monitor.finish_scroll();

    let report = monitor.report();
    assert!(report.contains("RUFIN_PERF_SCROLL route=Tracks scenario=manual"));
    assert!(report.contains("steps=2"));
    assert!(report.contains("max_adjustment=100"));
    assert!(report.contains("RUFIN_PERF_SCROLL route=Albums scenario=manual"));
}
#[test]
pub(in crate::ui) fn ui_perf_report_records_playback_startup_phases() {
    let mut options = ui_perf_test_options(false);
    options.observe_scroll = true;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_playback_event(&PlaybackPerfEvent {
        phase: "playing",
        server_id: ServerId::fake(1),
        track_id: TrackId::fake(7),
        elapsed_ms: 1_842,
    });

    let report = monitor.report();
    assert!(report.contains(
        "RUFIN_PERF_PLAYBACK phase=playing server_id=server-1 track_id=track-7 elapsed_ms=1842"
    ));
}
#[test]
pub(in crate::ui) fn ui_perf_report_hashes_asset_labels() {
    let monitor = super::UiPerfMonitor::new(ui_perf_test_options(false));

    monitor.record_cover_bind_request("/private/library/album/track.flac");
    monitor.record_cover_path_ready("/private/library/album/track.flac");
    monitor.record_cover_decode_start("/private/library/album/track.flac");
    monitor.record_cover_decode_ok("/private/library/album/track.flac");
    monitor.record_cover_bind_request("/private/library/album/pending.flac");

    let report = monitor.report();
    assert!(!report.contains("/private/library"));
    assert!(report.contains("RUFIN_PERF_ASSET key_hash="));
    assert!(report.contains("RUFIN_PERF_PENDING_ASSET key_hash="));
}
#[test]
pub(in crate::ui) fn ui_perf_plan_keeps_home_out_of_the_critical_window() {
    let plan = super::ui_perf_take_plan(
        vec![
            (Route::Tracks, super::UiPerfScenario::HumanScroll),
            (Route::Tracks, super::UiPerfScenario::FastScroll),
            (Route::Albums, super::UiPerfScenario::HumanScroll),
        ],
        vec![Route::Artists, Route::Home],
        2_000,
        500,
    );

    let routes = plan
        .iter()
        .map(|(route, _)| route.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        routes,
        vec![Route::Tracks, Route::Tracks, Route::Albums, Route::Artists]
    );
}
#[test]
pub(in crate::ui) fn ui_perf_route_render_budget_is_not_the_scroll_gap_budget() {
    let mut options = ui_perf_test_options(false);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_route_render("Albums".to_string(), std::time::Duration::from_millis(300));
    monitor.record_cover_cache_hit("cached-cover");

    assert!(!monitor.failed());
}
#[test]
pub(in crate::ui) fn ui_perf_route_visible_contract_fails_pending_route_ready_work() {
    let mut options = ui_perf_test_options(false);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_route_visible_contract(super::UiPerfRouteVisibleContract {
        phase: "route_ready",
        route: "Albums".to_string(),
        layout: "grid",
        visible_start: 0,
        visible_end: 6,
        expected_visible: 6,
        ready: 5,
        final_missing: 0,
        pending: 1,
        rendered_expected: 6,
        rendered_ready: 5,
        rendered_final_missing: 0,
        rendered_fallback: 1,
        fallback_after_reveal: 1,
        pending_assets: 1,
        active_decodes: 0,
        queued_decodes: 0,
        path_lookups: 0,
        pending_samples: Vec::new(),
    });
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
}
#[test]
pub(in crate::ui) fn ui_perf_route_visible_contract_fails_strict_drag_mid_pending_on_image_route() {
    let mut options = ui_perf_test_options(true);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_route_visible_contract(super::UiPerfRouteVisibleContract {
        phase: "drag_mid",
        route: "Tracks".to_string(),
        layout: "row",
        visible_start: 100,
        visible_end: 113,
        expected_visible: 13,
        ready: 8,
        final_missing: 0,
        pending: 5,
        rendered_expected: 13,
        rendered_ready: 8,
        rendered_final_missing: 0,
        rendered_fallback: 5,
        fallback_after_reveal: 5,
        pending_assets: 5,
        active_decodes: 5,
        queued_decodes: 0,
        path_lookups: 0,
        pending_samples: Vec::new(),
    });
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
}
#[test]
pub(in crate::ui) fn ui_perf_route_visible_contract_rejects_background_work_when_ready() {
    let mut options = ui_perf_test_options(false);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_route_visible_contract(super::UiPerfRouteVisibleContract {
        phase: "route_ready",
        route: "Albums".to_string(),
        layout: "grid",
        visible_start: 0,
        visible_end: 6,
        expected_visible: 6,
        ready: 6,
        final_missing: 0,
        pending: 0,
        rendered_expected: 6,
        rendered_ready: 6,
        rendered_final_missing: 0,
        rendered_fallback: 0,
        fallback_after_reveal: 0,
        pending_assets: 42,
        active_decodes: 2,
        queued_decodes: 7,
        path_lookups: 9,
        pending_samples: Vec::new(),
    });
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
}
#[test]
pub(in crate::ui) fn ui_perf_route_visible_contract_fails_unaccounted_visible_artwork() {
    let mut options = ui_perf_test_options(false);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_route_visible_contract(super::UiPerfRouteVisibleContract {
        phase: "route_ready",
        route: "Home".to_string(),
        layout: "home",
        visible_start: 0,
        visible_end: 6,
        expected_visible: 6,
        ready: 4,
        final_missing: 0,
        pending: 0,
        rendered_expected: 4,
        rendered_ready: 4,
        rendered_final_missing: 0,
        rendered_fallback: 0,
        fallback_after_reveal: 0,
        pending_assets: 0,
        active_decodes: 0,
        queued_decodes: 0,
        path_lookups: 0,
        pending_samples: Vec::new(),
    });
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
}
#[test]
pub(in crate::ui) fn ui_perf_route_visible_contract_fails_rendered_expected_fallback_tiles() {
    let mut options = ui_perf_test_options(false);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);
    let mut contract = route_visible_contract("route_ready", "Home");
    contract.rendered_ready = 11;
    contract.rendered_fallback = 1;

    monitor.record_route_visible_contract(contract);
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
    assert!(monitor.report().contains("rendered_fallback=1"));
}
#[test]
pub(in crate::ui) fn ui_perf_route_visible_contract_fails_unaccounted_rendered_tiles() {
    let mut options = ui_perf_test_options(false);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);
    let mut contract = route_visible_contract("route_ready", "Home");
    contract.rendered_expected = 12;
    contract.rendered_ready = 10;
    contract.rendered_final_missing = 1;
    contract.rendered_fallback = 0;

    monitor.record_route_visible_contract(contract);
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
    assert!(monitor.report().contains("rendered_expected=12"));
}
#[test]
pub(in crate::ui) fn ui_perf_route_visible_contract_fails_when_expected_tiles_have_not_rendered() {
    let mut options = ui_perf_test_options(false);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);
    let mut contract = route_visible_contract("route_ready", "SmartPlaylists");
    contract.expected_visible = 4;
    contract.ready = 4;
    contract.rendered_expected = 0;
    contract.rendered_ready = 0;

    monitor.record_route_visible_contract(contract);
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
    assert!(monitor.report().contains("rendered_expected=0"));
}

#[test]
pub(in crate::ui) fn ui_perf_route_visible_contract_accepts_rendered_final_missing_tiles() {
    let mut options = ui_perf_test_options(false);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);
    let mut contract = route_visible_contract("route_ready", "Genres");
    contract.expected_visible = 4;
    contract.ready = 0;
    contract.final_missing = 4;
    contract.rendered_expected = 4;
    contract.rendered_ready = 0;
    contract.rendered_final_missing = 4;

    monitor.record_route_visible_contract(contract);
    monitor.record_cover_cache_hit("cached-cover");

    assert!(!monitor.failed());
    assert!(monitor.report().contains("rendered_final_missing=4"));
}
#[test]
pub(in crate::ui) fn ui_perf_strict_full_launch_budget_starts_at_process_entry() {
    let mut options = ui_perf_test_options(true);
    options.launch_started_at = Instant::now() - Duration::from_millis(3_100);
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_startup_reveal();
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
    assert!(monitor.report().contains("RUFIN_ACCEPT_STARTUP"));
}
#[test]
pub(in crate::ui) fn ui_perf_strict_route_ready_budget_fails_slow_ready() {
    let monitor = super::UiPerfMonitor::new(ui_perf_test_options(true));

    monitor.record_route_ready(
        "Albums".to_string(),
        Duration::from_millis(251),
        Duration::ZERO,
    );
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
    assert!(
        monitor
            .report()
            .contains("RUFIN_ACCEPT_ROUTE_READY route=Albums")
    );
}
#[test]
pub(in crate::ui) fn ui_perf_strict_idle_gap_uses_frame_gap_budget() {
    let mut options = ui_perf_test_options(true);
    options.max_gap_ms = 120;
    options.route_ready_ms = 250;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_tick_gap(Duration::from_millis(180));
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
    assert!(monitor.report().contains("max_idle_gap_ms=180"));
    assert!(monitor.report().contains("over_budget_idle_ticks=1"));
}
#[test]
pub(in crate::ui) fn ui_perf_strict_idle_gap_fails_when_route_transition_budget_is_missed() {
    let mut options = ui_perf_test_options(true);
    options.asset_ms = 120;
    options.route_ready_ms = 250;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_tick_gap(Duration::from_millis(251));
    monitor.record_cover_cache_hit("cached-cover");

    assert!(monitor.failed());
}
#[test]
pub(in crate::ui) fn ui_perf_initial_tracks_row_sample_is_diagnostic() {
    let mut options = ui_perf_test_options(true);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_cover_cache_hit("cached-cover");
    monitor.record_tracks_row_contract(super::UiPerfTrackRowContract {
        scenario: "initial",
        visible_start: 0,
        visible_end: 12,
        ready: 0,
        coverless: 0,
        pending: 12,
        missing: 0,
    });

    assert!(!monitor.failed());
}
#[test]
pub(in crate::ui) fn ui_perf_image_route_drag_requires_all_checkpoint_samples() {
    let monitor = super::UiPerfMonitor::new(ui_perf_test_options(true));
    monitor.record_cover_cache_hit("cached-cover");
    monitor
        .inner
        .borrow_mut()
        .route_scrolls
        .push(super::UiPerfRouteScroll {
            route: "Tracks".to_string(),
            scenario: "drag_sweep",
            elapsed_ms: 900,
            steps: 24,
            max_gap_ms: 40,
            over_budget_ticks: 0,
            max_adjustment: 1_000.0,
            min_value: 0.0,
            max_value: 1_000.0,
            covers_ready: 0,
            decoded_covers: 0,
        });
    monitor.record_route_visible_contract(route_visible_contract("route_ready", "Tracks"));
    monitor.record_route_visible_contract(route_visible_contract("drag_done", "Tracks"));

    assert!(monitor.failed());

    for phase in ["ready_before_drag", "drag_25", "drag_50", "drag_75"] {
        monitor.record_route_visible_contract(route_visible_contract(phase, "Tracks"));
    }

    assert!(!monitor.failed());
    assert!(
        monitor
            .report()
            .contains("RUFIN_ACCEPT_IMAGE_ROUTE_DRAG_SUMMARY routes=1 failures=0")
    );
}

#[test]
pub(in crate::ui) fn ui_perf_image_route_drag_requires_clean_checkpoint_sample() {
    let monitor = super::UiPerfMonitor::new(ui_perf_test_options(true));
    monitor.record_cover_cache_hit("cached-cover");
    monitor
        .inner
        .borrow_mut()
        .route_scrolls
        .push(super::UiPerfRouteScroll {
            route: "Tracks".to_string(),
            scenario: "drag_sweep",
            elapsed_ms: 900,
            steps: 24,
            max_gap_ms: 40,
            over_budget_ticks: 0,
            max_adjustment: 1_000.0,
            min_value: 0.0,
            max_value: 1_000.0,
            covers_ready: 0,
            decoded_covers: 0,
        });

    monitor.record_route_visible_contract(route_visible_contract("route_ready", "Tracks"));
    monitor.record_route_visible_contract(route_visible_contract("ready_before_drag", "Tracks"));
    monitor.record_route_visible_contract(route_visible_contract("drag_25", "Tracks"));
    monitor.record_route_visible_contract(route_visible_contract("drag_75", "Tracks"));
    monitor.record_route_visible_contract(route_visible_contract("drag_done", "Tracks"));

    let mut pending_checkpoint = route_visible_contract("drag_50", "Tracks");
    pending_checkpoint.pending = 1;
    pending_checkpoint.fallback_after_reveal = 1;
    pending_checkpoint.pending_assets = 1;
    monitor.record_route_visible_contract(pending_checkpoint);

    assert!(monitor.failed());
    assert!(
        monitor
            .report()
            .contains("RUFIN_ACCEPT_IMAGE_ROUTE_DRAG_SUMMARY routes=1 failures=1")
    );
}

#[test]
pub(in crate::ui) fn ui_perf_image_route_drag_allows_background_decode_when_visible_samples_clean()
{
    let monitor = super::UiPerfMonitor::new(ui_perf_test_options(true));
    monitor.record_cover_cache_hit("cached-cover");
    monitor
        .inner
        .borrow_mut()
        .route_scrolls
        .push(super::UiPerfRouteScroll {
            route: "Tracks".to_string(),
            scenario: "drag_sweep",
            elapsed_ms: 900,
            steps: 24,
            max_gap_ms: 40,
            over_budget_ticks: 0,
            max_adjustment: 1_000.0,
            min_value: 0.0,
            max_value: 1_000.0,
            covers_ready: 1,
            decoded_covers: 1,
        });

    monitor.record_route_visible_contract(route_visible_contract("route_ready", "Tracks"));
    for phase in [
        "ready_before_drag",
        "drag_25",
        "drag_50",
        "drag_75",
        "drag_done",
    ] {
        monitor.record_route_visible_contract(route_visible_contract(phase, "Tracks"));
    }

    assert!(!monitor.failed());
    assert!(
        monitor
            .report()
            .contains("RUFIN_ACCEPT_IMAGE_ROUTE_DRAG_SUMMARY routes=1 failures=0")
    );
}

#[test]
pub(in crate::ui) fn ui_perf_image_route_drag_counts_favorites() {
    let monitor = super::UiPerfMonitor::new(ui_perf_test_options(true));
    monitor.record_cover_cache_hit("cached-cover");
    monitor
        .inner
        .borrow_mut()
        .route_scrolls
        .push(super::UiPerfRouteScroll {
            route: "Favorites".to_string(),
            scenario: "drag_sweep",
            elapsed_ms: 900,
            steps: 24,
            max_gap_ms: 40,
            over_budget_ticks: 0,
            max_adjustment: 1_000.0,
            min_value: 0.0,
            max_value: 1_000.0,
            covers_ready: 0,
            decoded_covers: 0,
        });

    monitor.record_route_visible_contract(route_visible_contract("route_ready", "Favorites"));
    let mut pending_done = route_visible_contract("drag_done", "Favorites");
    pending_done.pending = 1;
    for phase in ["ready_before_drag", "drag_25", "drag_50", "drag_75"] {
        monitor.record_route_visible_contract(route_visible_contract(phase, "Favorites"));
    }
    monitor.record_route_visible_contract(pending_done);

    assert!(monitor.failed());
    assert!(
        monitor
            .report()
            .contains("RUFIN_ACCEPT_IMAGE_ROUTE_DRAG_SUMMARY routes=1 failures=1")
    );
}

#[test]
pub(in crate::ui) fn ui_perf_image_routes_include_collection_cover_pages() {
    assert!(super::ui_perf_image_route("Home"));
    assert!(super::ui_perf_image_route("Playlists"));
    assert!(super::ui_perf_image_route("SmartPlaylists"));
}

#[test]
pub(in crate::ui) fn ui_perf_image_route_drag_rejects_in_flight_cover_work() {
    let monitor = super::UiPerfMonitor::new(ui_perf_test_options(true));
    monitor.record_cover_cache_hit("cached-cover");
    monitor
        .inner
        .borrow_mut()
        .route_scrolls
        .push(super::UiPerfRouteScroll {
            route: "Tracks".to_string(),
            scenario: "drag_sweep",
            elapsed_ms: 900,
            steps: 24,
            max_gap_ms: 40,
            over_budget_ticks: 0,
            max_adjustment: 1_000.0,
            min_value: 0.0,
            max_value: 1_000.0,
            covers_ready: 0,
            decoded_covers: 0,
        });

    monitor.record_route_visible_contract(route_visible_contract("route_ready", "Tracks"));
    monitor.record_route_visible_contract(route_visible_contract("ready_before_drag", "Tracks"));
    monitor.record_route_visible_contract(route_visible_contract("drag_25", "Tracks"));
    let mut in_flight_checkpoint = route_visible_contract("drag_50", "Tracks");
    in_flight_checkpoint.pending_assets = 1;
    in_flight_checkpoint.active_decodes = 1;
    in_flight_checkpoint.queued_decodes = 1;
    in_flight_checkpoint.path_lookups = 1;
    monitor.record_route_visible_contract(in_flight_checkpoint);
    monitor.record_route_visible_contract(route_visible_contract("drag_75", "Tracks"));
    monitor.record_route_visible_contract(route_visible_contract("drag_done", "Tracks"));

    assert!(monitor.failed());
    assert!(
        monitor
            .report()
            .contains("RUFIN_ACCEPT_IMAGE_ROUTE_DRAG_SUMMARY routes=1 failures=1")
    );
}
#[test]
pub(in crate::ui) fn ui_perf_visible_range_clamps_exact_bottom_to_last_row_window() {
    let (visible_start, visible_end) = super::ui_perf_visible_index_range_from_metrics(
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
pub(in crate::ui) fn ui_perf_initial_visible_count_uses_viewport_geometry() {
    assert_eq!(
        super::ui_perf_initial_visible_count_from_metrics(LibraryLayout::Row, 900, 720, 4, 160,),
        17
    );
    assert_eq!(
        super::ui_perf_initial_visible_count_from_metrics(LibraryLayout::Grid, 900, 720, 4, 160,),
        20
    );
}
#[test]
pub(in crate::ui) fn ui_perf_route_probe_waits_for_real_scroll_geometry() {
    assert!(super::ui_perf_route_probe_waits_for_scroll_geometry(
        true,
        1,
        0.0,
        Duration::from_millis(100),
    ));
    assert!(super::ui_perf_route_probe_waits_for_scroll_geometry(
        true,
        12,
        0.0,
        Duration::from_millis(100),
    ));
    assert!(!super::ui_perf_route_probe_waits_for_scroll_geometry(
        true,
        12,
        500.0,
        Duration::from_millis(100),
    ));
    assert!(!super::ui_perf_route_probe_waits_for_scroll_geometry(
        false,
        1,
        0.0,
        Duration::from_millis(100),
    ));
}
#[test]
pub(in crate::ui) fn ui_perf_route_ready_wait_ignores_pending_scroll_geometry() {
    assert!(!super::ui_perf_route_probe_waits_for_route_ready(
        true,
        12,
        0.0,
        Duration::from_millis(100),
        false,
    ));
    assert!(super::ui_perf_route_probe_waits_for_route_ready(
        true,
        12,
        500.0,
        Duration::from_millis(100),
        true,
    ));
}
#[test]
pub(in crate::ui) fn post_route_visible_warm_targets_tracks_after_home() {
    let targets = super::post_route_visible_warm_targets(&Route::Home);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].route, Route::Tracks);
    assert_eq!(targets[0].leading_rows, super::TRACK_ROUTE_PAGE_SIZE);
    assert!(super::post_route_visible_warm_targets(&Route::Tracks).is_empty());
}
#[test]
pub(in crate::ui) fn route_probe_reuses_already_rendered_home_route() {
    assert!(!super::ui_perf_route_probe_should_rerender_current_route(
        &Route::Home
    ));
    assert!(super::ui_perf_route_probe_should_rerender_current_route(
        &Route::Tracks
    ));
}
#[test]
pub(in crate::ui) fn ui_perf_route_rendered_wait_tracks_rendered_work_separately() {
    assert!(super::ui_perf_route_probe_waits_for_rendered_ready(
        true,
        12,
        0.0,
        Duration::from_millis(100),
        true,
    ));
    assert!(!super::ui_perf_route_probe_waits_for_rendered_ready(
        true,
        12,
        0.0,
        Duration::from_millis(100),
        false,
    ));
}
#[test]
pub(in crate::ui) fn ui_perf_route_ready_probe_polls_at_frame_sized_interval() {
    const { assert!(super::UI_PERF_ROUTE_READY_POLL_MS <= 16) };
}
#[test]
pub(in crate::ui) fn ui_perf_route_probe_mid_drag_sample_uses_short_settle() {
    assert_eq!(super::ui_perf_route_probe_mid_drag_sample_delay_ms(900), 64);
    assert_eq!(super::ui_perf_route_probe_mid_drag_sample_delay_ms(120), 30);
    assert_eq!(super::ui_perf_route_probe_mid_drag_sample_delay_ms(1), 1);
}
#[test]
pub(in crate::ui) fn ui_perf_route_probe_drag_checkpoints_use_spec_phases() {
    assert_eq!(
        super::ui_perf_route_probe_drag_checkpoint_phase(0.24, 0),
        None
    );
    assert_eq!(
        super::ui_perf_route_probe_drag_checkpoint_phase(0.25, 0),
        Some(("drag_25", 1, 0.25))
    );
    assert_eq!(
        super::ui_perf_route_probe_drag_checkpoint_phase(0.51, 1),
        Some(("drag_50", 2, 0.50))
    );
    assert_eq!(
        super::ui_perf_route_probe_drag_checkpoint_phase(0.99, 2),
        Some(("drag_75", 3, 0.75))
    );
}
#[test]
pub(in crate::ui) fn ui_perf_visible_range_clamps_exact_bottom_to_last_grid_window() {
    let (visible_start, visible_end) = super::ui_perf_visible_index_range_from_metrics(
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
pub(in crate::ui) fn ui_perf_strict_scroll_failure_has_no_grace_tick() {
    let mut options = ui_perf_test_options(true);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);

    monitor.record_cover_cache_hit("cached-cover");
    monitor
        .inner
        .borrow_mut()
        .route_scrolls
        .push(super::UiPerfRouteScroll {
            route: "Tracks".to_string(),
            scenario: "drag_sweep",
            elapsed_ms: 500,
            steps: 12,
            max_gap_ms: 126,
            over_budget_ticks: 1,
            max_adjustment: 1_000.0,
            min_value: 0.0,
            max_value: 50.0,
            covers_ready: 0,
            decoded_covers: 0,
        });
    assert!(monitor.failed());
}
#[test]
pub(in crate::ui) fn ui_perf_scroll_failure_ignores_nearly_static_routes() {
    let mut options = ui_perf_test_options(false);
    options.require_assets = true;
    let monitor = super::UiPerfMonitor::new(options);
    let tiny_scroll = super::UiPerfRouteScroll {
        route: "Playlists".to_string(),
        scenario: "human_scroll",
        elapsed_ms: 800,
        steps: 0,
        max_gap_ms: 650,
        over_budget_ticks: 3,
        max_adjustment: 97.0,
        min_value: 0.0,
        max_value: 97.0,
        covers_ready: 0,
        decoded_covers: 0,
    };
    assert!(!monitor.scroll_sample_failed(&tiny_scroll));
    let meaningful_scroll = super::UiPerfRouteScroll {
        max_adjustment: 1_000.0,
        ..tiny_scroll
    };
    assert!(monitor.scroll_sample_failed(&meaningful_scroll));
}
#[test]
pub(in crate::ui) fn queue_lyrics_position_clamps_to_available_height() {
    assert_eq!(clamp_queue_lyrics_position(800, 1701), 500);
    assert_eq!(clamp_queue_lyrics_position(800, 10), 120);
    assert_eq!(clamp_queue_lyrics_position(200, 1701), 120);
    assert_eq!(queue_lyrics_default_position(700), 400);
    assert_eq!(queue_lyrics_default_position(1400), 1000);
    assert_eq!(queue_lyrics_initial_position(700, None), 400);
    assert_eq!(queue_lyrics_initial_position(700, Some(0.5)), 350);
    assert_eq!(queue_lyrics_initial_position(700, Some(2.0)), 400);
    assert_eq!(queue_lyrics_initial_position(700, Some(f64::NAN)), 400);
    assert_eq!(queue_lyrics_position_from_ratio(700, 0.5), 350);
    assert_eq!(queue_lyrics_position_ratio(700, 350), 0.5);
    let saved_default_ratio = queue_lyrics_position_ratio(700, 400);
    assert_eq!(
        queue_lyrics_initial_position(1400, Some(saved_default_ratio)),
        800
    );
}
#[test]
pub(in crate::ui) fn current_playback_track_id_uses_restored_current_entry() {
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
pub(in crate::ui) fn playlist_entry_search_and_sort_use_track_fields() {
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
pub(in crate::ui) fn playlist_entry_sort_menu_omits_duration() {
    assert_eq!(
        PLAYLIST_ENTRY_SORTS.as_slice(),
        &[
            PlaylistEntrySort::Order,
            PlaylistEntrySort::Title,
            PlaylistEntrySort::Artist,
            PlaylistEntrySort::Album,
        ]
    );
}
#[test]
pub(in crate::ui) fn playlist_detail_layout_tightens_for_sidebar_panes() {
    assert!(playlist_detail_compact_for_width(550));
    assert_eq!(playlist_detail_route_margin_for_width(550), 16);
    assert!(!playlist_detail_compact_for_width(760));
    assert_eq!(playlist_detail_route_margin_for_width(760), 24);
}
#[test]
pub(in crate::ui) fn playlist_detail_showcase_stays_horizontal_in_sidebar_panes() {
    assert_eq!(
        playlist_detail_header_orientation_for_width(550),
        gtk::Orientation::Horizontal
    );
    assert_eq!(
        playlist_detail_header_orientation_for_width(760),
        gtk::Orientation::Horizontal
    );
}
#[test]
pub(in crate::ui) fn playlist_detail_showcase_cover_scales_up_with_frame() {
    assert_eq!(playlist_detail_cover_size_for_width(550), 182);
    assert_eq!(playlist_detail_cover_size_for_width(760), 208);
}
#[test]
pub(in crate::ui) fn playlist_detail_mosaic_reuses_grid_cover_decode_class() {
    assert_eq!(playlist_detail_cover_fetch_size(), GRID_COVER_SIZE);
    assert_eq!(
        playlist_detail_cover_decode_size_for_width(550, 4),
        GRID_COVER_SIZE as i32
    );
    assert_eq!(
        playlist_detail_cover_decode_size_for_width(760, 4),
        GRID_COVER_SIZE as i32
    );
}
#[test]
pub(in crate::ui) fn playlist_detail_toolbar_keeps_controls_in_one_row() {
    assert_eq!(
        playlist_detail_toolbar_orientation_for_width(550),
        gtk::Orientation::Horizontal
    );
    assert_eq!(
        playlist_detail_toolbar_orientation_for_width(760),
        gtk::Orientation::Horizontal
    );
}
#[test]
pub(in crate::ui) fn playlist_detail_sort_control_width_scales_for_compact_panes() {
    assert_eq!(playlist_detail_sort_width_for_width(360), 120);
    assert_eq!(playlist_detail_sort_width_for_width(550), 150);
    assert_eq!(playlist_detail_sort_width_for_width(760), 170);
}
#[test]
pub(in crate::ui) fn playlist_entry_play_count_text_matches_track_field_format() {
    assert_eq!(playlist_entry_play_count_text(None), "");
    assert_eq!(playlist_entry_play_count_text(Some(0)), "0");
    assert_eq!(playlist_entry_play_count_text(Some(42)), "42");
}
#[test]
pub(in crate::ui) fn playlist_tracks_starting_at_queues_clicked_entry_first() {
    let entries = (1..=4)
        .map(|number| {
            let mut track = test_track("Artist", None);
            track.id = TrackId::fake(number);
            PlaylistEntry {
                entry_id: format!("entry-{number}"),
                track,
            }
        })
        .collect::<Vec<_>>();

    let tracks = playlist_tracks_starting_at(&entries, 2);

    assert_eq!(
        tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![
            TrackId::fake(3),
            TrackId::fake(4),
            TrackId::fake(1),
            TrackId::fake(2)
        ]
    );
}
#[test]
pub(in crate::ui) fn playlist_drop_index_accounts_for_removed_source_row() {
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
pub(in crate::ui) fn track_artist_route_prefers_detail_and_falls_back_to_artist_search() {
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
pub(in crate::ui) fn album_artist_route_prefers_detail_and_falls_back_to_artist_search() {
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
pub(in crate::ui) fn compact_artist_track_sort_keeps_favorites_first() {
    let mut favorite_late = test_track("Artist", Some(ArtistId::fake(1)));
    favorite_late.id = TrackId::fake(1);
    favorite_late.title = "Zulu".to_string();
    favorite_late.favorite = true;
    let mut ordinary_first = test_track("Artist", Some(ArtistId::fake(1)));
    ordinary_first.id = TrackId::fake(2);
    ordinary_first.title = "Alpha".to_string();
    let mut favorite_first = test_track("Artist", Some(ArtistId::fake(1)));
    favorite_first.id = TrackId::fake(3);
    favorite_first.title = "Bravo".to_string();
    favorite_first.favorite = true;

    let mut tracks = vec![
        ordinary_first.clone(),
        favorite_late.clone(),
        favorite_first.clone(),
    ];
    let settings = TrackTableSettings {
        sort_key: TrackSortKey::Title,
        ..TrackTableSettings::default()
    };

    super::sort_tracks_with_options(&mut tracks, &settings, true);

    assert_eq!(
        tracks
            .iter()
            .map(|track| track.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Bravo", "Zulu", "Alpha"]
    );
}
#[test]
pub(in crate::ui) fn full_artist_track_sort_uses_selected_ranking() {
    let mut favorite_late = test_track("Artist", Some(ArtistId::fake(1)));
    favorite_late.id = TrackId::fake(1);
    favorite_late.title = "Zulu".to_string();
    favorite_late.favorite = true;
    let mut ordinary_first = test_track("Artist", Some(ArtistId::fake(1)));
    ordinary_first.id = TrackId::fake(2);
    ordinary_first.title = "Alpha".to_string();
    let mut favorite_first = test_track("Artist", Some(ArtistId::fake(1)));
    favorite_first.id = TrackId::fake(3);
    favorite_first.title = "Bravo".to_string();
    favorite_first.favorite = true;

    let mut tracks = vec![favorite_late, ordinary_first, favorite_first];
    let settings = TrackTableSettings {
        sort_key: TrackSortKey::Title,
        ..TrackTableSettings::default()
    };

    super::sort_tracks_with_options(&mut tracks, &settings, false);

    assert_eq!(
        tracks
            .iter()
            .map(|track| track.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Alpha", "Bravo", "Zulu"]
    );
}
#[test]
pub(in crate::ui) fn artist_discography_uses_responsive_cards() {
    assert!(super::route_uses_responsive_cards(
        &Route::ArtistDiscography(ArtistId::fake(1))
    ));
}
#[test]
pub(in crate::ui) fn smart_playlists_use_responsive_cards() {
    assert!(super::route_uses_responsive_cards(&Route::SmartPlaylists));
}
#[test]
pub(in crate::ui) fn route_boundary_keeps_route_items_inside_main_pane() {
    let spec = super::route_boundary_spec();

    assert_eq!(spec.horizontal_policy, gtk::PolicyType::External);
    assert_eq!(spec.vertical_policy, gtk::PolicyType::Never);
    assert_eq!(spec.overflow, gtk::Overflow::Hidden);
    assert_eq!(spec.min_content_width, 0);
    assert!(!spec.propagate_natural_width);
    assert!(spec.hexpand);
    assert!(spec.vexpand);
}
#[test]
pub(in crate::ui) fn route_boundary_hides_horizontal_scroll_for_library_routes() {
    for route in [Route::Artists, Route::SmartPlaylists] {
        let spec = super::route_boundary_spec_for_route(&route);
        assert_eq!(spec.horizontal_policy, gtk::PolicyType::External);
    }
}
#[test]
pub(in crate::ui) fn regular_playlist_routes_use_default_route_width_boundary() {
    let default = super::route_boundary_spec();

    assert_eq!(
        super::route_boundary_spec_for_route(&Route::Playlists).horizontal_policy,
        default.horizontal_policy
    );
    assert_eq!(
        super::route_boundary_spec_for_route(&Route::PlaylistDetail(PlaylistId::new("playlist")))
            .horizontal_policy,
        default.horizontal_policy
    );
}
#[test]
pub(in crate::ui) fn seekbar_target_seconds_uses_committed_clamped_value() {
    assert_eq!(seekbar_target_seconds(42.4, 180), 42);
    assert_eq!(seekbar_target_seconds(42.5, 180), 43);
    assert_eq!(seekbar_target_seconds(-10.0, 180), 0);
    assert_eq!(seekbar_target_seconds(220.0, 180), 180);
    assert_eq!(seekbar_target_seconds(f64::NAN, 180), 0);
}
#[test]
pub(in crate::ui) fn auto_lyrics_skip_action_only_enabled_for_unsuppressed_external_tracks() {
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
pub(in crate::ui) fn auto_lyrics_skip_action_is_hidden_for_server_lyrics() {
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
pub(in crate::ui) fn auto_lyrics_request_keeps_server_lookup_when_external_search_is_suppressed() {
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
pub(in crate::ui) fn preferences_toast_only_uses_server_settings_statuses() {
    assert_eq!(
        preferences_login_status_toast_message("Checking Jellyfin server..."),
        Some("Checking Jellyfin server...")
    );
    assert_eq!(
        preferences_login_status_toast_message("Server settings saved."),
        Some("Server settings saved.")
    );
    assert_eq!(
        preferences_login_status_toast_message("Server settings saved. Resyncing library..."),
        Some("Server settings saved. Resyncing library...")
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
pub(in crate::ui) fn lyrics_search_results_ignore_queries_from_previous_fields() {
    assert!(lyrics_search_response_matches_query(
        "", "Opening", "", "Opening",
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
pub(in crate::ui) fn lyrics_search_result_subtitle_prefers_synced_when_both_texts_exist() {
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
pub(in crate::ui) fn cover_draw_rect_crops_portrait_images_to_square_targets() {
    let rect = super::cover_draw_rect(100, 200, 34, 34);
    assert!((rect.scale - 0.34).abs() < f64::EPSILON);
    assert!((rect.x - 0.0).abs() < f64::EPSILON);
    assert!((rect.y + 17.0).abs() < f64::EPSILON);
}
#[test]
pub(in crate::ui) fn cover_draw_rect_crops_landscape_images_to_square_targets() {
    let rect = super::cover_draw_rect(200, 100, 44, 44);
    assert!((rect.scale - 0.44).abs() < f64::EPSILON);
    assert!((rect.x + 22.0).abs() < f64::EPSILON);
    assert!((rect.y - 0.0).abs() < f64::EPSILON);
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
pub(in crate::ui) fn test_image_ref(suffix: &str) -> ImageRef {
    ImageRef::new(format!("local:cover:file%3A%2F%2F{suffix}"), None)
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

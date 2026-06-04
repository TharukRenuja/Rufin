use super::lyrics_playback_state::allow_loaded_lyrics_cache_revisit;
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
    PlaylistEntryListState, PlaylistEntrySort, SnapshotRenderDecision, album_play_activation,
    auto_lyrics_request_for_settings, auto_lyrics_skip_action_enabled,
    cover::record_cover_path_lookup_request, current_playback_track_id,
    home_visible_sections::changed_visible_home_section_kinds, local_source_cache_gate_action,
    local_source_snapshot_is_syncing, lyrics_result_subtitle, lyrics_result_subtitle_markup,
    lyrics_result_title_markup, lyrics_search_response_matches_query,
    playlist_detail_compact_for_width, playlist_detail_cover_decode_size_for_width,
    playlist_detail_cover_fetch_size, playlist_detail_cover_size_for_width,
    playlist_detail_header_orientation_for_width, playlist_detail_route_margin_for_width,
    playlist_detail_sort_width_for_width, playlist_detail_toolbar_orientation_for_width,
    playlist_drop_index, playlist_entries_for_state, playlist_entry_play_count_text,
    playlist_play_activation, preferences_login_status_toast_message,
    queue_source_waits_for_snapshot, seekbar_target_seconds, snapshot_event_outcome,
};
use crate::controller::{
    LyricsSearchResult, NormalizedPlayTarget, PlayAnchor, PlayTarget,
    normalize_loaded_source_activation,
};
use gdk_pixbuf::{Colorspace, Pixbuf};
use rufin_core::{
    Album, AlbumId, AppSettings, ArtistId, HomeBlockKind, HomeSection, HomeSectionKind, ImageRef,
    LibraryLayout, LibrarySourceSelection, PlaylistId, QueueAnchor, QueueEntry, QueueEntryId,
    QueueSnapshot, RepeatMode, Route, SearchKind, ServerId, Track, TrackId, TrackSortKey,
    TrackTableSettings,
};
use rufin_provider::{LyricLine, Lyrics, LyricsSource, PlaylistEntry, SearchResults};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

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
        local_source_cache_gate_action(
            false,
            &source,
            true,
            true,
            false,
            false,
            "Cached library ready"
        ),
        LocalSourceCacheGateAction::None
    );
    assert_eq!(
        local_source_cache_gate_action(
            true,
            &source,
            false,
            false,
            false,
            false,
            "Cached library ready"
        ),
        LocalSourceCacheGateAction::None
    );
}
#[test]
pub(in crate::ui) fn local_source_cache_gate_enters_for_folder_change() {
    let source = Some(LibrarySourceSelection::Local);

    assert_eq!(
        local_source_cache_gate_action(
            false,
            &source,
            true,
            true,
            false,
            false,
            "Cached library ready"
        ),
        LocalSourceCacheGateAction::None
    );
    assert_eq!(
        local_source_cache_gate_action(
            true,
            &source,
            true,
            true,
            false,
            false,
            "Cached library ready"
        ),
        LocalSourceCacheGateAction::Enter
    );
}
#[test]
pub(in crate::ui) fn local_source_cache_gate_enters_for_uncached_local_sync_only() {
    let source = Some(LibrarySourceSelection::Local);

    assert_eq!(
        local_source_cache_gate_action(
            false,
            &source,
            true,
            false,
            false,
            false,
            "Syncing library..."
        ),
        LocalSourceCacheGateAction::Enter
    );
    assert_eq!(
        local_source_cache_gate_action(
            false,
            &source,
            true,
            true,
            false,
            false,
            "Syncing library..."
        ),
        LocalSourceCacheGateAction::None
    );
    assert_eq!(
        local_source_cache_gate_action(
            false,
            &source,
            true,
            true,
            false,
            false,
            "Cached library ready"
        ),
        LocalSourceCacheGateAction::None
    );
}
#[test]
pub(in crate::ui) fn local_source_cache_gate_waits_until_sync_snapshot_finishes() {
    let source = Some(LibrarySourceSelection::Local);

    assert_eq!(
        local_source_cache_gate_action(
            false,
            &source,
            true,
            false,
            true,
            false,
            "Cached library ready"
        ),
        LocalSourceCacheGateAction::Wait
    );
    assert!(local_source_snapshot_is_syncing("Syncing library..."));
    assert_eq!(
        local_source_cache_gate_action(
            false,
            &source,
            true,
            false,
            true,
            true,
            "Syncing library..."
        ),
        LocalSourceCacheGateAction::Wait
    );
    assert_eq!(
        local_source_cache_gate_action(
            false,
            &source,
            true,
            false,
            true,
            true,
            "Cached library ready"
        ),
        LocalSourceCacheGateAction::Reveal
    );
}
#[test]
pub(in crate::ui) fn local_source_cache_gate_cancels_when_source_leaves_local() {
    let source = Some(LibrarySourceSelection::Server(rufin_core::ServerId::new(
        "jellyfin:server:test",
    )));

    assert_eq!(
        local_source_cache_gate_action(
            false,
            &source,
            true,
            true,
            true,
            true,
            "Cached library ready"
        ),
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
pub(in crate::ui) fn visible_range_clamps_exact_bottom_to_last_row_window() {
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
pub(in crate::ui) fn initial_visible_count_uses_viewport_geometry() {
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
pub(in crate::ui) fn post_route_visible_warm_targets_tracks_after_home() {
    let targets = super::post_route_visible_warm_targets(&Route::Home);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].route, Route::Tracks);
    assert_eq!(targets[0].leading_rows, super::TRACK_ROUTE_PAGE_SIZE);
    assert!(super::post_route_visible_warm_targets(&Route::Tracks).is_empty());
}
#[test]
pub(in crate::ui) fn visible_range_clamps_exact_bottom_to_last_grid_window() {
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
pub(in crate::ui) fn playlist_activation_distinguishes_duplicate_track_occurrences() {
    let mut duplicate = test_track("Artist", None);
    duplicate.id = TrackId::fake(7);
    let activation = playlist_play_activation(
        PlaylistId::fake(3),
        vec![
            PlaylistEntry {
                entry_id: "entry-a".to_string(),
                track: duplicate.clone(),
            },
            PlaylistEntry {
                entry_id: "entry-b".to_string(),
                track: duplicate,
            },
        ],
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
pub(in crate::ui) fn album_activation_keeps_album_order_and_clicked_track_anchor() {
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
pub(in crate::ui) fn loaded_lyrics_allows_revisit_to_check_cache() {
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
pub(in crate::ui) fn lyrics_search_result_markup_escapes_external_text() {
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

use super::lyrics_playback_state::{
    allow_loaded_lyrics_cache_revisit, clear_matching_lyrics_loading,
    loaded_lyrics_matches_current, lyrics_loading_matches_current,
};
use super::now_playing_notification::{
    NativeNowPlayingNotification, now_playing_notification_artwork_uri,
    now_playing_notification_can_send, now_playing_notification_hints,
    now_playing_notification_matches_current, now_playing_notification_parameters,
    now_playing_notification_should_withdraw,
};
use super::responsive_layout_state::startup_loading_screen_active;
use super::right_panel::{
    clamp_queue_lyrics_position, queue_lyrics_default_position, queue_lyrics_height_for_position,
    queue_lyrics_initial_position, queue_lyrics_position_for_height, queue_lyrics_saved_height,
};
use super::startup_reveal::{
    StartupRevealAction, main_loop_stall_delay_ms, startup_route_reveal_action,
};
use super::{
    PlaylistEntryListState, PlaylistEntrySort, current_playback_track_id,
    home_visible_sections::changed_visible_home_section_kinds, lyrics_result_subtitle,
    lyrics_result_subtitle_markup, lyrics_result_title_markup,
    lyrics_search_response_matches_query, lyrics_search_result_has_content, playlist_cover_size,
    playlist_detail_compact_for_width, playlist_drop_index, playlist_entries_for_state,
    playlist_sort_width, queue_source_waits_for_snapshot, seekbar_target_seconds,
};
use crate::StoredSettings;
use crate::controller::{LibraryCounts, LibraryHomeUpdate, SearchRequestKey};
use ::library::LibraryDelta;
use ::library::{
    Album, AlbumId, ArtistCredit, ArtistId, HomeSection, HomeSectionKind, ImageRef, MusicFolderId,
    Playlist, PlaylistEntry, PlaylistId, SearchResults, SourceId, Track, TrackId,
};
use domain::{LibrarySourceSelection, Route, SearchKind, TrackSortKey, TrackTableSettings};
use gdk_pixbuf::{Colorspace, Pixbuf};
use metadata::{ExternalLyricsProvider, LyricLine, Lyrics, LyricsSearchResult, LyricsSource};
use playback::{
    ControlsView, OccurrenceId, PlaybackView, Provenance, QueueSummaryView, RepeatMode,
    SequenceEntry, TransportStatus, TransportView,
};
use sources::SourceIdentity;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn shell_hide_available() {
    let mut settings = StoredSettings::default();

    assert!(!super::tray::exit_tray_hide(&settings, true));

    settings.exit_to_tray = true;

    assert!(!super::tray::exit_tray_hide(&settings, true));

    settings.tray_enabled = true;

    assert!(super::tray::exit_tray_hide(&settings, true));
    assert!(!super::tray::exit_tray_hide(&settings, false));
}

#[test]
fn shell_start_available() {
    let mut settings = StoredSettings::default();

    assert!(!super::tray::should_start_minimized(&settings, true));

    settings.start_minimized = true;

    assert!(!super::tray::should_start_minimized(&settings, true));

    settings.tray_enabled = true;

    assert!(super::tray::should_start_minimized(&settings, true));
    assert!(!super::tray::should_start_minimized(&settings, false));
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
pub(in crate::ui) fn shell_now_playing_notification_gates_send_and_withdraw() {
    let image_ref = ImageRef::new("jellyfin:album:one", Some("tag-one".to_string()));
    let mut settings = StoredSettings {
        notifications_enabled: true,
        ..StoredSettings::default()
    };
    let mut playback = test_playback_view(
        Some(test_sequence_entry("Current", image_ref)),
        SourceId::fake(1),
        TransportStatus::Playing,
        0,
    );

    assert!(now_playing_notification_can_send(
        &settings,
        Some(&playback)
    ));
    assert!(!now_playing_notification_should_withdraw(
        &settings,
        Some(&playback)
    ));

    playback.transport.state = TransportStatus::Stopped;
    assert!(!now_playing_notification_can_send(
        &settings,
        Some(&playback)
    ));
    assert!(now_playing_notification_should_withdraw(
        &settings,
        Some(&playback)
    ));

    playback.transport.state = TransportStatus::Paused;
    assert!(!now_playing_notification_can_send(
        &settings,
        Some(&playback)
    ));
    assert!(!now_playing_notification_should_withdraw(
        &settings,
        Some(&playback)
    ));

    settings.notifications_enabled = false;
    assert!(!now_playing_notification_can_send(
        &settings,
        Some(&playback)
    ));
    assert!(now_playing_notification_should_withdraw(
        &settings,
        Some(&playback)
    ));
}

#[test]
pub(in crate::ui) fn shell_now_playing_notification_matches_run_not_track() {
    let current_ref = ImageRef::new("jellyfin:album:current", Some("tag-current".to_string()));
    let mut playback = test_playback_view(
        Some(test_sequence_entry("Current", current_ref)),
        SourceId::fake(1),
        TransportStatus::Playing,
        0,
    );
    let current_run = playback::RunId::new(7);
    playback.transport.run = Some(current_run);

    assert!(now_playing_notification_matches_current(
        Some(&playback),
        current_run
    ));

    playback.transport.run = Some(playback::RunId::new(8));

    assert!(!now_playing_notification_matches_current(
        Some(&playback),
        current_run
    ));
}

#[test]
pub(in crate::ui) fn shell_now_playing_native_notification_uses_image_hint() {
    let uri = "file:///tmp/rufin-cover.png";
    let hints = now_playing_notification_hints(Some(uri));

    assert_eq!(
        hints.lookup::<String>("desktop-entry").expect("lookup"),
        Some("io.github.screwys.Rufin".to_string())
    );
    assert_eq!(
        hints.lookup::<bool>("transient").expect("lookup"),
        Some(true)
    );
    assert_eq!(
        hints.lookup::<String>("image-path").expect("lookup"),
        Some(uri.to_string())
    );
    assert_eq!(
        hints.lookup::<String>("image_path").expect("lookup"),
        Some(uri.to_string())
    );
}

#[test]
pub(in crate::ui) fn shell_now_playing_native_notification_parameters_replace_previous() {
    let notification = NativeNowPlayingNotification {
        title: "Track".to_string(),
        body: "Artist - Album".to_string(),
        artwork_uri: Some("file:///tmp/rufin-cover.png".to_string()),
    };
    let parameters = now_playing_notification_parameters(&notification, 42);

    assert_eq!(parameters.type_().as_str(), "(susssasa{sv}i)");
    assert_eq!(
        parameters.try_child_get::<String>(0).expect("app name"),
        Some("Rufin".to_string())
    );
    assert_eq!(
        parameters.try_child_get::<u32>(1).expect("replace id"),
        Some(42)
    );
    assert_eq!(
        parameters.try_child_get::<String>(2).expect("app icon"),
        Some("io.github.screwys.Rufin".to_string())
    );
    assert_eq!(
        parameters.try_child_get::<String>(3).expect("title"),
        Some("Track".to_string())
    );
    assert_eq!(
        parameters.try_child_get::<String>(4).expect("body"),
        Some("Artist - Album".to_string())
    );
}

#[test]
pub(in crate::ui) fn shell_now_playing_artwork_path_becomes_file_uri() {
    let uri = now_playing_notification_artwork_uri(&PathBuf::from("/tmp/rufin cover.png"))
        .expect("file uri");

    assert_eq!(uri, "file:///tmp/rufin%20cover.png");
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
pub(in crate::ui) fn shell_active_commit_invalidates_changed_pages() {
    let mut library = test_library_snapshot();
    let server = test_server("active");
    let section = HomeSection {
        kind: HomeSectionKind::Explore,
        albums: Vec::new(),
        tracks: Vec::new(),
    };
    let track = test_track("Track", Some(ArtistId::fake(1)));
    library.source = Some(server.clone());
    library.selected_source = Some(LibrarySourceSelection::Source(server.id.clone()));
    library.tracks = vec![track.clone()];
    let playlist = test_playlist("Regular", test_image_ref("playlist"));
    library.playlists = vec![playlist.clone()];
    library.favorites = vec![track.clone()];
    library.search = SearchResults {
        tracks: vec![track.clone()],
        ..SearchResults::default()
    };

    let applied = super::apply_library_commit_to_snapshot(
        &mut library,
        &library_sync::LibraryCommitted {
            source_id: server.id.clone(),
            revision: 7,
            delta: LibraryDelta {
                tracks: ::library::TrackDelta {
                    fields: vec![track.id.clone()],
                    ..Default::default()
                },
                playlists: ::library::PlaylistDelta {
                    entries: vec![playlist.id.clone()],
                    ..Default::default()
                },
                home_changed: true,
                ..LibraryDelta::default()
            },
        },
        Some(LibraryCounts {
            tracks: 30_000,
            playlists: 40,
            ..LibraryCounts::default()
        }),
        Some(LibraryHomeUpdate {
            sections: vec![section.clone()],
            prefetched_explore: None,
        }),
    );

    assert!(applied);
    assert_eq!(
        library.cache,
        crate::controller::LibraryCacheState::Committed { revision: 7 }
    );
    assert!(library.tracks.is_empty());
    assert!(library.playlists.is_empty());
    assert_eq!(library.favorites, vec![track.clone()]);
    assert!(library.search.tracks.is_empty());
    assert_eq!(library.cached_track_count, 30_000);
    assert_eq!(library.cached_playlist_count, 40);
    assert_eq!(library.home_sections, vec![section]);
}

#[test]
pub(in crate::ui) fn shell_search_event_requires_current_request_and_identity() {
    let source_id = SourceId::new("server:active");
    let folder_id = MusicFolderId::new("folder:music");
    let current = SearchRequestKey {
        request_id: 2,
        query: "needle".to_string(),
        kind: SearchKind::All,
        source_id: Some(source_id.clone()),
        selected_music_folder_id: Some(folder_id.clone()),
    };

    assert!(super::search_route_state::search_event_matches(
        Some(&current),
        &current,
        "needle",
        &SearchKind::All,
        Some(&source_id),
        Some(&folder_id),
    ));

    let stale_request = SearchRequestKey {
        request_id: 1,
        ..current.clone()
    };
    assert!(!super::search_route_state::search_event_matches(
        Some(&current),
        &stale_request,
        "needle",
        &SearchKind::All,
        Some(&source_id),
        Some(&folder_id),
    ));
    assert!(!super::search_route_state::search_event_matches(
        Some(&current),
        &current,
        "other",
        &SearchKind::All,
        Some(&source_id),
        Some(&folder_id),
    ));
    assert!(!super::search_route_state::search_event_matches(
        Some(&current),
        &current,
        "needle",
        &SearchKind::Tracks,
        Some(&source_id),
        Some(&folder_id),
    ));
    assert!(!super::search_route_state::search_event_matches(
        Some(&current),
        &current,
        "needle",
        &SearchKind::All,
        Some(&SourceId::new("server:other")),
        Some(&folder_id),
    ));
    assert!(!super::search_route_state::search_event_matches(
        Some(&current),
        &current,
        "needle",
        &SearchKind::All,
        Some(&source_id),
        Some(&MusicFolderId::new("folder:other")),
    ));
}

#[test]
pub(in crate::ui) fn shell_commit_ignores_inactive_or_stale_update() {
    let mut library = test_library_snapshot();
    let server = test_server("active");
    library.source = Some(server.clone());
    library.selected_source = Some(LibrarySourceSelection::Source(server.id.clone()));
    library.cache = crate::controller::LibraryCacheState::Committed { revision: 5 };
    library.cached_track_count = 2;

    let inactive_applied = super::apply_library_commit_to_snapshot(
        &mut library,
        &library_sync::LibraryCommitted {
            source_id: SourceId::new("server:stale"),
            revision: 6,
            delta: LibraryDelta {
                home_changed: true,
                ..LibraryDelta::default()
            },
        },
        Some(LibraryCounts {
            tracks: 10,
            ..LibraryCounts::default()
        }),
        None,
    );
    let stale_applied = super::apply_library_commit_to_snapshot(
        &mut library,
        &library_sync::LibraryCommitted {
            source_id: server.id,
            revision: 5,
            delta: LibraryDelta {
                reset: Some(::library::LibraryReset::Source),
                ..LibraryDelta::default()
            },
        },
        Some(LibraryCounts::default()),
        None,
    );

    assert!(!inactive_applied);
    assert!(!stale_applied);
    assert_eq!(library.cache.revision(), 5);
    assert_eq!(library.cached_track_count, 2);
}
#[test]
pub(in crate::ui) fn shell_match_snapshot() {
    let old_source = SourceId::new("jellyfin:server:old");
    let next_source = SourceId::new("local:source");
    let playback = test_playback_view(None, next_source.clone(), TransportStatus::Stopped, 0);

    assert!(queue_source_waits_for_snapshot(
        Some(&playback),
        Some(&old_source)
    ));
    assert!(!queue_source_waits_for_snapshot(
        Some(&playback),
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
        startup_route_reveal_action(true, 0, Duration::from_millis(32)),
        StartupRevealAction::RevealReady
    );
    assert_eq!(
        startup_route_reveal_action(true, 0, Duration::ZERO),
        StartupRevealAction::RevealReady
    );
}
#[test]
pub(in crate::ui) fn main_loop_stall_delay() {
    assert_eq!(
        main_loop_stall_delay_ms(Duration::from_millis(100), Duration::from_millis(80)),
        0
    );
    assert_eq!(
        main_loop_stall_delay_ms(Duration::from_millis(100), Duration::from_millis(725)),
        625
    );
}
#[test]
pub(in crate::ui) fn shell_clamp_height() {
    assert_eq!(clamp_queue_lyrics_position(800, 1701), 799);
    assert_eq!(clamp_queue_lyrics_position(800, 10), 10);
    assert_eq!(clamp_queue_lyrics_position(200, 1701), 199);
    assert_eq!(queue_lyrics_default_position(700), 400);
    assert_eq!(queue_lyrics_default_position(1400), 1100);
    assert_eq!(queue_lyrics_initial_position(700, None), 400);
    assert_eq!(queue_lyrics_initial_position(700, Some(300)), 400);
    assert_eq!(queue_lyrics_initial_position(1400, Some(300)), 1100);
    assert_eq!(queue_lyrics_position_for_height(700, 300), 400);
    assert_eq!(queue_lyrics_position_for_height(700, 2_000), 1);
    assert_eq!(queue_lyrics_height_for_position(700, 400), 300);
    assert_eq!(queue_lyrics_height_for_position(700, 2_000), 1);
    assert_eq!(
        queue_lyrics_saved_height(700, queue_lyrics_position_for_height(700, 400)),
        Some(400)
    );
    assert_eq!(queue_lyrics_saved_height(1, 0), None);
}
#[test]
pub(in crate::ui) fn shell_use_entry() {
    let track_id = TrackId::fake(7);
    let mut entry = test_sequence_entry("Restored", test_image_ref("restored"));
    entry.track.id = track_id.clone();
    let playback = Some(test_playback_view(
        Some(entry),
        SourceId::fake(1),
        TransportStatus::Paused,
        0,
    ));

    assert_eq!(current_playback_track_id(&playback), Some(track_id));
    assert_eq!(current_playback_track_id(&None), None);
}

#[test]
pub(in crate::ui) fn shell_fullscreen_refresh_scopes_playback_ticks() {
    let mut previous = test_playback_view(
        Some(test_sequence_entry("Current", test_image_ref("current"))),
        SourceId::fake(1),
        TransportStatus::Playing,
        1_000,
    );

    let mut position_tick = previous.clone();
    position_tick.transport.position_millis = 1_500;
    assert_eq!(
        super::fullscreen_playback_refresh(Some(&previous), &position_tick),
        super::FullscreenPlaybackRefresh::None
    );

    let mut state_change = previous.clone();
    state_change.transport.state = TransportStatus::Paused;
    assert_eq!(
        super::fullscreen_playback_refresh(Some(&previous), &state_change),
        super::FullscreenPlaybackRefresh::Visualizer
    );

    let mut current_change = previous.clone();
    current_change.transport.current = Some(Arc::new(test_sequence_entry(
        "Next",
        test_image_ref("next"),
    )));
    assert_eq!(
        super::fullscreen_playback_refresh(Some(&previous), &current_change),
        super::FullscreenPlaybackRefresh::Static
    );

    previous.transport.source_id = SourceId::fake(2);
    assert_eq!(
        super::fullscreen_playback_refresh(Some(&position_tick), &previous),
        super::FullscreenPlaybackRefresh::Static
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
    assert_eq!(entries[filtered[0]].entry_id, "entry-beta");

    let sorted = playlist_entries_for_state(
        &entries,
        &PlaylistEntryListState {
            query: String::new(),
            sort: PlaylistEntrySort::Album,
            descending: true,
        },
    );
    assert_eq!(entries[sorted[0]].entry_id, "entry-alpha");
    assert_eq!(entries[sorted[1]].entry_id, "entry-beta");
}
#[test]
pub(in crate::ui) fn shell_playlist_panes() {
    assert!(playlist_detail_compact_for_width(550));
    assert!(!playlist_detail_compact_for_width(760));
    assert_eq!(playlist_cover_size(419), 150);
    assert_eq!(playlist_cover_size(450), 159);
    assert_eq!(playlist_cover_size(519), 181);
    assert_eq!(playlist_cover_size(550), 182);
    assert_eq!(playlist_cover_size(760), 208);
    assert_eq!(playlist_sort_width(360), 120);
    assert_eq!(playlist_sort_width(550), 150);
    assert_eq!(playlist_sort_width(760), 170);
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
    assert_eq!(super::track_artist_route(&track), None);

    let mut track = test_track("Credited Artist", None);
    track.artist_credits = vec![test_credit(ArtistId::fake(4), "Credited Artist")];
    assert_eq!(
        super::track_artist_route(&track),
        Some(Route::ArtistDetail(ArtistId::fake(4)))
    );

    let mut track = test_track("Album Artist", None);
    track.album_artist_credits = vec![test_credit(ArtistId::fake(6), "Album Artist")];
    assert_eq!(
        super::track_artist_route(&track),
        Some(Route::ArtistDetail(ArtistId::fake(6)))
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
    assert_eq!(super::album_artist_route(&album), None);

    let mut album = test_album("Linked Artist", None);
    album.album_artist_credits = vec![test_credit(ArtistId::fake(7), "Linked Artist")];
    assert_eq!(
        super::album_artist_route(&album),
        Some(Route::ArtistDetail(ArtistId::fake(7)))
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
pub(in crate::ui) fn shell_allow_cache() {
    let track_id = TrackId::fake(13);
    let previous_failed_track_id = TrackId::fake(14);
    let media = playback::MediaKey {
        source_id: SourceId::new("source-current"),
        track_id: track_id.clone(),
    };
    let previous_failed_media = playback::MediaKey {
        source_id: media.source_id.clone(),
        track_id: previous_failed_track_id,
    };
    let same_track_other_source = playback::MediaKey {
        source_id: SourceId::new("source-other"),
        track_id: track_id.clone(),
    };
    let mut attempted = HashSet::from([
        media.clone(),
        previous_failed_media.clone(),
        same_track_other_source.clone(),
    ]);
    let lyrics = Lyrics {
        track_id: track_id.clone(),
        source: LyricsSource::Remote,
        external_provider: None,
        lines: vec![LyricLine {
            text: "line one".to_string(),
            start_millis: Some(1_000),
        }],
    };

    allow_loaded_lyrics_cache_revisit(&mut attempted, &media, Some(&lyrics));

    assert!(!attempted.contains(&media));
    assert!(attempted.contains(&previous_failed_media));
    assert!(attempted.contains(&same_track_other_source));
    allow_loaded_lyrics_cache_revisit(&mut attempted, &previous_failed_media, None);
    assert!(attempted.contains(&previous_failed_media));
}
#[test]
pub(in crate::ui) fn shell_lyrics_loading_current() {
    let current_track = TrackId::fake(15);
    let old_track = TrackId::fake(16);
    let current_media = playback::MediaKey {
        source_id: SourceId::new("source-current"),
        track_id: current_track.clone(),
    };
    let old_media = playback::MediaKey {
        source_id: current_media.source_id.clone(),
        track_id: old_track,
    };
    let other_source_media = playback::MediaKey {
        source_id: SourceId::new("source-other"),
        track_id: current_track.clone(),
    };
    let lyrics = Lyrics {
        track_id: current_track.clone(),
        source: LyricsSource::Server,
        external_provider: None,
        lines: vec![LyricLine {
            text: "line one".to_string(),
            start_millis: None,
        }],
    };

    assert!(lyrics_loading_matches_current(
        Some(&current_media),
        Some(&current_media),
        None
    ));
    assert!(!lyrics_loading_matches_current(
        Some(&current_media),
        Some(&old_media),
        None
    ));
    assert!(!lyrics_loading_matches_current(
        Some(&current_media),
        Some(&other_source_media),
        None
    ));
    assert!(!lyrics_loading_matches_current(
        Some(&current_media),
        Some(&current_media),
        Some(&lyrics)
    ));

    let mut loading_media = Some(old_media.clone());
    clear_matching_lyrics_loading(&mut loading_media, &current_media);
    assert_eq!(loading_media, Some(old_media.clone()));
    clear_matching_lyrics_loading(&mut loading_media, &old_media);
    assert_eq!(loading_media, None);
}
#[test]
pub(in crate::ui) fn shell_reject_stale_lyrics() {
    let old_track = TrackId::fake(12);
    let current_track = TrackId::fake(13);
    let current_media = playback::MediaKey {
        source_id: SourceId::new("source-current"),
        track_id: current_track.clone(),
    };
    let old_media = playback::MediaKey {
        source_id: current_media.source_id.clone(),
        track_id: old_track.clone(),
    };
    let same_track_other_source = playback::MediaKey {
        source_id: SourceId::new("source-other"),
        track_id: current_track.clone(),
    };
    let old_lyrics = Lyrics {
        track_id: old_track.clone(),
        source: LyricsSource::Remote,
        external_provider: None,
        lines: vec![LyricLine {
            text: "old line".to_string(),
            start_millis: Some(1_000),
        }],
    };
    let current_lyrics = Lyrics {
        track_id: current_track.clone(),
        source: LyricsSource::Server,
        external_provider: None,
        lines: vec![LyricLine {
            text: "current line".to_string(),
            start_millis: None,
        }],
    };

    assert!(!loaded_lyrics_matches_current(
        Some(&current_media),
        &old_media,
        Some(&old_lyrics)
    ));
    assert!(!loaded_lyrics_matches_current(
        Some(&current_media),
        &old_media,
        None
    ));
    assert!(!loaded_lyrics_matches_current(
        Some(&current_media),
        &current_media,
        Some(&old_lyrics)
    ));
    assert!(!loaded_lyrics_matches_current(
        Some(&current_media),
        &same_track_other_source,
        Some(&current_lyrics)
    ));
    assert!(loaded_lyrics_matches_current(
        Some(&current_media),
        &current_media,
        Some(&current_lyrics)
    ));
    assert!(loaded_lyrics_matches_current(
        Some(&current_media),
        &current_media,
        None
    ));
    assert!(!loaded_lyrics_matches_current(None, &old_media, None));
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
        provider: ExternalLyricsProvider::Lrclib,
        id: "12".to_string(),
        track_name: "Example Track".to_string(),
        artist_name: "Example Artist".to_string(),
        album_name: "Example Album".to_string(),
        duration_seconds: 95,
        synced_lyrics: Some("[00:01.00]line".to_string()),
        plain_lyrics: Some("line".to_string()),
    };

    assert_eq!(
        lyrics_result_subtitle(&result),
        "LRCLIB - Example Album - 1:35 - Synced lyrics"
    );
}
#[test]
pub(in crate::ui) fn shell_deferred_lyrics_are_not_labeled_empty() {
    let result = LyricsSearchResult {
        provider: ExternalLyricsProvider::Netease,
        id: "13".to_string(),
        track_name: "Example Track".to_string(),
        artist_name: "Example Artist".to_string(),
        album_name: "Example Album".to_string(),
        duration_seconds: 95,
        synced_lyrics: None,
        plain_lyrics: None,
    };

    assert!(lyrics_search_result_has_content(&result));
    assert_eq!(
        lyrics_result_subtitle(&result),
        "NetEase - Example Album - 1:35 - Remote lyrics"
    );
}
#[test]
pub(in crate::ui) fn shell_lrclib_empty_result_is_not_loadable() {
    let result = LyricsSearchResult {
        provider: ExternalLyricsProvider::Lrclib,
        id: "14".to_string(),
        track_name: "Example Track".to_string(),
        artist_name: "Example Artist".to_string(),
        album_name: "Example Album".to_string(),
        duration_seconds: 95,
        synced_lyrics: None,
        plain_lyrics: None,
    };

    assert!(!lyrics_search_result_has_content(&result));
    assert_eq!(
        lyrics_result_subtitle(&result),
        "LRCLIB - Example Album - 1:35 - No lyrics"
    );
}
#[test]
pub(in crate::ui) fn shell_lyrics_text() {
    let result = LyricsSearchResult {
        provider: ExternalLyricsProvider::Lrclib,
        id: "13".to_string(),
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
        "LRCLIB - Hits &amp; Rarities - 1:35 - Synced lyrics"
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
pub(in crate::ui) fn test_library_snapshot() -> crate::controller::LibrarySnapshot {
    crate::controller::LibrarySnapshot {
        source: None,
        sources: Vec::new(),
        selected_source: None,
        local_folders: Vec::new(),
        source_local_access: Vec::new(),
        local_access: None,
        local_access_status: crate::controller::LocalAccessStatus::default(),
        music_folders: Vec::new(),
        selected_music_folder_id: None,
        first_run: false,
        cache: crate::controller::LibraryCacheState::NoCache { revision: 0 },
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
        playlist_entry_keys: HashMap::new(),
        favorites: Vec::new(),
        search: SearchResults::default(),
    }
}
pub(in crate::ui) fn test_server(suffix: &str) -> SourceIdentity {
    SourceIdentity {
        id: SourceId::new(format!("server:{suffix}")),
        kind: "test".to_string(),
        name: format!("Server {suffix}"),
        base_url: "http://localhost".to_string(),
    }
}
pub(in crate::ui) fn test_image_ref(suffix: &str) -> ImageRef {
    ImageRef::new(format!("local:cover:file%3A%2F%2F{suffix}"), None)
}
pub(in crate::ui) fn test_playlist(name: &str, image_ref: ImageRef) -> Playlist {
    Playlist {
        id: PlaylistId::fake(1),
        name: name.to_string(),
        owner: None,
        track_count: 1,
        duration_seconds: 180,
        top_genres: Vec::new(),
        image_ref: Some(image_ref),
        representative_albums: Vec::new(),
    }
}
pub(in crate::ui) fn test_sequence_entry(title: &str, image_ref: ImageRef) -> SequenceEntry {
    let mut track = test_track("Artist", None);
    track.title = title.to_string();
    track.image_ref = Some(image_ref);
    SequenceEntry {
        occurrence: OccurrenceId::new(format!("queue:{title}")),
        track,
        provenance: Provenance::Manual,
    }
}

pub(in crate::ui) fn test_playback_view(
    current: Option<SequenceEntry>,
    source_id: SourceId,
    state: TransportStatus,
    position_millis: u64,
) -> PlaybackView {
    let current_occurrence = current.as_ref().map(|entry| entry.occurrence.clone());
    let run = current.as_ref().map(|_| playback::RunId::new(1));
    PlaybackView {
        queue: QueueSummaryView {
            revision: 1,
            total: usize::from(current.is_some()),
            current_occurrence,
            current_index: current.as_ref().map(|_| 0),
            next_occurrence: None,
        },
        transport: TransportView {
            source_id,
            run,
            current: current.map(Arc::new),
            state,
            position_millis,
            duration_millis: 180_000,
            buffering_percent: None,
            error: None,
        },
        controls: ControlsView {
            repeat_mode: RepeatMode::Off,
            shuffle_enabled: false,
            auto_dj_enabled: false,
            volume: 1.0,
            muted: false,
            audio_output: None,
        },
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
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
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
        album_artwork: None,
        genres: Vec::new(),
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        local_path: None,
        source_format: None,
        comment: None,
        skip_count: None,
        bpm: None,
        moods: Vec::new(),
    }
}

fn test_credit(id: ArtistId, name: &str) -> ArtistCredit {
    ArtistCredit {
        id,
        name: name.to_string(),
        musicbrainz_artist_id: None,
    }
}

use super::events::{apply_commit_revision, queue_source_waits_for_presentation};
use crate::player::fullscreen::{FullscreenPlaybackRefresh, fullscreen_playback_refresh};
use crate::player::lyrics::search::{
    lyrics_result_subtitle, lyrics_result_subtitle_markup, lyrics_result_title_markup,
    lyrics_search_response_matches_query, lyrics_search_result_has_content,
};
use crate::routes::detail_links::{
    album_artist_route as resolve_album_artist_route,
    track_artist_route as resolve_track_artist_route,
};
use crate::routes::library_fields::sort_tracks;
use crate::routes::playlist_entries::{
    PlaylistEntryListState, playlist_drop_index, playlist_entries_for_state,
};
use crate::routes::route::Route;
use crate::{LibraryField, LibraryListKey, LibraryListSettings};
use ::library::LibraryDelta;
use ::library::{
    Album, AlbumId, ArtistCredit, ArtistId, ImageRef, PlaylistEntry, SourceId, Track, TrackId,
};
use metadata::{ExternalLyricsProvider, LyricsSearchResult};
use playback::{
    ControlsView, OccurrenceId, PlaybackView, Provenance, QueueSummaryView, RepeatMode,
    SequenceEntry, TransportStatus, TransportView,
};
use sources::{LibrarySourceSelection, SourceIdentity};
use std::sync::Arc;

#[test]
pub(crate) fn shell_commit_ignores_inactive_or_stale_update() {
    let mut library = test_source_presentation();
    let server = test_server("active");
    library.source = Some(server.clone());
    library.selected_source = Some(LibrarySourceSelection::Source(server.id.clone()));
    library.cache = sources::LibraryCacheState::Committed { revision: 5 };
    let inactive_applied = apply_commit_revision(
        &mut library,
        &library_sync::LibraryCommitted {
            source_id: SourceId::new("server:stale"),
            revision: 6,
            delta: LibraryDelta {
                home_changed: true,
                ..LibraryDelta::default()
            },
        },
    );
    let stale_applied = apply_commit_revision(
        &mut library,
        &library_sync::LibraryCommitted {
            source_id: server.id,
            revision: 5,
            delta: LibraryDelta {
                reset: Some(::library::LibraryReset::Source),
                ..LibraryDelta::default()
            },
        },
    );

    assert!(!inactive_applied);
    assert!(!stale_applied);
    assert_eq!(library.cache.revision(), 5);
}

#[test]
pub(crate) fn shell_match_snapshot() {
    let old_source = SourceId::new("jellyfin:server:old");
    let next_source = SourceId::new("local:source");
    let playback = test_playback_view(None, next_source.clone(), TransportStatus::Stopped, 0);

    assert!(queue_source_waits_for_presentation(
        Some(&playback),
        Some(&old_source)
    ));
    assert!(!queue_source_waits_for_presentation(
        Some(&playback),
        Some(&next_source)
    ));
    assert!(!queue_source_waits_for_presentation(
        None,
        Some(&old_source)
    ));
}
#[test]
pub(crate) fn shell_fullscreen_refresh_scopes_playback_ticks() {
    let mut previous = test_playback_view(
        Some(test_sequence_entry("Current", test_image_ref("current"))),
        SourceId::fake(1),
        TransportStatus::Playing,
        1_000,
    );

    let mut position_tick = previous.clone();
    position_tick.transport.position_millis = 1_500;
    assert_eq!(
        fullscreen_playback_refresh(Some(&previous), &position_tick),
        FullscreenPlaybackRefresh::None
    );

    let mut state_change = previous.clone();
    state_change.transport.state = TransportStatus::Paused;
    assert_eq!(
        fullscreen_playback_refresh(Some(&previous), &state_change),
        FullscreenPlaybackRefresh::Visualizer
    );

    let mut current_change = previous.clone();
    current_change.transport.current = Some(Arc::new(test_sequence_entry(
        "Next",
        test_image_ref("next"),
    )));
    assert_eq!(
        fullscreen_playback_refresh(Some(&previous), &current_change),
        FullscreenPlaybackRefresh::Static
    );

    previous.transport.source_id = SourceId::fake(2);
    assert_eq!(
        fullscreen_playback_refresh(Some(&position_tick), &previous),
        FullscreenPlaybackRefresh::Static
    );
}
#[test]
pub(crate) fn playlist_rows_follow_playlist_track_settings() {
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

    let mut settings = LibraryListSettings::for_key(LibraryListKey::PlaylistTracks);
    assert_eq!(settings.sort_key, LibraryField::RowIndex);
    let mut state = PlaylistEntryListState::for_settings(&settings);
    state.query = "needle".to_string();
    let filtered = playlist_entries_for_state(&entries, &state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(entries[filtered[0]].entry_id, "entry-beta");

    settings.sort_key = LibraryField::Album;
    settings.descending = true;
    let sorted =
        playlist_entries_for_state(&entries, &PlaylistEntryListState::for_settings(&settings));
    assert_eq!(entries[sorted[0]].entry_id, "entry-alpha");
    assert_eq!(entries[sorted[1]].entry_id, "entry-beta");
}
#[test]
pub(crate) fn shell_drop_source() {
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
pub(crate) fn track_artist_route() {
    let track = test_track("Track Artist", Some(ArtistId::fake(3)));
    assert_eq!(
        resolve_track_artist_route(&track),
        Some(Route::ArtistDetail(ArtistId::fake(3)))
    );

    let track = test_track("Loose Artist", None);
    assert_eq!(resolve_track_artist_route(&track), None);

    let mut track = test_track("Credited Artist", None);
    track.artist_credits = vec![test_credit(ArtistId::fake(4), "Credited Artist")];
    assert_eq!(
        resolve_track_artist_route(&track),
        Some(Route::ArtistDetail(ArtistId::fake(4)))
    );

    let mut track = test_track("Album Artist", None);
    track.album_artist_credits = vec![test_credit(ArtistId::fake(6), "Album Artist")];
    assert_eq!(
        resolve_track_artist_route(&track),
        Some(Route::ArtistDetail(ArtistId::fake(6)))
    );

    assert_eq!(resolve_track_artist_route(&test_track("   ", None)), None);
}
#[test]
pub(crate) fn album_artist_route() {
    let album = test_album("Album Artist", Some(ArtistId::fake(5)));
    assert_eq!(
        resolve_album_artist_route(&album),
        Some(Route::ArtistDetail(ArtistId::fake(5)))
    );

    let album = test_album("Compilation Artist", None);
    assert_eq!(resolve_album_artist_route(&album), None);

    let mut album = test_album("Linked Artist", None);
    album.album_artist_credits = vec![test_credit(ArtistId::fake(7), "Linked Artist")];
    assert_eq!(
        resolve_album_artist_route(&album),
        Some(Route::ArtistDetail(ArtistId::fake(7)))
    );

    assert_eq!(resolve_album_artist_route(&test_album("", None)), None);
}
#[test]
pub(crate) fn shell_track_option() {
    assert_eq!(
        sorted_artist_track_titles(true),
        vec!["Bravo".to_string(), "Zulu".to_string(), "Alpha".to_string()]
    );
    assert_eq!(
        sorted_artist_track_titles(false),
        vec!["Alpha".to_string(), "Bravo".to_string(), "Zulu".to_string()]
    );
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
    let settings = LibraryListSettings::for_key(LibraryListKey::Tracks);

    sort_tracks(&mut tracks, &settings, favorite_first);

    tracks.into_iter().map(|track| track.title).collect()
}
#[test]
pub(crate) fn shell_ignore_field() {
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
pub(crate) fn shell_lyrics_exist() {
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
pub(crate) fn shell_deferred_lyrics_are_not_labeled_empty() {
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
pub(crate) fn shell_lrclib_empty_result_is_not_loadable() {
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
pub(crate) fn shell_lyrics_text() {
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
pub(crate) fn test_source_presentation() -> sources::SourcePresentationState {
    sources::SourcePresentationState {
        source: None,
        sources: Vec::new(),
        selected_source: None,
        local_folders: Vec::new(),
        source_local_access: Vec::new(),
        local_access: None,
        local_access_status: sources::LocalAccessStatus::default(),
        music_folders: Vec::new(),
        selected_music_folder_id: None,
        first_run: false,
        cache: sources::LibraryCacheState::NoCache { revision: 0 },
    }
}
pub(crate) fn test_server(suffix: &str) -> SourceIdentity {
    SourceIdentity {
        id: SourceId::new(format!("server:{suffix}")),
        kind: "test".to_string(),
        name: format!("Server {suffix}"),
        base_url: "http://localhost".to_string(),
    }
}
pub(crate) fn test_image_ref(suffix: &str) -> ImageRef {
    ImageRef::new(format!("local:cover:file%3A%2F%2F{suffix}"), None)
}
pub(crate) fn test_sequence_entry(title: &str, image_ref: ImageRef) -> SequenceEntry {
    let mut track = test_track("Artist", None);
    track.title = title.to_string();
    track.image_ref = Some(image_ref);
    SequenceEntry {
        occurrence: OccurrenceId::new(format!("queue:{title}")),
        track,
        provenance: Provenance::Manual,
    }
}

pub(crate) fn test_playback_view(
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
pub(crate) fn test_album(artist: &str, artist_id: Option<ArtistId>) -> Album {
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
pub(crate) fn test_track(artist: &str, artist_id: Option<ArtistId>) -> Track {
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

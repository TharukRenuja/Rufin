use crate::{
    LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings, available_sort_fields,
};
use ::library::{
    Album, AlbumId, Playlist, PlaylistId, SmartPlaylist, SmartPlaylistDefinition, SmartPlaylistId,
    SmartPlaylistMatchMode, SmartPlaylistRuleGroup, SmartPlaylistSortField, Track, TrackId,
};
use gtk::prelude::{Cast, ListModelExt};
use std::collections::HashMap;

use super::album_detail::{
    AlbumCollectionModels, AlbumDetailItem, album_detail_items_for, album_detail_track_cells_width,
    album_detail_track_column_width, album_detail_track_field_widths,
    populate_album_collection_model, sort_album_detail_tracks,
};
use super::collections::{
    SMART_PLAYLIST_REORDER_WIDTH, TrackModelIndex, TrackTableSelection,
    capped_library_table_content_height, collection_column_width, library_table_content_height,
};
use super::columns::{track_column_fit_width, track_column_width};
use super::library_fields::{
    album_field, column_width, compact_header_column_width, playlist_field,
    replace_tracks_in_model, smart_playlist_field, sort_tracks, track_field,
};
use super::route_shell::{library_toolbar_end_margin, toolbar_sort_width_for_labels};
use super::table_sizing::fitted_column_widths;
#[test]
fn route_track_visible() {
    assert_eq!(library_table_content_height(0), 150);
    assert_eq!(library_table_content_height(3), 266);
}
#[test]
fn embedded_track_preview_keeps_scrollable_height() {
    assert_eq!(
        capped_library_table_content_height(12, Some(5)),
        library_table_content_height(5)
    );
    assert_eq!(
        capped_library_table_content_height(3, Some(5)),
        library_table_content_height(3)
    );
}

#[test]
fn track_model_replacement_preserves_unchanged_row_identity() {
    let model = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    let first = test_track(1, "first", 1, 1);
    let second = test_track(2, "second", 1, 2);
    let third = test_track(3, "third", 1, 3);
    replace_tracks_in_model(&model, vec![first.clone(), second.clone(), third.clone()]);
    let first_object = model.item(0).expect("first row object");
    let third_object = model.item(2).expect("third row object");
    let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorded = std::rc::Rc::clone(&changes);
    model.connect_items_changed(move |_, position, removed, added| {
        recorded.borrow_mut().push((position, removed, added));
    });

    let mut changed_second = second;
    changed_second.title = "changed".to_string();
    replace_tracks_in_model(&model, vec![first, changed_second, third]);

    assert_eq!(&*changes.borrow(), &[(1, 1, 1)]);
    assert_eq!(model.item(0).as_ref(), Some(&first_object));
    assert_eq!(model.item(2).as_ref(), Some(&third_object));
}

#[test]
fn multi_track_patch_preserves_selection_and_lookup() {
    gtk::init().expect("initialize GTK");
    let model = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    let first = test_track(1, "first", 1, 1);
    let second = test_track(2, "second", 1, 2);
    let third = test_track(3, "third", 1, 3);
    let fourth = test_track(4, "fourth", 1, 4);
    replace_tracks_in_model(
        &model,
        vec![first.clone(), second.clone(), third.clone(), fourth.clone()],
    );
    let first_object = model.item(0).expect("first row object");
    let fourth_object = model.item(3).expect("fourth row object");
    let positions = TrackModelIndex::new(&model);
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let track_selection = TrackTableSelection::new(&selection, positions.clone());
    track_selection.install_guard();
    track_selection.select_now_playing_track(Some(&third.id));
    let changes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorded = std::rc::Rc::clone(&changes);
    model.connect_items_changed(move |_, position, removed, added| {
        recorded.borrow_mut().push((position, removed, added));
    });

    let mut changed_second = second.clone();
    changed_second.play_count = Some(12);
    let mut changed_third = third.clone();
    changed_third.favorite = true;
    positions.replace_existing([changed_second, changed_third]);

    assert_eq!(&*changes.borrow(), &[(1, 2, 2)]);
    assert_eq!(model.item(0).as_ref(), Some(&first_object));
    assert_eq!(model.item(3).as_ref(), Some(&fourth_object));
    assert_eq!(selection.selected(), 2);

    track_selection.select_now_playing_track(Some(&second.id));
    assert_eq!(selection.selected(), 1);
}
#[test]
fn route_fit_pane() {
    let fields = [
        LibraryField::RowIndex,
        LibraryField::TitleMerged,
        LibraryField::Album,
        LibraryField::PlayCount,
    ];
    let smart_width: i32 = fields
        .iter()
        .map(|field| track_column_width(LibraryListKey::SmartPlaylistTracks, *field))
        .sum();
    let regular_width: i32 = fields
        .iter()
        .map(|field| track_column_width(LibraryListKey::PlaylistTracks, *field))
        .sum();

    assert!(smart_width + 32 <= 550);
    assert!(smart_width < regular_width);
}
#[test]
fn toolbar_sort_width_covers_every_route_label() {
    for key in LibraryListKey::all() {
        assert!(
            toolbar_sort_width_for_labels(
                available_sort_fields(key).iter().map(|field| field.title())
            ) >= 112,
            "{key:?}"
        );
    }
}
#[test]
fn route_sort_dropdown_reserves_longest_label() {
    let short = toolbar_sort_width_for_labels(["Title"]);
    let long = toolbar_sort_width_for_labels(["Title", "Number of songs"]);

    assert!(long > short);
    assert_eq!(
        long,
        toolbar_sort_width_for_labels(["Number of songs", "Title"])
    );
}
#[test]
fn route_toolbar_reserves_window_controls_when_queue_hidden() {
    assert_eq!(library_toolbar_end_margin(true), 10);
    assert_eq!(library_toolbar_end_margin(false), 44);
}
#[test]
fn route_shrink_width() {
    let base_widths = [48, 96, 68, 220, 76];
    let fitted = fitted_column_widths(&base_widths, 320);

    assert_eq!(fitted.len(), base_widths.len());
    assert_eq!(fitted.iter().sum::<i32>(), 320);
    assert!(fitted.iter().all(|width| *width > 0));
    assert!(fitted[3] > fitted[0]);
}
#[test]
fn route_allow_space() {
    let base_widths = [48, 96, 68];
    let fitted = fitted_column_widths(&base_widths, 400);

    assert_eq!(fitted.iter().sum::<i32>(), 400);
    assert_eq!(fitted[0], base_widths[0]);
    assert!(fitted[1] > base_widths[1]);
    assert_eq!(fitted[2], base_widths[2]);
}
#[test]
fn route_expands_text_columns() {
    let base_widths = [48, 220, 220, 220, 68, 56];
    let fitted = fitted_column_widths(&base_widths, 1200);

    assert_eq!(fitted.iter().sum::<i32>(), 1200);
    assert_eq!(fitted[0], base_widths[0]);
    assert!(fitted[1] > base_widths[1]);
    assert!(fitted[2] > base_widths[2]);
    assert!(fitted[3] > base_widths[3]);
    assert_eq!(fitted[4], base_widths[4]);
    assert_eq!(fitted[5], base_widths[5]);
}
#[test]
fn collection_aggregate_columns_keep_readable_widths() {
    let base_widths = [
        SMART_PLAYLIST_REORDER_WIDTH,
        column_width(LibraryField::Image),
        220,
        collection_column_width(LibraryField::SongCount),
        collection_column_width(LibraryField::Duration),
    ];
    let fitted = fitted_column_widths(&base_widths, 620);

    assert_eq!(fitted.iter().sum::<i32>(), 620);
    assert!(base_widths[3] >= compact_header_column_width("Number of songs", 96));
    assert!(base_widths[4] >= 128);
    assert!(fitted[3] >= base_widths[3]);
    assert_eq!(fitted[4], base_widths[4]);
}
#[test]
fn genre_track_rows_expand_title_and_album() {
    let settings = LibraryListSettings::for_key(LibraryListKey::GenreTracks);
    let base_widths = settings
        .row_fields
        .iter()
        .map(|field| track_column_width(LibraryListKey::GenreTracks, *field))
        .collect::<Vec<_>>();
    let fitted = fitted_column_widths(&base_widths, 1000);

    assert_eq!(settings.row_fields[0], LibraryField::RowIndex);
    assert_eq!(settings.row_fields[1], LibraryField::TitleMerged);
    assert_eq!(settings.row_fields[2], LibraryField::Album);
    assert_eq!(base_widths[0], 54);
    assert_eq!(base_widths[1], 320);
    assert_eq!(base_widths[2], 260);
    assert!(fitted[1] > base_widths[1]);
    assert!(fitted[2] > base_widths[2]);
    assert_eq!(fitted[0], base_widths[0]);
    assert_eq!(fitted[3], base_widths[3]);
    assert_eq!(fitted[4], base_widths[4]);
}
#[test]
fn merged_title_columns_get_extra_weight() {
    let title = track_column_width(LibraryListKey::Tracks, LibraryField::Title);
    let merged = track_column_width(LibraryListKey::Tracks, LibraryField::TitleMerged);

    assert_eq!(
        track_column_fit_width(LibraryListKey::Tracks, LibraryField::Title),
        title
    );
    assert!(track_column_fit_width(LibraryListKey::Tracks, LibraryField::TitleMerged) > merged);
}
#[test]
fn route_track_area() {
    let fields = [
        LibraryField::TrackNumber,
        LibraryField::Title,
        LibraryField::Duration,
    ];
    let field_widths = album_detail_track_field_widths(LibraryListKey::Albums, &fields, 320);

    assert_eq!(field_widths.len(), fields.len());
    assert!(field_widths.iter().all(|(_, width)| *width > 0));
    assert_eq!(album_detail_track_cells_width(&field_widths), 320);
    assert!(field_widths[1].1 > field_widths[0].1);
    assert_eq!(
        field_widths[0].1,
        album_detail_track_column_width(LibraryListKey::Albums, LibraryField::TrackNumber)
    );
    assert_eq!(
        field_widths[2].1,
        album_detail_track_column_width(LibraryListKey::Albums, LibraryField::Duration)
    );
}
#[test]
fn disc_track_order() {
    let mut tracks = vec![
        test_track(1, "Second", 1, 2),
        test_track(2, "Third", 2, 1),
        test_track(3, "First", 1, 1),
    ];

    sort_album_detail_tracks(&mut tracks);

    let titles = tracks
        .iter()
        .map(|track| track.title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(titles, vec!["First", "Second", "Third"]);
}
#[test]
fn album_header_order() {
    let settings = LibraryListSettings {
        layout: LibraryLayout::Detail,
        ..LibraryListSettings::for_key(LibraryListKey::Albums)
    };
    let album = test_album(1, "A Album");
    let other = test_album(2, "B Album");
    let mut tracks = HashMap::new();
    tracks.insert(
        album.id.clone(),
        vec![
            test_track(1, "Second", 1, 2),
            test_track(2, "First", 1, 1),
            test_track(3, "Fourth", 1, 4),
            test_track(4, "Third", 1, 3),
            test_track(5, "Fifth", 1, 5),
            test_track(6, "Sixth", 1, 6),
        ],
    );

    let rows = album_detail_items_for(&[other, album], &settings, &tracks);

    assert!(matches!(
        &rows[0],
        AlbumDetailItem::Lead {
            album,
            inline_tracks,
            last_in_album: true,
        } if album.title == "A Album"
            && inline_tracks
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>()
                == vec!["First", "Second", "Third", "Fourth", "Fifth", "Sixth"]
    ));
    assert!(matches!(
        &rows[1],
        AlbumDetailItem::Lead {
            album,
            inline_tracks,
            last_in_album: true,
        } if album.title == "B Album" && inline_tracks.is_empty()
    ));
}

#[test]
fn album_layout_models_never_change_item_type() {
    let models = AlbumCollectionModels::new();
    let album = test_album(1, "Album");
    let tracks = HashMap::new();
    let grid_settings = LibraryListSettings {
        layout: LibraryLayout::Grid,
        ..LibraryListSettings::for_key(LibraryListKey::Albums)
    };
    populate_album_collection_model(
        &models,
        std::slice::from_ref(&album),
        &grid_settings,
        &tracks,
    );

    let detail_settings = LibraryListSettings {
        layout: LibraryLayout::Detail,
        ..grid_settings
    };
    populate_album_collection_model(
        &models,
        std::slice::from_ref(&album),
        &detail_settings,
        &tracks,
    );

    let album_item = models
        .albums()
        .item(0)
        .expect("album presentation item")
        .downcast::<gtk::glib::BoxedAnyObject>()
        .expect("boxed album presentation item");
    assert_eq!(album_item.borrow::<Album>().id, album.id);

    let detail_item = models
        .detail()
        .item(0)
        .expect("detail presentation item")
        .downcast::<gtk::glib::BoxedAnyObject>()
        .expect("boxed detail presentation item");
    assert!(matches!(
        &*detail_item.borrow::<AlbumDetailItem>(),
        AlbumDetailItem::Lead { album: row, .. } if row.id == album.id
    ));
}
#[test]
fn route_duration_fields_use_contextual_text() {
    let mut album = test_album(1, "Album");
    let mut playlist = test_playlist(1, "Playlist");
    let mut smart_playlist = test_smart_playlist(1, "Smart Playlist");
    let mut track = test_track(1, "Track", 1, 1);

    album.duration_seconds = 308;
    playlist.duration_seconds = 308;
    smart_playlist.duration_seconds = 308;
    track.duration_seconds = 308;

    assert_eq!(album_field(&album, LibraryField::Duration), "5:08");
    assert_eq!(playlist_field(&playlist, LibraryField::Duration), "5m 8s");
    assert_eq!(
        smart_playlist_field(&smart_playlist, LibraryField::Duration),
        "5m 8s"
    );
    assert_eq!(track_field(&track, LibraryField::Duration), "5:08");
}

#[test]
fn route_bpm_is_blank_when_missing_and_sorts_stably() {
    let mut low = test_track(1, "Same title", 1, 1);
    let mut high_first = test_track(2, "Same title", 1, 1);
    let mut high_second = test_track(3, "Same title", 1, 1);
    let missing = test_track(4, "Same title", 1, 1);
    low.bpm = Some(90);
    high_first.bpm = Some(120);
    high_second.bpm = Some(120);
    let mut settings = LibraryListSettings {
        sort_key: LibraryField::Bpm,
        ..LibraryListSettings::for_key(LibraryListKey::Tracks)
    };
    let mut tracks = vec![
        missing.clone(),
        high_second.clone(),
        high_first.clone(),
        low.clone(),
    ];

    sort_tracks(&mut tracks, &settings, false);

    assert_eq!(
        tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![
            low.id.clone(),
            high_first.id.clone(),
            high_second.id.clone(),
            missing.id.clone(),
        ]
    );
    assert_eq!(track_field(&tracks[0], LibraryField::Bpm), "90");
    assert!(track_field(&tracks[3], LibraryField::Bpm).is_empty());

    settings.descending = true;
    sort_tracks(&mut tracks, &settings, false);
    assert_eq!(
        tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![high_second.id, high_first.id, low.id, missing.id]
    );
}

fn test_track(id: u32, title: &str, disc_number: u16, track_number: u16) -> Track {
    Track {
        id: TrackId::fake(id),
        album_id: AlbumId::fake(1),
        title: title.to_string(),
        artist: "Artist".to_string(),
        artist_id: None,
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
        disc_number,
        track_number,
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
fn test_album(id: u32, title: &str) -> Album {
    Album {
        id: AlbumId::fake(id),
        title: title.to_string(),
        artist: "Artist".to_string(),
        artist_id: None,
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year: 2026,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        track_count: 2,
        duration_seconds: 360,
        favorite: false,
        color_seed: id,
        image_ref: None,
        genres: Vec::new(),
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
    }
}
fn test_playlist(id: u32, name: &str) -> Playlist {
    Playlist {
        id: PlaylistId::fake(id),
        name: name.to_string(),
        owner: None,
        track_count: 1,
        duration_seconds: 180,
        top_genres: Vec::new(),
        image_ref: None,
        representative_albums: Vec::new(),
    }
}
fn test_smart_playlist(id: u32, name: &str) -> SmartPlaylist {
    SmartPlaylist {
        id: SmartPlaylistId::fake(id),
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
        representative_albums: Vec::new(),
    }
}

use rufin_core::{
    Album, AlbumId, ImageRef, LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings,
    Track, TrackId,
};
use std::collections::HashMap;
#[test]
fn library_route_inset_keeps_margins_inside_scrollers() {
    let spec = super::library_route_inset_spec();

    assert_eq!(spec.margin_start, super::PRIMARY_ROUTE_MARGIN_START);
    assert_eq!(spec.margin_end, 0);
    assert!(spec.hexpand);
}
#[test]
fn album_detail_meta_label_has_fixed_pixel_boundary() {
    let spec = super::album_detail_meta_label_spec(168);

    assert_eq!(spec.width, 168);
    assert_eq!(spec.height, super::ALBUM_DETAIL_META_LABEL_HEIGHT);
    assert_eq!(spec.horizontal_policy, gtk::PolicyType::Never);
    assert_eq!(spec.vertical_policy, gtk::PolicyType::Never);
    assert_eq!(spec.overflow, gtk::Overflow::Hidden);
    assert!(!spec.propagate_natural_width);
    assert!(!spec.propagate_natural_height);
    assert!(!spec.wrap);
}
#[test]
fn library_table_height_tracks_visible_rows() {
    assert_eq!(super::library_table_content_height(0), 150);
    assert_eq!(super::library_table_content_height(3), 266);
}
#[test]
fn smart_playlist_default_track_columns_fit_compact_pane() {
    let fields = [
        LibraryField::RowIndex,
        LibraryField::TitleMerged,
        LibraryField::Album,
        LibraryField::PlayCount,
    ];
    let smart_width: i32 = fields
        .iter()
        .map(|field| super::track_column_width(LibraryListKey::SmartPlaylistTracks, *field))
        .sum();
    let regular_width: i32 = fields
        .iter()
        .map(|field| super::track_column_width(LibraryListKey::PlaylistTracks, *field))
        .sum();

    assert!(smart_width + 32 <= 550);
    assert!(smart_width < regular_width);
}
#[test]
fn complete_page_policy_loads_small_library_layouts_fully() {
    let tracks_row = LibraryListSettings {
        layout: LibraryLayout::Row,
        ..LibraryListSettings::for_key(LibraryListKey::Tracks)
    };
    let tracks_grid = LibraryListSettings {
        layout: LibraryLayout::Grid,
        ..LibraryListSettings::for_key(LibraryListKey::Tracks)
    };
    let albums_grid = LibraryListSettings {
        layout: LibraryLayout::Grid,
        ..LibraryListSettings::for_key(LibraryListKey::Albums)
    };
    let albums_detail = LibraryListSettings {
        layout: LibraryLayout::Detail,
        ..LibraryListSettings::for_key(LibraryListKey::Albums)
    };
    let playlists_grid = LibraryListSettings {
        layout: LibraryLayout::Grid,
        ..LibraryListSettings::for_key(LibraryListKey::Playlists)
    };
    let artists_grid = LibraryListSettings {
        layout: LibraryLayout::Grid,
        ..LibraryListSettings::for_key(LibraryListKey::Artists)
    };

    assert!(super::library_layout_loads_complete_page(
        LibraryListKey::Tracks,
        &tracks_row
    ));
    assert!(!super::library_layout_loads_complete_page(
        LibraryListKey::Tracks,
        &tracks_grid
    ));
    assert!(super::library_layout_loads_complete_page(
        LibraryListKey::Albums,
        &albums_grid
    ));
    assert!(super::library_layout_loads_complete_page(
        LibraryListKey::Albums,
        &albums_detail
    ));
    assert!(super::library_layout_loads_complete_page(
        LibraryListKey::Playlists,
        &playlists_grid
    ));
    assert!(super::library_layout_loads_complete_page(
        LibraryListKey::Artists,
        &artists_grid
    ));
}
#[test]
fn album_route_cover_gate_is_detail_only() {
    let albums_row = LibraryListSettings {
        layout: LibraryLayout::Row,
        row_fields: vec![LibraryField::Image, LibraryField::Title],
        ..LibraryListSettings::for_key(LibraryListKey::Albums)
    };
    let albums_grid = LibraryListSettings {
        layout: LibraryLayout::Grid,
        ..LibraryListSettings::for_key(LibraryListKey::Albums)
    };
    let albums_detail = LibraryListSettings {
        layout: LibraryLayout::Detail,
        ..LibraryListSettings::for_key(LibraryListKey::Albums)
    };

    assert!(!super::album_route_cover_gate_enabled(&albums_row));
    assert!(!super::album_route_cover_gate_enabled(&albums_grid));
    assert!(super::album_route_cover_gate_enabled(&albums_detail));
}
#[test]
fn artist_route_cover_gate_is_disabled() {
    let artists_row = LibraryListSettings {
        layout: LibraryLayout::Row,
        row_fields: vec![LibraryField::Image, LibraryField::Title],
        ..LibraryListSettings::for_key(LibraryListKey::Artists)
    };
    let artists_grid = LibraryListSettings {
        layout: LibraryLayout::Grid,
        ..LibraryListSettings::for_key(LibraryListKey::Artists)
    };
    let album_artists_grid = LibraryListSettings {
        layout: LibraryLayout::Grid,
        ..LibraryListSettings::for_key(LibraryListKey::AlbumArtists)
    };

    assert!(!super::artist_route_cover_gate_enabled(&artists_row));
    assert!(!super::artist_route_cover_gate_enabled(&artists_grid));
    assert!(!super::artist_route_cover_gate_enabled(&album_artists_grid));
}
#[test]
fn track_cover_refs_for_settings_include_full_sorted_track_set_once() {
    let settings = LibraryListSettings {
        layout: LibraryLayout::Row,
        ..LibraryListSettings::for_key(LibraryListKey::Tracks)
    };
    let mut duplicate = test_track_with_image(90, "Duplicate", "shared-cover");
    duplicate.track_number = 90;
    let mut duplicate_later = test_track_with_image(91, "Duplicate Later", "shared-cover");
    duplicate_later.track_number = 91;
    let tracks = (0..80)
        .map(|index| {
            test_track_with_image(
                index,
                &format!("Track {index:02}"),
                &format!("cover-{index:02}"),
            )
        })
        .chain(std::iter::once(duplicate))
        .chain(std::iter::once(duplicate_later))
        .collect::<Vec<_>>();

    let refs = super::track_cover_refs_for_settings(&tracks, &settings);

    assert_eq!(refs.len(), 81);
    assert!(refs.iter().any(|image_ref| image_ref.item_id == "cover-79"));
    assert_eq!(
        refs.iter()
            .filter(|image_ref| image_ref.item_id == "shared-cover")
            .count(),
        1
    );
}
#[test]
fn album_detail_tracks_keep_disc_track_order() {
    let mut tracks = vec![
        test_track(1, "Second", 1, 2),
        test_track(2, "Third", 2, 1),
        test_track(3, "First", 1, 1),
    ];

    super::sort_album_detail_tracks(&mut tracks);

    let titles = tracks
        .iter()
        .map(|track| track.title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(titles, vec!["First", "Second", "Third"]);
}
#[test]
fn album_detail_items_keep_album_header_and_track_rows_in_display_order() {
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

    let rows = super::album_detail_items_for(&[other, album], &settings, &tracks);

    assert!(matches!(
        &rows[0],
        super::AlbumDetailItem::Lead {
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
        super::AlbumDetailItem::Lead {
            album,
            inline_tracks,
            last_in_album: true,
        } if album.title == "B Album" && inline_tracks.is_empty()
    ));
}
#[test]
fn album_detail_virtual_range_keeps_boundary_rows_visible() {
    let rows = vec![
        test_virtual_row(0, 100),
        test_virtual_row(100, 100),
        test_virtual_row(200, 100),
    ];

    assert_eq!(
        super::album_detail_virtual_range(&rows, 50.0, 150.0),
        (0, 2)
    );
    assert_eq!(
        super::album_detail_virtual_range(&rows, 101.0, 250.0),
        (1, 3)
    );
}
#[test]
fn complete_cached_page_expands_partial_page_to_total() {
    let page = rufin_provider::PagedResponse::new(vec![1, 2], 5);
    let page = super::complete_cached_page(
        page,
        true,
        |limit| {
            Ok(rufin_provider::PagedResponse::new(
                (0..limit).collect(),
                limit,
            ))
        },
        "numbers",
    );

    assert_eq!(page.items, vec![0, 1, 2, 3, 4]);
    assert_eq!(page.total, 5);
}
#[test]
fn complete_cached_page_leaves_partial_page_when_not_requested() {
    let page = rufin_provider::PagedResponse::new(vec![1, 2], 5);
    let page = super::complete_cached_page(
        page,
        false,
        |_| panic!("incremental layouts should not request the complete page"),
        "numbers",
    );

    assert_eq!(page.items, vec![1, 2]);
    assert_eq!(page.total, 5);
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
        genres: Vec::new(),
        local_path: None,
        source_format: None,
        comment: None,
        skip_count: None,
    }
}
fn test_track_with_image(id: u32, title: &str, image_id: &str) -> Track {
    Track {
        image_ref: Some(ImageRef::new(image_id.to_string(), None)),
        ..test_track(id, title, 1, id as u16)
    }
}
fn test_virtual_row(top: i32, height: i32) -> super::AlbumDetailVirtualRow {
    super::AlbumDetailVirtualRow {
        item: super::AlbumDetailItem::Track {
            track: test_track(top as u32, "Track", 1, 1),
            index: 0,
            last_in_album: false,
        },
        top,
        height,
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
    }
}

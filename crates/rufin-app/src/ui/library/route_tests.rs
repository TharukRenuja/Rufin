use rufin_core::{
    Album, AlbumId, ImageRef, LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings,
    Playlist, PlaylistId, SmartPlaylist, SmartPlaylistDefinition, SmartPlaylistId,
    SmartPlaylistMatchMode, SmartPlaylistRuleGroup, SmartPlaylistSortField, Track, TrackId,
};
use std::collections::HashMap;
#[test]
fn route_track_visible() {
    assert_eq!(super::library_table_content_height(0), 150);
    assert_eq!(super::library_table_content_height(3), 266);
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
fn route_stay_compact() {
    for key in LibraryListKey::all() {
        assert_eq!(
            super::library_toolbar_orientation_for_width(key, 550),
            gtk::Orientation::Horizontal,
            "{key:?}"
        );
        assert_eq!(super::toolbar_sort_width(key, 550), Some(137), "{key:?}");
    }
}
#[test]
fn route_shrink_width() {
    let base_widths = [48, 96, 68, 220, 76];
    let fitted = super::fitted_column_widths(&base_widths, 320);

    assert_eq!(fitted.len(), base_widths.len());
    assert_eq!(fitted.iter().sum::<i32>(), 320);
    assert!(fitted.iter().all(|width| *width > 0));
    assert!(fitted[3] > fitted[0]);
}
#[test]
fn route_allow_space() {
    let base_widths = [48, 96, 68];
    let fitted = super::fitted_column_widths(&base_widths, 400);

    assert_eq!(fitted.iter().sum::<i32>(), 400);
    assert_eq!(fitted[0], base_widths[0]);
    assert!(fitted[1] > base_widths[1]);
    assert_eq!(fitted[2], base_widths[2]);
}
#[test]
fn route_expands_text_columns() {
    let base_widths = [48, 220, 220, 220, 68, 56];
    let fitted = super::fitted_column_widths(&base_widths, 1200);

    assert_eq!(fitted.iter().sum::<i32>(), 1200);
    assert_eq!(fitted[0], base_widths[0]);
    assert!(fitted[1] > base_widths[1]);
    assert!(fitted[2] > base_widths[2]);
    assert!(fitted[3] > base_widths[3]);
    assert_eq!(fitted[4], base_widths[4]);
    assert_eq!(fitted[5], base_widths[5]);
}
#[test]
fn genre_track_rows_expand_title_and_album() {
    let settings = LibraryListSettings::for_key(LibraryListKey::GenreTracks);
    let base_widths = settings
        .row_fields
        .iter()
        .map(|field| super::track_column_width(LibraryListKey::GenreTracks, *field))
        .collect::<Vec<_>>();
    let fitted = super::fitted_column_widths(&base_widths, 1000);

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
fn route_track_area() {
    let fields = [
        LibraryField::RowIndex,
        LibraryField::Title,
        LibraryField::AlbumArtist,
    ];
    let field_widths =
        super::album_detail_track_field_widths(LibraryListKey::ArtistAlbums, &fields, 320);

    assert_eq!(field_widths.len(), fields.len());
    assert!(field_widths.iter().all(|(_, width)| *width > 0));
    assert_eq!(super::album_detail_track_cells_width(&field_widths), 320);
    assert!(field_widths[1].1 > field_widths[0].1);
}
#[test]
fn route_load_fully() {
    for key in LibraryListKey::all() {
        for layout in [
            LibraryLayout::Row,
            LibraryLayout::Grid,
            LibraryLayout::Detail,
        ] {
            if !key.supports_layout(layout) {
                continue;
            }
            let settings = LibraryListSettings {
                layout,
                ..LibraryListSettings::for_key(key)
            };

            assert!(
                super::library_layout_loads_complete_page(key, &settings),
                "{key:?} {layout:?} should not use route pagination"
            );
        }
    }
}

#[test]
fn route_load_fully_for_fallback_layouts() {
    let settings = LibraryListSettings {
        layout: LibraryLayout::Detail,
        ..LibraryListSettings::for_key(LibraryListKey::Tracks)
    };

    assert!(super::library_layout_loads_complete_page(
        LibraryListKey::Tracks,
        &settings
    ));
}

#[test]
fn route_warm_overscan() {
    let ranges =
        super::track_viewport_cover_ranges(2_000, 1_000, 13).expect("track viewport ranges");

    assert_eq!(ranges.priority_start, 1_000);
    assert_eq!(ranges.priority_end, 1_013);
    assert_eq!(ranges.warm_before_start, 984);
    assert_eq!(ranges.warm_before_end, 1_000);
    assert_eq!(ranges.warm_after_start, 1_013);
    assert_eq!(ranges.warm_after_end, 1_045);
}

#[test]
fn track_interaction_viewport() {
    let ranges = super::track_interaction_viewport_cover_ranges(2_000, 1_000, 13)
        .expect("track interaction viewport ranges");

    assert_eq!(ranges.priority_start, 952);
    assert_eq!(ranges.priority_end, 1_109);
    assert_eq!(ranges.warm_before_start, 936);
    assert_eq!(ranges.warm_before_end, 952);
    assert_eq!(ranges.warm_after_start, 1_109);
    assert_eq!(ranges.warm_after_end, 1_141);
}

#[test]
fn album_interaction_viewport() {
    let ranges = super::album_interaction_viewport_cover_ranges(265, 143, 18)
        .expect("album interaction viewport ranges");

    assert_eq!(ranges.priority_start, 119);
    assert_eq!(ranges.priority_end, 209);
    assert_eq!(ranges.warm_before_start, 103);
    assert_eq!(ranges.warm_before_end, 119);
    assert_eq!(ranges.warm_after_start, 209);
    assert_eq!(ranges.warm_after_end, 241);
}

#[test]
fn route_prime_settle() {
    let ranges = super::viewport_cover_ranges(
        129,
        45,
        15,
        3 * super::GRID_INTERACTION_BEHIND,
        3 * super::GRID_INTERACTION_AHEAD,
        6,
        18,
    )
    .expect("grid interaction viewport ranges");

    assert_eq!(ranges.priority_start, 21);
    assert_eq!(ranges.priority_end, 129);
    assert_eq!(ranges.warm_before_start, 15);
    assert_eq!(ranges.warm_before_end, 21);
    assert_eq!(ranges.warm_after_start, 129);
    assert_eq!(ranges.warm_after_end, 129);
}
#[test]
fn route_clip_bounds() {
    let ranges = super::track_viewport_cover_ranges(50, 45, 20).expect("bounded track ranges");

    assert_eq!(ranges.priority_start, 30);
    assert_eq!(ranges.priority_end, 50);
    assert_eq!(ranges.warm_before_start, 14);
    assert_eq!(ranges.warm_before_end, 30);
    assert_eq!(ranges.warm_after_start, 50);
    assert_eq!(ranges.warm_after_end, 50);
    assert!(super::track_viewport_cover_ranges(0, 0, 20).is_none());
}
#[test]
fn route_use_stale() {
    assert_eq!(super::viewport_page_size(1.0, 1_044, 900), 1_044.0);
    assert_eq!(super::viewport_page_size(760.0, 200, 600), 760.0);
}
#[test]
fn route_prioritize_visible() {
    let ranges = super::album_viewport_cover_ranges(300, 252, 13).expect("album viewport ranges");

    assert_eq!(ranges.priority_start, 252);
    assert_eq!(ranges.priority_end, 265);
    assert_eq!(ranges.warm_before_start, 236);
    assert_eq!(ranges.warm_before_end, 252);
    assert_eq!(ranges.warm_after_start, 265);
    assert_eq!(ranges.warm_after_end, 297);
}
#[test]
fn route_prioritize_overscan() {
    let ranges = super::TrackViewportCoverRanges {
        visible_start: 11,
        visible_end: 12,
        priority_start: 10,
        priority_end: 13,
        warm_before_start: 8,
        warm_before_end: 10,
        warm_after_start: 13,
        warm_after_end: 15,
    };

    let batches = super::viewport_cover_batches(ranges, |start, end| {
        (start..end)
            .map(|index| ImageRef::new(format!("cover-{index}"), None))
            .collect::<Vec<_>>()
    });
    let priority_ids = batches
        .priority_refs
        .iter()
        .map(|image_ref| image_ref.item_id.as_str())
        .collect::<Vec<_>>();
    let warm_ids = batches
        .warm_refs
        .iter()
        .map(|image_ref| image_ref.item_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(batches.visible_priority_len, 1);
    assert_eq!(priority_ids, vec!["cover-11", "cover-10", "cover-12"]);
    assert_eq!(warm_ids, vec!["cover-8", "cover-9", "cover-13", "cover-14"]);
}

#[test]
fn route_warm_lane() {
    let batches = super::ViewportCoverRefBatches {
        visible_priority_len: 3,
        priority_refs: (0..5)
            .map(|index| ImageRef::new(format!("priority-{index}"), None))
            .collect(),
        warm_refs: (0..2)
            .map(|index| ImageRef::new(format!("warm-{index}"), None))
            .collect(),
    };

    let (batches, overflowed) = super::cap_viewport_priority_cover_refs(batches, 3);
    let priority_ids = batches
        .priority_refs
        .iter()
        .map(|image_ref| image_ref.item_id.as_str())
        .collect::<Vec<_>>();
    let warm_ids = batches
        .warm_refs
        .iter()
        .map(|image_ref| image_ref.item_id.as_str())
        .collect::<Vec<_>>();

    assert!(overflowed);
    assert_eq!(priority_ids, vec!["priority-0", "priority-1", "priority-2"]);
    assert_eq!(
        warm_ids,
        vec!["priority-3", "priority-4", "warm-0", "warm-1"]
    );
}

#[test]
fn route_keep_priority() {
    let batches = super::ViewportCoverRefBatches {
        visible_priority_len: 5,
        priority_refs: (0..8)
            .map(|index| ImageRef::new(format!("priority-{index}"), None))
            .collect(),
        warm_refs: vec![ImageRef::new("warm-0", None)],
    };

    let (batches, overflowed) = super::cap_viewport_priority_cover_refs(batches, 3);
    let priority_ids = batches
        .priority_refs
        .iter()
        .map(|image_ref| image_ref.item_id.as_str())
        .collect::<Vec<_>>();
    let warm_ids = batches
        .warm_refs
        .iter()
        .map(|image_ref| image_ref.item_id.as_str())
        .collect::<Vec<_>>();

    assert!(overflowed);
    assert_eq!(
        priority_ids,
        vec![
            "priority-0",
            "priority-1",
            "priority-2",
            "priority-3",
            "priority-4"
        ]
    );
    assert_eq!(
        warm_ids,
        vec!["priority-5", "priority-6", "priority-7", "warm-0"]
    );
}
#[test]
fn playlist_viewport_cover() {
    let first = ImageRef::new("playlist-first", None);
    let second = ImageRef::new("playlist-second", None);
    let fallback = ImageRef::new("playlist-fallback", None);
    let model = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    model.append(&gtk::glib::BoxedAnyObject::new(test_playlist_with_refs(
        1,
        "Regular",
        vec![first.clone(), second.clone(), first.clone()],
        Some(fallback.clone()),
    )));

    let refs = super::playlist_cover_refs(&model, 0, 1);

    assert_eq!(refs, vec![first, second, fallback]);
}

#[test]
fn smart_playlist_viewport() {
    let first = ImageRef::new("smart-first", None);
    let second = ImageRef::new("smart-second", None);
    let fallback = ImageRef::new("smart-fallback", None);
    let model = gtk::gio::ListStore::new::<gtk::glib::BoxedAnyObject>();
    model.append(&gtk::glib::BoxedAnyObject::new(
        test_smart_playlist_with_refs(
            1,
            "Smart",
            vec![first.clone(), second.clone(), first.clone()],
            Some(fallback.clone()),
        ),
    ));

    let refs = super::smart_cover_refs(&model, 0, 1);

    assert_eq!(refs, vec![first, second, fallback]);
}

#[test]
fn route_match_behavior() {
    let playlist_ranges =
        super::viewport_cover_ranges(120, 24, 12, 0, 0, 6, 18).expect("playlist ranges");
    let genre_ranges =
        super::viewport_cover_ranges(120, 24, 12, 0, 0, 6, 18).expect("genre ranges");

    assert_eq!(playlist_ranges, genre_ranges);
}

#[test]
fn route_track_set() {
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
fn disc_track_order() {
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
fn route_keep_visible() {
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
fn route_expand_total() {
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
fn route_cache_page() {
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
fn test_playlist_with_refs(
    id: u32,
    name: &str,
    image_refs: Vec<ImageRef>,
    image_ref: Option<ImageRef>,
) -> Playlist {
    Playlist {
        id: PlaylistId::fake(id),
        name: name.to_string(),
        track_count: 1,
        duration_seconds: 180,
        image_refs,
        image_ref,
    }
}
fn test_smart_playlist_with_refs(
    id: u32,
    name: &str,
    image_refs: Vec<ImageRef>,
    image_ref: Option<ImageRef>,
) -> SmartPlaylist {
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
        image_refs,
        image_ref,
    }
}

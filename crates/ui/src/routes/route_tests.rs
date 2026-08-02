use crate::{LibraryField, LibraryListKey, LibraryListSettings, available_sort_fields};
use ::library::{
    AcceptedPlay, SmartPlaylistDefinition, SmartPlaylistRule, SmartPlaylistRuleField,
    SmartPlaylistRuleOperator, SmartPlaylistRuleValue, SmartPlaylistSortField, SourceId,
    SourceLibraryUpdate, Track, TrackId, TrackSort,
};
use gtk::prelude::ListModelExt;
use playback::{QueueOrigin, QueuePlacement, SourceSessionEpoch};

use super::album_detail::{
    album_detail_track_cells_width, album_detail_track_column_width,
    album_detail_track_field_widths,
};
use super::collections::{
    SMART_PLAYLIST_REORDER_WIDTH, capped_library_table_content_height, collection_column_width,
    library_table_content_height,
};
use super::columns::{track_column_fit_width, track_column_width};
use super::library_fields::{
    album_field, column_width, compact_header_column_width, item_at, playlist_field,
    smart_playlist_field, sort_tracks, track_field,
};
use super::route_shell::{library_toolbar_end_margin, toolbar_sort_width_for_labels};
use super::table_sizing::fitted_column_widths;
use super::track_model::{TrackCollectionModel, prepare_track_projection};
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
fn track_collection_model_is_complete_and_materializes_only_bound_rows() {
    const TRACK_COUNT: usize = 2_297;
    let source_id = SourceId::fake(1);
    let loaded = crate::test_support::loaded_source(
        source_id.clone(),
        Vec::new(),
        (0..TRACK_COUNT)
            .map(|index| {
                test_track(
                    index + 1,
                    format!("Track {index:04}"),
                    1,
                    u16::try_from(index + 1).expect("test Track number fits u16"),
                )
            })
            .collect(),
        Vec::new(),
    );
    let tracks = loaded
        .track_list(None, TrackSort::Title, false)
        .expect("complete Track list");
    let model = TrackCollectionModel::new(
        source_id,
        SourceSessionEpoch::new(1),
        tracks,
        LibraryListSettings::for_key(LibraryListKey::Tracks),
    );

    assert_eq!(model.n_items(), TRACK_COUNT as u32);
    assert_eq!(
        model.test_stats(),
        super::track_model::TrackModelTestStats {
            order_rebuilds: 1,
            point_updates: 0,
            point_order_slot_copies: 0,
            point_notified_items: 0,
            row_materializations: 0,
            live_rows: 0,
        }
    );

    let first = model.item(0).expect("first row");
    let middle = model.item((TRACK_COUNT / 2) as u32).expect("middle row");
    let last = model.item((TRACK_COUNT - 1) as u32).expect("last row");
    assert!(model.item(TRACK_COUNT as u32).is_none());
    assert_eq!(model.item(0).as_ref(), Some(&first));
    assert_eq!(model.test_stats().live_rows, 3);
    assert_eq!(model.test_stats().row_materializations, 3);

    drop((first, middle, last));
    assert_eq!(model.test_stats().live_rows, 0);
}

#[test]
fn track_collection_model_search_sort_and_positions_share_one_order() {
    let mut gamma = test_track(1, "Gamma", 1, 3);
    gamma.artist = "Needle Artist".to_string();
    let mut alpha = test_track(2, "Alpha", 1, 1);
    alpha.album = "Needle Album".to_string();
    let mut beta = test_track(3, "Beta", 1, 2);
    beta.year = 1999;
    let source_id = SourceId::fake(2);
    let loaded = crate::test_support::loaded_source(
        source_id.clone(),
        Vec::new(),
        vec![gamma.clone(), alpha.clone(), beta.clone()],
        Vec::new(),
    );
    let tracks = loaded
        .track_list(None, TrackSort::Title, false)
        .expect("complete Track list");
    let mut settings = LibraryListSettings::for_key(LibraryListKey::Tracks);
    let model = TrackCollectionModel::new(
        source_id,
        SourceSessionEpoch::new(1),
        tracks,
        settings.clone(),
    );

    assert_eq!(
        displayed_track_ids(&model),
        vec![alpha.id.clone(), beta.id.clone(), gamma.id.clone()]
    );
    model.set_query("  NEEDLE ARTIST  ");
    assert_eq!(model.query(), "NEEDLE ARTIST");
    assert_eq!(displayed_track_ids(&model), vec![gamma.id.clone()]);
    assert_eq!(model.position(&beta.id), None);
    model.set_query("needle album");
    assert_eq!(displayed_track_ids(&model), vec![alpha.id.clone()]);
    model.set_query("1999");
    assert_eq!(displayed_track_ids(&model), vec![beta.id.clone()]);
    let rebuilds = model.test_stats().order_rebuilds;
    assert!(!model.set_query("1999"));
    assert_eq!(model.test_stats().order_rebuilds, rebuilds);

    model.set_query("");
    assert_eq!(model.n_items(), 3);
    settings.descending = true;
    model.apply_settings(settings);
    assert_eq!(
        displayed_track_ids(&model),
        vec![gamma.id.clone(), beta.id.clone(), alpha.id.clone()]
    );
    assert_eq!(model.test_stats().order_rebuilds, 6);
}

#[test]
fn history_playback_keeps_the_selected_repeated_occurrence() {
    let source_id = SourceId::fake("history-repeated");
    let track = test_track(1, "Feather", 1, 1);
    let fixture = crate::test_support::source_fixture(
        source_id.clone(),
        Vec::new(),
        vec![track.clone()],
        Vec::new(),
    );
    for (play_id, played_at) in [("older", 1_700_000_000), ("newer", 1_700_000_100)] {
        let activity = fixture
            .library
            .record_play(
                &fixture.loaded,
                AcceptedPlay {
                    play_id: play_id.to_string(),
                    track_id: track.id.clone(),
                    played_at,
                    month: "2023-11".to_string(),
                },
            )
            .expect("record History play")
            .expect("new History play");
        fixture
            .library
            .apply_recorded_activity(&fixture.loaded, &activity)
            .expect("apply History play")
            .expect("History changed");
    }
    let settings = LibraryListSettings::for_key(LibraryListKey::History);
    let model = TrackCollectionModel::new(
        source_id,
        SourceSessionEpoch::new(1),
        fixture
            .loaded
            .history_track_list(None)
            .expect("History Tracks"),
        settings.clone(),
    );

    assert_eq!(model.played_at(0), Some(1_700_000_100));
    assert_eq!(model.played_at(1), Some(1_700_000_000));
    assert_eq!(model.n_items(), 2);
    let mut ascending = settings;
    ascending.descending = false;
    assert!(model.apply_settings(ascending));
    assert_eq!(model.played_at(0), Some(1_700_000_000));
    assert_eq!(model.played_at(1), Some(1_700_000_100));
    assert!(model.set_query("missing"));
    assert_eq!(model.n_items(), 0);
    assert!(model.set_query("Feather"));
    assert_eq!(model.n_items(), 2);
    assert_eq!(model.position_for_current(&track.id, Some(1)), Some(1));
    assert_eq!(model.position_for_current(&track.id, Some(99)), Some(0));
}

#[test]
fn prepared_track_projection_requires_the_same_query_and_settings() {
    let mut matching = test_track(1, "Alpha", 1, 1);
    matching.artist = "Needle Artist".to_string();
    let other = test_track(2, "Beta", 1, 2);
    let source_id = SourceId::fake(20);
    let loaded = crate::test_support::loaded_source(
        source_id.clone(),
        Vec::new(),
        vec![matching.clone(), other],
        Vec::new(),
    );
    let source = loaded
        .track_list(None, TrackSort::Title, false)
        .expect("complete Track list");
    let model = TrackCollectionModel::new(
        source_id,
        SourceSessionEpoch::new(1),
        source.clone(),
        LibraryListSettings::for_key(LibraryListKey::Tracks),
    );

    assert!(model.set_query("needle"));
    let stale = prepare_track_projection(source.clone(), model.projection_request())
        .expect("prepare Track projection");
    assert!(model.set_query("other"));
    assert!(!model.replace_prepared(stale));
    assert_eq!(model.query(), "other");

    let prepared = prepare_track_projection(source, model.projection_request())
        .expect("prepare current Track projection");
    let rebuilds = model.test_stats().order_rebuilds;
    assert!(model.replace_prepared(prepared));
    assert_eq!(model.test_stats().order_rebuilds, rebuilds + 1);
    assert_eq!(displayed_track_ids(&model), Vec::<TrackId>::new());
}

#[test]
fn track_collection_model_refreshes_values_and_replaces_membership() {
    use std::{cell::Cell, rc::Rc};

    let first = test_track(1, "Alpha", 1, 1);
    let second = test_track(2, "Beta", 1, 2);
    let third = test_track(3, "Gamma", 1, 3);
    let source_id = SourceId::fake(3);
    let fixture = crate::test_support::source_fixture(
        source_id.clone(),
        Vec::new(),
        vec![first.clone(), second.clone(), third.clone()],
        Vec::new(),
    );
    let model = TrackCollectionModel::new(
        source_id.clone(),
        SourceSessionEpoch::new(1),
        fixture
            .loaded
            .track_list(None, TrackSort::Title, false)
            .expect("complete Track list"),
        LibraryListSettings::for_key(LibraryListKey::Tracks),
    );
    let mut changed_second = second.clone();
    changed_second.favorite = true;
    let retained_first = model.item(0).expect("bound first row");
    let retained_second = model.item(1).expect("bound changed row");
    let translated_position = Rc::new(Cell::new(None));
    let translated = Rc::clone(&translated_position);
    let changed_id = second.id.clone();
    model.connect_items_changed(move |model, _, _, _| {
        translated.set(model.selection_position_after_point_change(&changed_id, 1));
    });
    let accepted = fixture
        .library
        .accept_source_update(
            &fixture.loaded,
            SourceLibraryUpdate {
                tracks: vec![changed_second.clone()],
                ..SourceLibraryUpdate::default()
            },
        )
        .expect("replace Track value")
        .expect("changed Track value");
    let before = model.test_stats();
    assert!(model.apply_track_replacement(&accepted.tracks, |_| true));
    let after = model.test_stats();
    assert_eq!(after.order_rebuilds, before.order_rebuilds);
    assert_eq!(after.point_updates, before.point_updates + 1);
    assert_eq!(
        after.point_order_slot_copies,
        before.point_order_slot_copies
    );
    assert_eq!(after.point_notified_items, before.point_notified_items + 1);
    assert_eq!(after.live_rows, 1);
    assert_eq!(translated_position.get(), Some(1));
    assert_eq!(
        model.track_at(model.position(&second.id).expect("changed Track position")),
        Some(changed_second)
    );

    let mut favorite_settings = LibraryListSettings::for_key(LibraryListKey::Tracks);
    favorite_settings.sort_key = LibraryField::Favorite;
    model.apply_settings(favorite_settings);
    assert_eq!(
        displayed_track_ids(&model),
        vec![first.id.clone(), third.id.clone(), second.id.clone()]
    );

    let added = test_track(4, "Delta", 1, 4);
    let replacement = crate::test_support::loaded_source(
        source_id,
        Vec::new(),
        vec![first.clone(), second.clone(), added.clone()],
        Vec::new(),
    );
    let prepared = prepare_track_projection(
        replacement
            .track_list(None, TrackSort::Title, false)
            .expect("replacement Track list"),
        model.projection_request(),
    )
    .expect("prepare replacement Track projection");
    assert!(model.replace_prepared(prepared));
    assert_eq!(model.n_items(), 3);
    assert_eq!(
        displayed_track_ids(&model),
        vec![first.id.clone(), second.id.clone(), added.id.clone()]
    );
    assert_eq!(model.position(&third.id), None);
    drop((retained_first, retained_second));
}

#[test]
fn favorite_track_model_inserts_and_removes_one_accepted_track() {
    let first = test_track(1, "Alpha", 1, 1);
    let mut second = test_track(2, "Beta", 1, 2);
    second.favorite = true;
    let source_id = SourceId::fake(32);
    let fixture = crate::test_support::source_fixture(
        source_id.clone(),
        Vec::new(),
        vec![first.clone(), second.clone()],
        Vec::new(),
    );
    let model = TrackCollectionModel::new(
        source_id,
        SourceSessionEpoch::new(1),
        fixture
            .loaded
            .favorite_track_list(None, TrackSort::Title, false)
            .expect("favorite Track list"),
        LibraryListSettings::for_key(LibraryListKey::FavoriteTracks),
    );
    assert_eq!(displayed_track_ids(&model), vec![second.id.clone()]);

    let mut favorite_first = first.clone();
    favorite_first.favorite = true;
    let accepted = fixture
        .library
        .accept_source_update(
            &fixture.loaded,
            SourceLibraryUpdate {
                tracks: vec![favorite_first],
                ..SourceLibraryUpdate::default()
            },
        )
        .expect("favorite Track")
        .expect("changed favorite");
    let before_insert = model.test_stats();
    assert!(model.apply_track_replacement(&accepted.tracks, |track| track.favorite));
    let after_insert = model.test_stats();
    assert_eq!(
        displayed_track_ids(&model),
        vec![first.id.clone(), second.id.clone()]
    );
    assert_eq!(
        after_insert.point_order_slot_copies,
        before_insert.point_order_slot_copies + 1
    );
    assert_eq!(
        after_insert.point_notified_items,
        before_insert.point_notified_items + 1
    );

    let mut unfavorite_second = second.clone();
    unfavorite_second.favorite = false;
    let accepted = fixture
        .library
        .accept_source_update(
            &fixture.loaded,
            SourceLibraryUpdate {
                tracks: vec![unfavorite_second],
                ..SourceLibraryUpdate::default()
            },
        )
        .expect("unfavorite Track")
        .expect("changed favorite");
    let before_remove = model.test_stats();
    assert!(model.apply_track_replacement(&accepted.tracks, |track| track.favorite));
    let after_remove = model.test_stats();
    assert_eq!(displayed_track_ids(&model), vec![first.id.clone()]);
    assert_eq!(
        after_remove.point_order_slot_copies,
        before_remove.point_order_slot_copies + 2
    );
    assert_eq!(
        after_remove.point_notified_items,
        before_remove.point_notified_items + 1
    );
}

#[test]
fn smart_playlist_member_row_refreshes_without_membership_reevaluation() {
    let source_id = SourceId::fake(31);
    let track = test_track(1, "Matching Track", 1, 1);
    let fixture = crate::test_support::source_fixture(
        source_id.clone(),
        Vec::new(),
        vec![track.clone()],
        Vec::new(),
    );
    let created = fixture
        .library
        .create_smart_playlist(
            &fixture.loaded,
            "Matching".to_string(),
            SmartPlaylistDefinition {
                match_all: vec![SmartPlaylistRule {
                    field: SmartPlaylistRuleField::Title,
                    operator: SmartPlaylistRuleOperator::Contains,
                    value: Some(SmartPlaylistRuleValue::Text("Matching".to_string())),
                }],
                match_any: Vec::new(),
                sort_field: SmartPlaylistSortField::Title,
                descending: false,
                limit: None,
            },
        )
        .expect("create smart Playlist")
        .expect("created smart Playlist");
    let created = created.smart_playlists;
    let smart_playlist_id = created.first().expect("created smart Playlist ID").clone();
    let detail = fixture
        .loaded
        .smart_playlist_detail(&smart_playlist_id, None)
        .expect("read smart Playlist")
        .expect("smart Playlist");
    let model = TrackCollectionModel::new(
        source_id,
        SourceSessionEpoch::new(1),
        detail.tracks.clone(),
        LibraryListSettings::for_key(LibraryListKey::SmartPlaylistTracks),
    );

    let activity = fixture
        .library
        .record_play(
            &fixture.loaded,
            AcceptedPlay {
                play_id: "smart-row-play".to_string(),
                track_id: track.id.clone(),
                played_at: 1_700_000_000,
                month: "2023-11".to_string(),
            },
        )
        .expect("record play")
        .expect("new play");
    let accepted = fixture
        .library
        .apply_recorded_activity(&fixture.loaded, &activity)
        .expect("apply play")
        .expect("changed activity");

    assert!(!accepted.smart_playlists.contains(&smart_playlist_id));
    assert!(model.apply_track_replacement(&accepted.tracks, |_| true));
    assert_eq!(
        model
            .track_at(0)
            .expect("updated smart Playlist Track")
            .play_count,
        Some(1)
    );
}

#[test]
fn track_collection_exact_query_changes_drop_shifted_row_cache_entries() {
    let mut first = test_track(1, "Alpha", 1, 1);
    first.artist = "Other".to_string();
    let mut second = test_track(2, "Charlie", 1, 2);
    second.artist = "Needle".to_string();
    let mut third = test_track(3, "Delta", 1, 3);
    third.artist = "Needle".to_string();
    let fixture = crate::test_support::source_fixture(
        SourceId::fake(4),
        Vec::new(),
        vec![first.clone(), second.clone(), third.clone()],
        Vec::new(),
    );
    let model = TrackCollectionModel::new(
        SourceId::fake(4),
        SourceSessionEpoch::new(1),
        fixture
            .loaded
            .track_list(None, TrackSort::Title, false)
            .expect("complete Track list"),
        LibraryListSettings::for_key(LibraryListKey::Tracks),
    );
    model.set_query("Needle");
    let retained_last = model.item(1).expect("retain last query row");

    let mut inserted = first.clone();
    inserted.artist = "Needle".to_string();
    let accepted = fixture
        .library
        .accept_source_update(
            &fixture.loaded,
            SourceLibraryUpdate {
                tracks: vec![inserted.clone()],
                ..SourceLibraryUpdate::default()
            },
        )
        .expect("insert Track into query")
        .expect("changed Track");
    assert!(model.apply_track_replacement(&accepted.tracks, |_| true));
    assert_eq!(
        item_at::<Track>(&model, 1).map(|track| track.id.clone()),
        Some(second.id.clone())
    );

    let retained_middle = model.item(1).expect("retain middle query row");
    let mut removed = inserted;
    removed.artist = "Other".to_string();
    let accepted = fixture
        .library
        .accept_source_update(
            &fixture.loaded,
            SourceLibraryUpdate {
                tracks: vec![removed],
                ..SourceLibraryUpdate::default()
            },
        )
        .expect("remove Track from query")
        .expect("changed Track");
    assert!(model.apply_track_replacement(&accepted.tracks, |_| true));
    assert_eq!(
        item_at::<Track>(&model, 1).map(|track| track.id.clone()),
        Some(third.id.clone())
    );
    drop((retained_last, retained_middle));
}

#[test]
fn track_collection_playback_uses_canonical_or_visible_order_once() {
    let first = test_track(1, "Alpha", 1, 1);
    let second = test_track(2, "Beta", 1, 2);
    let third = test_track(3, "Gamma", 1, 3);
    let source_id = SourceId::fake(5);
    let loaded = crate::test_support::loaded_source(
        source_id.clone(),
        Vec::new(),
        vec![third.clone(), first.clone(), second.clone()],
        Vec::new(),
    );
    let mut settings = LibraryListSettings::for_key(LibraryListKey::Tracks);
    settings.descending = true;
    let model = TrackCollectionModel::new(
        source_id.clone(),
        SourceSessionEpoch::new(7),
        loaded
            .track_list(None, TrackSort::Title, false)
            .expect("canonical Track list"),
        settings,
    );

    let visible = model
        .play_request(1, QueuePlacement::Now, "tracks", false)
        .expect("visible playback request");
    assert_eq!(visible.source_id, source_id);
    assert_eq!(visible.anchor_index, 1);
    assert_eq!(visible.placement, QueuePlacement::Now);
    assert!(matches!(visible.origin, QueueOrigin::Context(_)));
    assert!(matches!(
        &visible.tracks,
        playback::LoadedTrackSelection::Shallow(_)
    ));
    let visible_tracks = visible.tracks.materialize().expect("visible Track order");
    assert_eq!(
        visible_tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![third.id.clone(), second.id.clone(), first.id.clone()]
    );

    let canonical = model
        .source_play_request(QueuePlacement::Next, "tracks", false)
        .expect("canonical playback request");
    assert_eq!(canonical.placement, QueuePlacement::Next);
    assert!(matches!(
        &canonical.tracks,
        playback::LoadedTrackSelection::Shallow(_)
    ));
    let canonical_tracks = canonical
        .tracks
        .materialize()
        .expect("canonical Track order");
    assert_eq!(
        canonical_tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![first.id.clone(), second.id.clone(), third.id.clone()]
    );

    let mut changed_settings = LibraryListSettings::for_key(LibraryListKey::Tracks);
    changed_settings.sort_key = LibraryField::TrackNumber;
    changed_settings.descending = true;
    assert!(model.apply_settings(changed_settings));
    let canonical_after_sort = model
        .source_play_request(QueuePlacement::Next, "tracks", false)
        .expect("canonical playback request after visible sort");
    let canonical_after_sort_tracks = canonical_after_sort
        .tracks
        .materialize()
        .expect("canonical Track order after sort");
    assert_eq!(
        canonical_after_sort_tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![first.id.clone(), second.id.clone(), third.id.clone()]
    );
    assert_eq!(model.test_stats().row_materializations, 0);
}

#[test]
fn accepted_activity_repositions_exact_track_without_a_complete_rebuild() {
    use std::{cell::Cell, rc::Rc};

    let first = test_track(1, "Alpha", 1, 1);
    let second = test_track(2, "Beta", 1, 2);
    let third = test_track(3, "Gamma", 1, 3);
    let source_id = SourceId::new("local:ui-activity");
    let fixture = crate::test_support::source_fixture(
        source_id.clone(),
        Vec::new(),
        vec![first.clone(), second.clone(), third.clone()],
        Vec::new(),
    );
    let models = [LibraryField::PlayCount, LibraryField::LastPlayed].map(|field| {
        let mut settings = LibraryListSettings::for_key(LibraryListKey::Tracks);
        settings.sort_key = field;
        settings.descending = true;
        let sort = match field {
            LibraryField::PlayCount => TrackSort::PlayCount,
            LibraryField::LastPlayed => TrackSort::LastPlayed,
            _ => unreachable!("activity test fields are fixed"),
        };
        (
            TrackCollectionModel::new(
                source_id.clone(),
                SourceSessionEpoch::new(1),
                fixture
                    .loaded
                    .track_list(None, sort, true)
                    .expect("complete Track list"),
                settings,
            ),
            sort,
        )
    });
    let activity = fixture
        .library
        .record_play(
            &fixture.loaded,
            AcceptedPlay {
                play_id: "ui-activity-play".to_string(),
                track_id: first.id.clone(),
                played_at: 1_700_000_000,
                month: "2023-11".to_string(),
            },
        )
        .expect("record accepted activity")
        .expect("new accepted activity");
    let accepted = fixture
        .library
        .apply_recorded_activity(&fixture.loaded, &activity)
        .expect("apply accepted activity")
        .expect("changed accepted activity");

    for (model, sort) in models {
        let selected_before = model
            .position(&second.id)
            .expect("unchanged selected Track is visible");
        let translated_position = Rc::new(Cell::new(None));
        let translated = Rc::clone(&translated_position);
        let selected_id = second.id.clone();
        model.connect_items_changed(move |model, _, _, _| {
            translated
                .set(model.selection_position_after_point_change(&selected_id, selected_before));
        });
        let before = model.test_stats();
        assert!(model.apply_track_replacement(&accepted.tracks, |_| true));
        let after = model.test_stats();
        assert_eq!(after.order_rebuilds, before.order_rebuilds);
        assert_eq!(after.point_updates, before.point_updates + 1);
        assert_eq!(
            after.point_order_slot_copies,
            before.point_order_slot_copies + 3
        );
        assert_eq!(after.point_notified_items, before.point_notified_items + 3);
        assert_eq!(
            translated_position.get(),
            model.position(&second.id),
            "the exact point notification translates an unchanged selection"
        );
        let expected = fixture
            .loaded
            .track_list(None, sort, true)
            .expect("fresh activity projection")
            .materialize()
            .expect("materialize fresh activity projection")
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(displayed_track_ids(&model), expected);
        let source_order = model
            .source_play_request(QueuePlacement::Now, "activity-test", false)
            .expect("source play request")
            .tracks
            .materialize()
            .expect("materialize source order")
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(source_order, expected);
    }
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
fn album_detail_duration_starts_compact() {
    assert_eq!(
        track_column_width(LibraryListKey::AlbumDetailTracks, LibraryField::Duration),
        album_detail_track_column_width(LibraryListKey::Albums, LibraryField::Duration)
    );
    assert!(
        track_column_width(LibraryListKey::AlbumDetailTracks, LibraryField::Duration)
            < track_column_width(LibraryListKey::Tracks, LibraryField::Duration)
    );
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
fn album_detail_keeps_canonical_disc_and_track_order() {
    let album = crate::test_support::album(1, "A Album");
    let mut second_disc = test_track(1, "Disc two", 2, 1);
    let mut second = test_track(2, "Second", 1, 2);
    let mut first = test_track(3, "First", 1, 1);
    for track in [&mut second_disc, &mut second, &mut first] {
        track.album_id = Some(album.id.clone());
        track.album = album.title.clone();
    }
    let loaded = crate::test_support::loaded_source(
        SourceId::fake(6),
        vec![album.clone()],
        vec![second_disc.clone(), second.clone(), first.clone()],
        Vec::new(),
    );
    let detail = loaded
        .album_detail(&album.id, None)
        .expect("read Album detail")
        .expect("Album detail");

    assert_eq!(
        detail
            .tracks
            .materialize()
            .expect("materialize Album order")
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![first.id.clone(), second.id.clone(), second_disc.id.clone(),]
    );
}
#[test]
fn route_duration_fields_use_contextual_text() {
    let album = crate::test_support::album_summary(crate::test_support::album(1, "Album"), 1, 308);
    let playlist =
        crate::test_support::playlist_summary(crate::test_support::playlist(1, "Playlist"), 1, 308);
    let smart_playlist = crate::test_support::smart_playlist_summary(
        crate::test_support::smart_playlist(1, "Smart Playlist"),
        1,
        308,
    );
    let mut track = test_track(1, "Track", 1, 1);

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

    sort_tracks(&mut tracks, &settings);

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
    sort_tracks(&mut tracks, &settings);
    assert_eq!(
        tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>(),
        vec![
            high_second.id.clone(),
            high_first.id.clone(),
            low.id.clone(),
            missing.id.clone(),
        ]
    );
}

fn displayed_track_ids(model: &TrackCollectionModel) -> Vec<TrackId> {
    (0..model.n_items())
        .map(|position| {
            model
                .track_at(position)
                .expect("displayed Track")
                .id
                .clone()
        })
        .collect()
}

fn test_track(
    id: impl std::fmt::Display,
    title: impl Into<String>,
    disc_number: u16,
    track_number: u16,
) -> Track {
    let mut track = crate::test_support::track(id, title);
    track.disc_number = disc_number;
    track.track_number = track_number;
    track
}

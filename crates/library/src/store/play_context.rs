use crate::play_context::{
    ArtistTrackScope, MaterializedPlayContext, PlayContext, PlayContextAnchor,
    PlayContextDescriptor, PlayContextItem, PlayContextOrder, PlaylistSort, SearchSort,
    TrackFilter,
};

use super::library_track_sort::track_order_by_sql;
use super::sources::{collect_rows, fts_query, like_pattern, track_from_row_at};
use super::*;

const MATERIALIZATION_PAGE_SIZE: usize = 500;

struct ContextQuery {
    from: String,
    predicates: Vec<String>,
    params: Vec<Value>,
    order_by: String,
    source_item_id: &'static str,
}

impl Store {
    pub fn materialize_play_context(
        &self,
        source_id: &SourceId,
        context: &PlayContext,
        anchor: &PlayContextAnchor,
    ) -> StoreResult<MaterializedPlayContext> {
        self.read_snapshot(|store| store.materialize_play_context_inner(source_id, context, anchor))
    }

    fn materialize_play_context_inner(
        &self,
        source_id: &SourceId,
        context: &PlayContext,
        anchor: &PlayContextAnchor,
    ) -> StoreResult<MaterializedPlayContext> {
        let descriptor = &context.descriptor;
        if matches!(descriptor, PlayContextDescriptor::Folder { .. }) {
            return Err(StoreError::UnsupportedFolderPlayContext);
        }

        let items = match descriptor {
            PlayContextDescriptor::Playlist { playlist_id } => {
                self.materialize_playlist_context(source_id, playlist_id, &context.order)?
            }
            PlayContextDescriptor::SmartPlaylist {
                smart_playlist_id,
                definition_fingerprint,
                music_folder_id,
            } => self.materialize_smart_playlist_context(
                source_id,
                smart_playlist_id,
                definition_fingerprint,
                music_folder_id.as_ref(),
                &context.order,
            )?,
            _ => {
                let query = self.track_context_query(source_id, descriptor, &context.order)?;
                self.materialize_query_pages(source_id, query, 0, None)?
            }
        };

        let smart_playlist = matches!(descriptor, PlayContextDescriptor::SmartPlaylist { .. });
        let anchor_index = items
            .iter()
            .position(|item| {
                (smart_playlist || item.source_rank == anchor.source_rank)
                    && item.track.id == anchor.track_id
                    && item.source_item_id == anchor.source_item_id
            })
            .ok_or(StoreError::PlayContextAnchorNotFound)?;

        Ok(MaterializedPlayContext {
            items,
            anchor_index,
        })
    }

    fn materialize_playlist_context(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
        order: &PlayContextOrder,
    ) -> StoreResult<Vec<PlayContextItem>> {
        let (query, sort, descending) = match order {
            PlayContextOrder::Canonical => (None, PlaylistSort::Position, false),
            PlayContextOrder::Playlist {
                query,
                sort,
                descending,
            } => (query.as_deref(), *sort, *descending),
            _ => return Err(StoreError::UnsupportedPlayContext),
        };
        let mut predicates = vec![
            "pt.source_id = ?".to_string(),
            "pt.playlist_id = ?".to_string(),
        ];
        let mut params = vec![
            Value::Text(source_id.as_str().to_string()),
            Value::Text(playlist_id.as_str().to_string()),
        ];
        if let Some(pattern) = query.and_then(like_pattern) {
            predicates.push(
                "(LOWER(t.title) LIKE ? ESCAPE '\\'
                  OR LOWER(t.artist) LIKE ? ESCAPE '\\'
                  OR LOWER(t.album) LIKE ? ESCAPE '\\')"
                    .to_string(),
            );
            push_repeated(&mut params, &pattern, 3);
        }
        self.materialize_query_pages(
            source_id,
            ContextQuery {
                from: "playlist_tracks pt JOIN tracks t
                       ON t.source_id = pt.source_id AND t.track_id = pt.track_id"
                    .to_string(),
                predicates,
                params,
                order_by: playlist_order_by(sort, descending),
                source_item_id: "pt.entry_id",
            },
            0,
            None,
        )
    }

    fn materialize_smart_playlist_context(
        &self,
        source_id: &SourceId,
        smart_playlist_id: &SmartPlaylistId,
        expected_fingerprint: &str,
        music_folder_id: Option<&MusicFolderId>,
        order: &PlayContextOrder,
    ) -> StoreResult<Vec<PlayContextItem>> {
        let PlayContextOrder::SmartPlaylist = order else {
            return Err(StoreError::UnsupportedPlayContext);
        };
        let definition_json = self
            .connection
            .query_row(
                "SELECT definition_json FROM smart_playlists
                 WHERE source_id = ?1 AND smart_playlist_id = ?2",
                params![source_id.as_str(), smart_playlist_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(definition_json) = definition_json else {
            return Err(StoreError::PlayContextAnchorNotFound);
        };
        if definition_json != expected_fingerprint {
            return Err(StoreError::StaleSmartPlaylistDefinition);
        }
        let definition: SmartPlaylistDefinition = serde_json::from_str(&definition_json)?;
        let mut items = Vec::new();
        let mut offset = 0;
        loop {
            let page = self.smart_playlist_track_rows_in_folder(
                source_id,
                &definition,
                music_folder_id,
                offset,
                MATERIALIZATION_PAGE_SIZE,
            )?;
            let page_len = page.len();
            items.extend(
                page.into_iter()
                    .enumerate()
                    .map(|(index, track)| PlayContextItem {
                        track,
                        source_rank: offset + index,
                        source_item_id: None,
                    }),
            );
            if page_len < MATERIALIZATION_PAGE_SIZE {
                break;
            }
            offset += page_len;
        }
        Ok(items)
    }

    fn track_context_query(
        &self,
        source_id: &SourceId,
        descriptor: &PlayContextDescriptor,
        order: &PlayContextOrder,
    ) -> StoreResult<ContextQuery> {
        let music_folder_id = descriptor_music_folder(descriptor);
        let search_fts = match descriptor {
            PlayContextDescriptor::Search { query, .. } => match fts_query(query) {
                Some(query)
                    if self.search_fts_has_tracks(source_id, &query, music_folder_id)? =>
                {
                    Some(query)
                }
                _ => None,
            },
            _ => None,
        };
        let mut query = if let Some(search_fts) = search_fts.as_ref() {
            ContextQuery {
                from: "library_fts f JOIN tracks t
                       ON t.source_id = f.source_id AND t.track_id = f.item_id"
                    .to_string(),
                predicates: vec![
                    "f.source_id = ?".to_string(),
                    "f.item_type = 'track'".to_string(),
                    "library_fts MATCH ?".to_string(),
                ],
                params: vec![
                    Value::Text(source_id.as_str().to_string()),
                    Value::Text(search_fts.clone()),
                ],
                order_by: String::new(),
                source_item_id: "NULL",
            }
        } else {
            ContextQuery {
                from: "tracks t".to_string(),
                predicates: vec!["t.source_id = ?".to_string()],
                params: vec![Value::Text(source_id.as_str().to_string())],
                order_by: String::new(),
                source_item_id: "NULL",
            }
        };

        self.append_descriptor_filter(&mut query, descriptor, search_fts.is_some())?;
        if let Some(folder_id) = music_folder_id {
            query.predicates.push(
                "EXISTS (
                    SELECT 1 FROM track_music_folders tmf
                    WHERE tmf.source_id = t.source_id
                      AND tmf.track_id = t.track_id
                      AND tmf.folder_id = ?
                )"
                .to_string(),
            );
            query
                .params
                .push(Value::Text(folder_id.as_str().to_string()));
        }
        query.order_by = match order {
            PlayContextOrder::Canonical => canonical_track_order(descriptor),
            PlayContextOrder::Tracks {
                filter,
                sort,
                descending,
                favorite_first,
            } => {
                append_track_filter(&mut query, filter);
                displayed_track_order(*sort, *descending, *favorite_first)
            }
            PlayContextOrder::Search { sort }
                if matches!(descriptor, PlayContextDescriptor::Search { .. }) =>
            {
                search_order(*sort, search_fts.is_some())
            }
            _ => return Err(StoreError::UnsupportedPlayContext),
        };
        Ok(query)
    }

    fn append_descriptor_filter(
        &self,
        query: &mut ContextQuery,
        descriptor: &PlayContextDescriptor,
        search_uses_fts: bool,
    ) -> StoreResult<()> {
        match descriptor {
            PlayContextDescriptor::Album { album_id, .. } => {
                query.predicates.push("t.album_id = ?".to_string());
                query
                    .params
                    .push(Value::Text(album_id.as_str().to_string()));
            }
            PlayContextDescriptor::Artist {
                artist_id, scope, ..
            } => {
                let id = artist_id.as_str();
                match scope {
                    ArtistTrackScope::MainArtist => {
                        query.predicates.push("t.artist_id = ?".to_string());
                        query.params.push(Value::Text(id.to_string()));
                    }
                    ArtistTrackScope::AllCredits => {
                        query.predicates.push(
                            "(
                                t.artist_id = ?
                                OR EXISTS (
                                    SELECT 1 FROM track_artist_links tal
                                    WHERE tal.source_id = t.source_id
                                      AND tal.track_id = t.track_id
                                      AND tal.artist_id = ?
                                )
                                OR EXISTS (
                                    SELECT 1 FROM albums a
                                    WHERE a.source_id = t.source_id
                                      AND a.album_id = t.album_id
                                      AND a.artist_id = ?
                                )
                                OR EXISTS (
                                    SELECT 1 FROM album_artist_links aal
                                    WHERE aal.source_id = t.source_id
                                      AND aal.album_id = t.album_id
                                      AND aal.artist_id = ?
                                )
                            )"
                            .to_string(),
                        );
                        push_repeated(&mut query.params, id, 4);
                    }
                }
            }
            PlayContextDescriptor::Genre { genre_id, .. } => {
                query.predicates.push(
                    "EXISTS (
                        SELECT 1
                        FROM track_genres tg
                        JOIN genres g
                          ON g.source_id = tg.source_id AND g.name = tg.genre_name
                        WHERE tg.source_id = t.source_id
                          AND tg.track_id = t.track_id
                          AND g.genre_id = ?
                    )"
                    .to_string(),
                );
                query
                    .params
                    .push(Value::Text(genre_id.as_str().to_string()));
            }
            PlayContextDescriptor::Mood { mood_id, .. } => {
                query.predicates.push(
                    "EXISTS (
                        SELECT 1 FROM track_moods tm
                        WHERE tm.source_id = t.source_id
                          AND tm.track_id = t.track_id
                          AND tm.mood_name = ?
                    )"
                    .to_string(),
                );
                query.params.push(Value::Text(mood_id.as_str().to_string()));
            }
            PlayContextDescriptor::Favorites { .. } => query
                .predicates
                .push(format!("{} = 1", effective_track_favorite_sql("t"))),
            PlayContextDescriptor::Search { query: text, .. } if !search_uses_fts => {
                if let Some(pattern) = like_pattern(text) {
                    append_text_filter(query, &pattern);
                }
            }
            PlayContextDescriptor::Search { .. } | PlayContextDescriptor::Global { .. } => {}
            PlayContextDescriptor::Playlist { .. }
            | PlayContextDescriptor::SmartPlaylist { .. }
            | PlayContextDescriptor::Folder { .. } => {
                return Err(StoreError::UnsupportedPlayContext);
            }
        }
        Ok(())
    }

    fn search_fts_has_tracks(
        &self,
        source_id: &SourceId,
        query: &str,
        music_folder_id: Option<&MusicFolderId>,
    ) -> StoreResult<bool> {
        let folder_filter = if music_folder_id.is_some() {
            "AND EXISTS (
                SELECT 1 FROM track_music_folders tmf
                WHERE tmf.source_id = t.source_id
                  AND tmf.track_id = t.track_id
                  AND tmf.folder_id = ?3
            )"
        } else {
            ""
        };
        let sql = format!(
            "SELECT EXISTS (
                SELECT 1
                FROM library_fts f
                JOIN tracks t
                  ON t.source_id = f.source_id AND t.track_id = f.item_id
                WHERE f.source_id = ?1
                  AND f.item_type = 'track'
                  AND library_fts MATCH ?2
                  {folder_filter}
            )"
        );
        if let Some(folder_id) = music_folder_id {
            self.connection
                .query_row(
                    &sql,
                    params![source_id.as_str(), query, folder_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(StoreError::from)
        } else {
            self.connection
                .query_row(&sql, params![source_id.as_str(), query], |row| row.get(0))
                .map_err(StoreError::from)
        }
    }

    fn materialize_query_pages(
        &self,
        source_id: &SourceId,
        query: ContextQuery,
        start_offset: usize,
        max_items: Option<usize>,
    ) -> StoreResult<Vec<PlayContextItem>> {
        let sql = format!(
            "
            SELECT {source_item_id}, {track_columns}
            FROM {from}
            WHERE {where_clause}
            ORDER BY {order_by}
            LIMIT ? OFFSET ?
            ",
            source_item_id = query.source_item_id,
            track_columns = track_columns(),
            from = query.from,
            where_clause = query.predicates.join(" AND "),
            order_by = query.order_by,
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut items = Vec::new();
        let mut offset = start_offset;
        let max_items = max_items.unwrap_or(usize::MAX);
        while items.len() < max_items {
            let page_limit = MATERIALIZATION_PAGE_SIZE.min(max_items - items.len());
            let mut params = query.params.clone();
            params.push(Value::Integer(page_limit as i64));
            params.push(Value::Integer(offset as i64));
            let rows = collect_rows(statement.query_map(params_from_iter(params), |row| {
                Ok((row.get::<_, Option<String>>(0)?, track_from_row_at(row, 1)?))
            })?)?;
            let page_len = rows.len();
            let (source_item_ids, mut tracks): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
            self.attach_track_metadata(source_id, &mut tracks)?;
            items.extend(source_item_ids.into_iter().zip(tracks).enumerate().map(
                |(index, (source_item_id, track))| PlayContextItem {
                    track,
                    source_rank: offset + index,
                    source_item_id,
                },
            ));
            if page_len < page_limit {
                break;
            }
            offset += page_len;
        }
        Ok(items)
    }
}

fn descriptor_music_folder(descriptor: &PlayContextDescriptor) -> Option<&MusicFolderId> {
    match descriptor {
        PlayContextDescriptor::Album {
            music_folder_id, ..
        }
        | PlayContextDescriptor::SmartPlaylist {
            music_folder_id, ..
        }
        | PlayContextDescriptor::Folder {
            music_folder_id, ..
        }
        | PlayContextDescriptor::Artist {
            music_folder_id, ..
        }
        | PlayContextDescriptor::Genre {
            music_folder_id, ..
        }
        | PlayContextDescriptor::Mood {
            music_folder_id, ..
        }
        | PlayContextDescriptor::Favorites {
            music_folder_id, ..
        }
        | PlayContextDescriptor::Search {
            music_folder_id, ..
        }
        | PlayContextDescriptor::Global {
            music_folder_id, ..
        } => music_folder_id.as_ref(),
        PlayContextDescriptor::Playlist { .. } => None,
    }
}

fn append_track_filter(query: &mut ContextQuery, filter: &TrackFilter) {
    if let Some(pattern) = filter.query.as_deref().and_then(like_pattern) {
        append_text_filter(query, &pattern);
    }
    if filter.favorites_only {
        query
            .predicates
            .push(format!("{} = 1", effective_track_favorite_sql("t")));
    }
}

fn append_text_filter(query: &mut ContextQuery, pattern: &str) {
    query.predicates.push(
        "(
            LOWER(t.title) LIKE ? ESCAPE '\\'
            OR LOWER(t.artist) LIKE ? ESCAPE '\\'
            OR LOWER(t.album) LIKE ? ESCAPE '\\'
            OR CAST(t.year AS TEXT) LIKE ? ESCAPE '\\'
        )"
        .to_string(),
    );
    push_repeated(&mut query.params, pattern, 4);
}

fn push_repeated(params: &mut Vec<Value>, value: &str, count: usize) {
    params.extend(std::iter::repeat_n(Value::Text(value.to_string()), count));
}

fn canonical_track_order(descriptor: &PlayContextDescriptor) -> String {
    match descriptor {
        PlayContextDescriptor::Album { .. }
        | PlayContextDescriptor::Artist { .. }
        | PlayContextDescriptor::Genre { .. }
        | PlayContextDescriptor::Mood { .. } => {
            "t.album COLLATE NOCASE, t.disc_number, t.track_number,
             t.title COLLATE NOCASE, t.track_id"
                .to_string()
        }
        _ => "t.title COLLATE NOCASE, t.album COLLATE NOCASE,
              t.disc_number, t.track_number, t.track_id"
            .to_string(),
    }
}

fn displayed_track_order(sort: TrackSort, descending: bool, favorite_first: bool) -> String {
    let order = track_order_by_sql("t", sort, descending);
    if favorite_first {
        format!("{} DESC, {order}", effective_track_favorite_sql("t"))
    } else {
        order
    }
}

fn playlist_order_by(sort: PlaylistSort, descending: bool) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    let primary = match sort {
        PlaylistSort::Position => "pt.position",
        PlaylistSort::Title => "LOWER(t.title)",
        PlaylistSort::Artist => "LOWER(t.artist)",
        PlaylistSort::Album => "LOWER(t.album)",
    };
    format!("{primary} {direction}, pt.position {direction}, pt.entry_id {direction}")
}

fn search_order(sort: SearchSort, has_fts: bool) -> String {
    match (sort, has_fts) {
        (SearchSort::Relevance, true) => "bm25(library_fts), t.track_id".to_string(),
        (SearchSort::Relevance | SearchSort::Title, false) | (SearchSort::Title, true) => {
            "t.title COLLATE NOCASE, t.album COLLATE NOCASE,
             t.disc_number, t.track_number, t.track_id"
                .to_string()
        }
    }
}

fn track_columns() -> String {
    format!(
        "t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
         t.release_date, t.date_added, t.last_played, t.play_count, t.user_rating,
         t.duration_seconds, {} AS favorite, t.disc_number, t.track_number,
         t.image_item_id, t.image_tag, t.bpm, t.local_path, t.source_format, t.comment,
         t.skip_count",
        effective_track_favorite_sql("t")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play_context::TrackFilter;
    use crate::store::test_support::{LibraryObservation, StoreCase, album, artist, genre, track};
    use crate::{
        MusicFolder, SmartPlaylistDefinition, SmartPlaylistId, SmartPlaylistMatchMode,
        SmartPlaylistRule, SmartPlaylistRuleField, SmartPlaylistRuleGroup, SmartPlaylistRuleNode,
        SmartPlaylistRuleOperator, SmartPlaylistRuleValue, SmartPlaylistSortField,
    };

    #[test]
    fn materializes_complete_library_contexts_from_one_store_truth() {
        let case = StoreCase::open();
        let first_album = album(1);
        let second_album = album(2);
        let mut first = track(1, &first_album);
        first.genres = vec!["Genre 1".to_string()];
        first.moods = vec!["Focused".to_string()];
        let mut second = track(2, &first_album);
        second.genres = vec!["Genre 1".to_string()];
        let third = track(3, &second_album);
        let folder = MusicFolder {
            id: MusicFolderId::fake(1),
            name: "Selected".to_string(),
        };
        let generation = case.start_sync("begin context sync");
        case.commit_library(
            generation,
            LibraryObservation {
                albums: vec![first_album.clone(), second_album],
                tracks: vec![first.clone(), second.clone(), third],
                artists: vec![artist(1, None)],
                genres: vec![genre(1, None)],
                music_folders: vec![(folder.clone(), vec![first.clone(), second.clone()])],
                ..LibraryObservation::default()
            },
            "commit context library",
        );

        let folder_id = Some(folder.id.clone());
        let contexts = vec![
            PlayContext {
                descriptor: PlayContextDescriptor::Album {
                    album_id: first_album.id,
                    music_folder_id: folder_id.clone(),
                },
                order: PlayContextOrder::Canonical,
            },
            PlayContext {
                descriptor: PlayContextDescriptor::Artist {
                    artist_id: ArtistId::fake(1),
                    scope: ArtistTrackScope::AllCredits,
                    music_folder_id: folder_id.clone(),
                },
                order: PlayContextOrder::Canonical,
            },
            PlayContext {
                descriptor: PlayContextDescriptor::Genre {
                    genre_id: GenreId::fake(1),
                    music_folder_id: folder_id.clone(),
                },
                order: PlayContextOrder::Canonical,
            },
            PlayContext {
                descriptor: PlayContextDescriptor::Global {
                    music_folder_id: folder_id.clone(),
                },
                order: title_order(TrackFilter::default()),
            },
            PlayContext {
                descriptor: PlayContextDescriptor::Search {
                    query: "Track".to_string(),
                    music_folder_id: folder_id.clone(),
                },
                order: PlayContextOrder::Search {
                    sort: SearchSort::Relevance,
                },
            },
        ];
        for context in contexts {
            assert_eq!(
                materialized_ids(&case, &context, &first.id),
                vec![first.id.clone(), second.id.clone()]
            );
        }

        let mood = PlayContext {
            descriptor: PlayContextDescriptor::Mood {
                mood_id: MoodId::new("Focused"),
                music_folder_id: folder_id.clone(),
            },
            order: PlayContextOrder::Canonical,
        };
        assert_eq!(
            materialized_ids(&case, &mood, &first.id),
            vec![first.id.clone()]
        );

        let favorites = PlayContext {
            descriptor: PlayContextDescriptor::Favorites {
                music_folder_id: folder_id.clone(),
            },
            order: title_order(TrackFilter::default()),
        };
        assert_eq!(
            materialized_ids(&case, &favorites, &first.id),
            vec![first.id.clone()]
        );

        let artist_favorites = PlayContext {
            descriptor: PlayContextDescriptor::Artist {
                artist_id: ArtistId::fake(1),
                scope: ArtistTrackScope::AllCredits,
                music_folder_id: folder_id.clone(),
            },
            order: title_order(TrackFilter {
                query: None,
                favorites_only: true,
            }),
        };
        assert_eq!(
            materialized_ids(&case, &artist_favorites, &first.id),
            vec![first.id.clone()]
        );

        let definition = SmartPlaylistDefinition {
            root: SmartPlaylistRuleGroup {
                mode: SmartPlaylistMatchMode::All,
                rules: vec![SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                    field: SmartPlaylistRuleField::Title,
                    operator: SmartPlaylistRuleOperator::Contains,
                    value: Some(SmartPlaylistRuleValue::Text("Track".to_string())),
                })],
            },
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: None,
        };
        let smart_id = SmartPlaylistId::new("custom:context");
        case.save_smart_playlist(&case.id, &smart_id, "Context", &definition)
            .expect("save smart playlist");
        let fingerprint = serde_json::to_string(&definition).expect("definition fingerprint");
        let smart = PlayContext {
            descriptor: PlayContextDescriptor::SmartPlaylist {
                smart_playlist_id: smart_id,
                definition_fingerprint: fingerprint,
                music_folder_id: folder_id,
            },
            order: PlayContextOrder::SmartPlaylist,
        };
        let visible_rank_after_sorting = 0;
        let materialized = case
            .materialize_play_context(
                &case.id,
                &smart,
                &PlayContextAnchor {
                    track_id: second.id.clone(),
                    source_rank: visible_rank_after_sorting,
                    source_item_id: None,
                },
            )
            .expect("materialize smart playlist context");
        assert_eq!(
            materialized
                .items
                .iter()
                .map(|item| item.track.id.clone())
                .collect::<Vec<_>>(),
            vec![first.id, second.id]
        );
        assert_eq!(materialized.anchor_index, 1);
    }

    #[test]
    fn folder_context_reports_its_source_owned_boundary() {
        let case = StoreCase::open();
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Folder {
                path: vec!["Music".to_string()],
                music_folder_id: None,
            },
            order: PlayContextOrder::Canonical,
        };
        let error = case
            .materialize_play_context(
                &case.id,
                &context,
                &PlayContextAnchor {
                    track_id: TrackId::fake(1),
                    source_rank: 0,
                    source_item_id: None,
                },
            )
            .expect_err("folder context cannot come from Store");
        assert!(matches!(error, StoreError::UnsupportedFolderPlayContext));
    }

    #[test]
    fn complete_context_crosses_the_bounded_sql_page() {
        let case = StoreCase::open();
        let album = album(1);
        let tracks = (1..=520)
            .map(|number| track(number, &album))
            .collect::<Vec<_>>();
        let generation = case.start_sync("begin large context sync");
        case.commit_library(
            generation,
            LibraryObservation {
                albums: vec![album],
                tracks,
                ..LibraryObservation::default()
            },
            "commit large context library",
        );
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Global {
                music_folder_id: None,
            },
            order: PlayContextOrder::Tracks {
                filter: TrackFilter::default(),
                sort: TrackSort::TrackNumber,
                descending: false,
                favorite_first: false,
            },
        };
        let materialized = case
            .materialize_play_context(
                &case.id,
                &context,
                &PlayContextAnchor {
                    track_id: TrackId::fake(520),
                    source_rank: 519,
                    source_item_id: None,
                },
            )
            .expect("materialize complete large context");

        assert_eq!(materialized.items.len(), 520);
        assert_eq!(materialized.items[500].source_rank, 500);
        assert_eq!(materialized.items[519].track.id, TrackId::fake(520));
    }

    #[test]
    fn bpm_order_materializes_the_visible_sequence() {
        let case = StoreCase::open();
        let album = album(1);
        let mut tracks = (1..=4)
            .map(|number| track(number, &album))
            .collect::<Vec<_>>();
        for track in &mut tracks {
            track.title = "Same title".to_string();
            track.track_number = 1;
        }
        tracks[0].bpm = Some(120);
        tracks[1].bpm = Some(90);
        tracks[2].bpm = None;
        tracks[3].bpm = Some(120);
        let generation = case.start_sync("begin BPM context sync");
        case.commit_library(
            generation,
            LibraryObservation {
                albums: vec![album],
                tracks: tracks.clone(),
                ..LibraryObservation::default()
            },
            "commit BPM context library",
        );
        let context = PlayContext {
            descriptor: PlayContextDescriptor::Global {
                music_folder_id: None,
            },
            order: PlayContextOrder::Tracks {
                filter: TrackFilter::default(),
                sort: TrackSort::Bpm,
                descending: false,
                favorite_first: false,
            },
        };

        assert_eq!(
            materialized_ids(&case, &context, &tracks[1].id),
            vec![
                tracks[1].id.clone(),
                tracks[0].id.clone(),
                tracks[3].id.clone(),
                tracks[2].id.clone(),
            ]
        );
    }

    fn title_order(filter: TrackFilter) -> PlayContextOrder {
        PlayContextOrder::Tracks {
            filter,
            sort: TrackSort::Title,
            descending: false,
            favorite_first: false,
        }
    }

    fn materialized_ids(
        case: &StoreCase,
        context: &PlayContext,
        anchor_track_id: &TrackId,
    ) -> Vec<TrackId> {
        case.materialize_play_context(
            &case.id,
            context,
            &PlayContextAnchor {
                track_id: anchor_track_id.clone(),
                source_rank: 0,
                source_item_id: None,
            },
        )
        .expect("materialize play context")
        .items
        .into_iter()
        .map(|item| item.track.id)
        .collect()
    }
}

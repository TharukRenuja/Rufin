use super::servers::{
    COLLECTION_COVER_SMART_PLAYLIST, bool_to_i64, collect_rows, replace_collection_refs,
    track_from_row, u32_from_i64,
};
use super::*;
use domain::smart_playlists as smart_policy;

const SMART_TRACK_DEFAULT_LIMIT: usize = 25_000;
const RETIRED_SMART_PLAYLIST_BUILTIN_KEYS: &[&str] = &[
    "favorites",
    "highest_rated",
    "newest_tracks",
    "recently_played",
];

struct SmartPlaylistRow {
    id: SmartPlaylistId,
    name: String,
    position: u32,
    builtin: Option<SmartPlaylistBuiltin>,
    definition: SmartPlaylistDefinition,
}

struct SmartSql {
    clause: String,
    params: Vec<Value>,
}

struct SmartTrackQuery {
    from: String,
    where_clause: String,
    where_params: Vec<Value>,
    order_by: String,
}

impl Store {
    pub fn ensure_smart_playlist_defaults_seeded(&self, server_id: &ServerId) -> StoreResult<()> {
        self.delete_retired_builtin_smart_playlists(server_id)?;
        let seeded = self.connection.query_row(
            "
            SELECT EXISTS(
                SELECT 1
                FROM smart_playlist_seed_state
                WHERE server_id = ?1
            )
            ",
            params![server_id.as_str()],
            |row| row.get::<_, bool>(0),
        )?;
        if seeded {
            return Ok(());
        }
        for (position, builtin) in SmartPlaylistBuiltin::all().into_iter().enumerate() {
            self.insert_builtin_smart_playlist(server_id, builtin, position as i64)?;
        }
        self.connection.execute(
            "
            INSERT INTO smart_playlist_seed_state (server_id)
            VALUES (?1)
            ON CONFLICT(server_id) DO NOTHING
            ",
            params![server_id.as_str()],
        )?;
        Ok(())
    }

    fn delete_retired_builtin_smart_playlists(&self, server_id: &ServerId) -> StoreResult<()> {
        for key in RETIRED_SMART_PLAYLIST_BUILTIN_KEYS {
            let exists = self.connection.query_row(
                "
                SELECT EXISTS(
                    SELECT 1
                    FROM smart_playlists
                    WHERE server_id = ?1 AND builtin_key = ?2
                )
                ",
                params![server_id.as_str(), key],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                continue;
            }
            self.connection.execute(
                "
                DELETE FROM smart_playlists
                WHERE server_id = ?1 AND builtin_key = ?2
                ",
                params![server_id.as_str(), key],
            )?;
        }
        Ok(())
    }

    pub fn load_smart_playlists(
        &self,
        server_id: &ServerId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<SmartPlaylist>> {
        self.ensure_smart_playlist_defaults_seeded(server_id)?;
        let total = self.connection.query_row(
            "SELECT COUNT(*) FROM smart_playlists WHERE server_id = ?1",
            params![server_id.as_str()],
            |row| row.get::<_, i64>(0),
        )?;
        let mut statement = self.connection.prepare(
            "
            SELECT smart_playlist_id, name, builtin_key, definition_json, position
            FROM smart_playlists
            WHERE server_id = ?1
            ORDER BY position, name COLLATE NOCASE, smart_playlist_id
            LIMIT ?2 OFFSET ?3
            ",
        )?;
        let rows = collect_rows(statement.query_map(
            params![server_id.as_str(), limit as i64, offset as i64],
            smart_playlist_row_from_row,
        )?)?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(self.smart_playlist_from_record(server_id, row)?);
        }
        Ok(PagedResponse::new(items, u32_from_i64(total) as usize))
    }

    pub fn load_smart_playlist_detail(
        &self,
        server_id: &ServerId,
        smart_playlist_id: &SmartPlaylistId,
    ) -> StoreResult<Option<SmartPlaylistDetail>> {
        self.ensure_smart_playlist_defaults_seeded(server_id)?;
        let Some(row) = self.load_smart_playlist_row(server_id, smart_playlist_id)? else {
            return Ok(None);
        };
        let smart_playlist = self.smart_playlist_from_record(server_id, row)?;
        let limit = smart_playlist
            .definition
            .limit
            .unwrap_or(SMART_TRACK_DEFAULT_LIMIT);
        let mut tracks = self
            .query_smart_playlist_tracks(server_id, &smart_playlist.definition, 0, limit)?
            .items;
        self.attach_track_metadata(server_id, &mut tracks)?;
        Ok(Some(SmartPlaylistDetail {
            smart_playlist,
            tracks,
        }))
    }

    pub fn load_smart_playlist_tracks_page(
        &self,
        server_id: &ServerId,
        smart_playlist_id: &SmartPlaylistId,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Option<PagedResponse<Track>>> {
        self.ensure_smart_playlist_defaults_seeded(server_id)?;
        let Some(row) = self.load_smart_playlist_row(server_id, smart_playlist_id)? else {
            return Ok(None);
        };
        let mut page =
            self.query_smart_playlist_tracks(server_id, &row.definition, offset, limit)?;
        self.attach_track_metadata(server_id, &mut page.items)?;
        Ok(Some(page))
    }

    pub fn save_smart_playlist(
        &self,
        server_id: &ServerId,
        smart_playlist_id: &SmartPlaylistId,
        name: &str,
        definition: &SmartPlaylistDefinition,
    ) -> StoreResult<()> {
        let position = self.next_smart_playlist_position(server_id)?;
        let definition_json = serde_json::to_string(definition)?;
        self.connection.execute(
            "
            INSERT INTO smart_playlists (
                server_id, smart_playlist_id, name, builtin_key, definition_json, position
            )
            VALUES (?1, ?2, ?3, NULL, ?4, ?5)
            ON CONFLICT(server_id, smart_playlist_id) DO UPDATE SET
                name = excluded.name,
                definition_json = excluded.definition_json,
                updated_at = CURRENT_TIMESTAMP
            ",
            params![
                server_id.as_str(),
                smart_playlist_id.as_str(),
                name.trim(),
                definition_json,
                position
            ],
        )?;
        Ok(())
    }

    pub fn delete_smart_playlist(
        &self,
        server_id: &ServerId,
        smart_playlist_id: &SmartPlaylistId,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "
                DELETE FROM smart_playlists
                WHERE server_id = ?1 AND smart_playlist_id = ?2
                ",
                params![server_id.as_str(), smart_playlist_id.as_str()],
            )?;
            connection.execute(
                "
                DELETE FROM collection_cover_refs
                WHERE server_id = ?1
                  AND collection_type = ?2
                  AND collection_id = ?3
                ",
                params![
                    server_id.as_str(),
                    COLLECTION_COVER_SMART_PLAYLIST,
                    smart_playlist_id.as_str(),
                ],
            )?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn reorder_smart_playlist(
        &self,
        server_id: &ServerId,
        dragged_id: &SmartPlaylistId,
        target_id: &SmartPlaylistId,
        after: bool,
    ) -> StoreResult<bool> {
        self.ensure_smart_playlist_defaults_seeded(server_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT smart_playlist_id
            FROM smart_playlists
            WHERE server_id = ?1
            ORDER BY position, name COLLATE NOCASE, smart_playlist_id
            ",
        )?;
        let ids = collect_rows(statement.query_map(params![server_id.as_str()], |row| {
            row.get::<_, String>(0).map(SmartPlaylistId::new)
        })?)?;
        let Some(ids) = reorder_smart_playlist_ids(&ids, dragged_id, target_id, after) else {
            return Ok(false);
        };

        self.write_batch(|connection| {
            for (position, id) in ids.iter().enumerate() {
                connection.execute(
                    "
                    UPDATE smart_playlists
                    SET position = ?1,
                        updated_at = CURRENT_TIMESTAMP
                    WHERE server_id = ?2 AND smart_playlist_id = ?3
                    ",
                    params![position as i64, server_id.as_str(), id.as_str()],
                )?;
            }
            Ok(true)
        })
    }

    pub fn missing_builtin_smart_playlists(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<Vec<SmartPlaylistBuiltin>> {
        self.ensure_smart_playlist_defaults_seeded(server_id)?;
        let mut statement = self.connection.prepare(
            "
            SELECT builtin_key
            FROM smart_playlists
            WHERE server_id = ?1 AND builtin_key IS NOT NULL
            ",
        )?;
        let existing = collect_rows(
            statement.query_map(params![server_id.as_str()], |row| row.get::<_, String>(0))?,
        )?;
        Ok(SmartPlaylistBuiltin::all()
            .into_iter()
            .filter(|builtin| !existing.iter().any(|key| key == builtin.key()))
            .collect())
    }

    pub fn restore_builtin_smart_playlist(
        &self,
        server_id: &ServerId,
        builtin: SmartPlaylistBuiltin,
    ) -> StoreResult<SmartPlaylistId> {
        let position = self.next_smart_playlist_position(server_id)?;
        self.insert_builtin_smart_playlist(server_id, builtin, position)?;
        Ok(smart_builtin_id(builtin))
    }

    pub fn record_local_track_played(
        &self,
        server_id: &ServerId,
        track_id: &TrackId,
        session_key: &str,
    ) -> StoreResult<bool> {
        let changed = self.connection.execute(
            "
            INSERT INTO track_activity (
                server_id, track_id, play_count, last_played, skip_count,
                play_recorded_session, updated_at
            )
            VALUES (?1, ?2, 1, CURRENT_TIMESTAMP, 0, ?3, CURRENT_TIMESTAMP)
            ON CONFLICT(server_id, track_id) DO UPDATE SET
                play_count = CASE
                    WHEN play_recorded_session = excluded.play_recorded_session
                    THEN play_count
                    ELSE play_count + 1
                END,
                last_played = CASE
                    WHEN play_recorded_session = excluded.play_recorded_session
                    THEN last_played
                    ELSE CURRENT_TIMESTAMP
                END,
                play_recorded_session = excluded.play_recorded_session,
                updated_at = CURRENT_TIMESTAMP
            WHERE play_recorded_session IS NULL
               OR play_recorded_session != excluded.play_recorded_session
            ",
            params![server_id.as_str(), track_id.as_str(), session_key],
        )?;
        Ok(changed > 0)
    }

    pub fn increment_track_skip_count(
        &self,
        server_id: &ServerId,
        track_id: &TrackId,
    ) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO track_activity (
                server_id, track_id, play_count, last_played, skip_count, updated_at
            )
            VALUES (?1, ?2, 0, NULL, 1, CURRENT_TIMESTAMP)
            ON CONFLICT(server_id, track_id) DO UPDATE SET
                skip_count = skip_count + 1,
                updated_at = CURRENT_TIMESTAMP
            ",
            params![server_id.as_str(), track_id.as_str()],
        )?;
        Ok(())
    }

    fn load_smart_playlist_row(
        &self,
        server_id: &ServerId,
        smart_playlist_id: &SmartPlaylistId,
    ) -> StoreResult<Option<SmartPlaylistRow>> {
        self.connection
            .query_row(
                "
                SELECT smart_playlist_id, name, builtin_key, definition_json, position
                FROM smart_playlists
                WHERE server_id = ?1 AND smart_playlist_id = ?2
                ",
                params![server_id.as_str(), smart_playlist_id.as_str()],
                smart_playlist_row_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn smart_playlist_from_record(
        &self,
        server_id: &ServerId,
        row: SmartPlaylistRow,
    ) -> StoreResult<SmartPlaylist> {
        let (track_count, duration_seconds) =
            self.smart_playlist_stats(server_id, &row.definition)?;
        let mut image_refs = self.load_collection_cover_refs(
            server_id,
            COLLECTION_COVER_SMART_PLAYLIST,
            row.id.as_str(),
        )?;
        if image_refs.is_empty() {
            image_refs = self.smart_playlist_cover_image_refs(server_id, &row.definition)?;
        }
        Ok(SmartPlaylist {
            id: row.id,
            name: row.name,
            position: row.position,
            builtin: row.builtin,
            definition: row.definition,
            track_count,
            duration_seconds,
            image_ref: image_refs.first().cloned(),
            image_refs,
        })
    }

    fn smart_playlist_stats(
        &self,
        server_id: &ServerId,
        definition: &SmartPlaylistDefinition,
    ) -> StoreResult<(u32, u32)> {
        let total = self.count_smart_playlist_tracks(server_id, definition)?;
        let duration_seconds = if let Some(limit) = definition.limit {
            self.query_smart_playlist_tracks(server_id, definition, 0, limit)?
                .items
                .iter()
                .map(|track| track.duration_seconds)
                .sum()
        } else {
            self.sum_smart_playlist_duration(server_id, definition)?
        };
        Ok((total.min(u32::MAX as usize) as u32, duration_seconds))
    }

    fn smart_playlist_cover_image_refs(
        &self,
        server_id: &ServerId,
        definition: &SmartPlaylistDefinition,
    ) -> StoreResult<Vec<ImageRef>> {
        Ok(first_track_image_refs(
            self.query_smart_playlist_tracks(
                server_id,
                definition,
                0,
                definition.limit.map_or(4, |limit| limit.min(4)),
            )?
            .items,
        ))
    }

    pub(super) fn refresh_smart_playlist_cover_refs(
        &self,
        server_id: &ServerId,
    ) -> StoreResult<()> {
        self.ensure_smart_playlist_defaults_seeded(server_id)?;
        let rows = {
            let mut statement = self.connection.prepare(
                "
                SELECT smart_playlist_id, name, builtin_key, definition_json, position
                FROM smart_playlists
                WHERE server_id = ?1
                ORDER BY position, name COLLATE NOCASE, smart_playlist_id
                ",
            )?;
            collect_rows(
                statement.query_map(params![server_id.as_str()], smart_playlist_row_from_row)?,
            )?
        };
        let mut cover_refs = Vec::with_capacity(rows.len());
        for row in rows {
            cover_refs.push((
                row.id,
                self.smart_playlist_cover_image_refs(server_id, &row.definition)?,
            ));
        }
        self.write_batch(|connection| {
            connection.execute(
                "
                DELETE FROM collection_cover_refs
                WHERE server_id = ?1
                  AND collection_type = ?2
                ",
                params![server_id.as_str(), COLLECTION_COVER_SMART_PLAYLIST],
            )?;
            for (smart_playlist_id, image_refs) in cover_refs {
                replace_collection_refs(
                    connection,
                    server_id,
                    COLLECTION_COVER_SMART_PLAYLIST,
                    smart_playlist_id.as_str(),
                    &image_refs,
                )?;
            }
            Ok(())
        })
    }

    fn query_smart_playlist_tracks(
        &self,
        server_id: &ServerId,
        definition: &SmartPlaylistDefinition,
        offset: usize,
        limit: usize,
    ) -> StoreResult<PagedResponse<Track>> {
        let total = self.count_smart_playlist_tracks(server_id, definition)?;
        if definition
            .limit
            .is_some_and(|definition_limit| offset >= definition_limit)
        {
            return Ok(PagedResponse::new(Vec::new(), total));
        }
        let limit = definition
            .limit
            .map(|definition_limit| limit.min(definition_limit.saturating_sub(offset)))
            .unwrap_or(limit);
        let query = self.smart_track_query(server_id, definition)?;
        let mut values = query.where_params;
        values.push(Value::from(limit as i64));
        values.push(Value::from(offset as i64));
        let sql = format!(
            "
            SELECT t.track_id, t.album_id, t.title, t.artist, t.artist_id, t.album, t.year,
                   t.release_date, t.date_added, {last_played} AS last_played,
                   {play_count} AS play_count, t.user_rating, t.duration_seconds, t.favorite,
                   t.disc_number, t.track_number, t.image_item_id, t.image_tag,
                   t.local_path, t.source_format, t.comment, {skip_count} AS skip_count, t.bpm
            {from}
            WHERE {where_clause}
            ORDER BY {order_by}
            LIMIT ? OFFSET ?
            ",
            last_played = smart_last_played_expr(),
            play_count = smart_play_count_expr(),
            skip_count = smart_skip_count_expr(),
            from = query.from,
            where_clause = query.where_clause,
            order_by = query.order_by,
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut tracks =
            collect_rows(statement.query_map(params_from_iter(values), track_from_row)?)?;
        self.attach_track_metadata(server_id, &mut tracks)?;
        Ok(PagedResponse::new(tracks, total))
    }

    fn count_smart_playlist_tracks(
        &self,
        server_id: &ServerId,
        definition: &SmartPlaylistDefinition,
    ) -> StoreResult<usize> {
        let query = self.smart_track_query(server_id, definition)?;
        let sql = format!(
            "
            SELECT COUNT(*)
            {from}
            WHERE {where_clause}
            ",
            from = query.from,
            where_clause = query.where_clause,
        );
        let count =
            self.connection
                .query_row(&sql, params_from_iter(query.where_params), |row| {
                    row.get::<_, i64>(0)
                })?;
        let count = u32_from_i64(count) as usize;
        Ok(definition
            .limit
            .map(|limit| count.min(limit))
            .unwrap_or(count))
    }

    fn sum_smart_playlist_duration(
        &self,
        server_id: &ServerId,
        definition: &SmartPlaylistDefinition,
    ) -> StoreResult<u32> {
        let query = self.smart_track_query(server_id, definition)?;
        let sql = format!(
            "
            SELECT COALESCE(SUM(t.duration_seconds), 0)
            {from}
            WHERE {where_clause}
            ",
            from = query.from,
            where_clause = query.where_clause,
        );
        self.connection
            .query_row(&sql, params_from_iter(query.where_params), |row| {
                row.get::<_, i64>(0)
            })
            .map(u32_from_i64)
            .map_err(StoreError::from)
    }

    fn smart_track_query(
        &self,
        server_id: &ServerId,
        definition: &SmartPlaylistDefinition,
    ) -> StoreResult<SmartTrackQuery> {
        let selected_folder = self.selected_music_folder_id(server_id)?;
        let compiled = compile_group(&definition.root);
        let mut params = Vec::with_capacity(compiled.params.len() + 2);
        params.push(Value::from(server_id.as_str().to_string()));
        params.extend(compiled.params);
        let mut where_clause = format!("t.server_id = ? AND ({})", compiled.clause);
        if let Some(folder_id) = selected_folder {
            where_clause.push_str(
                "
                AND EXISTS (
                    SELECT 1
                    FROM track_music_folders tmf
                    WHERE tmf.server_id = t.server_id
                      AND tmf.track_id = t.track_id
                      AND tmf.folder_id = ?
                )
                ",
            );
            params.push(Value::from(folder_id.as_str().to_string()));
        }
        Ok(SmartTrackQuery {
            from: "
                FROM tracks t
                JOIN servers s ON s.server_id = t.server_id
                LEFT JOIN track_activity ta
                  ON ta.server_id = t.server_id AND ta.track_id = t.track_id
            "
            .to_string(),
            where_clause,
            where_params: params,
            order_by: smart_order_by(definition.sort_field, definition.descending),
        })
    }

    fn next_smart_playlist_position(&self, server_id: &ServerId) -> StoreResult<i64> {
        self.connection
            .query_row(
                "
                SELECT COALESCE(MAX(position), -1) + 1
                FROM smart_playlists
                WHERE server_id = ?1
                ",
                params![server_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(StoreError::from)
    }

    fn insert_builtin_smart_playlist(
        &self,
        server_id: &ServerId,
        builtin: SmartPlaylistBuiltin,
        position: i64,
    ) -> StoreResult<()> {
        let definition = smart_policy::builtin_definition(builtin);
        let definition_json = serde_json::to_string(&definition)?;
        let smart_playlist_id = smart_builtin_id(builtin);
        self.connection.execute(
            "
            INSERT INTO smart_playlists (
                server_id, smart_playlist_id, name, builtin_key, definition_json, position
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(server_id, smart_playlist_id) DO UPDATE SET
                name = excluded.name,
                builtin_key = excluded.builtin_key,
                definition_json = excluded.definition_json,
                position = excluded.position,
                updated_at = CURRENT_TIMESTAMP
            ",
            params![
                server_id.as_str(),
                smart_playlist_id.as_str(),
                builtin.title(),
                builtin.key(),
                definition_json,
                position
            ],
        )?;
        Ok(())
    }
}

fn first_track_image_refs(tracks: Vec<Track>) -> Vec<ImageRef> {
    let mut image_refs = Vec::new();
    for track in tracks {
        let Some(image_ref) = track.image_ref else {
            continue;
        };
        image_refs.push(image_ref);
        if image_refs.len() >= 4 {
            break;
        }
    }
    image_refs
}

fn smart_playlist_row_from_row(row: &Row<'_>) -> rusqlite::Result<SmartPlaylistRow> {
    let builtin = row
        .get::<_, Option<String>>(2)?
        .and_then(|key| SmartPlaylistBuiltin::from_key(&key));
    let definition_json = row.get::<_, String>(3)?;
    let definition = serde_json::from_str(&definition_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(SmartPlaylistRow {
        id: SmartPlaylistId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        position: u32_from_i64(row.get::<_, i64>(4)?),
        builtin,
        definition,
    })
}

fn reorder_smart_playlist_ids(
    ids: &[SmartPlaylistId],
    dragged_id: &SmartPlaylistId,
    target_id: &SmartPlaylistId,
    after: bool,
) -> Option<Vec<SmartPlaylistId>> {
    if dragged_id == target_id {
        return None;
    }
    let source_index = ids.iter().position(|id| id == dragged_id)?;
    let target_index = ids.iter().position(|id| id == target_id)?;
    let mut reordered = ids.to_vec();
    let dragged = reordered.remove(source_index);
    let mut insert_index = if after {
        target_index.saturating_add(1)
    } else {
        target_index
    };
    if source_index < insert_index {
        insert_index = insert_index.saturating_sub(1);
    }
    reordered.insert(insert_index.min(reordered.len()), dragged);
    (reordered != ids).then_some(reordered)
}

pub(super) fn smart_builtin_id(builtin: SmartPlaylistBuiltin) -> SmartPlaylistId {
    SmartPlaylistId::new(format!("builtin:{}", builtin.key()))
}

fn compile_group(group: &SmartPlaylistRuleGroup) -> SmartSql {
    if group.rules.is_empty() {
        return SmartSql {
            clause: "1 = 1".to_string(),
            params: Vec::new(),
        };
    }
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    for node in &group.rules {
        let compiled = match node {
            SmartPlaylistRuleNode::Group(group) => compile_group(group),
            SmartPlaylistRuleNode::Rule(rule) => compile_rule(rule),
        };
        clauses.push(format!("({})", compiled.clause));
        params.extend(compiled.params);
    }
    let joiner = match group.mode {
        SmartPlaylistMatchMode::All => " AND ",
        SmartPlaylistMatchMode::Any => " OR ",
    };
    SmartSql {
        clause: clauses.join(joiner),
        params,
    }
}

fn compile_rule(rule: &SmartPlaylistRule) -> SmartSql {
    match rule.field {
        SmartPlaylistRuleField::Title => compile_text_rule("t.title", rule),
        SmartPlaylistRuleField::Artist => compile_text_rule("t.artist", rule),
        SmartPlaylistRuleField::Album => compile_text_rule("t.album", rule),
        SmartPlaylistRuleField::Comment => compile_text_rule("COALESCE(t.comment, '')", rule),
        SmartPlaylistRuleField::Genre => {
            compile_linked_text_rule("track_genres", "genre_name", "tg", rule)
        }
        SmartPlaylistRuleField::Mood => {
            compile_linked_text_rule("track_moods", "mood_name", "tm", rule)
        }
        SmartPlaylistRuleField::Bpm => compile_number_rule("t.bpm", true, rule),
        SmartPlaylistRuleField::Rating => compile_number_rule("t.user_rating", true, rule),
        SmartPlaylistRuleField::Year => compile_number_rule("t.year", false, rule),
        SmartPlaylistRuleField::Favorite => compile_bool_rule("t.favorite", rule),
        SmartPlaylistRuleField::Played => compile_played_rule(rule),
        SmartPlaylistRuleField::PlayCount => {
            compile_number_rule(&smart_play_count_expr(), false, rule)
        }
        SmartPlaylistRuleField::SkipCount => {
            compile_number_rule(&smart_skip_count_expr(), false, rule)
        }
        SmartPlaylistRuleField::LastPlayed => compile_date_rule(&smart_last_played_expr(), rule),
        SmartPlaylistRuleField::DateAdded => compile_date_rule("t.date_added", rule),
    }
}

fn compile_text_rule(expression: &str, rule: &SmartPlaylistRule) -> SmartSql {
    match rule.operator {
        SmartPlaylistRuleOperator::IsEmpty => SmartSql {
            clause: format!("TRIM({expression}) = ''"),
            params: Vec::new(),
        },
        SmartPlaylistRuleOperator::IsNotEmpty => SmartSql {
            clause: format!("TRIM({expression}) != ''"),
            params: Vec::new(),
        },
        SmartPlaylistRuleOperator::Contains | SmartPlaylistRuleOperator::NotContains => {
            let Some(value) = smart_policy::text_value(rule) else {
                return false_sql();
            };
            let clause = format!("LOWER({expression}) LIKE ? ESCAPE '\\'");
            let clause = if rule.operator == SmartPlaylistRuleOperator::NotContains {
                format!("NOT ({clause})")
            } else {
                clause
            };
            SmartSql {
                clause,
                params: vec![Value::from(format!(
                    "%{}%",
                    escape_like(&value.to_lowercase())
                ))],
            }
        }
        SmartPlaylistRuleOperator::Equals | SmartPlaylistRuleOperator::NotEquals => {
            let Some(value) = smart_policy::text_value(rule) else {
                return false_sql();
            };
            let operator = if rule.operator == SmartPlaylistRuleOperator::Equals {
                "="
            } else {
                "!="
            };
            SmartSql {
                clause: format!("LOWER({expression}) {operator} ?"),
                params: vec![Value::from(value.to_lowercase())],
            }
        }
        SmartPlaylistRuleOperator::Above
        | SmartPlaylistRuleOperator::Below
        | SmartPlaylistRuleOperator::Between
        | SmartPlaylistRuleOperator::Is
        | SmartPlaylistRuleOperator::IsNot
        | SmartPlaylistRuleOperator::Before
        | SmartPlaylistRuleOperator::After => false_sql(),
    }
}

fn compile_linked_text_rule(
    table: &str,
    name_column: &str,
    alias: &str,
    rule: &SmartPlaylistRule,
) -> SmartSql {
    let Some(value) = smart_policy::text_value(rule) else {
        return false_sql();
    };
    let (operator, pattern) = match rule.operator {
        SmartPlaylistRuleOperator::Equals | SmartPlaylistRuleOperator::NotEquals => {
            ("=", value.to_lowercase())
        }
        SmartPlaylistRuleOperator::Contains | SmartPlaylistRuleOperator::NotContains => {
            ("LIKE", format!("%{}%", escape_like(&value.to_lowercase())))
        }
        SmartPlaylistRuleOperator::Above
        | SmartPlaylistRuleOperator::Below
        | SmartPlaylistRuleOperator::Between
        | SmartPlaylistRuleOperator::Is
        | SmartPlaylistRuleOperator::IsNot
        | SmartPlaylistRuleOperator::Before
        | SmartPlaylistRuleOperator::After
        | SmartPlaylistRuleOperator::IsEmpty
        | SmartPlaylistRuleOperator::IsNotEmpty => return false_sql(),
    };
    let comparison = if operator == "LIKE" {
        format!("LOWER({alias}.{name_column}) LIKE ? ESCAPE '\\'")
    } else {
        format!("LOWER({alias}.{name_column}) = ?")
    };
    let exists = format!(
        "
        EXISTS (
            SELECT 1
            FROM {table} {alias}
            WHERE {alias}.server_id = t.server_id
              AND {alias}.track_id = t.track_id
              AND {comparison}
        )
        "
    );
    let negated = matches!(
        rule.operator,
        SmartPlaylistRuleOperator::NotEquals | SmartPlaylistRuleOperator::NotContains
    );
    SmartSql {
        clause: if negated {
            format!("NOT ({exists})")
        } else {
            exists
        },
        params: vec![Value::from(pattern)],
    }
}

fn compile_number_rule(expression: &str, nullable: bool, rule: &SmartPlaylistRule) -> SmartSql {
    match rule.operator {
        SmartPlaylistRuleOperator::IsEmpty if nullable => SmartSql {
            clause: format!("{expression} IS NULL"),
            params: Vec::new(),
        },
        SmartPlaylistRuleOperator::IsNotEmpty if nullable => SmartSql {
            clause: format!("{expression} IS NOT NULL"),
            params: Vec::new(),
        },
        SmartPlaylistRuleOperator::Above
        | SmartPlaylistRuleOperator::Below
        | SmartPlaylistRuleOperator::Equals
        | SmartPlaylistRuleOperator::NotEquals => {
            let Some(value) = smart_policy::number_value(rule) else {
                return false_sql();
            };
            let operator = match rule.operator {
                SmartPlaylistRuleOperator::Above => ">",
                SmartPlaylistRuleOperator::Below => "<",
                SmartPlaylistRuleOperator::Equals => "=",
                SmartPlaylistRuleOperator::NotEquals => "!=",
                SmartPlaylistRuleOperator::Contains
                | SmartPlaylistRuleOperator::NotContains
                | SmartPlaylistRuleOperator::Between
                | SmartPlaylistRuleOperator::Is
                | SmartPlaylistRuleOperator::IsNot
                | SmartPlaylistRuleOperator::Before
                | SmartPlaylistRuleOperator::After
                | SmartPlaylistRuleOperator::IsEmpty
                | SmartPlaylistRuleOperator::IsNotEmpty => return false_sql(),
            };
            SmartSql {
                clause: format!("{expression} {operator} ?"),
                params: vec![Value::from(value)],
            }
        }
        SmartPlaylistRuleOperator::Between => {
            let Some((min, max)) = smart_policy::number_range_value(rule) else {
                return false_sql();
            };
            SmartSql {
                clause: format!("{expression} BETWEEN ? AND ?"),
                params: vec![Value::from(min), Value::from(max)],
            }
        }
        SmartPlaylistRuleOperator::Contains
        | SmartPlaylistRuleOperator::NotContains
        | SmartPlaylistRuleOperator::Is
        | SmartPlaylistRuleOperator::IsNot
        | SmartPlaylistRuleOperator::Before
        | SmartPlaylistRuleOperator::After
        | SmartPlaylistRuleOperator::IsEmpty
        | SmartPlaylistRuleOperator::IsNotEmpty => false_sql(),
    }
}

fn compile_bool_rule(expression: &str, rule: &SmartPlaylistRule) -> SmartSql {
    let Some(value) = smart_policy::bool_value(rule) else {
        return false_sql();
    };
    let expected = if matches!(rule.operator, SmartPlaylistRuleOperator::IsNot) {
        !value
    } else {
        value
    };
    SmartSql {
        clause: format!("{expression} = ?"),
        params: vec![Value::from(bool_to_i64(expected))],
    }
}

fn compile_played_rule(rule: &SmartPlaylistRule) -> SmartSql {
    let Some(value) = smart_policy::bool_value(rule) else {
        return false_sql();
    };
    let expected = if matches!(rule.operator, SmartPlaylistRuleOperator::IsNot) {
        !value
    } else {
        value
    };
    let played_clause = format!(
        "({play_count} > 0 OR {last_played} IS NOT NULL)",
        play_count = smart_play_count_expr(),
        last_played = smart_last_played_expr(),
    );
    SmartSql {
        clause: if expected {
            played_clause
        } else {
            format!("NOT ({played_clause})")
        },
        params: Vec::new(),
    }
}

fn compile_date_rule(expression: &str, rule: &SmartPlaylistRule) -> SmartSql {
    match rule.operator {
        SmartPlaylistRuleOperator::IsEmpty => SmartSql {
            clause: format!("{expression} IS NULL"),
            params: Vec::new(),
        },
        SmartPlaylistRuleOperator::IsNotEmpty => SmartSql {
            clause: format!("{expression} IS NOT NULL"),
            params: Vec::new(),
        },
        SmartPlaylistRuleOperator::Before
        | SmartPlaylistRuleOperator::After
        | SmartPlaylistRuleOperator::Equals
        | SmartPlaylistRuleOperator::NotEquals => {
            let Some(value) = smart_policy::date_value(rule) else {
                return false_sql();
            };
            let operator = match rule.operator {
                SmartPlaylistRuleOperator::Before => "<",
                SmartPlaylistRuleOperator::After => ">",
                SmartPlaylistRuleOperator::Equals => "=",
                SmartPlaylistRuleOperator::NotEquals => "!=",
                SmartPlaylistRuleOperator::Contains
                | SmartPlaylistRuleOperator::NotContains
                | SmartPlaylistRuleOperator::Above
                | SmartPlaylistRuleOperator::Below
                | SmartPlaylistRuleOperator::Between
                | SmartPlaylistRuleOperator::Is
                | SmartPlaylistRuleOperator::IsNot
                | SmartPlaylistRuleOperator::IsEmpty
                | SmartPlaylistRuleOperator::IsNotEmpty => return false_sql(),
            };
            SmartSql {
                clause: format!("{expression} {operator} ?"),
                params: vec![Value::from(value)],
            }
        }
        SmartPlaylistRuleOperator::Between => {
            let Some((start, end)) = smart_policy::date_range_value(rule) else {
                return false_sql();
            };
            SmartSql {
                clause: format!("{expression} BETWEEN ? AND ?"),
                params: vec![Value::from(start), Value::from(end)],
            }
        }
        SmartPlaylistRuleOperator::Contains
        | SmartPlaylistRuleOperator::NotContains
        | SmartPlaylistRuleOperator::Above
        | SmartPlaylistRuleOperator::Below
        | SmartPlaylistRuleOperator::Is
        | SmartPlaylistRuleOperator::IsNot => false_sql(),
    }
}

fn false_sql() -> SmartSql {
    SmartSql {
        clause: "1 = 0".to_string(),
        params: Vec::new(),
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn smart_play_count_expr() -> String {
    "CASE WHEN s.provider = 'local' THEN COALESCE(ta.play_count, t.play_count, 0) ELSE COALESCE(t.play_count, 0) END".to_string()
}

fn smart_last_played_expr() -> String {
    "CASE WHEN s.provider = 'local' THEN COALESCE(ta.last_played, t.last_played) ELSE t.last_played END".to_string()
}

fn smart_skip_count_expr() -> String {
    "COALESCE(ta.skip_count, t.skip_count, 0)".to_string()
}

fn smart_order_by(field: SmartPlaylistSortField, descending: bool) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    let expression = match field {
        SmartPlaylistSortField::Title => "t.title COLLATE NOCASE".to_string(),
        SmartPlaylistSortField::Artist => "t.artist COLLATE NOCASE".to_string(),
        SmartPlaylistSortField::Album => "t.album COLLATE NOCASE".to_string(),
        SmartPlaylistSortField::Year => "t.year".to_string(),
        SmartPlaylistSortField::DateAdded => "t.date_added".to_string(),
        SmartPlaylistSortField::LastPlayed => smart_last_played_expr(),
        SmartPlaylistSortField::PlayCount => smart_play_count_expr(),
        SmartPlaylistSortField::SkipCount => smart_skip_count_expr(),
        SmartPlaylistSortField::Bpm => "t.bpm".to_string(),
        SmartPlaylistSortField::Rating => "t.user_rating".to_string(),
        SmartPlaylistSortField::Duration => "t.duration_seconds".to_string(),
    };
    let missing = match field {
        SmartPlaylistSortField::DateAdded
        | SmartPlaylistSortField::LastPlayed
        | SmartPlaylistSortField::Bpm
        | SmartPlaylistSortField::Rating => format!("{expression} IS NULL ASC, "),
        SmartPlaylistSortField::Title
        | SmartPlaylistSortField::Artist
        | SmartPlaylistSortField::Album
        | SmartPlaylistSortField::Year
        | SmartPlaylistSortField::PlayCount
        | SmartPlaylistSortField::SkipCount
        | SmartPlaylistSortField::Duration => String::new(),
    };
    format!(
        "{missing}{expression} {direction}, t.album COLLATE NOCASE {direction}, t.disc_number {direction}, t.track_number {direction}, t.title COLLATE NOCASE {direction}, t.track_id {direction}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{album, album_with_image, saved_server, track};
    use domain::SmartPlaylistRuleValue;

    #[test]
    fn smart_playlist_restored() {
        let store = Store::open_memory().expect("store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let page = store
            .load_smart_playlists(&saved.server.id, 0, 20)
            .expect("defaults");
        assert_eq!(page.total, 3);

        let most_played = smart_builtin_id(SmartPlaylistBuiltin::MostPlayed);
        store
            .delete_smart_playlist(&saved.server.id, &most_played)
            .expect("delete");
        let page = store
            .load_smart_playlists(&saved.server.id, 0, 20)
            .expect("after delete");
        assert_eq!(page.total, 2);
        assert_eq!(
            store
                .missing_builtin_smart_playlists(&saved.server.id)
                .expect("missing"),
            vec![SmartPlaylistBuiltin::MostPlayed]
        );

        store
            .restore_builtin_smart_playlist(&saved.server.id, SmartPlaylistBuiltin::MostPlayed)
            .expect("restore");
        let page = store
            .load_smart_playlists(&saved.server.id, 0, 20)
            .expect("after restore");
        assert_eq!(page.total, 3);
    }

    #[test]
    fn smart_persist_position() {
        let store = Store::open_memory().expect("store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let page = store
            .load_smart_playlists(&saved.server.id, 0, 20)
            .expect("defaults");
        let ids = page
            .items
            .iter()
            .map(|playlist| playlist.id.clone())
            .collect::<Vec<_>>();

        assert!(
            store
                .reorder_smart_playlist(&saved.server.id, &ids[2], &ids[0], false)
                .expect("move before first")
        );
        let moved = store
            .load_smart_playlists(&saved.server.id, 0, 20)
            .expect("after move")
            .items
            .into_iter()
            .map(|playlist| playlist.id)
            .collect::<Vec<_>>();

        assert_eq!(moved, vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]);
    }

    #[test]
    fn smart_track_image() {
        let store = Store::open_memory().expect("store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let album = album_with_image(1);
        let album_image = album.image_ref.clone();
        let mut track = track(1, &album);
        track.play_count = Some(1);
        store
            .upsert_albums(&saved.server.id, std::slice::from_ref(&album), 1)
            .expect("album");
        store
            .upsert_tracks(&saved.server.id, std::slice::from_ref(&track), 1)
            .expect("track");
        store
            .complete_sync(&saved.server.id, 1)
            .expect("complete sync");

        let page = store
            .load_smart_playlists(&saved.server.id, 0, 20)
            .expect("smart playlist index");
        let most_played = page
            .items
            .iter()
            .find(|playlist| playlist.builtin == Some(SmartPlaylistBuiltin::MostPlayed))
            .expect("most played");
        assert_eq!(most_played.track_count, 1);
        assert_eq!(most_played.duration_seconds, track.duration_seconds);
        assert_eq!(most_played.image_ref, album_image);
        assert_eq!(
            most_played.image_refs,
            album_image.iter().cloned().collect::<Vec<_>>()
        );

        let detail = store
            .load_smart_playlist_detail(&saved.server.id, &most_played.id)
            .expect("smart playlist detail")
            .expect("smart playlist detail");
        assert_eq!(detail.smart_playlist.track_count, 1);
        assert_eq!(
            detail.smart_playlist.duration_seconds,
            track.duration_seconds
        );
        assert_eq!(detail.smart_playlist.image_ref, album_image);
        assert_eq!(
            detail.smart_playlist.image_refs,
            album_image.iter().cloned().collect::<Vec<_>>()
        );
    }

    #[test]
    fn smart_retired_sources() {
        let store = Store::open_memory().expect("store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        store
            .load_smart_playlists(&saved.server.id, 0, 20)
            .expect("seed defaults");
        let definition = serde_json::to_string(&smart_policy::builtin_definition(
            SmartPlaylistBuiltin::MostPlayed,
        ))
        .expect("definition");
        for key in RETIRED_SMART_PLAYLIST_BUILTIN_KEYS {
            let retired_id = format!("builtin:{key}");
            store
                .connection
                .execute(
                    "
                    INSERT INTO smart_playlists (
                        server_id, smart_playlist_id, name, builtin_key, definition_json, position
                    )
                    VALUES (?1, ?2, ?3, ?3, ?4, 100)
                    ",
                    params![
                        saved.server.id.as_str(),
                        retired_id,
                        key,
                        definition.as_str()
                    ],
                )
                .expect("insert retired default");
        }

        let page = store
            .load_smart_playlists(&saved.server.id, 0, 20)
            .expect("load after prune");

        assert_eq!(page.total, 3);
        assert!(page.items.iter().all(|playlist| playlist.builtin.is_some()));
        assert!(page.items.iter().all(|playlist| {
            !RETIRED_SMART_PLAYLIST_BUILTIN_KEYS
                .iter()
                .any(|key| playlist.id.as_str() == format!("builtin:{key}"))
        }));
    }

    #[test]
    fn smart_filter_activity() {
        let store = Store::open_memory().expect("store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let album = album(1);
        let mut first = track(1, &album);
        first.title = "Signal One".to_string();
        first.comment = Some("late night favorite".to_string());
        first.genres = vec!["Dream Pop".to_string()];
        let mut second = track(2, &album);
        second.title = "Static Two".to_string();
        second.comment = Some("morning".to_string());
        second.genres = vec!["Noise".to_string()];
        store
            .upsert_albums(&saved.server.id, &[album], 1)
            .expect("album");
        store
            .upsert_tracks(&saved.server.id, &[first.clone(), second], 1)
            .expect("tracks");
        store
            .increment_track_skip_count(&saved.server.id, &first.id)
            .expect("skip");
        let definition = SmartPlaylistDefinition {
            root: SmartPlaylistRuleGroup {
                mode: SmartPlaylistMatchMode::All,
                rules: vec![
                    SmartPlaylistRuleNode::Group(SmartPlaylistRuleGroup {
                        mode: SmartPlaylistMatchMode::Any,
                        rules: vec![
                            SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                                field: SmartPlaylistRuleField::Comment,
                                operator: SmartPlaylistRuleOperator::Contains,
                                value: Some(SmartPlaylistRuleValue::Text("night".to_string())),
                            }),
                            SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                                field: SmartPlaylistRuleField::Title,
                                operator: SmartPlaylistRuleOperator::Contains,
                                value: Some(SmartPlaylistRuleValue::Text("missing".to_string())),
                            }),
                        ],
                    }),
                    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                        field: SmartPlaylistRuleField::Genre,
                        operator: SmartPlaylistRuleOperator::NotContains,
                        value: Some(SmartPlaylistRuleValue::Text("noise".to_string())),
                    }),
                    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                        field: SmartPlaylistRuleField::SkipCount,
                        operator: SmartPlaylistRuleOperator::Above,
                        value: Some(SmartPlaylistRuleValue::Number(0)),
                    }),
                ],
            },
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: None,
        };
        let smart_id = SmartPlaylistId::new("custom:night");
        store
            .save_smart_playlist(&saved.server.id, &smart_id, "Night", &definition)
            .expect("save smart");
        let detail = store
            .load_smart_playlist_detail(&saved.server.id, &smart_id)
            .expect("detail")
            .expect("detail");
        assert_eq!(detail.tracks.len(), 1);
        assert_eq!(detail.tracks[0].id, first.id);
        assert_eq!(detail.tracks[0].skip_count, Some(1));
    }

    #[test]
    fn smart_filter_range() {
        let store = Store::open_memory().expect("store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let album = album(1);
        let mut first = track(1, &album);
        first.date_added = Some("2024-02-14".to_string());
        let mut second = track(2, &album);
        second.date_added = Some("2024-05-20".to_string());
        store
            .upsert_albums(&saved.server.id, &[album], 1)
            .expect("album");
        store
            .upsert_tracks(&saved.server.id, &[first.clone(), second], 1)
            .expect("tracks");
        let definition = SmartPlaylistDefinition {
            root: SmartPlaylistRuleGroup {
                mode: SmartPlaylistMatchMode::All,
                rules: vec![SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                    field: SmartPlaylistRuleField::DateAdded,
                    operator: SmartPlaylistRuleOperator::Between,
                    value: Some(SmartPlaylistRuleValue::DateRange {
                        start: "2024-01-01".to_string(),
                        end: "2024-03-01".to_string(),
                    }),
                })],
            },
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: None,
        };
        let smart_id = SmartPlaylistId::new("custom:date-range");
        store
            .save_smart_playlist(&saved.server.id, &smart_id, "Date Range", &definition)
            .expect("save smart");

        let detail = store
            .load_smart_playlist_detail(&saved.server.id, &smart_id)
            .expect("detail")
            .expect("detail");

        assert_eq!(detail.tracks.len(), 1);
        assert_eq!(detail.tracks[0].id, first.id);
    }

    #[test]
    fn smart_filter_genre() {
        let store = Store::open_memory().expect("store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let album = album(1);
        let mut first = track(1, &album);
        first.title = "Range Match".to_string();
        first.year = 2000;
        first.genres = vec!["Rock".to_string()];
        let mut second = track(2, &album);
        second.title = "Wrong Genre".to_string();
        second.year = 2000;
        second.genres = vec!["Jazz".to_string()];
        let mut third = track(3, &album);
        third.title = "Wrong Year".to_string();
        third.year = 2005;
        third.genres = vec!["Rock".to_string()];
        store
            .upsert_albums(&saved.server.id, &[album], 1)
            .expect("album");
        store
            .upsert_tracks(&saved.server.id, &[first.clone(), second, third], 1)
            .expect("tracks");
        let definition = SmartPlaylistDefinition {
            root: SmartPlaylistRuleGroup {
                mode: SmartPlaylistMatchMode::All,
                rules: vec![
                    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                        field: SmartPlaylistRuleField::Year,
                        operator: SmartPlaylistRuleOperator::Between,
                        value: Some(SmartPlaylistRuleValue::NumberRange {
                            min: 1999,
                            max: 2001,
                        }),
                    }),
                    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                        field: SmartPlaylistRuleField::Genre,
                        operator: SmartPlaylistRuleOperator::Contains,
                        value: Some(SmartPlaylistRuleValue::Text("rock".to_string())),
                    }),
                ],
            },
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: None,
        };
        let smart_id = SmartPlaylistId::new("custom:year-genre");
        store
            .save_smart_playlist(&saved.server.id, &smart_id, "Year Genre", &definition)
            .expect("save smart");

        let detail = store
            .load_smart_playlist_detail(&saved.server.id, &smart_id)
            .expect("detail")
            .expect("detail");

        assert_eq!(detail.tracks.len(), 1);
        assert_eq!(detail.tracks[0].id, first.id);
    }

    #[test]
    fn smart_filter_mood_and_bpm() {
        let store = Store::open_memory().expect("store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        let album = album(1);
        let mut first = track(1, &album);
        first.title = "Fast Focus".to_string();
        first.bpm = Some(128);
        first.moods = vec!["Focused".to_string(), "Energetic".to_string()];
        let mut second = track(2, &album);
        second.title = "Slow Focus".to_string();
        second.bpm = Some(82);
        second.moods = vec!["Focused".to_string()];
        let mut third = track(3, &album);
        third.title = "Fast Calm".to_string();
        third.bpm = Some(130);
        third.moods = vec!["Calm".to_string()];
        store
            .upsert_albums(&saved.server.id, &[album], 1)
            .expect("album");
        store
            .upsert_tracks(&saved.server.id, &[first.clone(), second, third], 1)
            .expect("tracks");
        let definition = SmartPlaylistDefinition {
            root: SmartPlaylistRuleGroup {
                mode: SmartPlaylistMatchMode::All,
                rules: vec![
                    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                        field: SmartPlaylistRuleField::Mood,
                        operator: SmartPlaylistRuleOperator::Equals,
                        value: Some(SmartPlaylistRuleValue::Text("focused".to_string())),
                    }),
                    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
                        field: SmartPlaylistRuleField::Bpm,
                        operator: SmartPlaylistRuleOperator::Between,
                        value: Some(SmartPlaylistRuleValue::NumberRange { min: 120, max: 140 }),
                    }),
                ],
            },
            sort_field: SmartPlaylistSortField::Bpm,
            descending: false,
            limit: None,
        };
        let smart_id = SmartPlaylistId::new("custom:mood-bpm");
        store
            .save_smart_playlist(&saved.server.id, &smart_id, "Mood BPM", &definition)
            .expect("save smart");

        let detail = store
            .load_smart_playlist_detail(&saved.server.id, &smart_id)
            .expect("detail")
            .expect("detail");

        assert_eq!(detail.tracks.len(), 1);
        assert_eq!(detail.tracks[0].id, first.id);
        assert_eq!(detail.tracks[0].bpm, Some(128));
        assert_eq!(
            detail.tracks[0].moods,
            vec!["Energetic".to_string(), "Focused".to_string()]
        );
    }
}

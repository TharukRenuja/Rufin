use super::servers::{bool_to_i64, collect_rows, track_from_row, u32_from_i64};
use super::*;

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
            SELECT smart_playlist_id, name, builtin_key, definition_json
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
        self.connection.execute(
            "
            DELETE FROM smart_playlists
            WHERE server_id = ?1 AND smart_playlist_id = ?2
            ",
            params![server_id.as_str(), smart_playlist_id.as_str()],
        )?;
        Ok(())
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
                SELECT smart_playlist_id, name, builtin_key, definition_json
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
        let (track_count, duration_seconds, image_ref) =
            self.smart_playlist_stats(server_id, &row.definition)?;
        Ok(SmartPlaylist {
            id: row.id,
            name: row.name,
            builtin: row.builtin,
            definition: row.definition,
            track_count,
            duration_seconds,
            image_ref,
        })
    }

    fn smart_playlist_stats(
        &self,
        server_id: &ServerId,
        definition: &SmartPlaylistDefinition,
    ) -> StoreResult<(u32, u32, Option<ImageRef>)> {
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
        let image_ref = self
            .query_smart_playlist_tracks(server_id, definition, 0, 1)?
            .items
            .into_iter()
            .find_map(|track| track.image_ref);
        Ok((
            total.min(u32::MAX as usize) as u32,
            duration_seconds,
            image_ref,
        ))
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
                   t.local_path, t.source_format, t.comment, {skip_count} AS skip_count
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
        let tracks = collect_rows(statement.query_map(params_from_iter(values), track_from_row)?)?;
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
        let definition = definition_for_builtin(builtin);
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
        builtin,
        definition,
    })
}

pub(super) fn smart_builtin_id(builtin: SmartPlaylistBuiltin) -> SmartPlaylistId {
    SmartPlaylistId::new(format!("builtin:{}", builtin.key()))
}

fn definition_for_builtin(builtin: SmartPlaylistBuiltin) -> SmartPlaylistDefinition {
    match builtin {
        SmartPlaylistBuiltin::MostPlayed => SmartPlaylistDefinition {
            root: group_all(vec![played_rule(true)]),
            sort_field: SmartPlaylistSortField::PlayCount,
            descending: true,
            limit: None,
        },
        SmartPlaylistBuiltin::NeverPlayed => SmartPlaylistDefinition {
            root: group_all(vec![played_rule(false)]),
            sort_field: SmartPlaylistSortField::Title,
            descending: false,
            limit: None,
        },
        SmartPlaylistBuiltin::MostSkipped => SmartPlaylistDefinition {
            root: group_all(vec![number_rule(
                SmartPlaylistRuleField::SkipCount,
                SmartPlaylistRuleOperator::Above,
                0,
            )]),
            sort_field: SmartPlaylistSortField::SkipCount,
            descending: true,
            limit: None,
        },
    }
}

fn group_all(rules: Vec<SmartPlaylistRuleNode>) -> SmartPlaylistRuleGroup {
    SmartPlaylistRuleGroup {
        mode: SmartPlaylistMatchMode::All,
        rules,
    }
}

fn played_rule(played: bool) -> SmartPlaylistRuleNode {
    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
        field: SmartPlaylistRuleField::Played,
        operator: SmartPlaylistRuleOperator::Is,
        value: Some(SmartPlaylistRuleValue::Bool(played)),
    })
}

fn number_rule(
    field: SmartPlaylistRuleField,
    operator: SmartPlaylistRuleOperator,
    value: i64,
) -> SmartPlaylistRuleNode {
    SmartPlaylistRuleNode::Rule(SmartPlaylistRule {
        field,
        operator,
        value: Some(SmartPlaylistRuleValue::Number(value)),
    })
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
        SmartPlaylistRuleField::Genre => compile_genre_rule(rule),
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
            let Some(value) = text_value(rule) else {
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
            let Some(value) = text_value(rule) else {
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
        _ => false_sql(),
    }
}

fn compile_genre_rule(rule: &SmartPlaylistRule) -> SmartSql {
    let Some(value) = text_value(rule) else {
        return false_sql();
    };
    let (operator, pattern) = match rule.operator {
        SmartPlaylistRuleOperator::Equals | SmartPlaylistRuleOperator::NotEquals => {
            ("=", value.to_lowercase())
        }
        SmartPlaylistRuleOperator::Contains | SmartPlaylistRuleOperator::NotContains => {
            ("LIKE", format!("%{}%", escape_like(&value.to_lowercase())))
        }
        _ => return false_sql(),
    };
    let comparison = if operator == "LIKE" {
        "LOWER(tg.genre_name) LIKE ? ESCAPE '\\'"
    } else {
        "LOWER(tg.genre_name) = ?"
    };
    let exists = format!(
        "
        EXISTS (
            SELECT 1
            FROM track_genres tg
            WHERE tg.server_id = t.server_id
              AND tg.track_id = t.track_id
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
            let Some(value) = number_value(rule) else {
                return false_sql();
            };
            let operator = match rule.operator {
                SmartPlaylistRuleOperator::Above => ">",
                SmartPlaylistRuleOperator::Below => "<",
                SmartPlaylistRuleOperator::Equals => "=",
                SmartPlaylistRuleOperator::NotEquals => "!=",
                _ => unreachable!(),
            };
            SmartSql {
                clause: format!("{expression} {operator} ?"),
                params: vec![Value::from(value)],
            }
        }
        SmartPlaylistRuleOperator::Between => {
            let Some((min, max)) = number_range_value(rule) else {
                return false_sql();
            };
            SmartSql {
                clause: format!("{expression} BETWEEN ? AND ?"),
                params: vec![Value::from(min), Value::from(max)],
            }
        }
        _ => false_sql(),
    }
}

fn compile_bool_rule(expression: &str, rule: &SmartPlaylistRule) -> SmartSql {
    let Some(value) = bool_value(rule) else {
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
    let Some(value) = bool_value(rule) else {
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
            let Some(value) = date_value(rule) else {
                return false_sql();
            };
            let operator = match rule.operator {
                SmartPlaylistRuleOperator::Before => "<",
                SmartPlaylistRuleOperator::After => ">",
                SmartPlaylistRuleOperator::Equals => "=",
                SmartPlaylistRuleOperator::NotEquals => "!=",
                _ => unreachable!(),
            };
            SmartSql {
                clause: format!("{expression} {operator} ?"),
                params: vec![Value::from(value)],
            }
        }
        _ => false_sql(),
    }
}

fn text_value(rule: &SmartPlaylistRule) -> Option<String> {
    match rule.value.as_ref()? {
        SmartPlaylistRuleValue::Text(value) => Some(value.trim().to_string()),
        _ => None,
    }
    .filter(|value| !value.is_empty())
}

fn number_value(rule: &SmartPlaylistRule) -> Option<i64> {
    match rule.value.as_ref()? {
        SmartPlaylistRuleValue::Number(value) => Some(*value),
        _ => None,
    }
}

fn number_range_value(rule: &SmartPlaylistRule) -> Option<(i64, i64)> {
    match rule.value.as_ref()? {
        SmartPlaylistRuleValue::NumberRange { min, max } => Some((*min, *max)),
        _ => None,
    }
}

fn bool_value(rule: &SmartPlaylistRule) -> Option<bool> {
    match rule.value.as_ref()? {
        SmartPlaylistRuleValue::Bool(value) => Some(*value),
        _ => None,
    }
}

fn date_value(rule: &SmartPlaylistRule) -> Option<String> {
    match rule.value.as_ref()? {
        SmartPlaylistRuleValue::Date(value) | SmartPlaylistRuleValue::Text(value) => {
            Some(value.trim().to_string())
        }
        _ => None,
    }
    .filter(|value| !value.is_empty())
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
        SmartPlaylistSortField::Rating => "t.user_rating".to_string(),
        SmartPlaylistSortField::Duration => "t.duration_seconds".to_string(),
    };
    let missing = match field {
        SmartPlaylistSortField::DateAdded
        | SmartPlaylistSortField::LastPlayed
        | SmartPlaylistSortField::Rating => format!("{expression} IS NULL ASC, "),
        _ => String::new(),
    };
    format!(
        "{missing}{expression} {direction}, t.album COLLATE NOCASE {direction}, t.disc_number {direction}, t.track_number {direction}, t.title COLLATE NOCASE {direction}, t.track_id {direction}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::{album, saved_server, track};

    #[test]
    fn smart_playlist_defaults_seed_once_and_can_be_restored() {
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
    fn retired_default_smart_playlists_are_removed_from_existing_sources() {
        let store = Store::open_memory().expect("store");
        let saved = saved_server();
        store.save_server(&saved).expect("save server");
        store
            .load_smart_playlists(&saved.server.id, 0, 20)
            .expect("seed defaults");
        let definition =
            serde_json::to_string(&definition_for_builtin(SmartPlaylistBuiltin::MostPlayed))
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
    fn smart_playlist_rules_filter_nested_comments_genres_and_activity() {
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
}

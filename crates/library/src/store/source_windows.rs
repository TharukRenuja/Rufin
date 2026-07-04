use domain::{PlaySourceDescriptor, PlaySourceKey, PlaylistEntrySortDescriptor, SourceOrder};
use rusqlite::{OptionalExtension, Row, params_from_iter, types::Value};

use super::sources::{collect_rows, like_pattern, track_from_row_at, u32_from_i64};
use super::*;

const PLAYLIST_SOURCE_OUTPUT_COLUMNS: &str = "
    entry_id,
    track_id,
    album_id,
    title,
    artist,
    artist_id,
    album,
    year,
    release_date,
    date_added,
    last_played,
    play_count,
    user_rating,
    duration_seconds,
    favorite,
    disc_number,
    track_number,
    image_item_id,
    image_tag
";

struct PlaylistSourceQuery<'a> {
    playlist_id: &'a PlaylistId,
    query_pattern: Option<String>,
    order_by: String,
}

impl Store {
    pub fn count_tracks_for_source(
        &self,
        source_id: &SourceId,
        source: &PlaySourceKey,
    ) -> StoreResult<usize> {
        let source = playlist_source_query(source)?;
        let mut values =
            playlist_source_params(source_id, source.playlist_id, &source.query_pattern);
        let sql = format!(
            "
            SELECT COUNT(*)
            FROM playlist_tracks pt
            JOIN tracks t
                ON t.source_id = pt.source_id AND t.track_id = pt.track_id
            WHERE pt.source_id = ? AND pt.playlist_id = ?
            {}
            ",
            playlist_query_filter(source.query_pattern.is_some())
        );
        let count = self
            .connection
            .query_row(&sql, params_from_iter(values.drain(..)), |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(u32_from_i64(count) as usize)
    }

    pub fn track_rank_for_source(
        &self,
        source_id: &SourceId,
        source: &PlaySourceKey,
        track_id: &TrackId,
        source_item_id: Option<&str>,
    ) -> StoreResult<Option<usize>> {
        let source = playlist_source_query(source)?;
        let mut values =
            playlist_source_params(source_id, source.playlist_id, &source.query_pattern);
        values.push(Value::Text(track_id.as_str().to_string()));
        let entry_filter = if let Some(source_item_id) = source_item_id {
            values.push(Value::Text(source_item_id.to_string()));
            "AND entry_id = ?"
        } else {
            ""
        };
        let sql = format!(
            "
            WITH displayed AS (
                SELECT
                    ROW_NUMBER() OVER (ORDER BY {}) - 1 AS source_index,
                    pt.entry_id AS entry_id,
                    pt.track_id AS track_id
                FROM playlist_tracks pt
                JOIN tracks t
                    ON t.source_id = pt.source_id AND t.track_id = pt.track_id
                WHERE pt.source_id = ? AND pt.playlist_id = ?
                {}
            )
            SELECT source_index
            FROM displayed
            WHERE track_id = ?
            {}
            ORDER BY source_index
            LIMIT 1
            ",
            source.order_by,
            playlist_query_filter(source.query_pattern.is_some()),
            entry_filter
        );
        let rank = self
            .connection
            .query_row(&sql, params_from_iter(values), |row| row.get::<_, i64>(0))
            .optional()?;
        Ok(rank.map(|rank| u32_from_i64(rank) as usize))
    }

    pub fn tracks_window_for_source(
        &self,
        source_id: &SourceId,
        source: &PlaySourceKey,
        anchor_rank: usize,
        before: usize,
        after: usize,
    ) -> StoreResult<StoreBackedSourceWindow> {
        let source_query = playlist_source_query(source)?;
        let total_source_items = self.count_tracks_for_source(source_id, source)?;
        let requested_len = before.saturating_add(after).saturating_add(1);
        let mut start_rank = anchor_rank.saturating_sub(before).min(total_source_items);
        let end_rank = anchor_rank
            .saturating_add(after)
            .saturating_add(1)
            .min(total_source_items);
        let len = end_rank.saturating_sub(start_rank);
        if len < requested_len {
            start_rank = start_rank.saturating_sub(requested_len - len);
        }
        if start_rank >= end_rank {
            return Ok(StoreBackedSourceWindow {
                start_rank,
                total_source_items,
                items: Vec::new(),
            });
        }

        let mut values = playlist_source_params(
            source_id,
            source_query.playlist_id,
            &source_query.query_pattern,
        );
        values.push(Value::Integer(start_rank as i64));
        values.push(Value::Integer(end_rank as i64));
        let sql = format!(
            "
            WITH displayed AS (
                SELECT
                    ROW_NUMBER() OVER (ORDER BY {}) - 1 AS source_index,
                    {}
                FROM playlist_tracks pt
                JOIN tracks t
                    ON t.source_id = pt.source_id AND t.track_id = pt.track_id
                WHERE pt.source_id = ? AND pt.playlist_id = ?
                {}
            )
            SELECT source_index, {}
            FROM displayed
            WHERE source_index >= ? AND source_index < ?
            ORDER BY source_index
            ",
            source_query.order_by,
            playlist_source_columns(),
            playlist_query_filter(source_query.query_pattern.is_some()),
            PLAYLIST_SOURCE_OUTPUT_COLUMNS
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut items = collect_rows(
            statement.query_map(params_from_iter(values), source_window_item_from_row)?,
        )?;
        let mut tracks = items
            .iter()
            .map(|item| item.track.clone())
            .collect::<Vec<_>>();
        self.attach_track_metadata(source_id, &mut tracks)?;
        for (item, track) in items.iter_mut().zip(tracks) {
            item.track = track;
        }
        Ok(StoreBackedSourceWindow {
            start_rank,
            total_source_items,
            items,
        })
    }
}

fn playlist_source_columns() -> String {
    format!(
        "
    pt.entry_id AS entry_id,
    t.track_id AS track_id,
    t.album_id AS album_id,
    t.title AS title,
    t.artist AS artist,
    t.artist_id AS artist_id,
    t.album AS album,
    t.year AS year,
    t.release_date AS release_date,
    t.date_added AS date_added,
    t.last_played AS last_played,
    t.play_count AS play_count,
    t.user_rating AS user_rating,
    t.duration_seconds AS duration_seconds,
    {} AS favorite,
    t.disc_number AS disc_number,
    t.track_number AS track_number,
    t.image_item_id AS image_item_id,
    t.image_tag AS image_tag
",
        effective_track_favorite_sql("t")
    )
}

fn playlist_source_query(source: &PlaySourceKey) -> StoreResult<PlaylistSourceQuery<'_>> {
    match (&source.descriptor, &source.order) {
        (
            PlaySourceDescriptor::Playlist { playlist_id },
            SourceOrder::PlaylistDisplayed {
                query,
                sort,
                descending,
            },
        ) => Ok(PlaylistSourceQuery {
            playlist_id,
            query_pattern: query.as_deref().and_then(like_pattern),
            order_by: playlist_order_by(sort, *descending),
        }),
        _ => Err(StoreError::UnsupportedSourceWindow),
    }
}

fn playlist_source_params(
    source_id: &SourceId,
    playlist_id: &PlaylistId,
    query_pattern: &Option<String>,
) -> Vec<Value> {
    let mut values = vec![
        Value::Text(source_id.as_str().to_string()),
        Value::Text(playlist_id.as_str().to_string()),
    ];
    if let Some(query_pattern) = query_pattern {
        for _ in 0..3 {
            values.push(Value::Text(query_pattern.clone()));
        }
    }
    values
}

fn playlist_query_filter(has_query: bool) -> &'static str {
    if has_query {
        "
              AND (
                  LOWER(t.title) LIKE ? ESCAPE '\\'
                  OR LOWER(t.artist) LIKE ? ESCAPE '\\'
                  OR LOWER(t.album) LIKE ? ESCAPE '\\'
              )
        "
    } else {
        ""
    }
}

fn playlist_order_by(sort: &PlaylistEntrySortDescriptor, descending: bool) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    let primary = match sort {
        PlaylistEntrySortDescriptor::Position => "pt.position",
        PlaylistEntrySortDescriptor::Title => "LOWER(t.title)",
        PlaylistEntrySortDescriptor::Artist => "LOWER(t.artist)",
        PlaylistEntrySortDescriptor::Album => "LOWER(t.album)",
    };
    format!("{primary} {direction}, pt.position {direction}, pt.entry_id {direction}")
}

fn source_window_item_from_row(row: &Row<'_>) -> rusqlite::Result<StoreBackedSourceItem> {
    Ok(StoreBackedSourceItem {
        source_index: u32_from_i64(row.get::<_, i64>(0)?) as usize,
        source_item_id: Some(row.get(1)?),
        track: track_from_row_at(row, 2)?,
    })
}

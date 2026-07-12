use super::sources::u32_from_i64;
use super::*;

pub const LEGACY_ACTIVITY_PERIOD: &str = "legacy";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityOutcome {
    pub source_id: SourceId,
    pub period: String,
    pub track_id: TrackId,
    pub qualified_plays: u32,
    pub skips: u32,
    pub last_played_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackActivitySummary {
    pub qualified_plays: u32,
    pub skips: u32,
    pub last_played_at: Option<String>,
}

impl Store {
    pub fn record_activity_outcome(&self, outcome: &ActivityOutcome) -> StoreResult<()> {
        self.connection.execute(
            "
            INSERT INTO track_activity_period (
                source_id, period, track_id, qualified_plays, skips,
                last_played_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, datetime(?6, 'unixepoch'), CURRENT_TIMESTAMP)
            ON CONFLICT(source_id, period, track_id) DO UPDATE SET
                qualified_plays = qualified_plays + excluded.qualified_plays,
                skips = skips + excluded.skips,
                last_played_at = CASE
                    WHEN excluded.last_played_at IS NULL THEN last_played_at
                    WHEN last_played_at IS NULL
                      OR excluded.last_played_at > last_played_at
                    THEN excluded.last_played_at
                    ELSE last_played_at
                END,
                updated_at = excluded.updated_at
            ",
            params![
                outcome.source_id.as_str(),
                outcome.period,
                outcome.track_id.as_str(),
                i64::from(outcome.qualified_plays),
                i64::from(outcome.skips),
                outcome.last_played_at,
            ],
        )?;
        Ok(())
    }

    pub fn track_activity_summary(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
    ) -> StoreResult<TrackActivitySummary> {
        self.connection
            .query_row(
                "
                SELECT COALESCE(SUM(qualified_plays), 0),
                       COALESCE(SUM(skips), 0),
                       MAX(last_played_at)
                FROM track_activity_period
                WHERE source_id = ?1 AND track_id = ?2
                ",
                params![source_id.as_str(), track_id.as_str()],
                |row| {
                    Ok(TrackActivitySummary {
                        qualified_plays: u32_from_i64(row.get(0)?),
                        skips: u32_from_i64(row.get(1)?),
                        last_played_at: row.get(2)?,
                    })
                },
            )
            .map_err(StoreError::from)
    }
}

pub(super) fn lifetime_activity_join_sql() -> &'static str {
    "
    LEFT JOIN (
        SELECT source_id, track_id,
               SUM(qualified_plays) AS qualified_plays,
               SUM(skips) AS skips,
               MAX(last_played_at) AS last_played_at
        FROM track_activity_period
        GROUP BY source_id, track_id
    ) ta ON ta.source_id = t.source_id AND ta.track_id = t.track_id
    "
}

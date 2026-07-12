use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaybackCheckpointRecord {
    pub source_id: SourceId,
    pub revision: u64,
    pub selected_occurrence_id: Option<String>,
    pub progress_millis: u64,
    pub repeat_mode: String,
    pub shuffle_enabled: bool,
    pub payload: String,
}

impl Store {
    pub fn load_playback_checkpoint(
        &self,
        source_id: &SourceId,
    ) -> StoreResult<Option<PlaybackCheckpointRecord>> {
        self.connection
            .query_row(
                "
                SELECT revision, selected_occurrence_id, progress_millis,
                       repeat_mode, shuffle_enabled, payload
                FROM playback_checkpoints
                WHERE source_id = ?1
                ",
                params![source_id.as_str()],
                |row| {
                    Ok(PlaybackCheckpointRecord {
                        source_id: source_id.clone(),
                        revision: u64_from_i64(row.get(0)?),
                        selected_occurrence_id: row.get(1)?,
                        progress_millis: u64_from_i64(row.get(2)?),
                        repeat_mode: row.get(3)?,
                        shuffle_enabled: row.get(4)?,
                        payload: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn save_playback_checkpoint(&self, record: &PlaybackCheckpointRecord) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "
            INSERT INTO playback_checkpoints (
                source_id, revision, selected_occurrence_id, progress_millis,
                repeat_mode, shuffle_enabled, payload, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CURRENT_TIMESTAMP)
            ON CONFLICT(source_id) DO UPDATE SET
                revision = excluded.revision,
                selected_occurrence_id = excluded.selected_occurrence_id,
                progress_millis = excluded.progress_millis,
                repeat_mode = excluded.repeat_mode,
                shuffle_enabled = excluded.shuffle_enabled,
                payload = excluded.payload,
                updated_at = excluded.updated_at
            WHERE excluded.revision > playback_checkpoints.revision
            ",
                params![
                    record.source_id.as_str(),
                    i64_from_u64(record.revision),
                    record.selected_occurrence_id,
                    i64_from_u64(record.progress_millis),
                    record.repeat_mode,
                    record.shuffle_enabled,
                    record.payload,
                ],
            )?;
            Ok(())
        })
    }

    pub fn delete_playback_checkpoint(&self, source_id: &SourceId) -> StoreResult<bool> {
        self.write_batch(|connection| {
            connection
                .execute(
                    "DELETE FROM playback_checkpoints WHERE source_id = ?1",
                    params![source_id.as_str()],
                )
                .map(|deleted| deleted > 0)
                .map_err(StoreError::from)
        })
    }

    pub fn save_playback_progress(
        &self,
        source_id: &SourceId,
        revision: u64,
        selected_occurrence_id: &str,
        progress_millis: u64,
    ) -> StoreResult<bool> {
        self.write_batch(|connection| {
            let updated = connection.execute(
                "
            UPDATE playback_checkpoints
            SET progress_millis = ?4, updated_at = CURRENT_TIMESTAMP
            WHERE source_id = ?1
              AND revision = ?2
              AND selected_occurrence_id = ?3
            ",
                params![
                    source_id.as_str(),
                    i64_from_u64(revision),
                    selected_occurrence_id,
                    i64_from_u64(progress_millis),
                ],
            )?;
            Ok(updated > 0)
        })
    }

    pub fn save_playback_state(
        &self,
        source_id: &SourceId,
        revision: u64,
        selected_occurrence_id: Option<&str>,
        progress_millis: u64,
        repeat_mode: &str,
        shuffle_enabled: bool,
    ) -> StoreResult<bool> {
        self.write_batch(|connection| {
            let updated = connection.execute(
                "
            UPDATE playback_checkpoints
            SET selected_occurrence_id = ?3,
                progress_millis = ?4,
                repeat_mode = ?5,
                shuffle_enabled = ?6,
                updated_at = CURRENT_TIMESTAMP
            WHERE source_id = ?1 AND revision = ?2
            ",
                params![
                    source_id.as_str(),
                    i64_from_u64(revision),
                    selected_occurrence_id,
                    i64_from_u64(progress_millis),
                    repeat_mode,
                    shuffle_enabled,
                ],
            )?;
            Ok(updated > 0)
        })
    }
}

fn i64_from_u64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn u64_from_i64(value: i64) -> u64 {
    value.max(0) as u64
}

use super::local_manifest::clear_local_manifest_on_connection;
use super::*;

pub(super) fn reset_database_files(path: &Path) -> StoreResult<()> {
    remove_file_if_exists(path)?;
    remove_file_if_exists(&sqlite_sidecar_path(path, "-wal"))?;
    remove_file_if_exists(&sqlite_sidecar_path(path, "-shm"))?;
    Ok(())
}
pub(super) fn remove_file_if_exists(path: &Path) -> StoreResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::from(error)),
    }
}
pub(super) fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}
pub fn image_cache_key(server_id: &ServerId, item_id: &str, image_tag: &str, size: u32) -> String {
    format!(
        "{}/{}/{}/{}",
        encode_key_part(server_id.as_str()),
        encode_key_part(item_id),
        encode_key_part(image_tag),
        size
    )
}
pub fn lyrics_cache_key(server_id: &ServerId, track_id: &str) -> String {
    format!(
        "{}/{}",
        encode_key_part(server_id.as_str()),
        encode_key_part(track_id)
    )
}
pub(super) const COLLECTION_COVER_GENRE: &str = "genre";
pub(super) const COLLECTION_COVER_PLAYLIST: &str = "playlist";
pub(super) const COLLECTION_COVER_SMART_PLAYLIST: &str = "smart_playlist";
pub(super) const IMAGE_ORIGIN_EXTERNAL: &str = "external";
pub(super) const IMAGE_ORIGIN_SOURCE: &str = "source";
pub(super) const IMAGE_ORIGIN_UNKNOWN: &str = "unknown";
pub(super) fn image_origin_for_source_ref(image_ref: Option<&ImageRef>) -> &'static str {
    match image_ref {
        Some(image_ref) if image_ref.item_id.starts_with("external:") => IMAGE_ORIGIN_EXTERNAL,
        Some(_) => IMAGE_ORIGIN_SOURCE,
        None => IMAGE_ORIGIN_UNKNOWN,
    }
}
pub(super) fn saved_server_from_row(row: &Row<'_>) -> rusqlite::Result<SavedServer> {
    Ok(SavedServer {
        server: ServerIdentity {
            id: ServerId::new(row.get::<_, String>(0)?),
            provider: row.get(1)?,
            name: row.get(2)?,
            base_url: row.get(3)?,
        },
        user_id: row.get(4)?,
        username: row.get(5)?,
        trust_invalid_cert: row.get::<_, i64>(6)? == 1,
    })
}
pub(super) fn album_from_row(row: &Row<'_>) -> rusqlite::Result<Album> {
    let artist_id = row.get::<_, Option<String>>(3)?.map(ArtistId::new);
    Ok(Album {
        id: AlbumId::new(row.get::<_, String>(0)?),
        title: row.get(1)?,
        artist: row.get(2)?,
        artist_id,
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year: u16_from_i64(row.get(4)?),
        release_date: row.get(5)?,
        date_added: row.get(6)?,
        last_played: row.get(7)?,
        play_count: optional_u32_from_row(row, 8)?,
        user_rating: optional_u8_from_row(row, 9)?,
        track_count: u16_from_i64(row.get(10)?),
        duration_seconds: u32_from_i64(row.get(11)?),
        favorite: row.get::<_, i64>(12)? == 1,
        color_seed: u32_from_i64(row.get(13)?),
        image_ref: image_ref_from_row(row, 14, 15)?,
        genres: Vec::new(),
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
    })
}
pub(super) fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    track_from_row_at(row, 0)
}
pub(super) fn playlist_entry_from_row(row: &Row<'_>) -> rusqlite::Result<PlaylistEntry> {
    Ok(PlaylistEntry {
        entry_id: row.get(0)?,
        track: track_from_row_at(row, 1)?,
    })
}
pub(super) fn track_from_row_at(row: &Row<'_>, offset: usize) -> rusqlite::Result<Track> {
    let artist_id = row.get::<_, Option<String>>(offset + 4)?.map(ArtistId::new);
    Ok(Track {
        id: TrackId::new(row.get::<_, String>(offset)?),
        album_id: AlbumId::new(row.get::<_, String>(offset + 1)?),
        title: row.get(offset + 2)?,
        artist: row.get(offset + 3)?,
        artist_id,
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: row.get(offset + 5)?,
        year: u16_from_i64(row.get(offset + 6)?),
        release_date: row.get(offset + 7)?,
        date_added: row.get(offset + 8)?,
        last_played: row.get(offset + 9)?,
        play_count: optional_u32_from_row(row, offset + 10)?,
        user_rating: optional_u8_from_row(row, offset + 11)?,
        duration_seconds: u32_from_i64(row.get(offset + 12)?),
        favorite: row.get::<_, i64>(offset + 13)? == 1,
        disc_number: u16_from_i64(row.get(offset + 14)?),
        track_number: u16_from_i64(row.get(offset + 15)?),
        image_ref: image_ref_from_row(row, offset + 16, offset + 17)?,
        genres: Vec::new(),
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        local_path: row.get::<_, Option<String>>(offset + 18).ok().flatten(),
        source_format: row.get::<_, Option<String>>(offset + 19).ok().flatten(),
        comment: row.get::<_, Option<String>>(offset + 20).ok().flatten(),
        skip_count: row
            .get::<_, Option<i64>>(offset + 21)
            .ok()
            .flatten()
            .map(u32_from_i64),
    })
}
pub(super) fn artist_from_row(row: &Row<'_>) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: ArtistId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        album_count: u32_from_i64(row.get(2)?),
        track_count: u32_from_i64(row.get(3)?),
        favorite: row.get::<_, i64>(4)? == 1,
        last_played: row.get(5)?,
        play_count: optional_u32_from_row(row, 6)?,
        user_rating: optional_u8_from_row(row, 7)?,
        musicbrainz_artist_id: None,
        image_ref: image_ref_from_row(row, 8, 9)?,
    })
}
pub(super) fn optional_u32_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<u32>> {
    row.get::<_, Option<i64>>(index)
        .map(|value| value.map(u32_from_i64))
}
pub(super) fn optional_u8_from_row(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<u8>> {
    row.get::<_, Option<i64>>(index)
        .map(|value| value.map(|value| u16_from_i64(value).min(u16::from(u8::MAX)) as u8))
}
pub(super) fn optional_string_column(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<String>> {
    optional_column(row, index).map(|value: Option<String>| {
        value.and_then(|value| (!value.trim().is_empty()).then_some(value))
    })
}
fn optional_column<T: rusqlite::types::FromSql>(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<T>> {
    match row.get::<_, Option<T>>(index) {
        Ok(value) => Ok(value),
        Err(rusqlite::Error::InvalidColumnIndex(_)) => Ok(None),
        Err(error) => Err(error),
    }
}
pub(super) fn album_release_types_json(types: &[String]) -> StoreResult<String> {
    Ok(serde_json::to_string(&normalize_release_types(types))?)
}
pub(super) fn album_release_types_from_json(
    value: Option<String>,
    index: usize,
) -> rusqlite::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let types = serde_json::from_str::<Vec<String>>(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;
    Ok(normalize_release_types(types))
}
pub(super) fn string_vec_json(values: &[String]) -> StoreResult<String> {
    Ok(serde_json::to_string(values)?)
}
pub(super) fn string_vec_from_json(
    value: Option<String>,
    index: usize,
) -> rusqlite::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = serde_json::from_str::<Vec<String>>(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;
    Ok(values)
}
pub(super) fn image_ref_from_row(
    row: &Row<'_>,
    item_index: usize,
    tag_index: usize,
) -> rusqlite::Result<Option<ImageRef>> {
    let Some(item_id) = row.get::<_, Option<String>>(item_index)? else {
        return Ok(None);
    };
    Ok(Some(ImageRef {
        item_id,
        tag: row.get::<_, Option<String>>(tag_index)?,
    }))
}
pub(super) fn image_ref_parts(image_ref: Option<&ImageRef>) -> (Option<&str>, Option<&str>) {
    match image_ref {
        Some(image_ref) => (Some(image_ref.item_id.as_str()), image_ref.tag.as_deref()),
        None => (None, None),
    }
}
pub(super) fn collection_cover_ref_from_row(
    row: &Row<'_>,
    item_index: usize,
    tag_index: usize,
) -> rusqlite::Result<ImageRef> {
    Ok(ImageRef {
        item_id: row.get(item_index)?,
        tag: row.get(tag_index)?,
    })
}
pub(super) fn replace_collection_refs(
    connection: &Connection,
    server_id: &ServerId,
    collection_type: &str,
    collection_id: &str,
    image_refs: &[ImageRef],
) -> StoreResult<()> {
    connection.execute(
        "
        DELETE FROM collection_cover_refs
        WHERE server_id = ?1
          AND collection_type = ?2
          AND collection_id = ?3
        ",
        params![server_id.as_str(), collection_type, collection_id],
    )?;
    let mut insert = connection.prepare(
        "
        INSERT INTO collection_cover_refs (
            server_id, collection_type, collection_id, position,
            image_item_id, image_tag, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
        ON CONFLICT(server_id, collection_type, collection_id, position) DO UPDATE SET
            image_item_id = excluded.image_item_id,
            image_tag = excluded.image_tag,
            updated_at = excluded.updated_at
        ",
    )?;
    let mut seen = HashSet::<(String, Option<String>)>::new();
    let mut position = 0usize;
    for image_ref in image_refs {
        if !seen.insert((image_ref.item_id.clone(), image_ref.tag.clone())) {
            continue;
        }
        if position >= 4 {
            break;
        }
        let (image_item_id, image_tag) = image_ref_parts(Some(image_ref));
        insert.execute(params![
            server_id.as_str(),
            collection_type,
            collection_id,
            position as i64,
            image_item_id,
            image_tag,
        ])?;
        position += 1;
    }
    Ok(())
}
pub(super) fn artist_fallback_image_refs_sql(
    album_artist: bool,
    values_placeholders: &str,
) -> String {
    if album_artist {
        return format!(
            "
            WITH wanted(artist_id) AS (VALUES {values_placeholders}),
                 candidates AS (
                    SELECT w.artist_id, a.image_item_id, a.image_tag,
                           0 AS priority, a.year, a.title
                    FROM wanted w
                    JOIN albums a
                        ON a.artist_id = w.artist_id
                    WHERE a.server_id = ?
                      AND a.image_item_id IS NOT NULL
                      AND a.image_origin IN ('source', 'unknown', 'external')
                    UNION ALL
                    SELECT w.artist_id, a.image_item_id, a.image_tag,
                           1 AS priority, a.year, a.title
                    FROM wanted w
                    JOIN album_artist_links aal
                        ON aal.artist_id = w.artist_id
                    JOIN albums a
                        ON a.server_id = aal.server_id AND a.album_id = aal.album_id
                    WHERE aal.server_id = ?
                      AND a.image_item_id IS NOT NULL
                      AND a.image_origin IN ('source', 'unknown', 'external')
                    UNION ALL
                    SELECT w.artist_id, a.image_item_id, a.image_tag,
                           2 AS priority, a.year, a.title
                    FROM wanted w
                    JOIN tracks t
                        ON t.artist_id = w.artist_id
                    JOIN albums a
                        ON a.server_id = t.server_id AND a.album_id = t.album_id
                    WHERE t.server_id = ?
                      AND a.image_item_id IS NOT NULL
                      AND a.image_origin IN ('source', 'unknown', 'external')
                    UNION ALL
                    SELECT w.artist_id, a.image_item_id, a.image_tag,
                           3 AS priority, a.year, a.title
                    FROM wanted w
                    JOIN track_artist_links tal
                        ON tal.artist_id = w.artist_id
                    JOIN albums a
                        ON a.server_id = tal.server_id AND a.album_id = tal.album_id
                    WHERE tal.server_id = ?
                      AND a.image_item_id IS NOT NULL
                      AND a.image_origin IN ('source', 'unknown', 'external')
                    UNION ALL
                    SELECT w.artist_id, t.image_item_id, t.image_tag,
                           4 AS priority, t.year, t.title
                    FROM wanted w
                    JOIN tracks t
                        ON t.artist_id = w.artist_id
                    WHERE t.server_id = ?
                      AND t.image_item_id IS NOT NULL
                      AND t.image_origin IN ('source', 'unknown', 'external')
                    UNION ALL
                    SELECT w.artist_id, t.image_item_id, t.image_tag,
                           5 AS priority, t.year, t.title
                    FROM wanted w
                    JOIN track_artist_links tal
                        ON tal.artist_id = w.artist_id
                    JOIN tracks t
                        ON t.server_id = tal.server_id AND t.track_id = tal.track_id
                    WHERE tal.server_id = ?
                      AND t.image_item_id IS NOT NULL
                      AND t.image_origin IN ('source', 'unknown', 'external')
                    UNION ALL
                    SELECT w.artist_id, a.image_item_id, a.image_tag,
                           6 AS priority, a.year, a.title
                    FROM wanted w
                    JOIN album_artists aa
                        ON aa.artist_id = w.artist_id
                    JOIN album_artist_links aal
                        ON aal.server_id = aa.server_id
                       AND aal.name = aa.name
                       AND aal.artist_id <> w.artist_id
                    JOIN albums a
                        ON a.server_id = aal.server_id AND a.album_id = aal.album_id
                    WHERE aa.server_id = ?
                      AND a.image_item_id IS NOT NULL
                      AND a.image_origin IN ('source', 'unknown', 'external')
                 )
            SELECT artist_id, image_item_id, image_tag
            FROM candidates
            ORDER BY CASE WHEN image_item_id LIKE 'external:%' THEN 1 ELSE 0 END,
                     priority, year, title COLLATE NOCASE
            "
        );
    }

    format!(
        "
        WITH wanted(artist_id) AS (VALUES {values_placeholders}),
             candidates AS (
                SELECT w.artist_id, a.image_item_id, a.image_tag,
                       0 AS priority, a.year, a.title
                FROM wanted w
                JOIN albums a
                    ON a.artist_id = w.artist_id
              WHERE a.server_id = ?
                AND a.image_item_id IS NOT NULL
                AND a.image_origin IN ('source', 'unknown', 'external')
                UNION ALL
                SELECT w.artist_id, a.image_item_id, a.image_tag,
                       1 AS priority, a.year, a.title
                FROM wanted w
                JOIN tracks t
                    ON t.artist_id = w.artist_id
                JOIN albums a
                    ON a.server_id = t.server_id AND a.album_id = t.album_id
              WHERE t.server_id = ?
                AND a.image_item_id IS NOT NULL
                AND a.image_origin IN ('source', 'unknown', 'external')
                UNION ALL
                SELECT w.artist_id, a.image_item_id, a.image_tag,
                       2 AS priority, a.year, a.title
                FROM wanted w
                JOIN track_artist_links tal
                    ON tal.artist_id = w.artist_id
                JOIN albums a
                    ON a.server_id = tal.server_id AND a.album_id = tal.album_id
              WHERE tal.server_id = ?
                AND a.image_item_id IS NOT NULL
                AND a.image_origin IN ('source', 'unknown', 'external')
                UNION ALL
                SELECT w.artist_id, a.image_item_id, a.image_tag,
                       3 AS priority, a.year, a.title
                FROM wanted w
                JOIN album_artist_links aal
                    ON aal.artist_id = w.artist_id
                JOIN albums a
                    ON a.server_id = aal.server_id AND a.album_id = aal.album_id
              WHERE aal.server_id = ?
                AND a.image_item_id IS NOT NULL
                AND a.image_origin IN ('source', 'unknown', 'external')
                UNION ALL
                SELECT w.artist_id, t.image_item_id, t.image_tag,
                       4 AS priority, t.year, t.title
                FROM wanted w
                JOIN tracks t
                    ON t.artist_id = w.artist_id
              WHERE t.server_id = ?
                AND t.image_item_id IS NOT NULL
                AND t.image_origin IN ('source', 'unknown', 'external')
                UNION ALL
                SELECT w.artist_id, t.image_item_id, t.image_tag,
                       5 AS priority, t.year, t.title
                FROM wanted w
                JOIN track_artist_links tal
                    ON tal.artist_id = w.artist_id
                JOIN tracks t
                    ON t.server_id = tal.server_id AND t.track_id = tal.track_id
              WHERE tal.server_id = ?
                AND t.image_item_id IS NOT NULL
                AND t.image_origin IN ('source', 'unknown', 'external')
             )
        SELECT artist_id, image_item_id, image_tag
        FROM candidates
        ORDER BY CASE WHEN image_item_id LIKE 'external:%' THEN 1 ELSE 0 END,
                 priority, year, title COLLATE NOCASE
        "
    )
}
pub(super) fn artist_list_filter(album_artist: bool) -> &'static str {
    if album_artist {
        ""
    } else {
        "
              AND track_count > 0
              AND (
                  EXISTS (
                      SELECT 1
                      FROM tracks t
                      WHERE t.server_id = artists.server_id
                        AND t.artist_id = artists.artist_id
                  )
                  OR NOT EXISTS (
                      SELECT 1
                      FROM track_artist_links tal
                      WHERE tal.server_id = artists.server_id
                        AND tal.artist_id = artists.artist_id
                  )
              )"
    }
}
pub(super) fn artist_list_filter_for_alias(album_artist: bool, alias: &str) -> String {
    if album_artist {
        String::new()
    } else {
        format!(
            "
              AND {alias}.track_count > 0
              AND (
                  EXISTS (
                      SELECT 1
                      FROM tracks t
                      WHERE t.server_id = {alias}.server_id
                        AND t.artist_id = {alias}.artist_id
                  )
                  OR NOT EXISTS (
                      SELECT 1
                      FROM track_artist_links tal
                      WHERE tal.server_id = {alias}.server_id
                        AND tal.artist_id = {alias}.artist_id
                  )
              )"
        )
    }
}
pub(super) fn album_artist_credits(album: &Album) -> Vec<ArtistCredit> {
    explicit_artist_credits(&album.album_artist_credits)
}
pub(super) fn track_artist_credits(track: &Track) -> Vec<ArtistCredit> {
    artist_credits_or_scalar(
        &track.artist_credits,
        track.artist_id.as_ref(),
        &track.artist,
    )
}
pub(super) fn explicit_artist_credits(credits: &[ArtistCredit]) -> Vec<ArtistCredit> {
    artist_credits_or_scalar(credits, None, "")
}
pub(super) fn canonical_album_for_delta(
    connection: &Connection,
    server_id: &ServerId,
    album: &Album,
) -> StoreResult<Album> {
    let mut normalized = album.clone();
    let mut album_artist_credits = Vec::new();
    for credit in &album.album_artist_credits {
        let parts = split_compound_credit_name(&credit.name);
        if parts.len() <= 1 {
            album_artist_credits.push(credit.clone());
            continue;
        }
        let mut resolved = Vec::new();
        for part in &parts {
            let Some(artist_id) =
                unique_track_artist_for_album_name(connection, server_id, album.id.as_str(), part)?
            else {
                resolved.clear();
                break;
            };
            resolved.push(ArtistCredit {
                id: artist_id,
                name: part.clone(),
                musicbrainz_artist_id: None,
            });
        }
        if resolved.len() == parts.len() {
            if normalized.artist_id.as_ref() == Some(&credit.id)
                && let Some(first) = resolved.first()
            {
                normalized.artist_id = Some(first.id.clone());
            }
            album_artist_credits.extend(resolved);
        } else {
            album_artist_credits.push(credit.clone());
        }
    }
    normalized.album_artist_credits = album_artist_credits;
    Ok(normalized)
}
pub(super) fn canonical_album_for_write(
    connection: &Connection,
    server_id: &ServerId,
    album: &Album,
) -> StoreResult<Album> {
    let mut album = canonical_album_for_delta(connection, server_id, album)?;
    for credit in &mut album.album_artist_credits {
        if let Some(entity_id) = album_artist_alias_target(connection, server_id, &credit.id)?
            && entity_id != credit.id
        {
            credit.id = entity_id;
        }
    }
    if let Some(artist_id) = album.artist_id.as_ref()
        && let Some(entity_id) = album_artist_alias_target(connection, server_id, artist_id)?
        && entity_id != *artist_id
    {
        album.artist_id = Some(entity_id);
    }
    Ok(album)
}
pub(super) fn artist_credits_or_scalar(
    credits: &[ArtistCredit],
    scalar_id: Option<&ArtistId>,
    scalar_name: &str,
) -> Vec<ArtistCredit> {
    let mut result = Vec::new();
    for credit in credits {
        if result
            .iter()
            .any(|existing: &ArtistCredit| existing.id == credit.id)
        {
            continue;
        }
        let name = credit.name.trim();
        result.push(ArtistCredit {
            id: credit.id.clone(),
            name: if name.is_empty() {
                credit.id.as_str().to_string()
            } else {
                name.to_string()
            },
            musicbrainz_artist_id: credit.musicbrainz_artist_id.clone(),
        });
    }

    if result.is_empty()
        && let Some(artist_id) = scalar_id
    {
        let name = scalar_name.trim();
        result.push(ArtistCredit {
            id: artist_id.clone(),
            name: if name.is_empty() {
                artist_id.as_str().to_string()
            } else {
                name.to_string()
            },
            musicbrainz_artist_id: None,
        });
    }

    result
}
pub(super) fn synthesize_album_from_tracks(album_id: &AlbumId, tracks: &[Track]) -> Album {
    let Some(first) = tracks.first() else {
        return Album {
            id: album_id.clone(),
            title: album_id.as_str().to_string(),
            artist: String::new(),
            artist_id: None,
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 0,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 0,
            duration_seconds: 0,
            favorite: false,
            color_seed: stable_seed(album_id.as_str()),
            image_ref: None,
            genres: Vec::new(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
        };
    };
    Album {
        id: album_id.clone(),
        title: first.album.clone(),
        artist: first.artist.clone(),
        artist_id: first.artist_id.clone(),
        album_artist_credits: first.album_artist_credits.clone(),
        artist_credits: Vec::new(),
        year: first.year,
        release_date: first.release_date.clone(),
        date_added: first.date_added.clone(),
        last_played: first.last_played.clone(),
        play_count: first.play_count,
        user_rating: first.user_rating,
        track_count: tracks.len().min(usize::from(u16::MAX)) as u16,
        duration_seconds: tracks
            .iter()
            .map(|track| track.duration_seconds)
            .fold(0_u32, u32::saturating_add),
        favorite: tracks.iter().any(|track| track.favorite),
        color_seed: stable_seed(album_id.as_str()),
        image_ref: first.image_ref.clone(),
        genres: first.genres.clone(),
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
    }
}
pub(super) fn track_matches_artist(
    track: &Track,
    artist_id: &ArtistId,
    artist_name_lower: Option<&str>,
) -> bool {
    if track.artist_id.as_ref() == Some(artist_id) {
        return true;
    }
    if track
        .artist_credits
        .iter()
        .any(|artist| &artist.id == artist_id)
    {
        return true;
    }

    track.artist_id.is_none()
        && artist_name_lower
            .map(|artist_name| track.artist.to_lowercase() == artist_name)
            .unwrap_or(false)
}
pub(super) fn artist_fallback_image_ref(
    albums: &[Album],
    appears_on: &[Album],
    tracks: &[Track],
) -> Option<ImageRef> {
    albums
        .first()
        .and_then(|album| album.image_ref.clone())
        .or_else(|| appears_on.first().and_then(|album| album.image_ref.clone()))
        .or_else(|| tracks.first().and_then(|track| track.image_ref.clone()))
}
pub(super) fn synthesize_artist_from_links(
    artist_id: &ArtistId,
    albums: &[Album],
    appears_on: &[Album],
    tracks: &[Track],
) -> Artist {
    let name = tracks
        .iter()
        .find(|track| track.artist_id.as_ref() == Some(artist_id))
        .map(|track| track.artist.clone())
        .or_else(|| {
            albums
                .iter()
                .find(|album| album.artist_id.as_ref() == Some(artist_id))
                .map(|album| album.artist.clone())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| artist_id.as_str().to_string());

    let mut album_ids = Vec::new();
    for album in albums.iter().chain(appears_on.iter()) {
        if !album_ids.contains(&album.id) {
            album_ids.push(album.id.clone());
        }
    }
    for track in tracks {
        if !album_ids.contains(&track.album_id) {
            album_ids.push(track.album_id.clone());
        }
    }

    Artist {
        id: artist_id.clone(),
        name,
        album_count: album_ids.len().min(u32::MAX as usize) as u32,
        track_count: tracks.len().min(u32::MAX as usize) as u32,
        favorite: false,
        last_played: None,
        play_count: None,
        user_rating: None,
        musicbrainz_artist_id: None,
        image_ref: artist_fallback_image_ref(albums, appears_on, tracks),
    }
}
pub(super) fn genre_from_row(row: &Row<'_>) -> rusqlite::Result<Genre> {
    Ok(Genre {
        id: GenreId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        album_count: u32_from_i64(row.get(2)?),
        track_count: u32_from_i64(row.get(3)?),
        duration_seconds: u32_from_i64(row.get(4)?),
        image_refs: Vec::new(),
        image_ref: image_ref_from_row(row, 5, 6)?,
    })
}
pub(super) fn playlist_from_row(row: &Row<'_>) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: PlaylistId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        track_count: u32_from_i64(row.get(2)?),
        duration_seconds: u32_from_i64(row.get(3)?),
        top_genres: string_vec_from_json(row.get(4)?, 4)?,
        image_refs: Vec::new(),
        image_ref: image_ref_from_row(row, 5, 6)?,
    })
}
pub(super) fn stable_seed(value: &str) -> u32 {
    value.bytes().fold(0x811c_9dc5, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    })
}
pub(super) fn repair_linked_artists(
    connection: &Connection,
    server_id: &ServerId,
    generation: i64,
) -> StoreResult<()> {
    let album_artist_aliases =
        normalize_compound_album_artist_links(connection, server_id, generation)?;
    connection.execute(
        "
        INSERT INTO artists (
            server_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT t.server_id,
               t.artist_id,
               MIN(t.artist),
               COUNT(DISTINCT t.album_id),
               COUNT(*),
               MAX(t.favorite),
               ?2
        FROM tracks t
        WHERE t.server_id = ?1
          AND t.artist_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM artists a
              WHERE a.server_id = t.server_id AND a.artist_id = t.artist_id
          )
        GROUP BY t.server_id, t.artist_id
        ",
        params![server_id.as_str(), generation],
    )?;
    connection.execute(
        "
        INSERT INTO artists (
            server_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT tal.server_id,
               tal.artist_id,
               MIN(tal.name),
               COUNT(DISTINCT tal.album_id),
               COUNT(DISTINCT tal.track_id),
               COALESCE(MAX(t.favorite), 0),
               ?2
        FROM track_artist_links tal
        LEFT JOIN tracks t
            ON t.server_id = tal.server_id AND t.track_id = tal.track_id
        WHERE tal.server_id = ?1
          AND NOT EXISTS (
              SELECT 1 FROM artists a
              WHERE a.server_id = tal.server_id AND a.artist_id = tal.artist_id
          )
        GROUP BY tal.server_id, tal.artist_id
        ",
        params![server_id.as_str(), generation],
    )?;
    connection.execute(
        "
        INSERT INTO album_artists (
            server_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT a.server_id,
               a.artist_id,
               MIN(a.artist),
               COUNT(*),
               COALESCE(SUM(a.track_count), 0),
               MAX(a.favorite),
               ?2
        FROM albums a
        WHERE a.server_id = ?1
          AND a.artist_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM album_artists aa
              WHERE aa.server_id = a.server_id AND aa.artist_id = a.artist_id
          )
        GROUP BY a.server_id, a.artist_id
        ",
        params![server_id.as_str(), generation],
    )?;
    connection.execute(
        "
        INSERT INTO album_artists (
            server_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT aal.server_id,
               aal.artist_id,
               MIN(aal.name),
               COUNT(DISTINCT aal.album_id),
               COALESCE(SUM(a.track_count), 0),
               COALESCE(MAX(a.favorite), 0),
               ?2
        FROM album_artist_links aal
        LEFT JOIN albums a
            ON a.server_id = aal.server_id AND a.album_id = aal.album_id
        WHERE aal.server_id = ?1
          AND NOT EXISTS (
              SELECT 1 FROM album_artists aa
              WHERE aa.server_id = aal.server_id AND aa.artist_id = aal.artist_id
          )
        GROUP BY aal.server_id, aal.artist_id
        ",
        params![server_id.as_str(), generation],
    )?;
    connection.execute(
        "
        UPDATE album_artists
        SET name = (
            SELECT MIN(aal.name)
            FROM album_artist_links aal
            WHERE aal.server_id = album_artists.server_id
              AND aal.artist_id = album_artists.artist_id
              AND TRIM(aal.name) <> ''
        )
        WHERE server_id = ?1
          AND EXISTS (
              SELECT 1
              FROM album_artist_links aal
              WHERE aal.server_id = album_artists.server_id
                AND aal.artist_id = album_artists.artist_id
                AND TRIM(aal.name) <> ''
          )
        ",
        params![server_id.as_str()],
    )?;
    for (alias_id, canonical_id) in album_artist_aliases {
        merge_album_artist_alias(connection, server_id, &canonical_id, &alias_id, generation)?;
    }
    refresh_artist_fts(connection, server_id, "artists", "artist")?;
    refresh_artist_fts(connection, server_id, "album_artists", "album_artist")?;
    Ok(())
}

pub(super) struct CanonicalArtist {
    pub artist: Artist,
    pub alias_ids: Vec<ArtistId>,
}

pub(super) fn canonical_album_artists_for_write(
    connection: &Connection,
    server_id: &ServerId,
    artists: &[Artist],
) -> StoreResult<Vec<CanonicalArtist>> {
    let mut musicbrainz_ids = HashMap::<String, ArtistId>::new();
    let mut indexes = HashMap::<ArtistId, usize>::new();
    let mut result = Vec::<CanonicalArtist>::new();
    for artist in artists {
        let alias_id = artist.id.clone();
        let musicbrainz_artist_id =
            clean_artist_identity_value(artist.musicbrainz_artist_id.as_deref());
        let canonical_id = canonical_album_artist_id_for_write(connection, server_id, artist)?
            .or_else(|| {
                musicbrainz_artist_id.and_then(|artist_id| musicbrainz_ids.get(artist_id).cloned())
            })
            .unwrap_or_else(|| artist.id.clone());
        if let Some(artist_id) = musicbrainz_artist_id {
            musicbrainz_ids
                .entry(artist_id.to_string())
                .or_insert_with(|| canonical_id.clone());
        }
        let alias_id = (canonical_id != alias_id).then_some(alias_id);
        if let Some(index) = indexes.get(&canonical_id).copied() {
            if let Some(alias_id) = alias_id {
                result[index].alias_ids.push(alias_id);
            }
            if result[index].artist.musicbrainz_artist_id.is_none()
                && artist.musicbrainz_artist_id.is_some()
            {
                result[index].artist.musicbrainz_artist_id = artist.musicbrainz_artist_id.clone();
            }
            continue;
        }
        let mut artist = artist.clone();
        artist.id = canonical_id.clone();
        let alias_ids = alias_id.into_iter().collect();
        indexes.insert(canonical_id, result.len());
        result.push(CanonicalArtist { artist, alias_ids });
    }
    Ok(result)
}

fn album_artist_alias_target(
    connection: &Connection,
    server_id: &ServerId,
    artist_id: &ArtistId,
) -> StoreResult<Option<ArtistId>> {
    connection
        .query_row(
            "
            SELECT entity_id
            FROM entity_identity_keys
            WHERE server_id = ?1
              AND entity_kind = 'album_artist'
              AND namespace = 'source:artist_id'
              AND value = ?2
            LIMIT 1
            ",
            params![server_id.as_str(), artist_id.as_str()],
            |row| row.get::<_, String>(0).map(ArtistId::new),
        )
        .optional()
        .map_err(StoreError::from)
}

pub(super) fn apply_album_artist_alias(
    connection: &Connection,
    server_id: &ServerId,
    canonical_id: &ArtistId,
    alias_id: &ArtistId,
) -> StoreResult<()> {
    connection.execute(
        "
        INSERT INTO entity_identity_keys (
            server_id, entity_kind, namespace, value, entity_id, source, strength, updated_at
        )
        VALUES (?1, 'album_artist', 'source:artist_id', ?2, ?3, 'provider', 100, CURRENT_TIMESTAMP)
        ON CONFLICT(server_id, entity_kind, namespace, value) DO UPDATE SET
            entity_id = excluded.entity_id,
            source = excluded.source,
            strength = excluded.strength,
            updated_at = excluded.updated_at
        ",
        params![server_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        INSERT INTO album_artist_links (
            server_id, album_id, artist_id, name, position, sync_generation
        )
        SELECT server_id, album_id, ?3, name, position, sync_generation
        FROM album_artist_links
        WHERE server_id = ?1
          AND artist_id = ?2
        ON CONFLICT(server_id, album_id, artist_id) DO UPDATE SET
            sync_generation = MAX(sync_generation, excluded.sync_generation)
        ",
        params![server_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM album_artist_links
        WHERE server_id = ?1
          AND artist_id = ?2
          AND artist_id <> ?3
        ",
        params![server_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        UPDATE albums
        SET artist_id = ?3
        WHERE server_id = ?1
          AND artist_id = ?2
          AND artist_id <> ?3
        ",
        params![server_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM entity_identity_keys
        WHERE server_id = ?1
          AND entity_kind = 'album_artist'
          AND entity_id = ?2
          AND entity_id <> ?3
        ",
        params![server_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM album_artists
        WHERE server_id = ?1
          AND artist_id = ?2
          AND artist_id <> ?3
        ",
        params![server_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM library_fts
        WHERE server_id = ?1
          AND item_type = 'album_artist'
          AND item_id = ?2
        ",
        params![server_id.as_str(), alias_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM entities
        WHERE server_id = ?1
          AND entity_kind = 'album_artist'
          AND entity_id = ?2
          AND entity_id <> ?3
        ",
        params![server_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    Ok(())
}

fn canonical_album_artist_id_for_write(
    connection: &Connection,
    server_id: &ServerId,
    artist: &Artist,
) -> StoreResult<Option<ArtistId>> {
    if let Some(artist_id) = clean_artist_identity_value(artist.musicbrainz_artist_id.as_deref())
        && let Some(entity_id) = connection
            .query_row(
                "
                SELECT entity_id
                FROM entity_identity_keys
                WHERE server_id = ?1
                  AND entity_kind = 'album_artist'
                  AND namespace = 'musicbrainz:artist'
                  AND value = ?2
                LIMIT 1
                ",
                params![server_id.as_str(), artist_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        && entity_id != artist.id.as_str()
    {
        return Ok(Some(ArtistId::new(entity_id)));
    }
    if let Some(artist_id) = clean_artist_identity_value(artist.musicbrainz_artist_id.as_deref())
        && let Some(entity_id) =
            relation_backed_album_artist_alias_target(connection, server_id, artist, artist_id)?
        && entity_id != artist.id
    {
        return Ok(Some(entity_id));
    }
    if let Some(entity_id) = connection
        .query_row(
            "
            SELECT entity_id
            FROM entity_identity_keys
            WHERE server_id = ?1
              AND entity_kind = 'album_artist'
              AND namespace = 'source:artist_id'
              AND value = ?2
            LIMIT 1
            ",
            params![server_id.as_str(), artist.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        && entity_id != artist.id.as_str()
    {
        return Ok(Some(ArtistId::new(entity_id)));
    }
    Ok(None)
}

fn relation_backed_album_artist_alias_target(
    connection: &Connection,
    server_id: &ServerId,
    artist: &Artist,
    musicbrainz_artist_id: &str,
) -> StoreResult<Option<ArtistId>> {
    let name = artist.name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let mut statement = connection.prepare(
        "
        WITH relation_artists AS (
            SELECT artist_id, name
            FROM album_artist_links
            WHERE server_id = ?1
              AND artist_id <> ?2
            UNION
            SELECT artist_id, artist AS name
            FROM albums
            WHERE server_id = ?1
              AND artist_id IS NOT NULL
              AND artist_id <> ?2
        )
        SELECT artist_id
        FROM relation_artists candidate
        WHERE LOWER(TRIM(candidate.name)) = LOWER(TRIM(?3))
          AND NOT EXISTS (
              SELECT 1
              FROM entity_identity_keys key
              WHERE key.server_id = ?1
                AND key.entity_kind = 'album_artist'
                AND key.namespace = 'musicbrainz:artist'
                AND key.entity_id = candidate.artist_id
                AND key.value <> ?4
          )
        GROUP BY artist_id
        ORDER BY artist_id
        LIMIT 2
        ",
    )?;
    let ids = collect_rows(statement.query_map(
        params![
            server_id.as_str(),
            artist.id.as_str(),
            name,
            musicbrainz_artist_id
        ],
        |row| row.get::<_, String>(0).map(ArtistId::new),
    )?)?;
    Ok((ids.len() == 1).then(|| ids[0].clone()))
}

fn clean_artist_identity_value(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn normalize_compound_album_artist_links(
    connection: &Connection,
    server_id: &ServerId,
    generation: i64,
) -> StoreResult<Vec<(ArtistId, ArtistId)>> {
    let mut statement = connection.prepare(
        "
        SELECT album_id, artist_id, name, position
        FROM album_artist_links
        WHERE server_id = ?1
          AND (name LIKE '%/%' OR name LIKE '%;%')
        ORDER BY album_id, position
        ",
    )?;
    let links = collect_rows(statement.query_map(params![server_id.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            ArtistId::new(row.get::<_, String>(1)?),
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?)?;
    let mut aliases = Vec::new();
    for (album_id, alias_id, name, position) in links {
        let parts = split_compound_credit_name(&name);
        if parts.len() <= 1 {
            continue;
        }
        let mut resolved = Vec::new();
        for part in &parts {
            let Some(artist_id) =
                unique_track_artist_for_album_name(connection, server_id, &album_id, part)?
            else {
                resolved.clear();
                break;
            };
            resolved.push((artist_id, part.clone()));
        }
        if resolved.len() != parts.len() {
            continue;
        }
        connection.execute(
            "
            DELETE FROM album_artist_links
            WHERE server_id = ?1
              AND album_id = ?2
              AND artist_id = ?3
            ",
            params![server_id.as_str(), album_id, alias_id.as_str()],
        )?;
        for (index, (artist_id, part)) in resolved.iter().enumerate() {
            connection.execute(
                "
                INSERT INTO album_artist_links (
                    server_id, album_id, artist_id, name, position, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(server_id, album_id, artist_id) DO UPDATE SET
                    name = excluded.name,
                    position = excluded.position,
                    sync_generation = excluded.sync_generation
                ",
                params![
                    server_id.as_str(),
                    album_id,
                    artist_id.as_str(),
                    part,
                    position + index as i64,
                    generation
                ],
            )?;
        }
        if let Some((canonical_id, _)) = resolved.first() {
            connection.execute(
                "
                UPDATE albums
                SET artist_id = ?3
                WHERE server_id = ?1
                  AND album_id = ?2
                  AND artist_id = ?4
                ",
                params![
                    server_id.as_str(),
                    album_id,
                    canonical_id.as_str(),
                    alias_id.as_str()
                ],
            )?;
            aliases.push((alias_id, canonical_id.clone()));
        }
    }
    Ok(aliases)
}

fn split_compound_credit_name(name: &str) -> Vec<String> {
    name.split(['/', ';'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn unique_track_artist_for_album_name(
    connection: &Connection,
    server_id: &ServerId,
    album_id: &str,
    name: &str,
) -> StoreResult<Option<ArtistId>> {
    let mut statement = connection.prepare(
        "
        SELECT artist_id
        FROM (
            SELECT DISTINCT artist_id
            FROM track_artist_links
            WHERE server_id = ?1
              AND album_id = ?2
              AND LOWER(TRIM(name)) = LOWER(TRIM(?3))
        )
        ORDER BY artist_id
        LIMIT 2
        ",
    )?;
    let ids = collect_rows(
        statement.query_map(params![server_id.as_str(), album_id, name], |row| {
            row.get::<_, String>(0).map(ArtistId::new)
        })?,
    )?;
    Ok((ids.len() == 1).then(|| ids[0].clone()))
}

fn merge_album_artist_alias(
    connection: &Connection,
    server_id: &ServerId,
    canonical_id: &ArtistId,
    alias_id: &ArtistId,
    generation: i64,
) -> StoreResult<()> {
    connection.execute(
        "
        UPDATE album_artists
        SET album_count = MAX(album_count, COALESCE((
                SELECT album_count FROM album_artists alias
                WHERE alias.server_id = album_artists.server_id
                  AND alias.artist_id = ?3
            ), 0)),
            track_count = MAX(track_count, COALESCE((
                SELECT track_count FROM album_artists alias
                WHERE alias.server_id = album_artists.server_id
                  AND alias.artist_id = ?3
            ), 0)),
            favorite = MAX(favorite, COALESCE((
                SELECT favorite FROM album_artists alias
                WHERE alias.server_id = album_artists.server_id
                  AND alias.artist_id = ?3
            ), 0)),
            last_played = COALESCE((
                SELECT last_played FROM album_artists alias
                WHERE alias.server_id = album_artists.server_id
                  AND alias.artist_id = ?3
                  AND alias.last_played IS NOT NULL
            ), last_played),
            play_count = COALESCE((
                SELECT play_count FROM album_artists alias
                WHERE alias.server_id = album_artists.server_id
                  AND alias.artist_id = ?3
                  AND alias.play_count IS NOT NULL
            ), play_count),
            user_rating = COALESCE((
                SELECT user_rating FROM album_artists alias
                WHERE alias.server_id = album_artists.server_id
                  AND alias.artist_id = ?3
                  AND alias.user_rating IS NOT NULL
            ), user_rating),
            image_item_id = COALESCE((
                SELECT image_item_id FROM album_artists alias
                WHERE alias.server_id = album_artists.server_id
                  AND alias.artist_id = ?3
                  AND alias.image_item_id IS NOT NULL
            ), image_item_id),
            image_tag = COALESCE((
                SELECT image_tag FROM album_artists alias
                WHERE alias.server_id = album_artists.server_id
                  AND alias.artist_id = ?3
                  AND alias.image_item_id IS NOT NULL
            ), image_tag),
            image_origin = COALESCE((
                SELECT image_origin FROM album_artists alias
                WHERE alias.server_id = album_artists.server_id
                  AND alias.artist_id = ?3
                  AND alias.image_item_id IS NOT NULL
            ), image_origin),
            sync_generation = ?4
        WHERE server_id = ?1
          AND artist_id = ?2
        ",
        params![
            server_id.as_str(),
            canonical_id.as_str(),
            alias_id.as_str(),
            generation
        ],
    )?;
    apply_album_artist_alias(connection, server_id, canonical_id, alias_id)
}

pub(super) fn repair_linked_genres(
    connection: &Connection,
    server_id: &ServerId,
    generation: i64,
) -> StoreResult<()> {
    let mut statement = connection.prepare(
        "
        SELECT genre_name
        FROM (
            SELECT genre_name
            FROM album_genres
            WHERE server_id = ?1
            UNION
            SELECT genre_name
            FROM track_genres
            WHERE server_id = ?1
        ) linked
        WHERE TRIM(linked.genre_name) != ''
          AND NOT EXISTS (
              SELECT 1
              FROM genres g
              WHERE g.server_id = ?1 AND g.name = linked.genre_name
          )
        ORDER BY linked.genre_name COLLATE NOCASE
        ",
    )?;
    let genre_names = collect_rows(
        statement.query_map(params![server_id.as_str()], |row| row.get::<_, String>(0))?,
    )?;
    let mut insert = connection.prepare(
        "
        INSERT INTO genres (
            server_id, genre_id, name, album_count, track_count, duration_seconds, sync_generation
        )
        VALUES (?1, ?2, ?3, 0, 0, 0, ?4)
        ON CONFLICT(server_id, genre_id) DO UPDATE SET
            name = excluded.name,
            sync_generation = excluded.sync_generation
        ",
    )?;
    for name in genre_names {
        let genre_id = format!("linked:genre:{:08x}", stable_seed(&name));
        insert.execute(params![server_id.as_str(), genre_id, name, generation])?;
    }
    Ok(())
}
pub(super) fn refresh_artist_fts(
    connection: &Connection,
    server_id: &ServerId,
    table: &str,
    item_type: &str,
) -> StoreResult<()> {
    connection.execute(
        "DELETE FROM library_fts WHERE server_id = ?1 AND item_type = ?2",
        params![server_id.as_str(), item_type],
    )?;
    let sql = format!(
        "
        INSERT INTO library_fts (server_id, item_type, item_id, title, subtitle)
        SELECT server_id, '{item_type}', artist_id, name, ''
        FROM {table}
        WHERE server_id = ?1
        "
    );
    connection.execute(&sql, params![server_id.as_str()])?;
    Ok(())
}
pub(super) fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&Row<'_>) -> rusqlite::Result<T>>,
) -> StoreResult<Vec<T>> {
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(StoreError::from)
}
pub(super) fn clear_library_cache_on_connection(
    connection: &Connection,
    server_id: &ServerId,
) -> StoreResult<()> {
    clear_local_manifest_on_connection(connection, server_id)?;
    for table in [
        "collection_cover_refs",
        "home_section_prefetch_items",
        "home_section_items",
        "playlist_tracks",
        "playlists",
        "genres",
        "track_genres",
        "track_music_folders",
        "track_local_matches",
        "server_music_folders",
        "album_genres",
        "track_artist_links",
        "album_artist_links",
        "album_artists",
        "artists",
        "tracks",
        "albums",
        "lyrics_cache",
        "cover_cache",
        "external_image_lookup_misses",
        "entity_content_refs",
        "entity_links",
        "entity_resolver_state",
        "entity_facts",
        "entity_grouping_keys",
        "entity_identity_keys",
        "entities",
        "source_objects",
    ] {
        let sql = format!("DELETE FROM {table} WHERE server_id = ?1");
        connection.execute(&sql, params![server_id.as_str()])?;
    }
    connection.execute(
        "DELETE FROM library_fts WHERE server_id = ?1",
        params![server_id.as_str()],
    )?;
    Ok(())
}
pub(super) fn home_section_kinds() -> [HomeSectionKind; 5] {
    [
        HomeSectionKind::Explore,
        HomeSectionKind::MostPlayed,
        HomeSectionKind::NewlyAdded,
        HomeSectionKind::RecentlyPlayed,
        HomeSectionKind::RecentlyReleased,
    ]
}
pub(super) fn home_section_kind_key(kind: HomeSectionKind) -> &'static str {
    match kind {
        HomeSectionKind::Explore => "explore",
        HomeSectionKind::MostPlayed => "most_played",
        HomeSectionKind::NewlyAdded => "newly_added",
        HomeSectionKind::RecentlyPlayed => "recently_played",
        HomeSectionKind::RecentlyReleased => "recently_released",
    }
}
pub(super) fn fts_query(query: &str) -> Option<String> {
    let tokens = query
        .split_whitespace()
        .filter_map(|token| {
            let token = token
                .chars()
                .filter(|character| character.is_alphanumeric())
                .collect::<String>();
            (!token.is_empty()).then(|| format!("{token}*"))
        })
        .collect::<Vec<_>>();

    (!tokens.is_empty()).then(|| tokens.join(" "))
}
pub(super) fn like_pattern(query: &str) -> Option<String> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return None;
    }

    let mut pattern = String::with_capacity(query.len() + 2);
    pattern.push('%');
    for character in query.chars() {
        match character {
            '%' | '_' | '\\' => {
                pattern.push('\\');
                pattern.push(character);
            }
            _ => pattern.push(character),
        }
    }
    pattern.push('%');
    Some(pattern)
}

pub(super) fn bool_to_i64(value: bool) -> i64 {
    i64::from(value)
}

pub(super) fn u16_from_i64(value: i64) -> u16 {
    value.clamp(0, i64::from(u16::MAX)) as u16
}

pub(super) fn u32_from_i64(value: i64) -> u32 {
    value.clamp(0, i64::from(u32::MAX)) as u32
}

pub(super) fn encode_key_part(value: &str) -> String {
    let encoded: String = value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => character,
            _ => '_',
        })
        .collect();

    if encoded.len() <= CACHE_KEY_PART_MAX_LEN {
        return encoded;
    }

    let prefix_len = CACHE_KEY_PART_MAX_LEN - CACHE_KEY_HASH_LEN - 1;
    let prefix = encoded.chars().take(prefix_len).collect::<String>();
    format!("{prefix}_{:016x}", stable_hash(value))
}

pub(super) fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

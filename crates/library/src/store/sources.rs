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
pub(super) fn stored_source_from_row(row: &Row<'_>) -> rusqlite::Result<StoredSource> {
    Ok(StoredSource {
        source_id: SourceId::new(row.get::<_, String>(0)?),
        kind: row.get(1)?,
        name: row.get(2)?,
        provider_payload: row.get(3)?,
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
        album_artwork: None,
        genres: Vec::new(),
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        bpm: row.get::<_, Option<i64>>(offset + 18)?.map(u16_from_i64),
        local_path: row.get::<_, Option<String>>(offset + 19).ok().flatten(),
        source_format: row.get::<_, Option<String>>(offset + 20).ok().flatten(),
        comment: row.get::<_, Option<String>>(offset + 21).ok().flatten(),
        skip_count: row
            .get::<_, Option<i64>>(offset + 22)
            .ok()
            .flatten()
            .map(u32_from_i64),
        moods: Vec::new(),
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
        representative_albums: Vec::new(),
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
                      WHERE t.source_id = artists.source_id
                        AND t.artist_id = artists.artist_id
                  )
                  OR NOT EXISTS (
                      SELECT 1
                      FROM track_artist_links tal
                      WHERE tal.source_id = artists.source_id
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
                      WHERE t.source_id = {alias}.source_id
                        AND t.artist_id = {alias}.artist_id
                  )
                  OR NOT EXISTS (
                      SELECT 1
                      FROM track_artist_links tal
                      WHERE tal.source_id = {alias}.source_id
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
    source_id: &SourceId,
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
                unique_track_artist_for_album_name(connection, source_id, album.id.as_str(), part)?
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
    source_id: &SourceId,
    album: &Album,
) -> StoreResult<Album> {
    let mut album = canonical_album_for_delta(connection, source_id, album)?;
    if !album.artist.trim().is_empty()
        && !album.album_artist_credits.iter().any(|credit| {
            credit.name.trim().eq_ignore_ascii_case(album.artist.trim())
                || album.artist_id.as_ref() == Some(&credit.id)
        })
        && let Some(artist_id) = unique_album_artist_for_name(connection, source_id, &album.artist)?
    {
        album.album_artist_credits.push(ArtistCredit {
            id: artist_id,
            name: album.artist.trim().to_string(),
            musicbrainz_artist_id: None,
        });
    }
    for credit in &mut album.album_artist_credits {
        if let Some(entity_id) = album_artist_alias_target(connection, source_id, &credit.id)?
            && entity_id != credit.id
        {
            credit.id = entity_id;
        } else if let Some(artist_id) = fallback_musicbrainz_artist_id(&credit.id)
            && let Some(entity_id) =
                album_artist_musicbrainz_target(connection, source_id, artist_id)?
            && entity_id != credit.id
        {
            credit.id = entity_id;
        }
    }
    if let Some(artist_id) = album.artist_id.as_ref()
        && let Some(entity_id) = album_artist_alias_target(connection, source_id, artist_id)?
        && entity_id != *artist_id
    {
        album.artist_id = Some(entity_id);
    } else if let Some(artist_id) = album
        .artist_id
        .as_ref()
        .and_then(fallback_musicbrainz_artist_id)
        && let Some(entity_id) = album_artist_musicbrainz_target(connection, source_id, artist_id)?
        && album.artist_id.as_ref() != Some(&entity_id)
    {
        album.artist_id = Some(entity_id);
    }
    Ok(album)
}

fn unique_album_artist_for_name(
    connection: &Connection,
    source_id: &SourceId,
    name: &str,
) -> StoreResult<Option<ArtistId>> {
    let mut statement = connection.prepare(
        "
        SELECT artist_id
        FROM (
            SELECT DISTINCT artist_id
            FROM album_artists
            WHERE source_id = ?1
              AND LOWER(TRIM(name)) = LOWER(TRIM(?2))
            UNION
            SELECT DISTINCT artist_id
            FROM artists
            WHERE source_id = ?1
              AND LOWER(TRIM(name)) = LOWER(TRIM(?2))
        ) candidates
        ORDER BY artist_id
        LIMIT 2
        ",
    )?;
    let ids = collect_rows(
        statement.query_map(params![source_id.as_str(), name], |row| {
            row.get::<_, String>(0).map(ArtistId::new)
        })?,
    )?;
    Ok((ids.len() == 1).then(|| ids[0].clone()))
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
pub(super) fn track_matches_artist(track: &Track, artist_id: &ArtistId) -> bool {
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
    false
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
        image_ref: None,
        representative_albums: Vec::new(),
    }
}
pub(super) fn genre_from_row(row: &Row<'_>) -> rusqlite::Result<Genre> {
    Ok(Genre {
        id: GenreId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        album_count: u32_from_i64(row.get(2)?),
        track_count: u32_from_i64(row.get(3)?),
        duration_seconds: u32_from_i64(row.get(4)?),
        image_ref: image_ref_from_row(row, 5, 6)?,
        representative_albums: Vec::new(),
    })
}
pub(super) fn mood_from_row(row: &Row<'_>) -> rusqlite::Result<Mood> {
    Ok(Mood {
        id: MoodId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        track_count: u32_from_i64(row.get(2)?),
        duration_seconds: u32_from_i64(row.get(3)?),
        representative_albums: Vec::new(),
    })
}
pub(super) fn playlist_from_row(row: &Row<'_>) -> rusqlite::Result<Playlist> {
    let owner = row.get::<_, String>(5)?;
    let owner = playlist_owner_from_str(&owner).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(Playlist {
        id: PlaylistId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        owner: Some(owner),
        track_count: u32_from_i64(row.get(2)?),
        duration_seconds: u32_from_i64(row.get(3)?),
        top_genres: string_vec_from_json(row.get(4)?, 4)?,
        image_ref: image_ref_from_row(row, 6, 7)?,
        representative_albums: Vec::new(),
    })
}
pub(super) fn stable_seed(value: &str) -> u32 {
    value.bytes().fold(0x811c_9dc5, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    })
}
pub(super) fn repair_linked_artists(
    connection: &Connection,
    source_id: &SourceId,
    generation: i64,
) -> StoreResult<()> {
    let album_artist_aliases =
        normalize_compound_album_artist_links(connection, source_id, generation)?;
    repair_artist_label_links(connection, source_id, generation)?;
    connection.execute(
        "
        INSERT INTO artists (
            source_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT t.source_id,
               t.artist_id,
               MIN(t.artist),
               COUNT(DISTINCT t.album_id),
               COUNT(*),
               MAX(t.favorite),
               ?2
        FROM tracks t
        WHERE t.source_id = ?1
          AND t.artist_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM artists a
              WHERE a.source_id = t.source_id AND a.artist_id = t.artist_id
          )
        GROUP BY t.source_id, t.artist_id
        ",
        params![source_id.as_str(), generation],
    )?;
    connection.execute(
        "
        INSERT INTO artists (
            source_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT tal.source_id,
               tal.artist_id,
               MIN(tal.name),
               COUNT(DISTINCT tal.album_id),
               COUNT(DISTINCT tal.track_id),
               COALESCE(MAX(t.favorite), 0),
               ?2
        FROM track_artist_links tal
        LEFT JOIN tracks t
            ON t.source_id = tal.source_id AND t.track_id = tal.track_id
        WHERE tal.source_id = ?1
          AND NOT EXISTS (
              SELECT 1 FROM artists a
              WHERE a.source_id = tal.source_id AND a.artist_id = tal.artist_id
          )
        GROUP BY tal.source_id, tal.artist_id
        ",
        params![source_id.as_str(), generation],
    )?;
    connection.execute(
        "
        INSERT INTO album_artists (
            source_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT a.source_id,
               a.artist_id,
               MIN(a.artist),
               COUNT(*),
               COALESCE(SUM(a.track_count), 0),
               MAX(a.favorite),
               ?2
        FROM albums a
        WHERE a.source_id = ?1
          AND a.artist_id IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM album_artists aa
              WHERE aa.source_id = a.source_id AND aa.artist_id = a.artist_id
          )
        GROUP BY a.source_id, a.artist_id
        ",
        params![source_id.as_str(), generation],
    )?;
    connection.execute(
        "
        INSERT INTO album_artists (
            source_id, artist_id, name, album_count, track_count, favorite,
            sync_generation
        )
        SELECT aal.source_id,
               aal.artist_id,
               MIN(aal.name),
               COUNT(DISTINCT aal.album_id),
               COALESCE(SUM(a.track_count), 0),
               COALESCE(MAX(a.favorite), 0),
               ?2
        FROM album_artist_links aal
        LEFT JOIN albums a
            ON a.source_id = aal.source_id AND a.album_id = aal.album_id
        WHERE aal.source_id = ?1
          AND NOT EXISTS (
              SELECT 1 FROM album_artists aa
              WHERE aa.source_id = aal.source_id AND aa.artist_id = aal.artist_id
          )
        GROUP BY aal.source_id, aal.artist_id
        ",
        params![source_id.as_str(), generation],
    )?;
    connection.execute(
        "
        UPDATE album_artists
        SET name = (
            SELECT MIN(aal.name)
            FROM album_artist_links aal
            WHERE aal.source_id = album_artists.source_id
              AND aal.artist_id = album_artists.artist_id
              AND TRIM(aal.name) <> ''
        )
        WHERE source_id = ?1
          AND EXISTS (
              SELECT 1
              FROM album_artist_links aal
              WHERE aal.source_id = album_artists.source_id
                AND aal.artist_id = album_artists.artist_id
                AND TRIM(aal.name) <> ''
          )
        ",
        params![source_id.as_str()],
    )?;
    merge_musicbrainz_album_artist_fallbacks(connection, source_id, generation)?;
    for (alias_id, canonical_id) in album_artist_aliases {
        merge_album_artist_alias(connection, source_id, &canonical_id, &alias_id, generation)?;
    }
    refresh_artist_fts(connection, source_id, "artists", "artist")?;
    refresh_artist_fts(connection, source_id, "album_artists", "album_artist")?;
    Ok(())
}

pub(super) fn repair_artist_label_links(
    connection: &Connection,
    source_id: &SourceId,
    generation: i64,
) -> StoreResult<()> {
    connection.execute(
        "
        WITH artist_targets AS (
            SELECT source_id,
                   LOWER(TRIM(name)) AS name_key,
                   MIN(artist_id) AS artist_id
            FROM artists
            WHERE source_id = ?1
              AND TRIM(name) <> ''
            GROUP BY source_id, LOWER(TRIM(name))
            HAVING COUNT(DISTINCT artist_id) = 1
        )
        INSERT INTO track_artist_links (
            source_id, track_id, album_id, artist_id, name, position, sync_generation
        )
        SELECT t.source_id,
               t.track_id,
               t.album_id,
               target.artist_id,
               TRIM(t.artist),
               COALESCE((
                   SELECT MAX(existing.position) + 1
                   FROM track_artist_links existing
                   WHERE existing.source_id = t.source_id
                     AND existing.track_id = t.track_id
               ), 0),
               ?2
        FROM tracks t
        JOIN artist_targets target
          ON target.source_id = t.source_id
         AND target.name_key = LOWER(TRIM(t.artist))
        WHERE t.source_id = ?1
          AND TRIM(t.artist) <> ''
          AND NOT EXISTS (
              SELECT 1
              FROM track_artist_links existing
              WHERE existing.source_id = t.source_id
                AND existing.track_id = t.track_id
                AND existing.artist_id = target.artist_id
          )
        ON CONFLICT(source_id, track_id, artist_id) DO UPDATE SET
            album_id = excluded.album_id,
            name = excluded.name,
            position = excluded.position,
            sync_generation = excluded.sync_generation
        ",
        params![source_id.as_str(), generation],
    )?;
    connection.execute(
        "
        WITH album_artist_targets AS (
            SELECT source_id,
                   LOWER(TRIM(name)) AS name_key,
                   MIN(artist_id) AS artist_id
            FROM (
                SELECT source_id, artist_id, name
                FROM album_artists
                WHERE source_id = ?1
                UNION
                SELECT source_id, artist_id, name
                FROM artists
                WHERE source_id = ?1
            ) candidates
            WHERE TRIM(name) <> ''
            GROUP BY source_id, LOWER(TRIM(name))
            HAVING COUNT(DISTINCT artist_id) = 1
        )
        INSERT INTO album_artist_links (
            source_id, album_id, artist_id, name, position, sync_generation
        )
        SELECT a.source_id,
               a.album_id,
               target.artist_id,
               TRIM(a.artist),
               COALESCE((
                   SELECT MAX(existing.position) + 1
                   FROM album_artist_links existing
                   WHERE existing.source_id = a.source_id
                     AND existing.album_id = a.album_id
               ), 0),
               ?2
        FROM albums a
        JOIN album_artist_targets target
          ON target.source_id = a.source_id
         AND target.name_key = LOWER(TRIM(a.artist))
        WHERE a.source_id = ?1
          AND TRIM(a.artist) <> ''
          AND NOT EXISTS (
              SELECT 1
              FROM album_artist_links existing
              WHERE existing.source_id = a.source_id
                AND existing.album_id = a.album_id
                AND existing.artist_id = target.artist_id
          )
        ON CONFLICT(source_id, album_id, artist_id) DO UPDATE SET
            name = excluded.name,
            position = excluded.position,
            sync_generation = excluded.sync_generation
        ",
        params![source_id.as_str(), generation],
    )?;
    Ok(())
}

pub(super) struct CanonicalArtist {
    pub artist: Artist,
    pub alias_ids: Vec<ArtistId>,
}

pub(super) fn canonical_album_artists_for_write(
    connection: &Connection,
    source_id: &SourceId,
    artists: &[Artist],
) -> StoreResult<Vec<CanonicalArtist>> {
    let mut musicbrainz_ids = HashMap::<String, ArtistId>::new();
    let mut indexes = HashMap::<ArtistId, usize>::new();
    let mut result = Vec::<CanonicalArtist>::new();
    for artist in artists {
        let alias_id = artist.id.clone();
        let musicbrainz_artist_id =
            clean_artist_identity_value(artist.musicbrainz_artist_id.as_deref());
        let canonical_id = canonical_album_artist_id_for_write(connection, source_id, artist)?
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
    source_id: &SourceId,
    artist_id: &ArtistId,
) -> StoreResult<Option<ArtistId>> {
    connection
        .query_row(
            "
            SELECT entity_id
            FROM entity_identity_keys
            WHERE source_id = ?1
              AND entity_kind = 'album_artist'
              AND namespace = 'source:artist_id'
              AND value = ?2
            LIMIT 1
            ",
            params![source_id.as_str(), artist_id.as_str()],
            |row| row.get::<_, String>(0).map(ArtistId::new),
        )
        .optional()
        .map_err(StoreError::from)
}

pub(super) fn apply_album_artist_alias(
    connection: &Connection,
    source_id: &SourceId,
    canonical_id: &ArtistId,
    alias_id: &ArtistId,
) -> StoreResult<()> {
    connection.execute(
        "
        INSERT INTO entity_identity_keys (
            source_id, entity_kind, namespace, value, entity_id, source, strength, updated_at
        )
        VALUES (?1, 'album_artist', 'source:artist_id', ?2, ?3, 'source', 100, CURRENT_TIMESTAMP)
        ON CONFLICT(source_id, entity_kind, namespace, value) DO UPDATE SET
            entity_id = excluded.entity_id,
            source = excluded.source,
            strength = excluded.strength,
            updated_at = excluded.updated_at
        ",
        params![source_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        INSERT INTO album_artist_links (
            source_id, album_id, artist_id, name, position, sync_generation
        )
        SELECT source_id, album_id, ?3, name, position, sync_generation
        FROM album_artist_links
        WHERE source_id = ?1
          AND artist_id = ?2
        ON CONFLICT(source_id, album_id, artist_id) DO UPDATE SET
            sync_generation = MAX(sync_generation, excluded.sync_generation)
        ",
        params![source_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM album_artist_links
        WHERE source_id = ?1
          AND artist_id = ?2
          AND artist_id <> ?3
        ",
        params![source_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        UPDATE albums
        SET artist_id = ?3
        WHERE source_id = ?1
          AND artist_id = ?2
          AND artist_id <> ?3
        ",
        params![source_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM entity_identity_keys
        WHERE source_id = ?1
          AND entity_kind = 'album_artist'
          AND entity_id = ?2
          AND entity_id <> ?3
        ",
        params![source_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM album_artists
        WHERE source_id = ?1
          AND artist_id = ?2
          AND artist_id <> ?3
        ",
        params![source_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM library_fts
        WHERE source_id = ?1
          AND item_type = 'album_artist'
          AND item_id = ?2
        ",
        params![source_id.as_str(), alias_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM entities
        WHERE source_id = ?1
          AND entity_kind = 'album_artist'
          AND entity_id = ?2
          AND entity_id <> ?3
        ",
        params![source_id.as_str(), alias_id.as_str(), canonical_id.as_str()],
    )?;
    Ok(())
}

fn canonical_album_artist_id_for_write(
    connection: &Connection,
    source_id: &SourceId,
    artist: &Artist,
) -> StoreResult<Option<ArtistId>> {
    if let Some(artist_id) = clean_artist_identity_value(artist.musicbrainz_artist_id.as_deref())
        && let Some(entity_id) = connection
            .query_row(
                "
                SELECT entity_id
                FROM entity_identity_keys
                WHERE source_id = ?1
                  AND entity_kind = 'album_artist'
                  AND namespace = 'musicbrainz:artist'
                  AND value = ?2
                LIMIT 1
                ",
                params![source_id.as_str(), artist_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        && entity_id != artist.id.as_str()
    {
        return Ok(Some(ArtistId::new(entity_id)));
    }
    if let Some(artist_id) = clean_artist_identity_value(artist.musicbrainz_artist_id.as_deref())
        && let Some(entity_id) =
            relation_backed_album_artist_alias_target(connection, source_id, artist, artist_id)?
        && entity_id != artist.id
    {
        return Ok(Some(entity_id));
    }
    if let Some(entity_id) = connection
        .query_row(
            "
            SELECT entity_id
            FROM entity_identity_keys
            WHERE source_id = ?1
              AND entity_kind = 'album_artist'
              AND namespace = 'source:artist_id'
              AND value = ?2
            LIMIT 1
            ",
            params![source_id.as_str(), artist.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        && entity_id != artist.id.as_str()
    {
        return Ok(Some(ArtistId::new(entity_id)));
    }
    Ok(None)
}

fn album_artist_musicbrainz_target(
    connection: &Connection,
    source_id: &SourceId,
    artist_id: &str,
) -> StoreResult<Option<ArtistId>> {
    connection
        .query_row(
            "
            SELECT entity_id
            FROM entity_identity_keys
            WHERE source_id = ?1
              AND entity_kind = 'album_artist'
              AND namespace = 'musicbrainz:artist'
              AND value = ?2
            LIMIT 1
            ",
            params![source_id.as_str(), artist_id],
            |row| row.get::<_, String>(0).map(ArtistId::new),
        )
        .optional()
        .map_err(StoreError::from)
}

fn fallback_musicbrainz_artist_id(artist_id: &ArtistId) -> Option<&str> {
    artist_id
        .as_str()
        .split_once(":artist:musicbrainz:")
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

fn merge_musicbrainz_album_artist_fallbacks(
    connection: &Connection,
    source_id: &SourceId,
    generation: i64,
) -> StoreResult<()> {
    let marker = ":artist:musicbrainz:";
    let mut statement = connection.prepare(
        "
        SELECT source.entity_id, target.entity_id
        FROM entity_identity_keys source
        JOIN entity_identity_keys target
          ON target.source_id = source.source_id
         AND target.entity_kind = 'album_artist'
         AND target.namespace = 'musicbrainz:artist'
         AND target.value = substr(source.value, instr(source.value, ?2) + length(?2))
        WHERE source.source_id = ?1
          AND source.entity_kind = 'album_artist'
          AND source.namespace = 'source:artist_id'
          AND instr(source.value, ?2) > 0
          AND source.entity_id <> target.entity_id
        ",
    )?;
    let aliases = collect_rows(statement.query_map(
        params![source_id.as_str(), marker],
        |row| {
            Ok((
                ArtistId::new(row.get::<_, String>(0)?),
                ArtistId::new(row.get::<_, String>(1)?),
            ))
        },
    )?)?;
    for (alias_id, canonical_id) in aliases {
        merge_album_artist_alias(connection, source_id, &canonical_id, &alias_id, generation)?;
    }
    Ok(())
}

fn relation_backed_album_artist_alias_target(
    connection: &Connection,
    source_id: &SourceId,
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
            WHERE source_id = ?1
              AND artist_id <> ?2
            UNION
            SELECT artist_id, artist AS name
            FROM albums
            WHERE source_id = ?1
              AND artist_id IS NOT NULL
              AND artist_id <> ?2
        )
        SELECT artist_id
        FROM relation_artists candidate
        WHERE LOWER(TRIM(candidate.name)) = LOWER(TRIM(?3))
          AND instr(candidate.artist_id, ':artist:musicbrainz:') = 0
          AND NOT EXISTS (
              SELECT 1
              FROM entity_identity_keys key
              WHERE key.source_id = ?1
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
            source_id.as_str(),
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
    source_id: &SourceId,
    generation: i64,
) -> StoreResult<Vec<(ArtistId, ArtistId)>> {
    let mut statement = connection.prepare(
        "
        SELECT album_id, artist_id, name, position
        FROM album_artist_links
        WHERE source_id = ?1
          AND (name LIKE '%/%' OR name LIKE '%;%')
        ORDER BY album_id, position
        ",
    )?;
    let links = collect_rows(statement.query_map(params![source_id.as_str()], |row| {
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
                unique_track_artist_for_album_name(connection, source_id, &album_id, part)?
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
            WHERE source_id = ?1
              AND album_id = ?2
              AND artist_id = ?3
            ",
            params![source_id.as_str(), album_id, alias_id.as_str()],
        )?;
        for (index, (artist_id, part)) in resolved.iter().enumerate() {
            connection.execute(
                "
                INSERT INTO album_artist_links (
                    source_id, album_id, artist_id, name, position, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(source_id, album_id, artist_id) DO UPDATE SET
                    name = excluded.name,
                    position = excluded.position,
                    sync_generation = excluded.sync_generation
                ",
                params![
                    source_id.as_str(),
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
                WHERE source_id = ?1
                  AND album_id = ?2
                  AND artist_id = ?4
                ",
                params![
                    source_id.as_str(),
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
    source_id: &SourceId,
    album_id: &str,
    name: &str,
) -> StoreResult<Option<ArtistId>> {
    let mut statement = connection.prepare(
        "
        SELECT artist_id
        FROM (
            SELECT DISTINCT artist_id
            FROM track_artist_links
            WHERE source_id = ?1
              AND album_id = ?2
              AND LOWER(TRIM(name)) = LOWER(TRIM(?3))
        )
        ORDER BY artist_id
        LIMIT 2
        ",
    )?;
    let ids = collect_rows(
        statement.query_map(params![source_id.as_str(), album_id, name], |row| {
            row.get::<_, String>(0).map(ArtistId::new)
        })?,
    )?;
    Ok((ids.len() == 1).then(|| ids[0].clone()))
}

fn merge_album_artist_alias(
    connection: &Connection,
    source_id: &SourceId,
    canonical_id: &ArtistId,
    alias_id: &ArtistId,
    generation: i64,
) -> StoreResult<()> {
    connection.execute(
        "
        UPDATE album_artists
        SET album_count = MAX(album_count, COALESCE((
                SELECT album_count FROM album_artists alias
                WHERE alias.source_id = album_artists.source_id
                  AND alias.artist_id = ?3
            ), 0)),
            track_count = MAX(track_count, COALESCE((
                SELECT track_count FROM album_artists alias
                WHERE alias.source_id = album_artists.source_id
                  AND alias.artist_id = ?3
            ), 0)),
            favorite = MAX(favorite, COALESCE((
                SELECT favorite FROM album_artists alias
                WHERE alias.source_id = album_artists.source_id
                  AND alias.artist_id = ?3
            ), 0)),
            last_played = COALESCE((
                SELECT last_played FROM album_artists alias
                WHERE alias.source_id = album_artists.source_id
                  AND alias.artist_id = ?3
                  AND alias.last_played IS NOT NULL
            ), last_played),
            play_count = COALESCE((
                SELECT play_count FROM album_artists alias
                WHERE alias.source_id = album_artists.source_id
                  AND alias.artist_id = ?3
                  AND alias.play_count IS NOT NULL
            ), play_count),
            user_rating = COALESCE((
                SELECT user_rating FROM album_artists alias
                WHERE alias.source_id = album_artists.source_id
                  AND alias.artist_id = ?3
                  AND alias.user_rating IS NOT NULL
            ), user_rating),
            image_item_id = COALESCE((
                SELECT image_item_id FROM album_artists alias
                WHERE alias.source_id = album_artists.source_id
                  AND alias.artist_id = ?3
                  AND alias.image_item_id IS NOT NULL
            ), image_item_id),
            image_tag = COALESCE((
                SELECT image_tag FROM album_artists alias
                WHERE alias.source_id = album_artists.source_id
                  AND alias.artist_id = ?3
                  AND alias.image_item_id IS NOT NULL
            ), image_tag),
            sync_generation = ?4
        WHERE source_id = ?1
          AND artist_id = ?2
        ",
        params![
            source_id.as_str(),
            canonical_id.as_str(),
            alias_id.as_str(),
            generation
        ],
    )?;
    apply_album_artist_alias(connection, source_id, canonical_id, alias_id)
}

pub(super) fn refresh_artist_fts(
    connection: &Connection,
    source_id: &SourceId,
    table: &str,
    item_type: &str,
) -> StoreResult<()> {
    connection.execute(
        "DELETE FROM library_fts WHERE source_id = ?1 AND item_type = ?2",
        params![source_id.as_str(), item_type],
    )?;
    let sql = format!(
        "
        INSERT INTO library_fts (source_id, item_type, item_id, title, subtitle)
        SELECT source_id, '{item_type}', artist_id, name, ''
        FROM {table}
        WHERE source_id = ?1
        "
    );
    connection.execute(&sql, params![source_id.as_str()])?;
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
    source_id: &SourceId,
) -> StoreResult<()> {
    clear_local_manifest_on_connection(connection, source_id)?;
    connection.execute(
        "
        DELETE FROM playlist_tracks
        WHERE source_id = ?1
          AND playlist_id IN (
              SELECT playlist_id
              FROM playlists
              WHERE source_id = ?1
                AND owner = 'native'
          )
        ",
        params![source_id.as_str()],
    )?;
    connection.execute(
        "
        DELETE FROM playlists
        WHERE source_id = ?1
          AND owner = 'native'
        ",
        params![source_id.as_str()],
    )?;
    for table in [
        "home_section_prefetch_items",
        "home_section_items",
        "genres",
        "track_genres",
        "track_music_folders",
        "track_local_matches",
        "source_music_folders",
        "album_genres",
        "track_artist_links",
        "album_artist_links",
        "album_artists",
        "artists",
        "tracks",
        "albums",
        "lyrics_cache",
        "entity_links",
        "entity_resolver_state",
        "entity_facts",
        "entity_grouping_keys",
        "entity_identity_keys",
        "entities",
        "source_objects",
    ] {
        let sql = format!("DELETE FROM {table} WHERE source_id = ?1");
        connection.execute(&sql, params![source_id.as_str()])?;
    }
    connection.execute(
        "DELETE FROM library_fts WHERE source_id = ?1",
        params![source_id.as_str()],
    )?;
    refresh_store_playlist_cache_after_source_clear(connection, source_id)?;
    Ok(())
}

fn refresh_store_playlist_cache_after_source_clear(
    connection: &Connection,
    source_id: &SourceId,
) -> StoreResult<()> {
    let mut statement = connection.prepare(
        "
        SELECT playlist_id
        FROM playlists
        WHERE source_id = ?1
          AND owner = 'store'
        ",
    )?;
    let playlist_ids = collect_rows(statement.query_map(params![source_id.as_str()], |row| {
        row.get::<_, String>(0).map(PlaylistId::new)
    })?)?;
    for playlist_id in playlist_ids {
        super::library_auxiliary_cache::refresh_playlist_stats(
            connection,
            source_id,
            &playlist_id,
        )?;
    }
    connection.execute(
        "
        INSERT INTO library_fts (source_id, item_type, item_id, title, subtitle)
        SELECT source_id, 'playlist', playlist_id, name, ''
        FROM playlists
        WHERE source_id = ?1
          AND owner = 'store'
        ",
        params![source_id.as_str()],
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
pub(super) fn home_membership(section: &HomeSection) -> Vec<(String, i64, String)> {
    section
        .albums
        .iter()
        .enumerate()
        .map(|(position, album)| {
            (
                "album".to_string(),
                position.min(i64::MAX as usize) as i64,
                album.id.as_str().to_string(),
            )
        })
        .chain(section.tracks.iter().enumerate().map(|(position, track)| {
            (
                "track".to_string(),
                position.min(i64::MAX as usize) as i64,
                track.id.as_str().to_string(),
            )
        }))
        .collect()
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

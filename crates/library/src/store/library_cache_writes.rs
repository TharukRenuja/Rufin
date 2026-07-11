use super::identity::{
    upsert_album_entity_data_on_connection, upsert_artist_credit_entity_data_on_connection,
    upsert_artist_entity_data_on_connection, upsert_track_entity_data_on_connection,
};
use super::sources::*;
use super::*;

impl Store {
    pub(super) fn upsert_albums_delta(
        &self,
        source_id: &SourceId,
        albums: &[Album],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        let mut changed = Vec::new();
        for album in albums {
            let delta_album = canonical_album_for_write(&self.connection, source_id, album)?;
            match self.load_album_for_delta(source_id, &album.id)? {
                Some(existing) => {
                    if !album_observation_changed(&existing, &delta_album) {
                        continue;
                    }
                    if album_stats_changed(&existing, &delta_album) {
                        delta.albums.stats.push(album.id.clone());
                    }
                    if album_links_changed(&existing, &delta_album) {
                        delta.albums.links.push(album.id.clone());
                    }
                    if existing.image_ref != delta_album.image_ref {
                        delta.albums.cover_refs.push(album.id.clone());
                    }
                    if album_fields_changed(&existing, &delta_album) {
                        delta.albums.fields.push(album.id.clone());
                    }
                }
                None => delta.albums.added.push(album.id.clone()),
            }
            changed.push(album.clone());
        }
        self.upsert_albums(source_id, &changed, generation)?;
        Ok(delta)
    }

    pub(super) fn upsert_tracks_delta(
        &self,
        source_id: &SourceId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let (changed, playlist_stat_track_ids, delta) =
            self.plan_tracks_delta(source_id, tracks)?;
        let playlist_stats_before =
            self.playlists_for_track_stat_changes(source_id, &playlist_stat_track_ids)?;
        self.upsert_tracks(source_id, &changed, generation)?;
        self.finish_tracks_delta(source_id, playlist_stats_before, delta)
    }

    pub(super) fn plan_tracks_delta(
        &self,
        source_id: &SourceId,
        tracks: &[Track],
    ) -> StoreResult<(Vec<Track>, Vec<TrackId>, LibraryDelta)> {
        let mut delta = LibraryDelta::default();
        let mut playlist_stat_track_ids = Vec::<TrackId>::new();
        let mut changed = Vec::new();
        for track in tracks {
            match self.load_track_for_delta(source_id, &track.id)? {
                Some(existing) => {
                    if !track_changed(&existing, track) {
                        continue;
                    }
                    let duration_changed = existing.duration_seconds != track.duration_seconds;
                    if existing.favorite != track.favorite {
                        delta.tracks.favorite.push(track.id.clone());
                    }
                    if existing.image_ref != track.image_ref {
                        delta.tracks.cover_refs.push(track.id.clone());
                    }
                    if track_stats_changed(&existing, track) {
                        delta.tracks.stats.push(track.id.clone());
                    }
                    if track_fields_changed(&existing, track) {
                        delta.tracks.fields.push(track.id.clone());
                    }
                    if track_metadata_changed(&existing, track) {
                        delta.tracks.metadata.push(track.id.clone());
                    }
                    if existing.duration_seconds != track.duration_seconds
                        || existing.genres != track.genres
                    {
                        playlist_stat_track_ids.push(track.id.clone());
                    }
                    if existing.album_id != track.album_id || duration_changed {
                        delta.albums.links.push(existing.album_id.clone());
                        delta.albums.links.push(track.album_id.clone());
                    }
                    if track_artist_links_changed(&existing, track) {
                        if let Some(artist_id) = existing.artist_id.clone() {
                            delta.artists.links.push(artist_id);
                        }
                        if let Some(artist_id) = track.artist_id.clone() {
                            delta.artists.links.push(artist_id);
                        }
                    }
                    if artist_credits_changed(
                        &existing.album_artist_credits,
                        &track.album_artist_credits,
                    ) {
                        delta.album_artists.links.extend(
                            existing
                                .album_artist_credits
                                .iter()
                                .chain(track.album_artist_credits.iter())
                                .map(|credit| credit.id.clone()),
                        );
                    }
                    if existing.genres != track.genres || duration_changed {
                        delta.genres.links.extend(
                            existing
                                .genres
                                .iter()
                                .chain(track.genres.iter())
                                .map(|name| GenreId::new(name.clone())),
                        );
                    }
                }
                None => {
                    delta.tracks.added.push(track.id.clone());
                    playlist_stat_track_ids.push(track.id.clone());
                    delta.albums.links.push(track.album_id.clone());
                    if let Some(artist_id) = track.artist_id.clone() {
                        delta.artists.links.push(artist_id);
                    }
                    delta.album_artists.links.extend(
                        track
                            .album_artist_credits
                            .iter()
                            .map(|credit| credit.id.clone()),
                    );
                    delta
                        .genres
                        .links
                        .extend(track.genres.iter().map(|name| GenreId::new(name.clone())));
                    if track.bpm.is_some() || !track.moods.is_empty() {
                        delta.tracks.metadata.push(track.id.clone());
                    }
                }
            }
            changed.push(track.clone());
        }
        Ok((changed, playlist_stat_track_ids, delta))
    }

    pub(super) fn finish_tracks_delta(
        &self,
        source_id: &SourceId,
        playlist_stats_before: Vec<(PlaylistId, Option<Playlist>)>,
        mut delta: LibraryDelta,
    ) -> StoreResult<LibraryDelta> {
        self.refresh_track_dependent_playlist_stats(source_id, playlist_stats_before, &mut delta)?;
        Ok(delta)
    }

    pub(super) fn upsert_artists_delta(
        &self,
        source_id: &SourceId,
        artists: &[Artist],
        album_artist: bool,
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        let canonical_artists = if album_artist {
            Some(canonical_album_artists_for_write(
                &self.connection,
                source_id,
                artists,
            )?)
        } else {
            None
        };
        let delta_artists = canonical_artists
            .as_ref()
            .map(|artists| {
                artists
                    .iter()
                    .map(|artist| &artist.artist)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| artists.iter().collect());
        let mut changed_input_ids = HashSet::new();
        for artist in delta_artists {
            match self.load_artist_for_delta(source_id, &artist.id, album_artist)? {
                Some(existing) if !artist_projection_changed(&existing, artist) => continue,
                Some(existing) => {
                    let entity = if album_artist {
                        &mut delta.album_artists
                    } else {
                        &mut delta.artists
                    };
                    if artist_stats_changed(&existing, artist) {
                        entity.stats.push(artist.id.clone());
                    }
                    if existing.image_ref != artist.image_ref {
                        entity.cover_refs.push(artist.id.clone());
                    }
                    if artist_fields_changed(&existing, artist) {
                        entity.fields.push(artist.id.clone());
                    }
                }
                None => {
                    if album_artist {
                        delta.album_artists.added.push(artist.id.clone());
                    } else {
                        delta.artists.added.push(artist.id.clone());
                    }
                }
            }
            if let Some(canonical) = canonical_artists
                .as_ref()
                .and_then(|artists| artists.iter().find(|item| item.artist.id == artist.id))
            {
                changed_input_ids.insert(canonical.artist.id.clone());
                changed_input_ids.extend(canonical.alias_ids.iter().cloned());
            } else {
                changed_input_ids.insert(artist.id.clone());
            }
        }
        let changed = artists
            .iter()
            .filter(|artist| changed_input_ids.contains(&artist.id))
            .cloned()
            .collect::<Vec<_>>();
        self.upsert_artists(source_id, &changed, album_artist, generation)?;
        Ok(delta)
    }

    pub(super) fn upsert_genres_delta(
        &self,
        source_id: &SourceId,
        genres: &[Genre],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        let mut changed = Vec::new();
        for genre in genres {
            match self.load_genre_for_delta(source_id, &genre.id)? {
                Some(existing) if genre_delta_unchanged(&existing, genre) => continue,
                Some(existing) => {
                    if existing.image_ref != genre.image_ref {
                        delta.genres.cover_refs.push(genre.id.clone());
                    }
                    if existing.name != genre.name {
                        delta.genres.fields.push(genre.id.clone());
                    }
                }
                None => delta.genres.added.push(genre.id.clone()),
            }
            changed.push(genre.clone());
        }
        self.upsert_genres_without_count_refresh(source_id, &changed, generation)?;
        Ok(delta)
    }

    pub(super) fn upsert_playlists_delta(
        &self,
        source_id: &SourceId,
        playlists: &[Playlist],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut delta = LibraryDelta::default();
        let mut changed = Vec::new();
        for playlist in playlists {
            match self.load_playlist_for_delta(source_id, &playlist.id)? {
                Some(existing) if playlist_summary_matches(&existing, playlist) => continue,
                Some(existing) => {
                    if existing.image_ref != playlist.image_ref {
                        delta.playlists.cover_refs.push(playlist.id.clone());
                    }
                    if existing.name != playlist.name {
                        delta.playlists.fields.push(playlist.id.clone());
                    }
                }
                None => delta.playlists.added.push(playlist.id.clone()),
            }
            changed.push(playlist.clone());
        }
        self.upsert_playlists(source_id, &changed, generation)?;
        Ok(delta)
    }

    pub(super) fn upsert_home_sections_delta(
        &self,
        source_id: &SourceId,
        sections: &[HomeSection],
        generation: i64,
    ) -> StoreResult<LibraryDelta> {
        let mut changed = false;
        for kind in home_section_kinds() {
            let before = self.load_home_membership_from("home_section_items", source_id, kind)?;
            let after = sections
                .iter()
                .find(|section| section.kind == kind)
                .map(home_membership)
                .unwrap_or_default();
            if before != after {
                changed = true;
                break;
            }
        }
        if !changed {
            return Ok(LibraryDelta::default());
        }
        self.upsert_home_sections(source_id, sections, generation)?;
        Ok(LibraryDelta {
            home_changed: true,
            ..LibraryDelta::default()
        })
    }

    fn playlists_for_track_stat_changes(
        &self,
        source_id: &SourceId,
        track_ids: &[TrackId],
    ) -> StoreResult<Vec<(PlaylistId, Option<Playlist>)>> {
        if track_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut playlist_ids = Vec::<PlaylistId>::new();
        let mut seen = HashSet::<PlaylistId>::new();
        for chunk in track_ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT DISTINCT playlist_id
                FROM playlist_tracks
                WHERE source_id = ?
                  AND track_id IN ({placeholders})
                ORDER BY playlist_id
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(source_id.as_str());
            values.extend(chunk.iter().map(TrackId::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                row.get::<_, String>(0).map(PlaylistId::new)
            })?;
            for row in rows {
                let playlist_id = row?;
                if seen.insert(playlist_id.clone()) {
                    playlist_ids.push(playlist_id);
                }
            }
        }
        playlist_ids
            .into_iter()
            .map(|playlist_id| {
                let playlist = self.load_playlist_for_delta(source_id, &playlist_id)?;
                Ok((playlist_id, playlist))
            })
            .collect()
    }

    fn refresh_track_dependent_playlist_stats(
        &self,
        source_id: &SourceId,
        before: Vec<(PlaylistId, Option<Playlist>)>,
        delta: &mut LibraryDelta,
    ) -> StoreResult<()> {
        if before.is_empty() {
            return Ok(());
        }
        self.write_batch(|connection| {
            for (playlist_id, before_playlist) in before {
                super::library_auxiliary_cache::refresh_playlist_stats(
                    connection,
                    source_id,
                    &playlist_id,
                )?;
                let after_playlist = self.load_playlist_for_delta(source_id, &playlist_id)?;
                if super::library_auxiliary_cache::playlist_stats_changed(
                    before_playlist,
                    after_playlist,
                ) {
                    delta.playlists.entries.push(playlist_id);
                }
            }
            Ok(())
        })
    }

    pub fn fail_sync(
        &self,
        source_id: &SourceId,
        generation: i64,
        error: &str,
    ) -> StoreResult<bool> {
        let updated = self.connection.execute(
            "
            UPDATE sync_state
            SET status = 'error',
                last_error = ?2
            WHERE source_id = ?1
              AND generation = ?3
              AND status = 'running'
            ",
            params![source_id.as_str(), error, generation],
        )?;
        Ok(updated > 0)
    }
    pub fn finish_sync_without_commit(
        &self,
        source_id: &SourceId,
        generation: i64,
    ) -> StoreResult<()> {
        self.connection.execute(
            "
            UPDATE sync_state
            SET status = 'idle',
                last_error = NULL
            WHERE source_id = ?1
              AND generation = ?2
              AND status = 'running'
            ",
            params![source_id.as_str(), generation],
        )?;
        Ok(())
    }
    pub fn clear_library_cache(&self, source_id: &SourceId) -> StoreResult<()> {
        self.write_batch(|connection| {
            clear_library_cache_on_connection(connection, source_id)?;
            connection.execute(
                "
                UPDATE sync_state
                SET generation = 0,
                    cache_revision = cache_revision + 1,
                    status = 'idle',
                    last_started_at = NULL,
                    last_completed_at = NULL,
                    last_all_completed_at = NULL,
                    last_error = NULL
                WHERE source_id = ?1
                ",
                params![source_id.as_str()],
            )?;
            Ok(())
        })
    }
    pub fn forget_source(&self, source_id: &SourceId) -> StoreResult<()> {
        self.write_batch(|connection| {
            clear_library_cache_on_connection(connection, source_id)?;
            connection.execute(
                "DELETE FROM queue_snapshots WHERE source_id = ?1",
                params![source_id.as_str()],
            )?;
            connection.execute(
                "DELETE FROM active_source WHERE source_id = ?1",
                params![source_id.as_str()],
            )?;
            connection.execute(
                "DELETE FROM sync_state WHERE source_id = ?1",
                params![source_id.as_str()],
            )?;
            connection.execute(
                "DELETE FROM sources WHERE source_id = ?1",
                params![source_id.as_str()],
            )?;
            Ok(())
        })
    }
    pub fn upsert_albums(
        &self,
        source_id: &SourceId,
        albums: &[Album],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            self.require_current_sync_generation(source_id, generation)?;
            let albums = albums
                .iter()
                .map(|album| canonical_album_for_write(connection, source_id, album))
                .collect::<StoreResult<Vec<_>>>()?;
            let mut statement = connection.prepare(
                "
                INSERT INTO albums (
                    source_id, album_id, title, artist, artist_id, year, release_date,
                    date_added, last_played, play_count, user_rating, track_count,
                    duration_seconds, favorite, color_seed, image_item_id, image_tag,
                    image_origin, release_types_json, is_compilation, musicbrainz_album_id,
                    musicbrainz_release_group_id, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
                ON CONFLICT(source_id, album_id) DO UPDATE SET
                    title = excluded.title,
                    artist = excluded.artist,
                    artist_id = excluded.artist_id,
                    year = excluded.year,
                    release_date = excluded.release_date,
                    date_added = excluded.date_added,
                    last_played = excluded.last_played,
                    play_count = excluded.play_count,
                    user_rating = excluded.user_rating,
                    track_count = excluded.track_count,
                    duration_seconds = excluded.duration_seconds,
                    favorite = excluded.favorite,
                    color_seed = excluded.color_seed,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    image_origin = excluded.image_origin,
                    release_types_json = CASE
                        WHEN excluded.release_types_json <> '[]' THEN excluded.release_types_json
                        WHEN COALESCE(NULLIF(excluded.musicbrainz_album_id, ''), '') =
                             COALESCE(NULLIF(albums.musicbrainz_album_id, ''), '')
                         AND COALESCE(NULLIF(excluded.musicbrainz_release_group_id, ''), '') =
                             COALESCE(NULLIF(albums.musicbrainz_release_group_id, ''), '')
                         AND albums.release_types_json <> '[]' THEN albums.release_types_json
                        ELSE COALESCE((
                            SELECT fact.value_json
                            FROM entity_facts fact
                            WHERE fact.source_id = albums.source_id
                              AND fact.entity_kind = 'album'
                              AND fact.entity_id = albums.album_id
                              AND fact.fact_key = 'release_types'
                              AND fact.status = 'resolved'
                              AND COALESCE(NULLIF(excluded.musicbrainz_album_id, ''), '') =
                                  COALESCE(NULLIF(albums.musicbrainz_album_id, ''), '')
                              AND COALESCE(NULLIF(excluded.musicbrainz_release_group_id, ''), '') =
                                  COALESCE(NULLIF(albums.musicbrainz_release_group_id, ''), '')
                        ), excluded.release_types_json)
                    END,
                    is_compilation = CASE
                        WHEN excluded.is_compilation IS NOT NULL THEN excluded.is_compilation
                        WHEN COALESCE(NULLIF(excluded.musicbrainz_album_id, ''), '') =
                             COALESCE(NULLIF(albums.musicbrainz_album_id, ''), '')
                         AND COALESCE(NULLIF(excluded.musicbrainz_release_group_id, ''), '') =
                             COALESCE(NULLIF(albums.musicbrainz_release_group_id, ''), '')
                         THEN COALESCE(albums.is_compilation, (
                            SELECT CASE fact.value_json
                                     WHEN 'true' THEN 1
                                     WHEN 'false' THEN 0
                                     ELSE NULL
                                   END
                            FROM entity_facts fact
                            WHERE fact.source_id = albums.source_id
                              AND fact.entity_kind = 'album'
                              AND fact.entity_id = albums.album_id
                              AND fact.fact_key = 'is_compilation'
                              AND fact.status = 'resolved'
                         ))
                        ELSE NULL
                    END,
                    musicbrainz_album_id = NULLIF(excluded.musicbrainz_album_id, ''),
                    musicbrainz_release_group_id = NULLIF(excluded.musicbrainz_release_group_id, ''),
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_genres = connection.prepare(
                "DELETE FROM album_genres WHERE source_id = ?1 AND album_id = ?2",
            )?;
            let mut delete_artist_links = connection.prepare(
                "DELETE FROM album_artist_links WHERE source_id = ?1 AND album_id = ?2",
            )?;
            let mut insert_genre = connection.prepare(
                "
                INSERT INTO album_genres (source_id, album_id, genre_name, sync_generation)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(source_id, album_id, genre_name) DO UPDATE SET
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut insert_artist_link = connection.prepare(
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
            )?;
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE source_id = ?1 AND item_type = 'album' AND item_id = ?2",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (source_id, item_type, item_id, title, subtitle)
                VALUES (?1, 'album', ?2, ?3, ?4)
                ",
            )?;

            for album in &albums {
                let (image_item_id, image_tag) = image_ref_parts(album.image_ref.as_ref());
                let image_origin = image_origin_for_source_ref(album.image_ref.as_ref());
                let release_types_json = album_release_types_json(&album.release_types)?;
                statement.execute(params![
                    source_id.as_str(),
                    album.id.as_str(),
                    album.title,
                    album.artist,
                    album.artist_id.as_ref().map(ArtistId::as_str),
                    i64::from(album.year),
                    album.release_date.as_deref(),
                    album.date_added.as_deref(),
                    album.last_played.as_deref(),
                    album.play_count.map(i64::from),
                    album.user_rating.map(i64::from),
                    i64::from(album.track_count),
                    i64::from(album.duration_seconds),
                    bool_to_i64(album.favorite),
                    i64::from(album.color_seed),
                    image_item_id,
                    image_tag,
                    image_origin,
                    release_types_json,
                    album.is_compilation.map(bool_to_i64),
                    album.musicbrainz_album_id.as_deref(),
                    album.musicbrainz_release_group_id.as_deref(),
                    generation,
                ])?;
                upsert_album_entity_data_on_connection(connection, source_id, album)?;
                delete_genres.execute(params![source_id.as_str(), album.id.as_str()])?;
                delete_artist_links.execute(params![source_id.as_str(), album.id.as_str()])?;
                for genre in &album.genres {
                    if !genre.trim().is_empty() {
                        insert_genre.execute(params![
                            source_id.as_str(),
                            album.id.as_str(),
                            genre.trim(),
                            generation,
                        ])?;
                    }
                }
                for (position, artist) in album_artist_credits(album).iter().enumerate() {
                    upsert_artist_credit_entity_data_on_connection(
                        connection,
                        source_id,
                        "album_artist",
                        artist,
                    )?;
                    insert_artist_link.execute(params![
                        source_id.as_str(),
                        album.id.as_str(),
                        artist.id.as_str(),
                        artist.name.trim(),
                        position as i64,
                        generation,
                    ])?;
                }
                delete_fts.execute(params![source_id.as_str(), album.id.as_str()])?;
                insert_fts.execute(params![
                    source_id.as_str(),
                    album.id.as_str(),
                    album.title,
                    album.artist,
                ])?;
            }
            Ok(())
        })
    }
    pub fn upsert_tracks(
        &self,
        source_id: &SourceId,
        tracks: &[Track],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            self.require_current_sync_generation(source_id, generation)?;
            let mut statement = connection.prepare(
                "
                INSERT INTO tracks (
                    source_id, track_id, album_id, title, artist, artist_id, album,
                    year, release_date, date_added, last_played, play_count, user_rating,
                    duration_seconds, favorite, disc_number, track_number,
                    image_item_id, image_tag, image_origin, local_path, source_format, comment, skip_count,
                    bpm,
                    sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
                ON CONFLICT(source_id, track_id) DO UPDATE SET
                    album_id = excluded.album_id,
                    title = excluded.title,
                    artist = excluded.artist,
                    artist_id = excluded.artist_id,
                    album = excluded.album,
                    year = excluded.year,
                    release_date = excluded.release_date,
                    date_added = excluded.date_added,
                    last_played = excluded.last_played,
                    play_count = excluded.play_count,
                    user_rating = excluded.user_rating,
                    duration_seconds = excluded.duration_seconds,
                    favorite = excluded.favorite,
                    disc_number = excluded.disc_number,
                    track_number = excluded.track_number,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    image_origin = excluded.image_origin,
                    local_path = excluded.local_path,
                    source_format = excluded.source_format,
                    comment = excluded.comment,
                    skip_count = excluded.skip_count,
                    bpm = excluded.bpm,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut delete_genres = connection.prepare(
                "DELETE FROM track_genres WHERE source_id = ?1 AND track_id = ?2",
            )?;
            let mut delete_moods = connection.prepare(
                "DELETE FROM track_moods WHERE source_id = ?1 AND track_id = ?2",
            )?;
            let mut delete_artist_links = connection.prepare(
                "DELETE FROM track_artist_links WHERE source_id = ?1 AND track_id = ?2",
            )?;
            let mut insert_genre = connection.prepare(
                "
                INSERT INTO track_genres (source_id, track_id, genre_name, sync_generation)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(source_id, track_id, genre_name) DO UPDATE SET
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut insert_mood = connection.prepare(
                "
                INSERT INTO track_moods (source_id, track_id, mood_name, sync_generation)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(source_id, track_id, mood_name) DO UPDATE SET
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut insert_artist_link = connection.prepare(
                "
                INSERT INTO track_artist_links (
                    source_id, track_id, album_id, artist_id, name, position, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(source_id, track_id, artist_id) DO UPDATE SET
                    album_id = excluded.album_id,
                    name = excluded.name,
                    position = excluded.position,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            let mut insert_album_artist_link = connection.prepare(
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
            )?;
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE source_id = ?1 AND item_type = 'track' AND item_id = ?2",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (source_id, item_type, item_id, title, subtitle)
                VALUES (?1, 'track', ?2, ?3, ?4)
                ",
            )?;

            for track in tracks {
                let (image_item_id, image_tag) = image_ref_parts(track.image_ref.as_ref());
                let image_origin = image_origin_for_source_ref(track.image_ref.as_ref());
                statement.execute(params![
                    source_id.as_str(),
                    track.id.as_str(),
                    track.album_id.as_str(),
                    track.title,
                    track.artist,
                    track.artist_id.as_ref().map(ArtistId::as_str),
                    track.album,
                    i64::from(track.year),
                    track.release_date.as_deref(),
                    track.date_added.as_deref(),
                    track.last_played.as_deref(),
                    track.play_count.map(i64::from),
                    track.user_rating.map(i64::from),
                    i64::from(track.duration_seconds),
                    bool_to_i64(track.favorite),
                    i64::from(track.disc_number),
                    i64::from(track.track_number),
                    image_item_id,
                    image_tag,
                    image_origin,
                    track.local_path.as_deref(),
                    track.source_format.as_deref(),
                    track.comment.as_deref(),
                    track.skip_count.map(i64::from),
                    track.bpm.map(i64::from),
                    generation,
                ])?;
                upsert_track_entity_data_on_connection(connection, source_id, track)?;
                delete_genres.execute(params![source_id.as_str(), track.id.as_str()])?;
                delete_moods.execute(params![source_id.as_str(), track.id.as_str()])?;
                delete_artist_links.execute(params![source_id.as_str(), track.id.as_str()])?;
                for genre in &track.genres {
                    if !genre.trim().is_empty() {
                        insert_genre.execute(params![
                            source_id.as_str(),
                            track.id.as_str(),
                            genre.trim(),
                            generation,
                        ])?;
                    }
                }
                for mood in &track.moods {
                    if !mood.trim().is_empty() {
                        insert_mood.execute(params![
                            source_id.as_str(),
                            track.id.as_str(),
                            mood.trim(),
                            generation,
                        ])?;
                    }
                }
                for (position, artist) in track_artist_credits(track).iter().enumerate() {
                    upsert_artist_credit_entity_data_on_connection(
                        connection, source_id, "artist", artist,
                    )?;
                    insert_artist_link.execute(params![
                        source_id.as_str(),
                        track.id.as_str(),
                        track.album_id.as_str(),
                        artist.id.as_str(),
                        artist.name.trim(),
                        position as i64,
                        generation,
                    ])?;
                }
                for (position, artist) in track.album_artist_credits.iter().enumerate() {
                    if artist.name.trim().is_empty() {
                        continue;
                    }
                    upsert_artist_credit_entity_data_on_connection(
                        connection,
                        source_id,
                        "album_artist",
                        artist,
                    )?;
                    insert_album_artist_link.execute(params![
                        source_id.as_str(),
                        track.album_id.as_str(),
                        artist.id.as_str(),
                        artist.name.trim(),
                        position as i64,
                        generation,
                    ])?;
                }
                delete_fts.execute(params![source_id.as_str(), track.id.as_str()])?;
                insert_fts.execute(params![
                    source_id.as_str(),
                    track.id.as_str(),
                    track.title,
                    format!("{} {}", track.artist, track.album),
                ])?;
            }
            let track_ids = tracks
                .iter()
                .map(|track| track.id.clone())
                .collect::<Vec<_>>();
            refresh_playlists_for_track_ids_on_connection(connection, source_id, &track_ids)?;
            Ok(())
        })
    }

    pub fn refresh_library_counts(&self, source_id: &SourceId) -> StoreResult<()> {
        self.write_batch(|connection| {
            self.bind_album_fallback_image_refs(source_id)?;
            self.bind_album_artist_fallback_image_refs(source_id)?;
            self.bind_album_external_identity_image_refs(source_id)?;
            self.bind_track_album_fallback_image_refs(source_id)?;
            self.bind_artist_fallback_image_refs(source_id, false)?;
            self.bind_artist_fallback_image_refs(source_id, true)?;
            self.refresh_selected_cover_content_refs(source_id)?;
            refresh_genre_counts_on_connection(connection, source_id)?;
            connection.execute(
                "
                UPDATE albums
                SET track_count = (
                    SELECT COUNT(*)
                    FROM tracks
                    WHERE tracks.source_id = albums.source_id
                      AND tracks.album_id = albums.album_id
                ),
                    duration_seconds = (
                    SELECT COALESCE(SUM(duration_seconds), 0)
                    FROM tracks
                    WHERE tracks.source_id = albums.source_id
                      AND tracks.album_id = albums.album_id
                )
                WHERE source_id = ?1
                  AND (
                      track_count != (
                          SELECT COUNT(*)
                          FROM tracks
                          WHERE tracks.source_id = albums.source_id
                            AND tracks.album_id = albums.album_id
                      )
                      OR duration_seconds != (
                          SELECT COALESCE(SUM(duration_seconds), 0)
                          FROM tracks
                          WHERE tracks.source_id = albums.source_id
                            AND tracks.album_id = albums.album_id
                      )
                  )
                ",
                params![source_id.as_str()],
            )?;
            connection.execute(
                "
                WITH artist_tracks AS MATERIALIZED (
                    SELECT source_id, artist_id, track_id, album_id
                    FROM tracks
                    WHERE source_id = ?1 AND artist_id IS NOT NULL
                    UNION
                    SELECT tracks.source_id, links.artist_id,
                           tracks.track_id, tracks.album_id
                    FROM track_artist_links links
                    JOIN tracks
                      ON tracks.source_id = links.source_id
                     AND tracks.track_id = links.track_id
                    WHERE links.source_id = ?1
                ),
                computed AS MATERIALIZED (
                    SELECT artists.rowid AS row_id,
                           COUNT(DISTINCT artist_tracks.track_id) AS track_count,
                           COUNT(DISTINCT artist_tracks.album_id) AS album_count
                    FROM artists
                    LEFT JOIN artist_tracks
                      ON artist_tracks.source_id = artists.source_id
                     AND artist_tracks.artist_id = artists.artist_id
                    WHERE artists.source_id = ?1
                    GROUP BY artists.rowid
                )
                UPDATE artists
                SET track_count = (
                        SELECT track_count FROM computed WHERE row_id = artists.rowid
                    ),
                    album_count = (
                        SELECT album_count FROM computed WHERE row_id = artists.rowid
                    )
                WHERE rowid IN (
                    SELECT row_id
                    FROM computed
                    WHERE computed.track_count != artists.track_count
                       OR computed.album_count != artists.album_count
                )
                ",
                params![source_id.as_str()],
            )?;
            connection.execute(
                "
                UPDATE album_artists
                SET track_count = (
                    SELECT COALESCE(SUM(track_count), 0)
                    FROM albums
                    WHERE albums.source_id = album_artists.source_id
                      AND (
                          albums.artist_id = album_artists.artist_id
                          OR EXISTS (
                              SELECT 1
                              FROM album_artist_links aal
                              WHERE aal.source_id = albums.source_id
                                AND aal.album_id = albums.album_id
                                AND aal.artist_id = album_artists.artist_id
                          )
                      )
                ),
                    album_count = (
                    SELECT COUNT(DISTINCT album_id)
                    FROM albums
                    WHERE albums.source_id = album_artists.source_id
                      AND (
                          albums.artist_id = album_artists.artist_id
                          OR EXISTS (
                              SELECT 1
                              FROM album_artist_links aal
                              WHERE aal.source_id = albums.source_id
                                AND aal.album_id = albums.album_id
                                AND aal.artist_id = album_artists.artist_id
                          )
                      )
                )
                WHERE source_id = ?1
                  AND (
                      track_count != (
                          SELECT COALESCE(SUM(track_count), 0)
                          FROM albums
                          WHERE albums.source_id = album_artists.source_id
                            AND (
                                albums.artist_id = album_artists.artist_id
                                OR EXISTS (
                                    SELECT 1 FROM album_artist_links aal
                                    WHERE aal.source_id = albums.source_id
                                      AND aal.album_id = albums.album_id
                                      AND aal.artist_id = album_artists.artist_id
                                )
                            )
                      )
                      OR album_count != (
                          SELECT COUNT(DISTINCT album_id)
                          FROM albums
                          WHERE albums.source_id = album_artists.source_id
                            AND (
                                albums.artist_id = album_artists.artist_id
                                OR EXISTS (
                                    SELECT 1 FROM album_artist_links aal
                                    WHERE aal.source_id = albums.source_id
                                      AND aal.album_id = albums.album_id
                                      AND aal.artist_id = album_artists.artist_id
                                )
                            )
                      )
                  )
                ",
                params![source_id.as_str()],
            )?;
            Ok(())
        })
    }
    pub fn upsert_artists(
        &self,
        source_id: &SourceId,
        artists: &[Artist],
        album_artist: bool,
        generation: i64,
    ) -> StoreResult<()> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        self.write_batch(|connection| {
            self.require_current_sync_generation(source_id, generation)?;
            let canonical_artists;
            let artists = if album_artist {
                canonical_artists =
                    canonical_album_artists_for_write(connection, source_id, artists)?;
                canonical_artists
                    .iter()
                    .map(|artist| (&artist.artist, artist.alias_ids.as_slice()))
                    .collect::<Vec<_>>()
            } else {
                artists
                    .iter()
                    .map(|artist| (artist, &[][..]))
                    .collect::<Vec<_>>()
            };
            let sql = format!(
                "
                INSERT INTO {table} (
                    source_id, artist_id, name, album_count, track_count, favorite,
                    last_played, play_count, user_rating, image_item_id, image_tag,
                    image_origin, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(source_id, artist_id) DO UPDATE SET
                    name = excluded.name,
                    album_count = excluded.album_count,
                    track_count = excluded.track_count,
                    favorite = excluded.favorite,
                    last_played = excluded.last_played,
                    play_count = excluded.play_count,
                    user_rating = excluded.user_rating,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    image_origin = excluded.image_origin,
                    sync_generation = excluded.sync_generation
                "
            );
            let mut statement = connection.prepare(&sql)?;
            let item_type = if album_artist {
                "album_artist"
            } else {
                "artist"
            };
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE source_id = ?1 AND item_type = ?2 AND item_id = ?3",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (source_id, item_type, item_id, title, subtitle)
                VALUES (?1, ?2, ?3, ?4, '')
                ",
            )?;

            for (artist, alias_ids) in artists {
                let (image_item_id, image_tag) = image_ref_parts(artist.image_ref.as_ref());
                let image_origin = image_origin_for_source_ref(artist.image_ref.as_ref());
                statement.execute(params![
                    source_id.as_str(),
                    artist.id.as_str(),
                    artist.name,
                    i64::from(artist.album_count),
                    i64::from(artist.track_count),
                    bool_to_i64(artist.favorite),
                    artist.last_played.as_deref(),
                    artist.play_count.map(i64::from),
                    artist.user_rating.map(i64::from),
                    image_item_id,
                    image_tag,
                    image_origin,
                    generation,
                ])?;
                upsert_artist_entity_data_on_connection(
                    connection,
                    source_id,
                    if album_artist {
                        "album_artist"
                    } else {
                        "artist"
                    },
                    artist,
                )?;
                for alias_id in alias_ids {
                    apply_album_artist_alias(connection, source_id, &artist.id, alias_id)?;
                }
                delete_fts.execute(params![source_id.as_str(), item_type, artist.id.as_str()])?;
                insert_fts.execute(params![
                    source_id.as_str(),
                    item_type,
                    artist.id.as_str(),
                    artist.name,
                ])?;
            }
            Ok(())
        })
    }
    pub fn upsert_genres(
        &self,
        source_id: &SourceId,
        genres: &[Genre],
        generation: i64,
    ) -> StoreResult<()> {
        self.upsert_genres_with_count_refresh(source_id, genres, generation, true)
    }

    fn upsert_genres_without_count_refresh(
        &self,
        source_id: &SourceId,
        genres: &[Genre],
        generation: i64,
    ) -> StoreResult<()> {
        self.upsert_genres_with_count_refresh(source_id, genres, generation, false)
    }

    fn upsert_genres_with_count_refresh(
        &self,
        source_id: &SourceId,
        genres: &[Genre],
        generation: i64,
        refresh_counts: bool,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            self.require_current_sync_generation(source_id, generation)?;
            let mut statement = connection.prepare(
                "
                INSERT INTO genres (
                    source_id, genre_id, name, album_count, track_count, duration_seconds,
                    image_item_id, image_tag, image_origin, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(source_id, genre_id) DO UPDATE SET
                    name = excluded.name,
                    album_count = excluded.album_count,
                    track_count = excluded.track_count,
                    duration_seconds = CASE
                        WHEN excluded.duration_seconds > 0 THEN excluded.duration_seconds
                        ELSE genres.duration_seconds
                    END,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    image_origin = excluded.image_origin,
                    sync_generation = excluded.sync_generation
                ",
            )?;
            for genre in genres {
                let (image_item_id, image_tag) = image_ref_parts(genre.image_ref.as_ref());
                let image_origin = image_origin_for_source_ref(genre.image_ref.as_ref());
                statement.execute(params![
                    source_id.as_str(),
                    genre.id.as_str(),
                    genre.name,
                    i64::from(genre.album_count),
                    i64::from(genre.track_count),
                    i64::from(genre.duration_seconds),
                    image_item_id,
                    image_tag,
                    image_origin,
                    generation,
                ])?;
                connection.execute(
                    "
                    DELETE FROM genres
                    WHERE source_id = ?1
                      AND genre_id != ?2
                      AND genre_id LIKE 'linked:genre:%'
                      AND name = ?3
                    ",
                    params![source_id.as_str(), genre.id.as_str(), genre.name],
                )?;
                let cover_refs = if genre.image_refs.is_empty() {
                    genre.image_ref.iter().cloned().collect::<Vec<_>>()
                } else {
                    genre.image_refs.clone()
                };
                replace_collection_refs(
                    connection,
                    source_id,
                    COLLECTION_COVER_GENRE,
                    genre.id.as_str(),
                    &cover_refs,
                )?;
            }
            if refresh_counts {
                refresh_genre_counts_on_connection(connection, source_id)?;
            }
            Ok(())
        })
    }
    pub fn upsert_playlists(
        &self,
        source_id: &SourceId,
        playlists: &[Playlist],
        generation: i64,
    ) -> StoreResult<()> {
        self.upsert_playlists_with_mode(
            source_id,
            playlists,
            PlaylistWriteMode::NativeSync { generation },
        )
    }

    pub fn upsert_playlists_with_mode(
        &self,
        source_id: &SourceId,
        playlists: &[Playlist],
        mode: PlaylistWriteMode,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            if let PlaylistWriteMode::NativeSync { generation } = mode {
                self.require_current_sync_generation(source_id, generation)?;
            }
            let owner = mode.owner();
            let generation = mode.sync_generation();
            let mut statement = connection.prepare(
                "
                INSERT INTO playlists (
                    source_id, playlist_id, name, track_count, duration_seconds,
                    top_genres_json, image_item_id, image_tag, image_origin, owner, sync_generation
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(source_id, playlist_id) DO UPDATE SET
                    name = excluded.name,
                    track_count = CASE
                        WHEN EXISTS (
                            SELECT 1
                            FROM playlist_tracks
                            WHERE source_id = excluded.source_id
                                AND playlist_id = excluded.playlist_id
                        ) THEN playlists.track_count
                        ELSE excluded.track_count
                    END,
                    duration_seconds = CASE
                        WHEN EXISTS (
                            SELECT 1
                            FROM playlist_tracks
                            WHERE source_id = excluded.source_id
                                AND playlist_id = excluded.playlist_id
                        ) THEN playlists.duration_seconds
                        ELSE excluded.duration_seconds
                    END,
                    top_genres_json = CASE
                        WHEN EXISTS (
                            SELECT 1
                            FROM playlist_tracks
                            WHERE source_id = excluded.source_id
                                AND playlist_id = excluded.playlist_id
                        ) THEN playlists.top_genres_json
                        ELSE excluded.top_genres_json
                    END,
                    image_item_id = excluded.image_item_id,
                    image_tag = excluded.image_tag,
                    image_origin = excluded.image_origin,
                    sync_generation = excluded.sync_generation
                WHERE playlists.owner = excluded.owner
                ",
            )?;
            let mut delete_fts = connection.prepare(
                "DELETE FROM library_fts WHERE source_id = ?1 AND item_type = 'playlist' AND item_id = ?2",
            )?;
            let mut insert_fts = connection.prepare(
                "
                INSERT INTO library_fts (source_id, item_type, item_id, title, subtitle)
                VALUES (?1, 'playlist', ?2, ?3, '')
                ",
            )?;

            for playlist in playlists {
                let (image_item_id, image_tag) = image_ref_parts(playlist.image_ref.as_ref());
                let image_origin = image_origin_for_source_ref(playlist.image_ref.as_ref());
                let changed = statement.execute(params![
                    source_id.as_str(),
                    playlist.id.as_str(),
                    playlist.name,
                    i64::from(playlist.track_count),
                    i64::from(playlist.duration_seconds),
                    string_vec_json(&playlist.top_genres)?,
                    image_item_id,
                    image_tag,
                    image_origin,
                    playlist_owner_to_str(owner),
                    generation,
                ])?;
                if changed == 0 {
                    return Err(StoreError::InvalidPlaylistOwner(format!(
                        "playlist {} is not owned by {}",
                        playlist.id.as_str(),
                        playlist_owner_to_str(owner)
                    )));
                }
                let cover_refs = if playlist.image_refs.is_empty() {
                    playlist.image_ref.iter().cloned().collect::<Vec<_>>()
                } else {
                    playlist.image_refs.clone()
                };
                replace_collection_refs(
                    connection,
                    source_id,
                    COLLECTION_COVER_PLAYLIST,
                    playlist.id.as_str(),
                    &cover_refs,
                )?;
                delete_fts.execute(params![source_id.as_str(), playlist.id.as_str()])?;
                insert_fts.execute(params![
                    source_id.as_str(),
                    playlist.id.as_str(),
                    playlist.name,
                ])?;
            }
            Ok(())
        })
    }
    pub fn upsert_home_sections(
        &self,
        source_id: &SourceId,
        sections: &[HomeSection],
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            self.require_current_sync_generation(source_id, generation)?;
            connection.execute(
                "DELETE FROM home_section_items WHERE source_id = ?1",
                params![source_id.as_str()],
            )?;
            for section in sections {
                Self::insert_home_section_items(connection, source_id, section, generation)?;
            }
            Ok(())
        })
    }
    pub fn upsert_home_section(
        &self,
        source_id: &SourceId,
        section: &HomeSection,
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            self.require_current_sync_generation(source_id, generation)?;
            connection.execute(
                "
                DELETE FROM home_section_items
                WHERE source_id = ?1
                  AND section_kind = ?2
                ",
                params![source_id.as_str(), home_section_kind_key(section.kind)],
            )?;
            Self::insert_home_section_items(connection, source_id, section, generation)
        })
    }
    pub fn upsert_home_section_prefetch(
        &self,
        source_id: &SourceId,
        section: &HomeSection,
        generation: i64,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            self.require_current_sync_generation(source_id, generation)?;
            connection.execute(
                "
                DELETE FROM home_section_prefetch_items
                WHERE source_id = ?1
                  AND section_kind = ?2
                ",
                params![source_id.as_str(), home_section_kind_key(section.kind)],
            )?;
            Self::insert_home_items(
                connection,
                "home_section_prefetch_items",
                source_id,
                section,
                generation,
            )
        })
    }
    pub fn clear_home_section_prefetch(
        &self,
        source_id: &SourceId,
        kind: HomeSectionKind,
    ) -> StoreResult<()> {
        self.write_batch(|connection| {
            connection.execute(
                "
                DELETE FROM home_section_prefetch_items
                WHERE source_id = ?1
                  AND section_kind = ?2
                ",
                params![source_id.as_str(), home_section_kind_key(kind)],
            )?;
            Ok(())
        })
    }
    pub(super) fn insert_home_section_items(
        connection: &Connection,
        source_id: &SourceId,
        section: &HomeSection,
        generation: i64,
    ) -> StoreResult<()> {
        Self::insert_home_items(
            connection,
            "home_section_items",
            source_id,
            section,
            generation,
        )
    }
    pub(super) fn insert_home_items(
        connection: &Connection,
        table: &str,
        source_id: &SourceId,
        section: &HomeSection,
        generation: i64,
    ) -> StoreResult<()> {
        let sql = format!(
            "
            INSERT INTO {table} (
                source_id, section_kind, item_type, item_id, position, sync_generation
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(source_id, section_kind, item_type, item_id) DO UPDATE SET
                position = excluded.position,
                sync_generation = excluded.sync_generation
            "
        );
        let mut insert_item = connection.prepare(&sql)?;
        let section_kind = home_section_kind_key(section.kind);
        for (position, album) in section.albums.iter().enumerate() {
            insert_item.execute(params![
                source_id.as_str(),
                section_kind,
                "album",
                album.id.as_str(),
                position as i64,
                generation,
            ])?;
        }
        for (position, track) in section.tracks.iter().enumerate() {
            insert_item.execute(params![
                source_id.as_str(),
                section_kind,
                "track",
                track.id.as_str(),
                position as i64,
                generation,
            ])?;
        }
        Ok(())
    }
}

impl Store {
    pub(super) fn load_album_for_delta(
        &self,
        source_id: &SourceId,
        album_id: &AlbumId,
    ) -> StoreResult<Option<Album>> {
        let mut album = self
            .connection
            .query_row(
                "
                SELECT album_id, title, artist, artist_id, year, release_date, date_added,
                       last_played, play_count, user_rating, track_count, duration_seconds,
                       favorite, color_seed,
                       CASE WHEN image_origin = 'source' THEN image_item_id END,
                       CASE WHEN image_origin = 'source' THEN image_tag END,
                       release_types_json, is_compilation, musicbrainz_album_id,
                       musicbrainz_release_group_id
                FROM albums
                WHERE source_id = ?1 AND album_id = ?2
                ",
                params![source_id.as_str(), album_id.as_str()],
                album_from_row,
            )
            .optional()?;
        if let Some(album) = album.as_mut() {
            self.attach_album_genres(source_id, std::slice::from_mut(album))?;
            self.attach_album_release_metadata(source_id, std::slice::from_mut(album))?;
            let credits = self.load_artist_links(
                source_id,
                "album_artist_links",
                "album_id",
                &[album.id.as_str().to_string()],
            )?;
            album.album_artist_credits =
                credits.get(album.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(album)
    }

    pub(super) fn load_artist_for_delta(
        &self,
        source_id: &SourceId,
        artist_id: &ArtistId,
        album_artist: bool,
    ) -> StoreResult<Option<Artist>> {
        let table = if album_artist {
            "album_artists"
        } else {
            "artists"
        };
        let sql = format!(
            "
            SELECT artist_id, name, album_count, track_count, favorite,
                   last_played, play_count, user_rating,
                   CASE WHEN image_origin = 'source' THEN image_item_id END,
                   CASE WHEN image_origin = 'source' THEN image_tag END
            FROM {table}
            WHERE source_id = ?1 AND artist_id = ?2
            "
        );
        self.connection
            .query_row(
                &sql,
                params![source_id.as_str(), artist_id.as_str()],
                artist_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub(super) fn load_track_for_delta(
        &self,
        source_id: &SourceId,
        track_id: &TrackId,
    ) -> StoreResult<Option<Track>> {
        let mut track = self
            .connection
            .query_row(
                "
                SELECT track_id, album_id, title, artist, artist_id, album, year,
                       release_date, date_added, last_played, play_count, user_rating,
                       duration_seconds, favorite, disc_number, track_number,
                       CASE WHEN image_origin = 'source' THEN image_item_id END,
                       CASE WHEN image_origin = 'source' THEN image_tag END,
                       local_path, source_format, comment, skip_count,
                       bpm
                FROM tracks
                WHERE source_id = ?1 AND track_id = ?2
                ",
                params![source_id.as_str(), track_id.as_str()],
                track_from_row,
            )
            .optional()?;
        if let Some(track) = track.as_mut() {
            self.attach_track_metadata(source_id, std::slice::from_mut(track))?;
        }
        Ok(track)
    }

    fn load_genre_for_delta(
        &self,
        source_id: &SourceId,
        genre_id: &GenreId,
    ) -> StoreResult<Option<Genre>> {
        let genre = self
            .connection
            .query_row(
                "
                SELECT genre_id, name, album_count, track_count, duration_seconds,
                       CASE WHEN image_origin = 'source' THEN image_item_id END,
                       CASE WHEN image_origin = 'source' THEN image_tag END
                FROM genres
                WHERE source_id = ?1 AND genre_id = ?2
                ",
                params![source_id.as_str(), genre_id.as_str()],
                genre_from_row,
            )
            .optional()?;
        Ok(genre)
    }

    pub(super) fn load_playlist_for_delta(
        &self,
        source_id: &SourceId,
        playlist_id: &PlaylistId,
    ) -> StoreResult<Option<Playlist>> {
        let playlist = self
            .connection
            .query_row(
                "
                SELECT playlist_id, name, track_count, duration_seconds, top_genres_json,
                       owner,
                       CASE WHEN image_origin = 'source' THEN image_item_id END,
                       CASE WHEN image_origin = 'source' THEN image_tag END
                FROM playlists
                WHERE source_id = ?1 AND playlist_id = ?2
                ",
                params![source_id.as_str(), playlist_id.as_str()],
                playlist_from_row,
            )
            .optional()?;
        Ok(playlist)
    }
}

fn album_stats_changed(left: &Album, right: &Album) -> bool {
    left.play_count != right.play_count
        || left.last_played != right.last_played
        || left.user_rating != right.user_rating
}

fn album_observation_changed(left: &Album, right: &Album) -> bool {
    album_stats_changed(left, right)
        || album_links_changed(left, right)
        || album_fields_changed(left, right)
        || left.image_ref != right.image_ref
}

fn playlist_summary_matches(left: &Playlist, right: &Playlist) -> bool {
    left.id == right.id && left.name == right.name && left.image_ref == right.image_ref
}

fn refresh_playlists_for_track_ids_on_connection(
    connection: &Connection,
    source_id: &SourceId,
    track_ids: &[TrackId],
) -> StoreResult<()> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let mut playlist_ids = Vec::<PlaylistId>::new();
    let mut seen = HashSet::<PlaylistId>::new();
    for chunk in track_ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "
            SELECT DISTINCT playlist_id
            FROM playlist_tracks
            WHERE source_id = ?
              AND track_id IN ({placeholders})
            ORDER BY playlist_id
            "
        );
        let mut values = Vec::with_capacity(chunk.len() + 1);
        values.push(source_id.as_str());
        values.extend(chunk.iter().map(TrackId::as_str));
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values), |row| {
            row.get::<_, String>(0).map(PlaylistId::new)
        })?;
        for row in rows {
            let playlist_id = row?;
            if seen.insert(playlist_id.clone()) {
                playlist_ids.push(playlist_id);
            }
        }
    }
    for playlist_id in playlist_ids {
        super::library_auxiliary_cache::refresh_playlist_stats(
            connection,
            source_id,
            &playlist_id,
        )?;
        super::library_auxiliary_cache::refresh_playlist_refs(connection, source_id, &playlist_id)?;
    }
    Ok(())
}

fn album_links_changed(left: &Album, right: &Album) -> bool {
    left.artist_id != right.artist_id
        || artist_credits_changed(&left.album_artist_credits, &right.album_artist_credits)
        || left.genres != right.genres
}

fn album_fields_changed(left: &Album, right: &Album) -> bool {
    left.title != right.title
        || left.artist != right.artist
        || left.year != right.year
        || left.release_date != right.release_date
        || left.date_added != right.date_added
        || left.favorite != right.favorite
        || left.color_seed != right.color_seed
        || left.release_types != right.release_types
        || left.is_compilation != right.is_compilation
        || left.musicbrainz_album_id != right.musicbrainz_album_id
        || left.musicbrainz_release_group_id != right.musicbrainz_release_group_id
}

fn track_changed(left: &Track, right: &Track) -> bool {
    track_fields_changed(left, right)
        || track_metadata_changed(left, right)
        || track_stats_changed(left, right)
        || left.album_id != right.album_id
        || track_artist_links_changed(left, right)
        || left.genres != right.genres
        || left.favorite != right.favorite
        || left.image_ref != right.image_ref
}

fn track_metadata_changed(left: &Track, right: &Track) -> bool {
    left.bpm != right.bpm || left.moods != right.moods
}

fn track_fields_changed(left: &Track, right: &Track) -> bool {
    left.title != right.title
        || left.artist != right.artist
        || left.album != right.album
        || left.year != right.year
        || left.release_date != right.release_date
        || left.date_added != right.date_added
        || left.duration_seconds != right.duration_seconds
        || left.disc_number != right.disc_number
        || left.track_number != right.track_number
        || left.local_path != right.local_path
        || left.source_format != right.source_format
        || left.musicbrainz_recording_id != right.musicbrainz_recording_id
        || left.musicbrainz_release_track_id != right.musicbrainz_release_track_id
        || left.comment != right.comment
}

fn track_stats_changed(left: &Track, right: &Track) -> bool {
    left.last_played != right.last_played
        || left.play_count != right.play_count
        || left.user_rating != right.user_rating
        || left.skip_count != right.skip_count
}

fn track_artist_links_changed(left: &Track, right: &Track) -> bool {
    left.artist_id != right.artist_id
        || (!right.artist_credits.is_empty()
            && artist_credits_changed(&left.artist_credits, &right.artist_credits))
}

fn artist_credits_changed(left: &[ArtistCredit], right: &[ArtistCredit]) -> bool {
    left.len() != right.len()
        || left.iter().zip(right.iter()).any(|(left, right)| {
            left.id != right.id
                || left.name != right.name
                || right
                    .musicbrainz_artist_id
                    .as_ref()
                    .is_some_and(|right_id| left.musicbrainz_artist_id.as_ref() != Some(right_id))
        })
}

fn artist_projection_changed(left: &Artist, right: &Artist) -> bool {
    left.id != right.id
        || left.name != right.name
        || left.favorite != right.favorite
        || left.last_played != right.last_played
        || left.play_count != right.play_count
        || left.user_rating != right.user_rating
        || left.image_ref != right.image_ref
}

fn artist_stats_changed(left: &Artist, right: &Artist) -> bool {
    left.last_played != right.last_played
        || left.play_count != right.play_count
        || left.user_rating != right.user_rating
}

fn artist_fields_changed(left: &Artist, right: &Artist) -> bool {
    left.name != right.name
        || left.favorite != right.favorite
        || left.musicbrainz_artist_id != right.musicbrainz_artist_id
}

fn refresh_genre_counts_on_connection(
    connection: &Connection,
    source_id: &SourceId,
) -> StoreResult<()> {
    connection.execute(
        "
        WITH linked_albums AS MATERIALIZED (
            SELECT links.source_id, links.genre_name, albums.album_id
            FROM album_genres links
            JOIN albums
              ON albums.source_id = links.source_id
             AND albums.album_id = links.album_id
            WHERE links.source_id = ?1
            UNION
            SELECT links.source_id, links.genre_name, albums.album_id
            FROM track_genres links
            JOIN tracks
              ON tracks.source_id = links.source_id
             AND tracks.track_id = links.track_id
            JOIN albums
              ON albums.source_id = tracks.source_id
             AND albums.album_id = tracks.album_id
            WHERE links.source_id = ?1
        ),
        album_counts AS MATERIALIZED (
            SELECT source_id, genre_name, COUNT(*) AS album_count
            FROM linked_albums
            GROUP BY source_id, genre_name
        ),
        track_counts AS MATERIALIZED (
            SELECT links.source_id, links.genre_name,
                   COUNT(DISTINCT links.track_id) AS track_count,
                   COALESCE(SUM(tracks.duration_seconds), 0) AS duration_seconds
            FROM track_genres links
            LEFT JOIN tracks
              ON tracks.source_id = links.source_id
             AND tracks.track_id = links.track_id
            WHERE links.source_id = ?1
            GROUP BY links.source_id, links.genre_name
        ),
        computed AS MATERIALIZED (
            SELECT genres.rowid AS row_id,
                   COALESCE(album_counts.album_count, 0) AS album_count,
                   COALESCE(track_counts.track_count, 0) AS track_count,
                   COALESCE(track_counts.duration_seconds, 0) AS duration_seconds
            FROM genres
            LEFT JOIN album_counts
              ON album_counts.source_id = genres.source_id
             AND album_counts.genre_name = genres.name
            LEFT JOIN track_counts
              ON track_counts.source_id = genres.source_id
             AND track_counts.genre_name = genres.name
            WHERE genres.source_id = ?1
        )
        UPDATE genres
        SET album_count = (SELECT album_count FROM computed WHERE row_id = genres.rowid),
            track_count = (SELECT track_count FROM computed WHERE row_id = genres.rowid),
            duration_seconds = (SELECT duration_seconds FROM computed WHERE row_id = genres.rowid)
        WHERE rowid IN (
            SELECT row_id
            FROM computed
            WHERE computed.album_count != genres.album_count
               OR computed.track_count != genres.track_count
               OR computed.duration_seconds != genres.duration_seconds
        )
        ",
        params![source_id.as_str()],
    )?;
    Ok(())
}

fn genre_delta_unchanged(left: &Genre, right: &Genre) -> bool {
    left.id == right.id && left.name == right.name && left.image_ref == right.image_ref
}

#[cfg(test)]
mod tests {
    use super::test_support::{StoreCase, album, artist, credit, genre, track};
    use super::*;

    #[test]
    fn track_activity_changes_are_stats() {
        let album = album(1);
        let original = track(1, &album);
        let mut played = original.clone();
        played.play_count = Some(1);
        played.last_played = Some("2026-06-08T12:00:00Z".to_string());
        played.skip_count = Some(1);

        assert!(track_stats_changed(&original, &played));
        assert!(!track_fields_changed(&original, &played));

        let mut renamed = original.clone();
        renamed.title = "Updated title".to_string();
        assert!(track_fields_changed(&original, &renamed));
    }

    #[test]
    fn linked_artist_and_genre_counts_are_distinct() {
        let case = StoreCase::open();
        let generation = case.start_sync("begin sync");
        let mut first_album = album(1);
        first_album.genres = vec!["Genre 1".to_string()];
        let mut second_album = album(2);
        second_album.artist = "Artist 2".to_string();
        second_album.artist_id = Some(ArtistId::fake(2));
        let mut first_track = track(1, &first_album);
        first_track
            .artist_credits
            .push(credit(ArtistId::fake(2), "Artist 2"));
        let mut second_track = track(2, &second_album);
        second_track.genres = vec!["Genre 1".to_string()];
        let primary_artist = artist(1, None);
        let mut credited_artist = artist(2, None);
        credited_artist.album_count = 0;
        credited_artist.track_count = 0;
        let mut linked_genre = genre(1, None);
        linked_genre.album_count = 0;
        linked_genre.track_count = 0;
        linked_genre.duration_seconds = 0;

        case.upsert_albums(&case.id, &[first_album, second_album], generation)
            .expect("upsert albums");
        case.upsert_tracks(&case.id, &[first_track, second_track], generation)
            .expect("upsert tracks");
        case.upsert_artists(
            &case.id,
            &[primary_artist, credited_artist.clone()],
            false,
            generation,
        )
        .expect("upsert artists");
        case.upsert_genres(&case.id, std::slice::from_ref(&linked_genre), generation)
            .expect("upsert genre");
        case.refresh_library_counts(&case.id)
            .expect("refresh counts");

        let credited_artist = case
            .load_artists(&case.id, false, 0, 10)
            .expect("load artists")
            .items
            .into_iter()
            .find(|artist| artist.id == credited_artist.id)
            .expect("credited artist");
        let linked_genre = case
            .load_genres(&case.id, 0, 10)
            .expect("load genres")
            .items
            .into_iter()
            .find(|genre| genre.id == linked_genre.id)
            .expect("linked genre");

        assert_eq!(
            (credited_artist.album_count, credited_artist.track_count),
            (2, 2)
        );
        assert_eq!((linked_genre.album_count, linked_genre.track_count), (2, 2));
        assert_eq!(linked_genre.duration_seconds, 360);
    }
}

use super::servers::*;
use super::*;

impl Store {
    pub(super) fn attach_album_genres(
        &self,
        server_id: &ServerId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
        if albums.is_empty() {
            return Ok(());
        }
        let ids = albums
            .iter()
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        let genres = self.load_genre_links(server_id, "album_genres", "album_id", &ids)?;
        for album in albums {
            album.genres = genres.get(album.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }

    pub(super) fn attach_album_metadata(
        &self,
        server_id: &ServerId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
        self.attach_album_genres(server_id, albums)?;
        self.attach_album_track_fallback_image_refs(server_id, albums)?;
        if albums.is_empty() {
            return Ok(());
        }
        let ids = albums
            .iter()
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        let credits = self.load_artist_links(server_id, "album_artist_links", "album_id", &ids)?;
        for album in albums {
            album.album_artist_credits =
                credits.get(album.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }

    pub(super) fn attach_album_track_fallback_image_refs(
        &self,
        server_id: &ServerId,
        albums: &mut [Album],
    ) -> StoreResult<()> {
        let missing_ids = albums
            .iter()
            .filter(|album| album.image_ref.is_none())
            .map(|album| album.id.as_str().to_string())
            .collect::<Vec<_>>();
        if missing_ids.is_empty() {
            return Ok(());
        }

        let mut fallback_by_album = HashMap::<String, ImageRef>::new();
        for chunk in missing_ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT album_id, image_item_id, image_tag
                FROM tracks
                WHERE server_id = ?
                  AND album_id IN ({placeholders})
                  AND image_item_id IS NOT NULL
                ORDER BY album_id, disc_number, track_number, title COLLATE NOCASE
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ImageRef {
                        item_id: row.get(1)?,
                        tag: row.get(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (album_id, image_ref) = row?;
                fallback_by_album.entry(album_id).or_insert(image_ref);
            }
        }

        for album in albums {
            if album.image_ref.is_none()
                && let Some(image_ref) = fallback_by_album.remove(album.id.as_str())
            {
                album.image_ref = Some(image_ref);
            }
        }
        Ok(())
    }

    pub fn load_album_image_refs(
        &self,
        server_id: &ServerId,
        album_ids: &[AlbumId],
    ) -> StoreResult<HashMap<AlbumId, ImageRef>> {
        let mut unique_ids = Vec::<AlbumId>::new();
        for album_id in album_ids {
            if !unique_ids.iter().any(|existing| existing == album_id) {
                unique_ids.push(album_id.clone());
            }
        }
        if unique_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut image_refs = HashMap::<AlbumId, ImageRef>::new();
        for chunk in unique_ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT album_id, image_item_id, image_tag
                FROM albums
                WHERE server_id = ?
                  AND album_id IN ({placeholders})
                  AND image_item_id IS NOT NULL
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(AlbumId::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    AlbumId::new(row.get::<_, String>(0)?),
                    ImageRef {
                        item_id: row.get(1)?,
                        tag: row.get(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (album_id, image_ref) = row?;
                image_refs.entry(album_id).or_insert(image_ref);
            }
        }

        let missing_ids = unique_ids
            .iter()
            .filter(|album_id| !image_refs.contains_key(*album_id))
            .cloned()
            .collect::<Vec<_>>();
        for chunk in missing_ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT album_id, image_item_id, image_tag
                FROM tracks
                WHERE server_id = ?
                  AND album_id IN ({placeholders})
                  AND image_item_id IS NOT NULL
                ORDER BY album_id, disc_number, track_number, title COLLATE NOCASE
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(AlbumId::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    AlbumId::new(row.get::<_, String>(0)?),
                    ImageRef {
                        item_id: row.get(1)?,
                        tag: row.get(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (album_id, image_ref) = row?;
                image_refs.entry(album_id).or_insert(image_ref);
            }
        }
        Ok(image_refs)
    }

    pub(super) fn attach_track_genres(
        &self,
        server_id: &ServerId,
        tracks: &mut [Track],
    ) -> StoreResult<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        let ids = tracks
            .iter()
            .map(|track| track.id.as_str().to_string())
            .collect::<Vec<_>>();
        let genres = self.load_genre_links(server_id, "track_genres", "track_id", &ids)?;
        for track in tracks {
            track.genres = genres.get(track.id.as_str()).cloned().unwrap_or_default();
        }
        Ok(())
    }

    pub(super) fn attach_track_metadata(
        &self,
        server_id: &ServerId,
        tracks: &mut [Track],
    ) -> StoreResult<()> {
        self.attach_track_genres(server_id, tracks)?;
        if tracks.is_empty() {
            return Ok(());
        }
        let track_ids = tracks
            .iter()
            .map(|track| track.id.as_str().to_string())
            .collect::<Vec<_>>();
        let artist_credits =
            self.load_artist_links(server_id, "track_artist_links", "track_id", &track_ids)?;
        let album_ids = tracks
            .iter()
            .map(|track| track.album_id.as_str().to_string())
            .collect::<Vec<_>>();
        let album_artist_credits =
            self.load_artist_links(server_id, "album_artist_links", "album_id", &album_ids)?;
        for track in tracks {
            track.artist_credits = artist_credits
                .get(track.id.as_str())
                .cloned()
                .unwrap_or_default();
            track.album_artist_credits = album_artist_credits
                .get(track.album_id.as_str())
                .cloned()
                .unwrap_or_default();
        }
        Ok(())
    }

    pub(super) fn attach_artist_fallback_image_refs(
        &self,
        server_id: &ServerId,
        artists: &mut [Artist],
        album_artist: bool,
    ) -> StoreResult<()> {
        let missing_ids = artists
            .iter()
            .filter(|artist| artist.image_ref.is_none())
            .map(|artist| artist.id.as_str().to_string())
            .collect::<Vec<_>>();
        if missing_ids.is_empty() {
            return Ok(());
        }
        let mut fallback_by_artist = HashMap::<String, ImageRef>::new();
        for chunk in missing_ids.chunks(500) {
            let values_placeholders = std::iter::repeat_n("(?)", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = artist_fallback_image_refs_sql(album_artist, &values_placeholders);
            let mut values = Vec::with_capacity(chunk.len() + if album_artist { 2 } else { 4 });
            values.extend(chunk.iter().map(String::as_str));
            values.extend(std::iter::repeat_n(
                server_id.as_str(),
                if album_artist { 2 } else { 4 },
            ));

            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ImageRef {
                        item_id: row.get(1)?,
                        tag: row.get(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (artist_id, image_ref) = row?;
                fallback_by_artist.entry(artist_id).or_insert(image_ref);
            }
        }
        for artist in artists {
            if artist.image_ref.is_none()
                && let Some(image_ref) = fallback_by_artist.remove(artist.id.as_str())
            {
                artist.image_ref = Some(image_ref);
            }
        }
        Ok(())
    }

    pub fn load_artist_fallback_albums(
        &self,
        server_id: &ServerId,
        album_artist: bool,
        artist_ids: &[ArtistId],
    ) -> StoreResult<HashMap<ArtistId, Album>> {
        let mut fallback_by_artist = HashMap::<ArtistId, Album>::new();
        if artist_ids.is_empty() {
            return Ok(fallback_by_artist);
        }

        for chunk in artist_ids.chunks(500) {
            let values_placeholders = std::iter::repeat_n("(?)", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = artist_fallback_albums_sql(album_artist, &values_placeholders);
            let mut values = Vec::with_capacity(chunk.len() + if album_artist { 2 } else { 4 });
            values.extend(chunk.iter().map(ArtistId::as_str));
            values.extend(std::iter::repeat_n(
                server_id.as_str(),
                if album_artist { 2 } else { 4 },
            ));

            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    ArtistId::new(row.get::<_, String>(16)?),
                    album_from_row(row)?,
                ))
            })?;
            for row in rows {
                let (artist_id, album) = row?;
                fallback_by_artist.entry(artist_id).or_insert(album);
            }
        }

        Ok(fallback_by_artist)
    }

    pub(super) fn load_genre_links(
        &self,
        server_id: &ServerId,
        table: &str,
        id_column: &str,
        ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<String>>> {
        let mut by_item = HashMap::<String, Vec<String>>::new();
        for chunk in ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT {id_column}, genre_name
                FROM {table}
                WHERE server_id = ?
                  AND {id_column} IN ({placeholders})
                ORDER BY genre_name COLLATE NOCASE
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (item_id, genre_name) = row?;
                by_item.entry(item_id).or_default().push(genre_name);
            }
        }
        Ok(by_item)
    }

    pub(super) fn load_artist_links(
        &self,
        server_id: &ServerId,
        table: &str,
        id_column: &str,
        ids: &[String],
    ) -> StoreResult<HashMap<String, Vec<ArtistCredit>>> {
        let mut by_item = HashMap::<String, Vec<ArtistCredit>>::new();
        for chunk in ids.chunks(500) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT {id_column}, artist_id, name
                FROM {table}
                WHERE server_id = ?
                  AND {id_column} IN ({placeholders})
                ORDER BY position
                "
            );
            let mut values = Vec::with_capacity(chunk.len() + 1);
            values.push(server_id.as_str());
            values.extend(chunk.iter().map(String::as_str));
            let mut statement = self.connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(values), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ArtistCredit {
                        id: ArtistId::new(row.get::<_, String>(1)?),
                        name: row.get::<_, String>(2)?,
                    },
                ))
            })?;
            for row in rows {
                let (item_id, credit) = row?;
                by_item.entry(item_id).or_default().push(credit);
            }
        }
        Ok(by_item)
    }
}

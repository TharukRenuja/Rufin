use super::*;

impl Shell {
    pub(in crate::ui) fn genre_detail_view(
        self: &Rc<Self>,
        genre_id: domain::GenreId,
    ) -> gtk::Widget {
        let detail = self
            .genre_detail_from_memory(&genre_id)
            .or_else(|| {
                self.controller
                    .cached_genre_detail(&genre_id)
                    .ok()
                    .flatten()
            })
            .or_else(|| {
                let library = self.state.library.borrow();
                let genre = library
                    .genres
                    .iter()
                    .find(|genre| genre.id.as_str() == genre_id.as_str())
                    .cloned()?;
                Some(CachedGenreDetail {
                    genre,
                    albums: Vec::new(),
                    tracks: Vec::new(),
                })
            });
        let Some(detail) = detail else {
            return self.placeholder_view("Genre", "The selected cached genre was not found.");
        };
        let seed = stable_seed(detail.genre.id.as_str());
        let mut summary_items = vec![(
            "route-tracks-symbolic",
            track_count_text(detail.genre.track_count.into()),
        )];
        if detail.genre.duration_seconds > 0 {
            summary_items.push((
                "appointment-soon-symbolic",
                format_duration_units(detail.genre.duration_seconds),
            ));
        }
        let cover_refs = if detail.genre.image_refs.is_empty() {
            grouped_cover_refs_for_items(&detail.albums, &detail.tracks)
        } else {
            detail.genre.image_refs.clone()
        };
        let mut genre = detail.genre;
        genre.image_refs = cover_refs;
        let artwork = crate::cover_art_policy::selected_genre_artwork(&genre);
        self.grouped_detail_view(GroupedDetailData {
            title: genre.name,
            artwork,
            seed,
            summary_items,
            tracks: detail.tracks,
            table_context: "genre-detail",
            source_descriptor: Some(PlaySourceDescriptor::GenreTracks {
                genre_id,
                selected_music_folder_id: selected_music_folder_id(self),
            }),
        })
    }

    fn genre_detail_from_memory(
        self: &Rc<Self>,
        genre_id: &domain::GenreId,
    ) -> Option<CachedGenreDetail> {
        let library = self.state.library.borrow();
        if library.cached_track_count > library.tracks.len() {
            return None;
        }
        let genre = library
            .genres
            .iter()
            .find(|genre| genre.id.as_str() == genre_id.as_str())
            .cloned()?;
        let tracks = library
            .tracks
            .iter()
            .filter(|track| track.genres.iter().any(|name| name == &genre.name))
            .cloned()
            .collect::<Vec<_>>();
        Some(CachedGenreDetail {
            genre,
            albums: Vec::new(),
            tracks,
        })
    }
}

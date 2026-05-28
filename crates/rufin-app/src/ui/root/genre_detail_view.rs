use super::*;

impl Shell {
    pub(in crate::ui) fn genre_detail_view(
        self: &Rc<Self>,
        genre_id: rufin_core::GenreId,
    ) -> gtk::Widget {
        let detail = self
            .controller
            .cached_genre_detail(&genre_id)
            .ok()
            .flatten()
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
        let summary = format!("{} {}", detail.genre.track_count, tr("tracks"));
        let cover_refs = grouped_cover_refs_for_items(&detail.albums, &detail.tracks);
        self.grouped_detail_view(GroupedDetailData {
            title: detail.genre.name,
            image_ref: detail.genre.image_ref,
            cover_refs,
            seed,
            summary,
            tracks: detail.tracks,
            table_context: "genre-detail",
        })
    }
}

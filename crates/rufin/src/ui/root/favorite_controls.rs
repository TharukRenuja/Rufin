use super::*;

impl Shell {
    pub(in crate::ui) fn register_favorite_button(
        &self,
        key: FavoriteControlKey,
        button: &gtk::Button,
    ) {
        register_favorite_control(&self.state.favorite_controls, key, button);
    }
    pub(in crate::ui) fn register_dynamic_favorite_button(
        &self,
        key: Rc<dyn Fn() -> Option<FavoriteControlKey>>,
        button: &gtk::Button,
    ) {
        register_dynamic_favorite_control(&self.state.favorite_controls, key, button);
    }
    pub(in crate::ui) fn update_visible_favorite_buttons(
        &self,
        item_id: &FavoriteItemId,
        favorite: bool,
    ) {
        let key = favorite_control_key(item_id);
        update_favorite_controls(&self.state.favorite_controls, &key, favorite);
    }
    pub(in crate::ui) fn set_favorite_with_feedback(
        self: &Rc<Self>,
        item_id: FavoriteItemId,
        favorite: bool,
        button: Option<&gtk::Button>,
    ) {
        if let Some(button) = button {
            set_favorite_button_active(button, favorite);
        }
        if let FavoriteItemId::Track(track_id) = &item_id
            && let Some(current) = self.state.player.borrow_mut().current.as_mut()
            && current.track_id == *track_id
        {
            current.favorite = favorite;
        }
        self.update_visible_favorite_buttons(&item_id, favorite);
        match item_id {
            FavoriteItemId::Album(album_id) => {
                self.controller.set_album_favorite(album_id, favorite)
            }
            FavoriteItemId::Track(track_id) => {
                self.controller.set_track_favorite(track_id, favorite)
            }
            FavoriteItemId::Artist(artist_id) => {
                self.controller.set_artist_favorite(artist_id, favorite)
            }
        }
        let title = if favorite {
            tr("Added to favorites")
        } else {
            tr("Removed from favorites")
        };
        self.show_control_feedback_toast(title);
    }
    pub(in crate::ui) fn apply_favorite_changed(
        self: &Rc<Self>,
        item_id: FavoriteItemId,
        favorite: bool,
        snapshot: LibrarySnapshot,
    ) {
        let route = self.state.routes.borrow().current().clone();
        {
            let mut library = self.state.library.borrow_mut();
            merge_favorite_snapshot(
                &mut library,
                snapshot,
                &item_id,
                favorite,
                matches!(route, Route::Search { .. }),
            );
        }

        self.update_search_favorite(&item_id, favorite);
        if let FavoriteItemId::Track(track_id) = &item_id
            && let Some(current) = self.state.player.borrow_mut().current.as_mut()
            && current.track_id == *track_id
        {
            current.favorite = favorite;
            set_favorite_button_active(&self.player_controls.favorite_button, favorite);
        }
        self.update_visible_favorite_buttons(&item_id, favorite);
        let track_sort_key = self.state.settings.borrow().track_table.sort_key;
        if favorite_change_needs_route_render(&route, &item_id, track_sort_key) {
            self.render_current_route_preserving_scroll();
        }
    }
}

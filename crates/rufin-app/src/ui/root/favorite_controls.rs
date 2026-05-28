use super::*;

impl Shell {
    pub(in crate::ui) fn register_favorite_button(
        &self,
        key: FavoriteControlKey,
        button: &gtk::Button,
    ) {
        register_favorite_control(&self.state.favorite_controls, key, button);
    }
    pub(in crate::ui) fn unregister_favorite_button(
        &self,
        key: &FavoriteControlKey,
        button: &gtk::Button,
    ) {
        unregister_favorite_control(&self.state.favorite_controls, key, button);
    }
    pub(in crate::ui) fn update_visible_favorite_buttons(
        &self,
        item_id: &FavoriteItemId,
        favorite: bool,
    ) {
        let key = favorite_control_key(item_id);
        update_favorite_controls(&self.state.favorite_controls, &key, favorite);
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

        self.update_visible_favorite_buttons(&item_id, favorite);
        let track_sort_key = self.state.settings.borrow().track_table.sort_key;
        if favorite_change_needs_route_render(&route, &item_id, track_sort_key) {
            self.render_current_route();
        }
    }
}

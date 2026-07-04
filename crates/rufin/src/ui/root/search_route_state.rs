use super::*;

impl Shell {
    pub(in crate::ui) fn start_search_for_route(self: &Rc<Self>, route: &Route) {
        let Route::Search { query, kind } = route else {
            self.state
                .search_request_generation
                .set(self.state.search_request_generation.get().saturating_add(1));
            *self.state.search_state.borrow_mut() = SearchRouteState::default();
            return;
        };

        let request_id = self.state.search_request_generation.get().saturating_add(1);
        self.state.search_request_generation.set(request_id);
        let key = self.search_key_for_route(request_id, query.clone(), kind.clone());
        *self.state.search_state.borrow_mut() = SearchRouteState {
            key: Some(key.clone()),
            loading: true,
            results: SearchResults::default(),
            error: None,
        };
        self.controller.load_search_for_active(key);
    }

    pub(in crate::ui) fn apply_search_loaded(
        self: &Rc<Self>,
        key: SearchRequestKey,
        results: SearchResults,
    ) {
        if !self.search_event_is_current(&key) {
            return;
        }
        {
            let mut state = self.state.search_state.borrow_mut();
            state.loading = false;
            state.results = results;
            state.error = None;
        }
        if self.search_route_matches_key(&key) {
            self.render_current_route_preserving_scroll();
        }
    }

    pub(in crate::ui) fn apply_search_failed(
        self: &Rc<Self>,
        key: SearchRequestKey,
        error: String,
    ) {
        warn!(%error, "search failed");
        if !self.search_event_is_current(&key) {
            return;
        }
        {
            let mut state = self.state.search_state.borrow_mut();
            state.loading = false;
            state.results = SearchResults::default();
            state.error = Some(error);
        }
        if self.search_route_matches_key(&key) {
            self.render_current_route_preserving_scroll();
        }
    }

    pub(in crate::ui) fn current_search_view(
        &self,
        query: &str,
        kind: &SearchKind,
    ) -> (SearchResults, bool, Option<String>) {
        let state = self.state.search_state.borrow();
        let Some(key) = &state.key else {
            return (SearchResults::default(), false, None);
        };
        if !self.search_identity_matches(key, query, kind) {
            return (SearchResults::default(), false, None);
        }
        (state.results.clone(), state.loading, state.error.clone())
    }

    pub(in crate::ui) fn update_search_favorite(&self, item_id: &FavoriteItemId, favorite: bool) {
        let mut state = self.state.search_state.borrow_mut();
        apply_search_favorite_change(&mut state.results, item_id, favorite);
    }

    fn search_key_for_route(
        &self,
        request_id: u64,
        query: String,
        kind: SearchKind,
    ) -> SearchRequestKey {
        let library = self.state.library.borrow();
        SearchRequestKey {
            request_id,
            query,
            kind,
            source_id: library.source.as_ref().map(|server| server.id.clone()),
            selected_music_folder_id: library.selected_music_folder_id.clone(),
        }
    }

    fn search_event_is_current(&self, key: &SearchRequestKey) -> bool {
        let state = self.state.search_state.borrow();
        match self.state.routes.borrow().current() {
            Route::Search { query, kind } => {
                let library = self.state.library.borrow();
                search_event_matches(
                    state.key.as_ref(),
                    key,
                    query,
                    kind,
                    library.source.as_ref().map(|server| &server.id),
                    library.selected_music_folder_id.as_ref(),
                )
            }
            _ => false,
        }
    }

    fn search_route_matches_key(&self, key: &SearchRequestKey) -> bool {
        match self.state.routes.borrow().current() {
            Route::Search { query, kind } => self.search_identity_matches(key, query, kind),
            _ => false,
        }
    }

    fn search_identity_matches(
        &self,
        key: &SearchRequestKey,
        query: &str,
        kind: &SearchKind,
    ) -> bool {
        let library = self.state.library.borrow();
        search_key_matches(
            key,
            query,
            kind,
            library.source.as_ref().map(|server| &server.id),
            library.selected_music_folder_id.as_ref(),
        )
    }
}

pub(in crate::ui) fn search_key_matches(
    key: &SearchRequestKey,
    query: &str,
    kind: &SearchKind,
    source_id: Option<&SourceId>,
    selected_music_folder_id: Option<&MusicFolderId>,
) -> bool {
    key.query == query
        && key.kind == *kind
        && key.source_id.as_ref() == source_id
        && key.selected_music_folder_id.as_ref() == selected_music_folder_id
}

pub(in crate::ui) fn search_event_matches(
    current_key: Option<&SearchRequestKey>,
    event_key: &SearchRequestKey,
    query: &str,
    kind: &SearchKind,
    source_id: Option<&SourceId>,
    selected_music_folder_id: Option<&MusicFolderId>,
) -> bool {
    current_key.is_some_and(|current| current == event_key)
        && search_key_matches(event_key, query, kind, source_id, selected_music_folder_id)
}

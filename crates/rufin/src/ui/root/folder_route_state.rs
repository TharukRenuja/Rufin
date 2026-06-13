use super::*;

impl Shell {
    pub(in crate::ui) fn start_folder_load(self: &Rc<Self>, path: Vec<FolderPathItem>) {
        let request_id = self.state.folder_request_generation.get().saturating_add(1);
        self.state.folder_request_generation.set(request_id);
        *self.state.folder_state.borrow_mut() = FolderRouteState {
            request_id,
            path: path.clone(),
            loading: true,
            detail: None,
            error: None,
        };
        self.controller.load_folder_for_active(request_id, path);
    }
    pub(in crate::ui) fn apply_folder_loaded(
        self: &Rc<Self>,
        request_id: u64,
        path: Vec<FolderPathItem>,
        detail: FolderDetail,
    ) {
        let should_render = {
            let mut state = self.state.folder_state.borrow_mut();
            if state.request_id != request_id || state.path != path {
                return;
            }
            state.loading = false;
            state.detail = Some(detail);
            state.error = None;
            matches!(
                self.state.routes.borrow().current(),
                Route::Folders { path: current_path } if current_path == &state.path
            )
        };
        if should_render {
            self.render_current_route();
        }
    }
    pub(in crate::ui) fn apply_folder_load_failed(
        self: &Rc<Self>,
        request_id: u64,
        path: Vec<FolderPathItem>,
        error: String,
    ) {
        warn!(%error, "folder load failed");
        let should_render = {
            let mut state = self.state.folder_state.borrow_mut();
            if state.request_id != request_id || state.path != path {
                return;
            }
            state.loading = false;
            state.detail = None;
            state.error = Some(error);
            matches!(
                self.state.routes.borrow().current(),
                Route::Folders { path: current_path } if current_path == &state.path
            )
        };
        if should_render {
            self.render_current_route();
        }
    }
}

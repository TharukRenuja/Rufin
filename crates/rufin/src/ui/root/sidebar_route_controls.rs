use super::*;

impl Shell {
    pub(in crate::ui) fn update_server_selector(self: &Rc<Self>) {
        source_selector::update_server_selector(self);
    }
    pub(in crate::ui) fn present_library_preferences_dialog(self: &Rc<Self>) {
        present_library_preferences_dialog(self);
    }
    pub(in crate::ui) fn rebuild_sidebar_navigation(self: &Rc<Self>) {
        rebuild_navigation(self);
        self.update_layout();
    }
    pub(in crate::ui) fn set_history_buttons_sensitive(&self, _can_back: bool, _can_forward: bool) {
    }
}

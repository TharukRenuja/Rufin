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
    pub(in crate::ui) fn set_history_buttons_sensitive(&self, can_back: bool, can_forward: bool) {
        self.normal_back_button.set_sensitive(can_back);
        self.compact_back_button.set_sensitive(can_back);
        self.normal_forward_button.set_sensitive(can_forward);
        self.compact_forward_button.set_sensitive(can_forward);
    }
}

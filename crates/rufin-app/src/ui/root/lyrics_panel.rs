use super::*;

const LYRICS_SEARCH_DEBOUNCE_MILLIS: u64 = 600;

impl Shell {
    pub(in crate::ui) fn render_lyrics_panel(self: &Rc<Self>) {
        self.render_lyrics_pane(&self.lyrics_pane);
        self.render_lyrics_pane(&self.fullscreen_player.lyrics_pane);
        self.update_lyrics_highlight();
        self.request_auto_lyrics_if_needed();
    }
    pub(in crate::ui) fn render_lyrics_pane(self: &Rc<Self>, pane: &LyricsPane) {
        let settings = self.state.settings.borrow();
        let current_track_id = current_playback_track_id(&self.state.player.borrow());
        let has_current_track = current_track_id.is_some();
        let (search_label, search_enabled) = if settings.private_mode {
            (tr("Private mode is on"), false)
        } else if has_current_track {
            (tr("Search lyrics"), true)
        } else {
            (tr("No track playing"), false)
        };
        let lyrics = self.state.lyrics.borrow();
        let clear_auto_search_enabled =
            auto_lyrics_skip_action_enabled(&settings, current_track_id.as_ref(), lyrics.as_ref());
        drop(settings);
        pane.set_search_action(&search_label, search_enabled);
        pane.set_clear_auto_search_action(
            &tr("Clear fetched lyrics for this track"),
            clear_auto_search_enabled,
        );
        let empty_status = self.lyrics_empty_status();
        let seek_shell = Rc::clone(self);
        let seek: Rc<dyn Fn(u64)> = Rc::new(move |position_millis| {
            seek_shell.seek_to_lyrics_position(position_millis);
        });
        pane.set_content(lyrics.as_ref(), empty_status, seek);
        drop(lyrics);
    }
    pub(in crate::ui) fn present_lyrics_search_dialog(self: &Rc<Self>) {
        if let Some(dialog) = self.state.lyrics_search_dialog.borrow().as_ref() {
            dialog.dialog.present(Some(&self.window));
            dialog.title_entry.grab_focus();
            return;
        }

        let Some(current) = self.state.player.borrow().current.clone() else {
            return;
        };
        if self.state.settings.borrow().private_mode {
            return;
        }

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_width_request(420);
        content.set_height_request(500);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&tr("Search Lyrics")));
        title.add_css_class("title");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        header.append(&title);
        let close_button = icon_button("window-close-symbolic", "Close");
        header.append(&close_button);
        content.append(&header);

        let helper = gtk::Label::new(Some(&tr(
            "Tap to the entry to load the lyrics and save in the app cache, click save only if you also want to save it in your music folder",
        )));
        helper.add_css_class("lyrics-search-helper");
        helper.set_xalign(0.0);
        helper.set_wrap(true);
        content.append(&helper);

        let artist_entry = gtk::Entry::new();
        artist_entry.set_placeholder_text(Some(&tr("Artist")));
        artist_entry.set_text(&current.artist);
        artist_entry.set_hexpand(true);
        content.append(&artist_entry);

        let title_entry = gtk::Entry::new();
        title_entry.set_placeholder_text(Some(&tr("Song")));
        title_entry.set_text(&current.title);
        title_entry.set_hexpand(true);
        content.append(&title_entry);

        let status = gtk::Label::new(Some(&tr("Ready")));
        status.add_css_class("muted");
        status.set_xalign(0.0);
        status.set_wrap(true);
        content.append(&status);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        content.append(&scroller);

        let dialog = adw::Dialog::builder()
            .content_width(520)
            .content_height(560)
            .child(&content)
            .build();
        let search_dialog = LyricsSearchDialog {
            dialog: dialog.clone(),
            track_id: current.track_id,
            artist_entry: artist_entry.clone(),
            title_entry: title_entry.clone(),
            search_debounce_source: Rc::new(RefCell::new(None)),
            list,
            status,
        };
        *self.state.lyrics_search_dialog.borrow_mut() = Some(search_dialog.clone());

        let close_shell = Rc::clone(self);
        let close_debounce_source = Rc::clone(&search_dialog.search_debounce_source);
        dialog.connect_closed(move |_| {
            if let Some(source) = close_debounce_source.borrow_mut().take() {
                source.remove();
            }
            close_shell.state.lyrics_search_dialog.borrow_mut().take();
        });

        let close_dialog = dialog.clone();
        close_button.connect_clicked(move |_| {
            close_dialog.close();
        });

        let search_shell = Rc::clone(self);
        artist_entry.connect_activate(move |_| submit_lyrics_search(&search_shell));
        let edit_shell = Rc::clone(self);
        artist_entry.connect_changed(move |_| edit_shell.schedule_lyrics_search());

        let search_shell = Rc::clone(self);
        title_entry.connect_activate(move |_| submit_lyrics_search(&search_shell));
        let edit_shell = Rc::clone(self);
        title_entry.connect_changed(move |_| edit_shell.schedule_lyrics_search());

        dialog.present(Some(&self.window));
        search_dialog.title_entry.grab_focus();
        self.schedule_lyrics_search();
    }
    pub(in crate::ui) fn apply_lyrics_search_results(
        self: &Rc<Self>,
        track_id: rufin_core::TrackId,
        artist_name: String,
        track_name: String,
        results: Vec<LyricsSearchResult>,
    ) {
        let Some(dialog) = self.state.lyrics_search_dialog.borrow().clone() else {
            return;
        };
        if dialog.track_id != track_id {
            debug!(
                dialog_track_id = %dialog.track_id,
                response_track_id = %track_id,
                "ignored lyric search response for another track"
            );
            return;
        }
        if !lyrics_search_response_matches_query(
            &artist_name,
            &track_name,
            dialog.artist_entry.text().as_str(),
            dialog.title_entry.text().as_str(),
        ) {
            debug!(
                response_artist_name = %artist_name,
                response_track_name = %track_name,
                current_artist_name = %dialog.artist_entry.text(),
                current_track_name = %dialog.title_entry.text(),
                results = results.len(),
                "ignored stale lyric search response"
            );
            return;
        }
        debug!(
            artist_name = %artist_name,
            track_name = %track_name,
            results = results.len(),
            "applying lyric search response"
        );
        clear_list_box(&dialog.list);
        if results.is_empty() {
            dialog.status.set_text(&tr("No lyrics found."));
            return;
        }

        dialog
            .status
            .set_text(&format!("{} {}", results.len(), tr("results")));
        self.render_lyrics_search_result_rows(&dialog, &track_id, &results);
    }
    fn render_lyrics_search_result_rows(
        self: &Rc<Self>,
        dialog: &LyricsSearchDialog,
        track_id: &rufin_core::TrackId,
        results: &[LyricsSearchResult],
    ) {
        let mut current_provider = None;
        for result in results {
            if current_provider != Some(result.provider) {
                current_provider = Some(result.provider);
                let header = adw::ActionRow::builder()
                    .title(result.provider.title())
                    .activatable(false)
                    .build();
                header.add_css_class("property");
                dialog.list.append(&header);
            }
            let title = lyrics_result_title_markup(result);
            let subtitle = lyrics_result_subtitle_markup(result);
            let has_content = lyrics_search_result_has_content(result);
            let row = adw::ActionRow::builder()
                .title(title.as_str())
                .subtitle(subtitle.as_str())
                .build();
            row.set_activatable(has_content);
            let button = gtk::Button::with_label(&tr("Save"));
            button.set_valign(gtk::Align::Center);
            button.add_css_class("suggested-action");
            button.set_sensitive(has_content);
            row.add_suffix(&button);

            if has_content {
                let preview_shell = Rc::clone(self);
                let preview_track_id = track_id.clone();
                let preview_result = result.clone();
                row.connect_activated(move |_| {
                    preview_shell.controller.preview_lyrics_search_result(
                        preview_track_id.clone(),
                        preview_result.clone(),
                    );
                    if let Some(dialog) = preview_shell.state.lyrics_search_dialog.borrow().as_ref()
                    {
                        dialog.status.set_text(&tr("Loaded in lyrics panel."));
                    }
                });
            }
            let save_shell = Rc::clone(self);
            let save_track_id = track_id.clone();
            let save_result = result.clone();
            button.connect_clicked(move |_| {
                let shell = Rc::clone(&save_shell);
                let track_id = save_track_id.clone();
                let result = save_result.clone();
                gtk::glib::spawn_future_local(async move {
                    let dialog = gtk::FileDialog::builder()
                        .title(tr("Save Lyrics"))
                        .initial_name(lyrics_save_filename(&result.track_name))
                        .build();
                    let Ok(file) = dialog.save_future(Some(&shell.window)).await else {
                        return;
                    };
                    let Some(path) = file.path() else {
                        return;
                    };
                    shell
                        .controller
                        .save_lyrics_search_result(track_id, result, path);
                });
            });
            dialog.list.append(&row);
        }
    }
    pub(in crate::ui) fn apply_lyrics_search_failed(
        self: &Rc<Self>,
        track_id: rufin_core::TrackId,
        artist_name: String,
        track_name: String,
        error: String,
    ) {
        let Some(dialog) = self.state.lyrics_search_dialog.borrow().clone() else {
            return;
        };
        if dialog.track_id != track_id {
            debug!(
                dialog_track_id = %dialog.track_id,
                response_track_id = %track_id,
                "ignored lyric search failure for another track"
            );
            return;
        }
        if !lyrics_search_response_matches_query(
            &artist_name,
            &track_name,
            dialog.artist_entry.text().as_str(),
            dialog.title_entry.text().as_str(),
        ) {
            debug!(
                response_artist_name = %artist_name,
                response_track_name = %track_name,
                current_artist_name = %dialog.artist_entry.text(),
                current_track_name = %dialog.title_entry.text(),
                %error,
                "ignored stale lyric search failure"
            );
            return;
        }
        debug!(
            artist_name = %artist_name,
            track_name = %track_name,
            %error,
            "applying lyric search failure"
        );
        clear_list_box(&dialog.list);
        dialog.status.set_text(&tr("Search failed."));
    }
    fn schedule_lyrics_search(self: &Rc<Self>) {
        let Some(dialog) = self.state.lyrics_search_dialog.borrow().clone() else {
            return;
        };
        if let Some(source) = dialog.search_debounce_source.borrow_mut().take() {
            source.remove();
        }
        if dialog.artist_entry.text().trim().is_empty()
            && dialog.title_entry.text().trim().is_empty()
        {
            self.clear_stale_lyrics_search_results();
            return;
        }
        let search_debounce_source = Rc::clone(&dialog.search_debounce_source);
        let search_shell = Rc::clone(self);
        let source = glib::timeout_add_local_once(
            Duration::from_millis(LYRICS_SEARCH_DEBOUNCE_MILLIS),
            move || {
                *search_debounce_source.borrow_mut() = None;
                submit_lyrics_search(&search_shell);
            },
        );
        *dialog.search_debounce_source.borrow_mut() = Some(source);
    }
    fn clear_stale_lyrics_search_results(self: &Rc<Self>) {
        let Some(dialog) = self.state.lyrics_search_dialog.borrow().clone() else {
            return;
        };
        clear_list_box(&dialog.list);
        dialog.status.set_text(&tr("Ready"));
    }
    pub(in crate::ui) fn apply_lyrics_saved(self: &Rc<Self>, path: PathBuf, lyrics: Lyrics) {
        let track_id = lyrics.track_id.clone();
        self.apply_loaded_lyrics(Some(lyrics));
        if let Some(dialog) = self.state.lyrics_search_dialog.borrow().as_ref()
            && dialog.track_id == track_id
        {
            dialog
                .status
                .set_text(&format!("{} {}", tr("Saved to"), path.display()));
        }
    }
}

pub(in crate::ui) fn lyrics_save_filename(track_title: &str) -> String {
    let stem = track_title
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            other if other.is_control() => '_',
            other => other,
        })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .to_string();
    let stem = if stem.is_empty() { "lyrics" } else { &stem };
    format!("{stem}.lrc")
}

impl Shell {
    fn render_lyrics_panel(self: &Rc<Self>) {
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
        self.lyrics_pane
            .set_search_action(&search_label, search_enabled);
        self.lyrics_pane.set_clear_auto_search_action(
            &tr("Disable automatic lyric search for this track"),
            clear_auto_search_enabled,
        );
        let empty_status = self.lyrics_empty_status();
        let seek_shell = Rc::clone(self);
        let seek: Rc<dyn Fn(u64)> = Rc::new(move |position_millis| {
            seek_shell.seek_to_lyrics_position(position_millis);
        });
        self.lyrics_pane
            .set_content(lyrics.as_ref(), empty_status, seek);
        drop(lyrics);
        self.update_lyrics_highlight();
        self.request_auto_lyrics_if_needed();
    }
    fn present_lyrics_search_dialog(self: &Rc<Self>) {
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

        let search_button = text_button("system-search-symbolic", "Search");
        search_button.set_halign(gtk::Align::End);
        content.append(&search_button);

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
            search_button: search_button.clone(),
            list,
            status,
        };
        *self.state.lyrics_search_dialog.borrow_mut() = Some(search_dialog.clone());

        let close_shell = Rc::clone(self);
        dialog.connect_closed(move |_| {
            close_shell.state.lyrics_search_dialog.borrow_mut().take();
        });

        let close_dialog = dialog.clone();
        close_button.connect_clicked(move |_| {
            close_dialog.close();
        });

        let search_shell = Rc::clone(self);
        search_button.connect_clicked(move |_| submit_lyrics_search(&search_shell));

        let search_shell = Rc::clone(self);
        artist_entry.connect_activate(move |_| submit_lyrics_search(&search_shell));

        let search_shell = Rc::clone(self);
        title_entry.connect_activate(move |_| submit_lyrics_search(&search_shell));

        dialog.present(Some(&self.window));
        search_dialog.title_entry.grab_focus();
        submit_lyrics_search(self);
    }
    fn apply_lyrics_search_results(
        self: &Rc<Self>,
        track_id: rufin_core::TrackId,
        _artist_name: String,
        _track_name: String,
        results: Vec<LyricsSearchResult>,
    ) {
        let Some(dialog) = self.state.lyrics_search_dialog.borrow().clone() else {
            return;
        };
        if dialog.track_id != track_id {
            return;
        }
        dialog.search_button.set_sensitive(true);
        clear_list_box(&dialog.list);
        if results.is_empty() {
            dialog.status.set_text(&tr("No lyrics found."));
            return;
        }

        dialog
            .status
            .set_text(&format!("{} {}", results.len(), tr("results")));
        for result in results {
            let title = format!("{} - {}", result.artist_name, result.track_name);
            let subtitle = lyrics_result_subtitle(&result);
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(subtitle)
                .build();
            let button = gtk::Button::with_label(&tr("Save"));
            button.set_valign(gtk::Align::Center);
            button.add_css_class("suggested-action");
            button.set_sensitive(lyrics_search_result_has_content(&result));
            row.add_suffix(&button);
            row.set_activatable_widget(Some(&button));

            let save_shell = Rc::clone(self);
            let save_track_id = track_id.clone();
            button.connect_clicked(move |_| {
                if save_shell.state.settings.borrow().ask_lyrics_save_path {
                    let shell = Rc::clone(&save_shell);
                    let track_id = save_track_id.clone();
                    let result = result.clone();
                    gtk::glib::spawn_future_local(async move {
                        let dialog = gtk::FileDialog::builder().title(tr("Save Lyrics")).build();
                        let Ok(file) = dialog.save_future(Some(&shell.window)).await else {
                            return;
                        };
                        let Some(path) = file.path() else {
                            return;
                        };
                        shell
                            .controller
                            .save_lyrics_search_result(track_id, result, Some(path));
                    });
                } else {
                    save_shell.controller.save_lyrics_search_result(
                        save_track_id.clone(),
                        result.clone(),
                        None,
                    );
                }
            });
            dialog.list.append(&row);
        }
    }
    fn apply_lyrics_saved(self: &Rc<Self>, path: PathBuf, lyrics: Lyrics) {
        let track_id = lyrics.track_id.clone();
        *self.state.lyrics.borrow_mut() = Some(lyrics);
        self.render_lyrics_panel();
        if let Some(dialog) = self.state.lyrics_search_dialog.borrow().as_ref()
            && dialog.track_id == track_id
        {
            dialog
                .status
                .set_text(&format!("{} {}", tr("Saved to"), path.display()));
        }
    }
}

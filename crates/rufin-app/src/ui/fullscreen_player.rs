use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use rufin_core::QueueEntry;

use crate::i18n::tr;
use crate::lyrics::LyricsPane;

use super::{ArtworkTile, DETAIL_COVER_SIZE, Shell, icon_button, player::BOTTOM_PLAYER_HEIGHT};

const MAIN_VIEW_NAME: &str = "main";
const FULLSCREEN_PLAYER_VIEW_NAME: &str = "fullscreen-player";
const FULLSCREEN_PLAYER_TRANSITION_MS: u32 = 240;
const FULLSCREEN_PLAYER_DEFERRED_UPDATE_MS: u64 = 16;
const FULLSCREEN_PLAYER_DEFERRED_COVER_MS: u64 = 80;
const FULLSCREEN_PLAYER_DEFERRED_RENDER_MS: u64 = 240;
const FULLSCREEN_PLAYER_DEFAULT_COVER_SIZE: i32 = 320;
const FULLSCREEN_PLAYER_MIN_COVER_SIZE: i32 = 140;
const FULLSCREEN_PLAYER_MAX_COVER_SIZE: i32 = 320;
const FULLSCREEN_PLAYER_HORIZONTAL_MARGIN: i32 = 64;
const FULLSCREEN_PLAYER_VERTICAL_RESERVED: i32 = 360;

pub(super) struct FullscreenPlayerParts {
    pub(super) root: gtk::Box,
    pub(super) close_button: gtk::Button,
    pub(super) cover: ArtworkTile,
    pub(super) cover_key: RefCell<Option<String>>,
    pub(super) title: gtk::Label,
    pub(super) artist: gtk::Label,
    pub(super) album: gtk::Label,
    pub(super) meta: gtk::Label,
    pub(super) stack: adw::ViewStack,
    pub(super) lyrics_pane: LyricsPane,
    pub(super) queue_panel: gtk::Box,
}

pub(super) fn build_fullscreen_player() -> FullscreenPlayerParts {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("fullscreen-player");
    root.set_hexpand(true);
    root.set_vexpand(true);

    let top_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    top_bar.add_css_class("fullscreen-player-top-bar");
    top_bar.set_valign(gtk::Align::Center);

    let close_button = icon_button("go-down-symbolic", "Close fullscreen player");
    close_button.add_css_class("fullscreen-player-close-button");
    top_bar.append(&close_button);
    root.append(&top_bar);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.add_css_class("fullscreen-player-body");
    body.set_hexpand(true);
    body.set_vexpand(true);

    let hero = gtk::Box::new(gtk::Orientation::Vertical, 8);
    hero.add_css_class("fullscreen-player-hero");
    hero.set_halign(gtk::Align::Center);
    hero.set_hexpand(true);

    let cover = ArtworkTile::new(FULLSCREEN_PLAYER_DEFAULT_COVER_SIZE, 42);
    cover.area.add_css_class("fullscreen-player-cover");
    cover.area.set_halign(gtk::Align::Center);
    hero.append(&cover.area);

    let title = fullscreen_player_label("fullscreen-player-title");
    let artist = fullscreen_player_label("fullscreen-player-artist");
    let album = fullscreen_player_label("fullscreen-player-album");
    let meta = fullscreen_player_label("fullscreen-player-meta");
    meta.add_css_class("muted");
    hero.append(&title);
    hero.append(&artist);
    hero.append(&album);
    hero.append(&meta);
    body.append(&hero);

    let stack = adw::ViewStack::builder()
        .hhomogeneous(false)
        .vhomogeneous(false)
        .hexpand(true)
        .vexpand(true)
        .build();
    let lyrics_pane = LyricsPane::new(&tr("Lyrics"));
    lyrics_pane.set_title("");
    lyrics_pane.widget().add_css_class("fullscreen-player-pane");
    stack.add_titled_with_icon(
        lyrics_pane.widget(),
        Some("lyrics"),
        &tr("Lyrics"),
        "insert-text-symbolic",
    );

    let queue_panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
    queue_panel.add_css_class("fullscreen-player-pane");
    queue_panel.add_css_class("fullscreen-player-queue-panel");
    queue_panel.set_hexpand(true);
    queue_panel.set_vexpand(true);
    stack.add_titled_with_icon(
        &queue_panel,
        Some("queue"),
        &tr("Queue"),
        "view-list-ordered-symbolic",
    );

    let switcher = adw::ViewSwitcher::builder()
        .policy(adw::ViewSwitcherPolicy::Wide)
        .stack(&stack)
        .build();
    let switcher_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    switcher_bar.add_css_class("fullscreen-player-tab-bar");
    switcher_bar.set_halign(gtk::Align::Center);
    switcher_bar.append(&switcher);
    body.append(&switcher_bar);
    body.append(&stack);
    root.append(&body);

    FullscreenPlayerParts {
        root,
        close_button,
        cover,
        cover_key: RefCell::new(None),
        title,
        artist,
        album,
        meta,
        stack,
        lyrics_pane,
        queue_panel,
    }
}

pub(super) fn connect_fullscreen_player_controls(shell: &Rc<Shell>) {
    let close_shell = Rc::clone(shell);
    shell
        .fullscreen_player
        .close_button
        .connect_clicked(move |_| close_shell.close_fullscreen_player());

    let key_shell = Rc::clone(shell);
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape && key_shell.state.fullscreen_player_visible.get() {
            key_shell.close_fullscreen_player();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    shell.window.add_controller(key);

    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("width"), move |_, _| {
            if resize_shell.state.fullscreen_player_visible.get() {
                resize_shell.update_fullscreen_player();
            }
        });
    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("height"), move |_, _| {
            if resize_shell.state.fullscreen_player_visible.get() {
                resize_shell.update_fullscreen_player();
            }
        });
}

impl Shell {
    pub(super) fn open_fullscreen_player(self: &Rc<Self>) {
        if self.state.player.borrow().current.is_none() {
            return;
        }
        self.state.fullscreen_player_visible.set(true);
        self.app_content_stack
            .set_transition_duration(FULLSCREEN_PLAYER_TRANSITION_MS);
        self.app_content_stack.set_visible_child_full(
            FULLSCREEN_PLAYER_VIEW_NAME,
            gtk::StackTransitionType::OverUp,
        );
        reset_fullscreen_stack(self);
        let update_shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(FULLSCREEN_PLAYER_DEFERRED_UPDATE_MS),
            move || {
                if update_shell.state.fullscreen_player_visible.get() {
                    let player = update_shell.state.player.borrow().clone();
                    update_shell.update_fullscreen_player_text(&player);
                }
            },
        );
        let cover_shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(FULLSCREEN_PLAYER_DEFERRED_COVER_MS),
            move || {
                if cover_shell.state.fullscreen_player_visible.get() {
                    let player = cover_shell.state.player.borrow().clone();
                    cover_shell.update_fullscreen_player_cover(&player);
                }
            },
        );
        let render_shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(FULLSCREEN_PLAYER_DEFERRED_RENDER_MS),
            move || {
                if !render_shell.state.fullscreen_player_visible.get() {
                    return;
                }
                render_shell.render_queue_panel();
                render_shell.render_lyrics_panel();
            },
        );
        let _focused = self.fullscreen_player.close_button.grab_focus();
    }

    pub(super) fn close_fullscreen_player(self: &Rc<Self>) {
        if !self.state.fullscreen_player_visible.replace(false) {
            return;
        }
        self.app_content_stack
            .set_transition_duration(FULLSCREEN_PLAYER_TRANSITION_MS);
        self.app_content_stack
            .set_visible_child_full(MAIN_VIEW_NAME, gtk::StackTransitionType::UnderDown);
        reset_fullscreen_stack(self);
    }

    pub(super) fn toggle_fullscreen_player(self: &Rc<Self>) {
        if self.state.fullscreen_player_visible.get() {
            self.close_fullscreen_player();
        } else {
            self.open_fullscreen_player();
        }
    }

    pub(super) fn update_fullscreen_player(self: &Rc<Self>) {
        if !self.state.fullscreen_player_visible.get() {
            return;
        }
        let player = self.state.player.borrow().clone();
        self.update_fullscreen_player_text(&player);
        self.update_fullscreen_player_cover(&player);
    }

    fn update_fullscreen_player_cover(
        self: &Rc<Self>,
        player: &crate::controller::PlaybackSnapshot,
    ) {
        let cover_size = self.update_fullscreen_player_cover_size();
        let cover_seed = player
            .current
            .as_ref()
            .map(|entry| entry.duration_seconds)
            .unwrap_or(42);
        self.fullscreen_player.cover.set_seed(cover_seed);

        if let Some(image_ref) = player
            .current
            .as_ref()
            .and_then(|entry| entry.image_ref.as_ref())
        {
            if let Some(key) = self.cover_cache_key(image_ref, DETAIL_COVER_SIZE) {
                let request_key = format!("{key}:{cover_size}");
                let cover_key_changed = self.fullscreen_player.cover_key.borrow().as_deref()
                    != Some(request_key.as_str());
                if cover_key_changed {
                    let has_decoded_cover = self.decoded_cover_has_min_size(&key, cover_size);
                    let has_cached_cover_file = self
                        .controller
                        .cached_cover_path(image_ref, DETAIL_COVER_SIZE)
                        .is_some();
                    if has_decoded_cover || has_cached_cover_file {
                        self.fullscreen_player.cover.advance_generation();
                    } else {
                        self.fullscreen_player.cover.clear_image();
                    }
                    self.request_cover_for_tile(
                        &self.fullscreen_player.cover,
                        key,
                        image_ref.clone(),
                        cover_size,
                        DETAIL_COVER_SIZE,
                    );
                    *self.fullscreen_player.cover_key.borrow_mut() = Some(request_key);
                }
            } else {
                self.clear_fullscreen_player_cover();
            }
        } else {
            self.clear_fullscreen_player_cover();
        }
    }

    fn update_fullscreen_player_text(&self, player: &crate::controller::PlaybackSnapshot) {
        let title = player
            .current
            .as_ref()
            .map(|entry| entry.title.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| tr("Nothing playing"));
        let artist = player
            .current
            .as_ref()
            .map(|entry| entry.artist.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| tr("Queue a track to begin"));
        let album = player
            .current
            .as_ref()
            .map(|entry| entry.album.as_str())
            .unwrap_or("");
        self.fullscreen_player.title.set_text(&title);
        self.fullscreen_player.artist.set_text(&artist);
        self.fullscreen_player.album.set_text(album);
        self.fullscreen_player
            .title
            .set_sensitive(player.current.is_some());
        self.fullscreen_player.artist.set_sensitive(
            player
                .current
                .as_ref()
                .is_some_and(|entry| !entry.artist.is_empty()),
        );
        self.fullscreen_player.album.set_sensitive(
            player
                .current
                .as_ref()
                .is_some_and(|entry| !entry.album.is_empty()),
        );
        self.fullscreen_player
            .meta
            .set_text(&self.fullscreen_player_meta_text(player));
        self.fullscreen_player
            .meta
            .set_visible(player.current.is_some());
    }

    fn fullscreen_player_meta_text(&self, player: &crate::controller::PlaybackSnapshot) -> String {
        let source_label = player
            .current
            .as_ref()
            .and_then(|entry| self.current_track_source_label(entry));
        fullscreen_player_meta_text(player.current.as_ref(), source_label.as_deref())
    }

    fn current_track_source_label(&self, entry: &QueueEntry) -> Option<String> {
        if let Some(source) = entry
            .source_format
            .as_deref()
            .and_then(audio_source_label_from_format)
        {
            return Some(source);
        }
        if let Some(source) = entry
            .local_path
            .as_deref()
            .and_then(audio_source_label_from_path)
        {
            return Some(source);
        }
        if let Some(source) = self
            .controller
            .cached_track_source_format(&entry.track_id)
            .as_deref()
            .and_then(audio_source_label_from_format)
        {
            return Some(source);
        }
        if let Some(source) = self
            .controller
            .cached_track_local_path(&entry.track_id)
            .as_deref()
            .and_then(audio_source_label_from_path)
        {
            return Some(source);
        }

        let library = self.state.library.borrow();
        library
            .tracks
            .iter()
            .chain(library.favorites.iter())
            .chain(library.search.tracks.iter())
            .chain(
                library
                    .home_sections
                    .iter()
                    .flat_map(|section| section.tracks.iter()),
            )
            .find(|track| track.id == entry.track_id)
            .and_then(|track| {
                track
                    .source_format
                    .as_deref()
                    .and_then(audio_source_label_from_format)
                    .or_else(|| {
                        track
                            .local_path
                            .as_deref()
                            .and_then(audio_source_label_from_path)
                    })
            })
    }

    fn update_fullscreen_player_cover_size(&self) -> i32 {
        let width = self
            .fullscreen_player
            .root
            .width()
            .max(self.window.width())
            .max(1);
        let fallback_height = (self.window.height() - BOTTOM_PLAYER_HEIGHT).max(1);
        let height = self
            .fullscreen_player
            .root
            .height()
            .max(fallback_height)
            .max(1);
        let size = fullscreen_artwork_size_for(width, height);
        self.fullscreen_player.cover.set_square_size(size);
        size
    }

    fn clear_fullscreen_player_cover(&self) {
        self.fullscreen_player.cover.clear_image();
        *self.fullscreen_player.cover_key.borrow_mut() = None;
    }
}

fn reset_fullscreen_stack(shell: &Rc<Shell>) {
    let reset_shell = Rc::clone(shell);
    glib::timeout_add_local(
        Duration::from_millis(u64::from(FULLSCREEN_PLAYER_TRANSITION_MS) + 16),
        move || {
            if reset_shell.app_content_stack.is_transition_running() {
                return glib::ControlFlow::Continue;
            }
            reset_shell
                .app_content_stack
                .set_transition_type(gtk::StackTransitionType::None);
            reset_shell.app_content_stack.set_transition_duration(0);
            glib::ControlFlow::Break
        },
    );
}

fn fullscreen_player_label(css_class: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class(css_class);
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(1);
    label.set_max_width_chars(48);
    label.set_halign(gtk::Align::Center);
    label
}

fn fullscreen_player_meta_text(entry: Option<&QueueEntry>, source_label: Option<&str>) -> String {
    let Some(entry) = entry else {
        return String::new();
    };
    fullscreen_player_meta_parts(entry.year, source_label)
}

fn fullscreen_player_meta_parts(year: u16, source_label: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(source) = source_label
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        parts.push(source.to_string());
    }
    if year > 0 {
        parts.push(year.to_string());
    }
    parts.join(" - ")
}

fn audio_source_label_from_path(path: &str) -> Option<String> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let extension = Path::new(path).extension()?.to_str()?.trim();
    audio_source_label_from_format(extension)
}

fn audio_source_label_from_format(value: &str) -> Option<String> {
    let value = value
        .rsplit('/')
        .next()
        .unwrap_or(value)
        .trim()
        .trim_start_matches('.');
    if value.is_empty() {
        return None;
    }
    let normalized = match value.to_ascii_lowercase().as_str() {
        "mpeg" | "mpga" => "MP3".to_string(),
        other => other.to_ascii_uppercase(),
    };
    Some(normalized)
}

pub(super) fn fullscreen_artwork_size_for(width: i32, height: i32) -> i32 {
    let width_limit = (width - FULLSCREEN_PLAYER_HORIZONTAL_MARGIN).max(1);
    let height_limit = (height - FULLSCREEN_PLAYER_VERTICAL_RESERVED).max(1);
    width_limit.min(height_limit).clamp(
        FULLSCREEN_PLAYER_MIN_COVER_SIZE,
        FULLSCREEN_PLAYER_MAX_COVER_SIZE,
    )
}

#[cfg(test)]
mod tests {
    use super::fullscreen_artwork_size_for;

    #[test]
    fn fullscreen_stay_windows() {
        assert_eq!(fullscreen_artwork_size_for(480, 360), 140);
    }

    #[test]
    fn fullscreen_cap_windows() {
        assert_eq!(fullscreen_artwork_size_for(1440, 900), 320);
    }

    #[test]
    fn fullscreen_use_width() {
        assert_eq!(fullscreen_artwork_size_for(900, 560), 200);
    }

    #[test]
    fn fullscreen_use_duration() {
        assert_eq!(
            super::fullscreen_player_meta_parts(2013, Some("FLAC")),
            "FLAC - 2013"
        );
    }

    #[test]
    fn fullscreen_use_extension() {
        assert_eq!(
            super::audio_source_label_from_path("/music/album/track.mpc").as_deref(),
            Some("MPC")
        );
    }

    #[test]
    fn fullscreen_ignore_query() {
        assert_eq!(
            super::audio_source_label_from_path("/music/album/track.flac?token=redacted")
                .as_deref(),
            Some("FLAC")
        );
    }

    #[test]
    fn fullscreen_normalize_type() {
        assert_eq!(
            super::audio_source_label_from_format("audio/mpeg").as_deref(),
            Some("MP3")
        );
    }
}

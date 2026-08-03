use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::{gio, glib};
use library::{Album, Artist, LoadedLibrary, SearchResults, Track};
use localization::{msgid, tr};
use playback::QueuePlacement;

use crate::format_duration;
use crate::localization::{bind_search_placeholder, localized_label};
use crate::runtime::SelectedLibrary;
use crate::runtime::source::SearchRequest;
use crate::shell::Shell;
use crate::shell::cover::THUMB_COVER_SIZE;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::route::MountedRoute;

use super::collections::{configure_library_route_scroller, library_route_inset};
use super::route::Route;
use super::route_layout::{ROUTE_TOP_MARGIN, route_scroller_widget};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const SEARCH_ROW_COVER_SIZE: i32 = 48;
const SEARCH_ROW_SPACING: i32 = 12;

#[derive(Clone)]
enum SearchRow {
    Header(&'static str),
    Artist { artist: Artist, navigable: bool },
    Album { album: Album, navigable: bool },
    Track(Track),
}

impl SearchRow {
    fn activatable(&self) -> bool {
        match self {
            Self::Header(_) => false,
            Self::Artist { navigable, .. } | Self::Album { navigable, .. } => *navigable,
            Self::Track(_) => true,
        }
    }
}

struct SearchRouteProjection {
    root: gtk::Widget,
    shell: Weak<Shell>,
    source_id: library::SourceId,
    source_session_epoch: playback::SourceSessionEpoch,
    loaded: Arc<LoadedLibrary>,
    search: gtk::SearchEntry,
    status: gtk::Stack,
    error: gtk::Label,
    model: gio::ListStore,
    generation: Cell<u64>,
    debounce: RefCell<Option<glib::SourceId>>,
}

impl SearchRouteProjection {
    fn new(shell: &Rc<Shell>, selected: &SelectedLibrary) -> Rc<Self> {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.add_css_class("search-route");
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_margin_bottom(8);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);

        let search = gtk::SearchEntry::new();
        search.set_hexpand(true);
        bind_search_placeholder(&search, "Search");
        wrapper.append(&library_route_inset(search.clone().upcast()));
        shell.set_route_search(Some(search.clone()));

        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let list = search_result_list(shell, &model);
        let scroller = gtk::ScrolledWindow::new();
        configure_library_route_scroller(&scroller);
        scroller.set_child(Some(&library_route_inset(list.upcast())));

        let status = gtk::Stack::new();
        status.set_hexpand(true);
        status.set_vexpand(true);
        status.add_named(
            &shell.route_empty_view(msgid("Type to search")),
            Some("initial"),
        );
        status.add_named(&search_loading(), Some("loading"));
        status.add_named(
            &shell.route_empty_view(msgid(r"No results ¯\_(°╭╮°)_/¯")),
            Some("empty"),
        );
        let error = gtk::Label::new(None);
        error.add_css_class("muted");
        error.set_justify(gtk::Justification::Center);
        error.set_wrap(true);
        error.set_max_width_chars(48);
        let error_view = centered_widget(error.clone().upcast());
        status.add_named(&error_view, Some("error"));
        status.add_named(&route_scroller_widget(scroller), Some("results"));
        status.set_visible_child_name("initial");
        wrapper.append(&status);

        let projection = Rc::new(Self {
            root: wrapper.upcast(),
            shell: Rc::downgrade(shell),
            source_id: selected.source_id.clone(),
            source_session_epoch: selected.source_session_epoch,
            loaded: Arc::clone(&selected.loaded),
            search,
            status,
            error,
            model,
            generation: Cell::new(0),
            debounce: RefCell::new(None),
        });
        projection.connect_search();
        projection
    }

    fn connect_search(self: &Rc<Self>) {
        let projection = Rc::downgrade(self);
        self.search.connect_search_changed(move |entry| {
            let Some(projection) = projection.upgrade() else {
                return;
            };
            if let Some(pending) = projection.debounce.borrow_mut().take() {
                pending.remove();
            }
            let query = entry.text().trim().to_string();
            if query.is_empty() {
                projection.reset();
                return;
            }
            let delayed = Rc::downgrade(&projection);
            let source = glib::timeout_add_local_once(SEARCH_DEBOUNCE, move || {
                let Some(projection) = delayed.upgrade() else {
                    return;
                };
                projection.debounce.borrow_mut().take();
                projection.submit(query);
            });
            projection.debounce.replace(Some(source));
        });
    }

    fn reset(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.model.remove_all();
        self.status.set_visible_child_name("initial");
    }

    fn submit(self: &Rc<Self>, query: String) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.model.remove_all();
        self.status.set_visible_child_name("loading");

        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let receiver = shell.products.source.search(SearchRequest {
            source_id: self.source_id.clone(),
            source_session_epoch: self.source_session_epoch,
            search: library::SearchRequest::new(query),
        });
        let projection = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("the Search request stopped".to_string()));
            let Some(projection) = projection.upgrade() else {
                return;
            };
            if projection.generation.get() != generation || !projection.is_active() {
                return;
            }
            match result {
                Ok(results) => projection.apply(results),
                Err(error) => projection.show_error(&error),
            }
        });
    }

    fn is_active(&self) -> bool {
        let Some(shell) = self.shell.upgrade() else {
            return false;
        };
        if shell.navigation.routes.borrow().current() != &Route::Search {
            return false;
        }
        shell
            .library
            .selected
            .borrow()
            .as_ref()
            .is_some_and(|selected| {
                selected.source_id == self.source_id
                    && selected.source_session_epoch == self.source_session_epoch
            })
    }

    fn apply(&self, results: SearchResults) {
        if results.is_empty() {
            self.model.remove_all();
            self.status.set_visible_child_name("empty");
            return;
        }
        let mut rows = Vec::new();
        if !results.tracks.is_empty() {
            rows.push(SearchRow::Header(msgid("Tracks")));
            rows.extend(results.tracks.into_iter().map(SearchRow::Track));
        }
        if !results.albums.is_empty() {
            rows.push(SearchRow::Header(msgid("Albums")));
            rows.extend(results.albums.into_iter().map(|album| {
                let navigable = self
                    .loaded
                    .album(&album.id)
                    .is_ok_and(|album| album.is_some());
                SearchRow::Album { album, navigable }
            }));
        }
        if !results.artists.is_empty() {
            rows.push(SearchRow::Header(msgid("Artists")));
            rows.extend(results.artists.into_iter().map(|artist| {
                let navigable = self
                    .loaded
                    .artist(&artist.id)
                    .is_ok_and(|artist| artist.is_some());
                SearchRow::Artist { artist, navigable }
            }));
        }
        let additions = rows
            .into_iter()
            .map(glib::BoxedAnyObject::new)
            .collect::<Vec<_>>();
        self.model.splice(0, self.model.n_items(), &additions);
        self.status.set_visible_child_name("results");
    }

    fn show_error(&self, message: &str) {
        self.model.remove_all();
        self.error.set_label(message);
        self.status.set_visible_child_name("error");
    }

    fn resume(&self) {
        if let Some(shell) = self.shell.upgrade() {
            shell.set_route_search(Some(self.search.clone()));
        }
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }
}

impl Drop for SearchRouteProjection {
    fn drop(&mut self) {
        if let Some(pending) = self.debounce.borrow_mut().take() {
            pending.remove();
        }
    }
}

impl Shell {
    pub(crate) fn search_route(self: &Rc<Self>, selected: &SelectedLibrary) -> MountedRoute {
        let projection = SearchRouteProjection::new(self, selected);
        let resume_projection = Rc::clone(&projection);
        MountedRoute::new(
            projection.widget(),
            Rc::new(move || resume_projection.resume()),
        )
    }
}

fn search_result_list(shell: &Rc<Shell>, model: &gio::ListStore) -> gtk::ListView {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 0);
        cell.set_hexpand(true);
        item.set_child(Some(&cell));
    });
    let bind_shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = search_row(item.item()) else {
            return;
        };
        let Some(cell) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        else {
            return;
        };
        clear_box(&cell);
        item.set_selectable(row.activatable());
        item.set_activatable(row.activatable());
        match &row {
            SearchRow::Header(title) => cell.append(&search_header(title)),
            SearchRow::Artist { artist, .. } => {
                cell.append(&result_row(
                    &bind_shell,
                    ArtworkBinding::artist(artist, &[]),
                    stable_seed(artist.id.as_str()),
                    &artist.name,
                    tr("Artist"),
                    None,
                ));
            }
            SearchRow::Album { album, .. } => {
                cell.append(&result_row(
                    &bind_shell,
                    ArtworkBinding::album(album),
                    stable_seed(album.id.as_str()),
                    &album.title,
                    album.artist.clone(),
                    None,
                ));
            }
            SearchRow::Track(track) => {
                let subtitle = [track.artist.as_str(), track.album.as_str()]
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                cell.append(&result_row(
                    &bind_shell,
                    ArtworkBinding::track(track),
                    stable_seed(track.id.as_str()),
                    &track.title,
                    subtitle,
                    Some(format_duration(track.duration_seconds)),
                ));
            }
        }
    });
    factory.connect_unbind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        item.set_selectable(false);
        item.set_activatable(false);
        if let Some(cell) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        {
            clear_box(&cell);
        }
    });

    let selection = gtk::SingleSelection::new(Some(model.clone()));
    selection.set_autoselect(false);
    selection.set_can_unselect(true);
    let list = gtk::ListView::new(Some(selection), Some(factory));
    list.add_css_class("search-results");
    list.set_single_click_activate(true);
    list.set_hexpand(true);
    list.set_vexpand(true);

    let activate_model = model.clone();
    let activate_shell = Rc::clone(shell);
    list.connect_activate(move |_, position| {
        let Some(row) = activate_model
            .item(position)
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        match &*row.borrow::<SearchRow>() {
            SearchRow::Artist {
                artist,
                navigable: true,
            } => activate_shell.navigate(Route::ArtistDetail(artist.id.clone())),
            SearchRow::Album {
                album,
                navigable: true,
            } => activate_shell.navigate(Route::AlbumDetail(album.id.clone())),
            SearchRow::Track(track) => {
                let Some(selected) = activate_shell.library.selected.borrow().as_ref().cloned()
                else {
                    return;
                };
                activate_shell
                    .products
                    .playback
                    .queue
                    .play_loaded(selected.one_track(track.clone(), QueuePlacement::Now));
            }
            _ => {}
        }
    });
    list
}

fn search_row(item: Option<glib::Object>) -> Option<SearchRow> {
    item?
        .downcast::<glib::BoxedAnyObject>()
        .ok()
        .map(|row| row.borrow::<SearchRow>().clone())
}

fn search_header(title: &'static str) -> gtk::Widget {
    let label = localized_label(title);
    label.add_css_class("section-heading");
    label.set_xalign(0.0);
    label.set_margin_top(14);
    label.set_margin_bottom(6);
    label.set_margin_start(6);
    label.upcast()
}

fn result_row(
    shell: &Rc<Shell>,
    artwork: ArtworkBinding,
    seed: u32,
    title: &str,
    subtitle: String,
    trailing: Option<String>,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, SEARCH_ROW_SPACING);
    row.set_margin_top(5);
    row.set_margin_bottom(5);
    row.set_margin_start(6);
    row.set_margin_end(6);
    row.set_hexpand(true);
    row.append(&shell.cover_tile_for_candidates(
        artwork,
        seed,
        SEARCH_ROW_COVER_SIZE,
        THUMB_COVER_SIZE,
    ));

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    labels.set_valign(gtk::Align::Center);
    let title_label = gtk::Label::new(Some(title));
    title_label.set_xalign(0.0);
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let subtitle_label = gtk::Label::new(Some(&subtitle));
    subtitle_label.add_css_class("muted");
    subtitle_label.set_xalign(0.0);
    subtitle_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    labels.append(&title_label);
    labels.append(&subtitle_label);
    row.append(&labels);

    if let Some(trailing) = trailing {
        let label = gtk::Label::new(Some(&trailing));
        label.add_css_class("muted");
        label.set_valign(gtk::Align::Center);
        row.append(&label);
        let play = gtk::Image::from_icon_name("rufin-play-symbolic");
        play.set_valign(gtk::Align::Center);
        row.append(&play);
    }
    row.upcast()
}

fn search_loading() -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let spinner = gtk::Spinner::new();
    spinner.set_spinning(true);
    content.append(&spinner);
    content.append(&localized_label(msgid("Searching...")));
    centered_widget(content.upcast())
}

fn centered_widget(widget: gtk::Widget) -> gtk::Widget {
    let center = gtk::CenterBox::new();
    center.set_hexpand(true);
    center.set_vexpand(true);
    center.set_center_widget(Some(&widget));
    center.upcast()
}

fn clear_box(cell: &gtk::Box) {
    while let Some(child) = cell.first_child() {
        cell.remove(&child);
    }
}

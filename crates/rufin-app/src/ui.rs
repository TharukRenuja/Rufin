use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use rufin_core::{
    Album, AlbumId, Artist, DensityMode, EffectiveDensity, Genre, HomeSection, Playlist, Route,
    RouteStack, SearchKind, Track, format_duration,
};
use rufin_test_support::FakeScale;
use tracing::{debug, info, warn};

use crate::controller::{AppController, ControllerEvent, LibrarySnapshot};
use crate::i18n::tr;

#[derive(Clone, Debug)]
pub struct AppOptions {
    pub fake_scale: Option<FakeScale>,
    pub smoke_exit_ms: Option<u64>,
}

struct AppState {
    routes: RefCell<RouteStack>,
    density_mode: Cell<DensityMode>,
    effective_density: Cell<EffectiveDensity>,
    library: RefCell<LibrarySnapshot>,
}

struct Shell {
    state: AppState,
    controller: AppController,
    window: adw::ApplicationWindow,
    normal_nav: gtk::Box,
    compact_nav: gtk::Box,
    route_title: gtk::Label,
    route_host: gtk::Box,
    back_button: gtk::Button,
    forward_button: gtk::Button,
    right_panel: gtk::Box,
}

pub fn build(app: &adw::Application, options: AppOptions) {
    install_css();

    let loaded_at = std::time::Instant::now();
    let (controller, events, library) = AppController::bootstrap(options.fake_scale);
    info!(
        albums = library.albums.len(),
        tracks = library.tracks.len(),
        first_run = library.first_run,
        elapsed_ms = loaded_at.elapsed().as_millis(),
        "loaded cached music library snapshot"
    );

    let state = AppState {
        routes: RefCell::new(RouteStack::new(Route::Home)),
        density_mode: Cell::new(DensityMode::Auto),
        effective_density: Cell::new(DensityMode::Auto.resolve(1_400)),
        library: RefCell::new(library),
    };

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Rufin")
        .default_width(1_400)
        .default_height(860)
        .width_request(900)
        .height_request(700)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("app-root");

    let upper = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    upper.set_vexpand(true);

    let normal_nav = gtk::Box::new(gtk::Orientation::Vertical, 10);
    normal_nav.add_css_class("wide-sidebar");
    normal_nav.set_width_request(220);

    let compact_nav = gtk::Box::new(gtk::Orientation::Vertical, 8);
    compact_nav.add_css_class("compact-rail");
    compact_nav.set_width_request(76);

    let main_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main_area.add_css_class("main-area");
    main_area.set_hexpand(true);
    main_area.set_vexpand(true);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("route-header");

    let back_button = icon_button("go-previous-symbolic", "Back");
    let forward_button = icon_button("go-next-symbolic", "Forward");
    let route_title = gtk::Label::new(None);
    route_title.add_css_class("route-title");
    route_title.set_xalign(0.0);
    route_title.set_hexpand(true);
    let settings_button = icon_button("emblem-system-symbolic", "Settings");

    header.append(&back_button);
    header.append(&forward_button);
    header.append(&route_title);
    header.append(&settings_button);

    let route_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    route_host.set_hexpand(true);
    route_host.set_vexpand(true);

    main_area.append(&header);
    main_area.append(&route_host);

    let right_panel = build_right_panel();
    let bottom_player = build_bottom_player();

    upper.append(&normal_nav);
    upper.append(&compact_nav);
    upper.append(&main_area);
    upper.append(&right_panel);
    root.append(&upper);
    root.append(&bottom_player);

    window.set_content(Some(&root));

    let shell = Rc::new(Shell {
        state,
        controller,
        window,
        normal_nav,
        compact_nav,
        route_title,
        route_host,
        back_button,
        forward_button,
        right_panel,
    });

    build_normal_navigation(&shell);
    build_compact_navigation(&shell);
    connect_shell_actions(&shell, settings_button);
    shell.update_density();
    shell.render_current_route();
    install_event_pump(&shell, events);

    if options.fake_scale.is_none() {
        shell.controller.start_background_sync_for_active();
    }

    if let Some(delay_ms) = options.smoke_exit_ms {
        let app = app.clone();
        glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
            info!(delay_ms, "smoke exit requested");
            app.quit();
        });
    }

    shell.window.present();
}

impl Shell {
    fn navigate(self: &Rc<Self>, route: Route) {
        debug!(?route, "navigate");
        self.state.routes.borrow_mut().navigate(route);
        self.render_current_route();
    }

    fn go_back(self: &Rc<Self>) {
        let route = self.state.routes.borrow_mut().back().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate back");
            self.render_current_route();
        }
    }

    fn go_forward(self: &Rc<Self>) {
        let route = self.state.routes.borrow_mut().forward().cloned();
        if let Some(route) = route {
            debug!(?route, "navigate forward");
            self.render_current_route();
        }
    }

    fn set_density_mode(self: &Rc<Self>, density_mode: DensityMode) {
        self.state.density_mode.set(density_mode);
        self.update_density();
    }

    fn update_density(self: &Rc<Self>) {
        let width = self.window.width().max(1);
        let next = self.state.density_mode.get().resolve(width);
        let previous = self.state.effective_density.replace(next);
        self.normal_nav
            .set_visible(next == EffectiveDensity::Normal);
        self.compact_nav
            .set_visible(next == EffectiveDensity::Compact);
        self.right_panel
            .set_width_request(if next == EffectiveDensity::Compact {
                306
            } else {
                340
            });

        if next != previous {
            debug!(?next, width, "effective density changed");
            self.render_current_route();
        }
    }

    fn render_current_route(self: &Rc<Self>) {
        while let Some(child) = self.route_host.first_child() {
            self.route_host.remove(&child);
        }

        let library = self.state.library.borrow().clone();
        if library.first_run {
            self.route_title.set_text(&tr("Add Jellyfin Server"));
            self.back_button.set_sensitive(false);
            self.forward_button.set_sensitive(false);
            let view = self.add_server_view();
            self.route_host.append(&view);
            return;
        }

        let route = self.state.routes.borrow().current().clone();
        self.route_title.set_text(&tr(route.title()));
        self.back_button
            .set_sensitive(self.state.routes.borrow().can_back());
        self.forward_button
            .set_sensitive(self.state.routes.borrow().can_forward());

        let view = match route {
            Route::Home => self.home_view(),
            Route::Albums => self.albums_view(),
            Route::AlbumDetail(album_id) => self.album_detail_view(album_id),
            Route::Tracks => self.tracks_view(library.tracks.clone(), &tr("Tracks")),
            Route::Settings => self.settings_view(),
            Route::Favorites => self.tracks_view(library.favorites.clone(), &tr("Favorites")),
            Route::Artists => self.artist_list_view(library.artists.clone(), &tr("Artists")),
            Route::ArtistDetail(_) => self.placeholder_view(
                "Artist",
                "Artist detail will use cached album and track groups.",
            ),
            Route::AlbumArtists => {
                self.artist_list_view(library.album_artists.clone(), &tr("Album Artists"))
            }
            Route::Genres => self.genre_list_view(library.genres.clone(), &tr("Genres")),
            Route::GenreDetail(_) => {
                self.placeholder_view("Genre", "Genre detail keeps albums above tracks.")
            }
            Route::Playlists => {
                self.playlist_list_view(library.playlists.clone(), &tr("Playlists"))
            }
            Route::PlaylistDetail(_) => {
                self.placeholder_view("Playlist", "Playlist detail will use the track table.")
            }
            Route::Search { query, .. } => self.search_view(&query, library),
        };

        self.route_host.append(&view);
    }

    fn add_server_view(self: &Rc<Self>) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(42);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(48);
        wrapper.set_margin_end(48);
        wrapper.set_width_request(520);
        wrapper.set_halign(gtk::Align::Center);

        let heading = gtk::Label::new(Some(&tr("Add Jellyfin Server")));
        heading.add_css_class("detail-title");
        heading.set_xalign(0.0);
        let subtitle = gtk::Label::new(Some(&tr(
            "Tokens are saved in native Secret Service. Cached library metadata is saved in SQLite.",
        )));
        subtitle.add_css_class("muted");
        subtitle.set_wrap(true);
        subtitle.set_xalign(0.0);

        let url = gtk::Entry::new();
        url.set_placeholder_text(Some(&tr("Server URL")));
        url.set_text("https://");
        let username = gtk::Entry::new();
        username.set_placeholder_text(Some(&tr("Username")));
        let password = gtk::PasswordEntry::new();
        password.set_placeholder_text(Some(&tr("Password")));
        let trust = gtk::Switch::new();
        trust.set_active(false);
        let trust_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let trust_label = gtk::Label::new(Some(&tr("Trust invalid certificate for this server")));
        trust_label.set_xalign(0.0);
        trust_label.set_hexpand(true);
        trust_row.append(&trust_label);
        trust_row.append(&trust);

        let status = gtk::Label::new(Some(&self.state.library.borrow().sync_status));
        status.add_css_class("muted");
        status.set_wrap(true);
        status.set_xalign(0.0);
        if let Some(error) = &self.state.library.borrow().last_error {
            status.set_text(error);
            status.add_css_class("error-text");
        }

        let login = text_button("network-server-symbolic", "Connect");
        let controller = self.controller.clone();
        let url_input = url.clone();
        let username_input = username.clone();
        let password_input = password.clone();
        let trust_input = trust.clone();
        login.connect_clicked(move |_| {
            controller.login(
                url_input.text().to_string(),
                username_input.text().to_string(),
                password_input.text().to_string(),
                trust_input.is_active(),
            );
        });

        wrapper.append(&heading);
        wrapper.append(&subtitle);
        wrapper.append(&url);
        wrapper.append(&username);
        wrapper.append(&password);
        wrapper.append(&trust_row);
        wrapper.append(&login);
        wrapper.append(&status);
        wrapper.upcast()
    }

    fn home_view(self: &Rc<Self>) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 26);
        content.add_css_class("route-content");
        content.set_margin_top(24);
        content.set_margin_bottom(36);
        content.set_margin_start(28);
        content.set_margin_end(28);

        for section in &self.state.library.borrow().home_sections {
            content.append(&self.home_album_section(section));
        }

        if self.state.library.borrow().home_sections.is_empty() {
            content.append(&self.placeholder_view(
                "Home",
                "Cached library data will appear here as sync pages finish.",
            ));
        }

        scroller.set_child(Some(&content));
        scroller.upcast()
    }

    fn home_album_section(self: &Rc<Self>, section_data: &HomeSection) -> gtk::Widget {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let heading = gtk::Label::new(Some(&tr(section_data.kind.title())));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        section.append(&heading);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("album-strip");
        for album in &section_data.albums {
            let card = self.album_card(album, true);
            let shell = Rc::clone(self);
            let album_id = album.id.clone();
            card.connect_clicked(move |_| shell.navigate(Route::AlbumDetail(album_id.clone())));
            row.append(&card);
        }

        section.append(&row);
        section.upcast()
    }

    fn albums_view(self: &Rc<Self>) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);

        let albums = self.state.library.borrow().albums.clone();
        let model = album_model(&albums);
        let selection = gtk::SingleSelection::new(Some(model));
        let factory = gtk::SignalListItemFactory::new();
        let density = self.state.effective_density.get();

        factory.connect_bind(move |_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(item) = list_item.item() else {
                return;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let album = boxed.borrow::<Album>();
            list_item.set_child(Some(&album_card_widget(&album, density, false)));
        });

        factory.connect_unbind(|_, list_item| {
            if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
                list_item.set_child(None::<&gtk::Widget>);
            }
        });

        let grid = gtk::GridView::new(Some(selection), Some(factory));
        grid.add_css_class("album-grid");
        grid.set_margin_top(24);
        grid.set_margin_bottom(36);
        grid.set_margin_start(28);
        grid.set_margin_end(28);
        grid.set_single_click_activate(true);
        grid.set_min_columns(1);
        grid.set_max_columns(if density == EffectiveDensity::Compact {
            4
        } else {
            10
        });

        let shell = Rc::clone(self);
        grid.connect_activate(move |_, position| {
            if let Some(album) = shell.state.library.borrow().albums.get(position as usize) {
                shell.navigate(Route::AlbumDetail(album.id.clone()));
            }
        });

        scroller.set_child(Some(&grid));
        scroller.upcast()
    }

    fn album_detail_view(self: &Rc<Self>, album_id: AlbumId) -> gtk::Widget {
        let Some(album) = self
            .state
            .library
            .borrow()
            .albums
            .iter()
            .find(|album| album.id.as_str() == album_id.as_str())
            .cloned()
        else {
            return self.placeholder_view("Album", "The selected cached album was not found.");
        };

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 22);
        content.add_css_class("route-content");
        content.set_margin_top(28);
        content.set_margin_bottom(36);
        content.set_margin_start(32);
        content.set_margin_end(32);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 22);
        let cover = cover_tile(album.color_seed, 188);
        header.append(&cover);

        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_valign(gtk::Align::Center);
        let kind = gtk::Label::new(Some(&tr("Album")));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);
        let title = gtk::Label::new(Some(&album.title));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        let artist = gtk::Label::new(Some(&album.artist));
        artist.add_css_class("detail-artist");
        artist.set_xalign(0.0);
        let facts = gtk::Label::new(Some(&format!(
            "{} • {} {} • {}",
            album.year,
            album.track_count,
            tr("tracks"),
            format_duration(album.duration_seconds)
        )));
        facts.add_css_class("muted");
        facts.set_xalign(0.0);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.append(&text_button("media-playback-start-symbolic", "Play"));
        actions.append(&text_button("media-skip-forward-symbolic", "Next"));
        actions.append(&text_button("emblem-favorite-symbolic", "Favorite"));

        metadata.append(&kind);
        metadata.append(&title);
        metadata.append(&artist);
        metadata.append(&actions);
        metadata.append(&facts);
        header.append(&metadata);
        content.append(&header);

        let tracks = self
            .state
            .library
            .borrow()
            .tracks
            .iter()
            .filter(|track| track.album_id.as_str() == album_id.as_str())
            .cloned()
            .collect::<Vec<_>>();
        let table = self.tracks_table(tracks);
        content.append(&table);

        scroller.set_child(Some(&content));
        scroller.upcast()
    }

    fn tracks_view(self: &Rc<Self>, tracks: Vec<Track>, title: &str) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(28);
        wrapper.set_margin_end(28);
        wrapper.set_vexpand(true);

        let heading = gtk::Label::new(Some(title));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        wrapper.append(&heading);
        wrapper.append(&self.tracks_table(tracks));
        wrapper.upcast()
    }

    fn tracks_table(self: &Rc<Self>, tracks: Vec<Track>) -> gtk::Widget {
        let model = track_model(&tracks);
        let selection = gtk::MultiSelection::new(Some(model));
        let table = gtk::ColumnView::new(Some(selection));
        table.add_css_class("track-table");
        table.set_vexpand(true);
        table.set_hexpand(true);

        table.append_column(&track_column("#", 54, |track| {
            track.track_number.to_string()
        }));
        table.append_column(&track_column("Title", 240, |track| track.title.clone()));
        table.append_column(&track_column("Artist", 180, |track| track.artist.clone()));
        table.append_column(&track_column("Album", 220, |track| track.album.clone()));
        table.append_column(&track_column("Year", 70, |track| track.year.to_string()));
        table.append_column(&track_column("Duration", 90, |track| {
            format_duration(track.duration_seconds)
        }));
        table.append_column(&track_column("Favorite", 76, |track| {
            if track.favorite {
                "Yes".to_string()
            } else {
                String::new()
            }
        }));

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&table));
        scroller.upcast()
    }

    fn artist_list_view(self: &Rc<Self>, artists: Vec<Artist>, title: &str) -> gtk::Widget {
        let rows = artists
            .into_iter()
            .map(|artist| {
                (
                    artist.name,
                    format!(
                        "{} {} / {} {}",
                        artist.album_count,
                        tr("albums"),
                        artist.track_count,
                        tr("tracks")
                    ),
                )
            })
            .collect::<Vec<_>>();
        self.simple_list_view(title, rows, "avatar-default-symbolic")
    }

    fn genre_list_view(self: &Rc<Self>, genres: Vec<Genre>, title: &str) -> gtk::Widget {
        let rows = genres
            .into_iter()
            .map(|genre| {
                (
                    genre.name,
                    format!(
                        "{} {} / {} {}",
                        genre.album_count,
                        tr("albums"),
                        genre.track_count,
                        tr("tracks")
                    ),
                )
            })
            .collect::<Vec<_>>();
        self.simple_list_view(title, rows, "flag-symbolic")
    }

    fn playlist_list_view(self: &Rc<Self>, playlists: Vec<Playlist>, title: &str) -> gtk::Widget {
        let rows = playlists
            .into_iter()
            .map(|playlist| {
                (
                    playlist.name,
                    format!(
                        "{} {} • {}",
                        playlist.track_count,
                        tr("tracks"),
                        format_duration(playlist.duration_seconds)
                    ),
                )
            })
            .collect::<Vec<_>>();
        self.simple_list_view(title, rows, "folder-music-symbolic")
    }

    fn simple_list_view(
        self: &Rc<Self>,
        title: &str,
        rows: Vec<(String, String)>,
        icon_name: &str,
    ) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(28);
        wrapper.set_margin_end(28);
        wrapper.set_vexpand(true);

        let heading = gtk::Label::new(Some(title));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        wrapper.append(&heading);

        if rows.is_empty() {
            wrapper.append(&self.placeholder_view(
                title,
                "Cached rows will appear here after the background sync finishes.",
            ));
            return wrapper.upcast();
        }

        let list = gtk::ListBox::new();
        list.add_css_class("cached-list");
        for (name, subtitle) in rows {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("cached-row");
            row.append(&gtk::Image::from_icon_name(icon_name));
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
            labels.set_hexpand(true);
            let name_label = gtk::Label::new(Some(&name));
            name_label.set_xalign(0.0);
            name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let subtitle_label = gtk::Label::new(Some(&subtitle));
            subtitle_label.add_css_class("muted");
            subtitle_label.set_xalign(0.0);
            labels.append(&name_label);
            labels.append(&subtitle_label);
            row.append(&labels);
            list.append(&row);
        }

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        wrapper.append(&scroller);
        wrapper.upcast()
    }

    fn search_view(self: &Rc<Self>, query: &str, library: LibrarySnapshot) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(28);
        wrapper.set_margin_end(28);
        wrapper.set_vexpand(true);

        let heading = gtk::Label::new(Some(&format!("{}: {query}", tr("Search"))));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        wrapper.append(&heading);

        let has_albums = !library.search.albums.is_empty();
        let has_tracks = !library.search.tracks.is_empty();
        let has_artists = !library.search.artists.is_empty();
        let has_playlists = !library.search.playlists.is_empty();
        let albums = library.search.albums;
        if !albums.is_empty() {
            let section = HomeSection {
                kind: rufin_core::HomeSectionKind::Explore,
                albums,
            };
            wrapper.append(&self.home_album_section(&section));
        }

        if has_tracks {
            wrapper.append(&self.tracks_table(library.search.tracks));
        } else if !has_albums && !has_artists && !has_playlists {
            wrapper.append(&self.placeholder_view(
                "Search",
                "Type a query in the sidebar search field to search the local cache.",
            ));
        }

        wrapper.upcast()
    }

    fn settings_view(self: &Rc<Self>) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let group = gtk::Box::new(gtk::Orientation::Vertical, 12);
        group.add_css_class("settings-group");

        let heading = gtk::Label::new(Some(&tr("Layout density")));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);

        let options = gtk::StringList::new(&[&tr("Auto"), &tr("Normal"), &tr("Compact")]);
        let dropdown = gtk::DropDown::new(Some(options), None::<gtk::Expression>);
        dropdown.set_selected(match self.state.density_mode.get() {
            DensityMode::Auto => 0,
            DensityMode::Normal => 1,
            DensityMode::Compact => 2,
        });

        let shell = Rc::clone(self);
        dropdown.connect_selected_notify(move |dropdown| {
            let density = match dropdown.selected() {
                1 => DensityMode::Normal,
                2 => DensityMode::Compact,
                _ => DensityMode::Auto,
            };
            shell.set_density_mode(density);
        });

        let note = gtk::Label::new(Some(&tr(
            "M0 keeps this setting in memory so the shell can exercise adaptive layouts.",
        )));
        note.add_css_class("muted");
        note.set_wrap(true);
        note.set_xalign(0.0);

        group.append(&heading);
        group.append(&dropdown);
        group.append(&note);
        wrapper.append(&group);

        let library = self.state.library.borrow();
        let server_name = library
            .server
            .as_ref()
            .map(|server| server.name.as_str())
            .unwrap_or("No server");
        let username = library.username.as_deref().unwrap_or("no account");
        let status = gtk::Label::new(Some(&format!(
            "{} ({username}): {} {} / {} {} • {}",
            server_name,
            library.albums.len(),
            tr("albums"),
            library.tracks.len(),
            tr("tracks"),
            library.sync_status
        )));
        status.add_css_class("muted");
        status.set_xalign(0.0);
        wrapper.append(&status);

        let server_group = gtk::Box::new(gtk::Orientation::Vertical, 12);
        server_group.add_css_class("settings-group");
        let server_heading = gtk::Label::new(Some(&tr("Jellyfin Server")));
        server_heading.add_css_class("section-heading");
        server_heading.set_xalign(0.0);
        server_group.append(&server_heading);

        let server_url = library
            .server
            .as_ref()
            .map(|server| server.base_url.clone())
            .unwrap_or_else(|| tr("No active server"));
        let details = gtk::Label::new(Some(&format!(
            "{}\n{}: {}\n{}: {} {} / {} {}",
            server_url,
            tr("User"),
            username,
            tr("Cached"),
            library.albums.len(),
            tr("albums"),
            library.tracks.len(),
            tr("tracks")
        )));
        details.add_css_class("muted");
        details.set_wrap(true);
        details.set_xalign(0.0);
        server_group.append(&details);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let resync = text_button("view-refresh-symbolic", "Resync Library");
        let clear_cache = text_button("edit-clear-symbolic", "Clear Cached Library");
        let forget = text_button("user-trash-symbolic", "Forget Server");
        forget.add_css_class("destructive-action");

        let controller = self.controller.clone();
        resync.connect_clicked(move |_| controller.resync_active_server());

        let clear_shell = Rc::clone(self);
        clear_cache.connect_clicked(move |_| clear_shell.confirm_clear_cache());

        let forget_shell = Rc::clone(self);
        forget.connect_clicked(move |_| forget_shell.confirm_forget_server());

        actions.append(&resync);
        actions.append(&clear_cache);
        actions.append(&forget);
        server_group.append(&actions);
        wrapper.append(&server_group);

        wrapper.upcast()
    }

    fn confirm_clear_cache(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Clear Cached Library"))
            .body(tr(
                "This removes cached Jellyfin library metadata for the active server. Login stays saved.",
            ))
            .build();
        let cancel = tr("Cancel");
        let clear = tr("Clear Cache");
        dialog.add_responses(&[("cancel", cancel.as_str()), ("clear", clear.as_str())]);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        let controller = self.controller.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() == "clear" {
                    controller.clear_active_server_cache();
                }
            },
        );
    }

    fn confirm_forget_server(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Forget Server"))
            .body(tr(
                "This removes the active server, cached library metadata, queue snapshot, and saved token.",
            ))
            .build();
        let cancel = tr("Cancel");
        let forget = tr("Forget Server");
        dialog.add_responses(&[("cancel", cancel.as_str()), ("forget", forget.as_str())]);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("forget", adw::ResponseAppearance::Destructive);
        let controller = self.controller.clone();
        dialog.choose(
            Some(&self.window),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() == "forget" {
                    controller.forget_active_server();
                }
            },
        );
    }

    fn placeholder_view(&self, title: &str, body: &str) -> gtk::Widget {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 12);
        wrapper.add_css_class("empty-state");
        wrapper.set_vexpand(true);
        wrapper.set_hexpand(true);
        wrapper.set_valign(gtk::Align::Center);
        wrapper.set_halign(gtk::Align::Center);

        let heading = gtk::Label::new(Some(&tr(title)));
        heading.add_css_class("section-heading");
        let label = gtk::Label::new(Some(&tr(body)));
        label.add_css_class("muted");
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
        wrapper.append(&heading);
        wrapper.append(&label);
        wrapper.upcast()
    }

    fn album_card(&self, album: &Album, compact: bool) -> gtk::Button {
        let button = gtk::Button::new();
        button.add_css_class("album-button");
        let density = if compact {
            EffectiveDensity::Compact
        } else {
            self.state.effective_density.get()
        };
        button.set_child(Some(&album_card_widget(album, density, compact)));
        button
    }
}

fn connect_shell_actions(shell: &Rc<Shell>, settings_button: gtk::Button) {
    let back_shell = Rc::clone(shell);
    shell
        .back_button
        .connect_clicked(move |_| back_shell.go_back());

    let forward_shell = Rc::clone(shell);
    shell
        .forward_button
        .connect_clicked(move |_| forward_shell.go_forward());

    let settings_shell = Rc::clone(shell);
    settings_button.connect_clicked(move |_| settings_shell.navigate(Route::Settings));

    let resize_shell = Rc::clone(shell);
    shell
        .window
        .connect_notify_local(Some("width"), move |_, _| {
            if resize_shell.state.density_mode.get() == DensityMode::Auto {
                resize_shell.update_density();
            }
        });
}

fn install_event_pump(shell: &Rc<Shell>, receiver: Receiver<ControllerEvent>) {
    let shell = Rc::clone(shell);
    glib::timeout_add_local(Duration::from_millis(100), move || {
        while let Ok(event) = receiver.try_recv() {
            match event {
                ControllerEvent::Snapshot(snapshot) => {
                    *shell.state.library.borrow_mut() = *snapshot;
                    shell.render_current_route();
                }
                ControllerEvent::LoginStatus(status) => {
                    shell.state.library.borrow_mut().sync_status = status;
                    shell.render_current_route();
                }
                ControllerEvent::Error(error) => {
                    warn!(%error, "controller error");
                    let mut library = shell.state.library.borrow_mut();
                    library.sync_status = "Action failed.".to_string();
                    library.last_error = Some(error);
                    drop(library);
                    shell.render_current_route();
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

fn build_normal_navigation(shell: &Rc<Shell>) {
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&tr("Search")));
    search.set_margin_top(18);
    search.set_margin_start(16);
    search.set_margin_end(16);
    let search_shell = Rc::clone(shell);
    search.connect_activate(move |entry| {
        let query = entry.text().trim().to_string();
        if query.is_empty() {
            return;
        }
        search_shell.controller.search(query.clone());
        search_shell.navigate(Route::Search {
            query,
            kind: SearchKind::All,
        });
    });
    shell.normal_nav.append(&search);

    let heading = gtk::Label::new(Some(&tr("My Library")));
    heading.add_css_class("nav-heading");
    heading.set_xalign(0.0);
    heading.set_margin_start(18);
    heading.set_margin_top(18);
    shell.normal_nav.append(&heading);

    for item in nav_items() {
        shell.normal_nav.append(&nav_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
            false,
        ));
    }

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    shell.normal_nav.append(&spacer);

    let server = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    server.add_css_class("server-card");
    server.set_margin_start(14);
    server.set_margin_end(14);
    server.set_margin_bottom(14);
    server.append(&gtk::Image::from_icon_name("audio-x-generic-symbolic"));
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let name = gtk::Label::new(Some("Rufin"));
    name.set_xalign(0.0);
    let subtitle = gtk::Label::new(Some(&tr("Cached library")));
    subtitle.add_css_class("muted");
    subtitle.set_xalign(0.0);
    labels.append(&name);
    labels.append(&subtitle);
    server.append(&labels);
    shell.normal_nav.append(&server);
}

fn build_compact_navigation(shell: &Rc<Shell>) {
    shell.compact_nav.append(&rail_button(
        shell,
        "open-menu-symbolic",
        "Menu",
        Route::Home,
    ));
    for item in nav_items() {
        shell.compact_nav.append(&rail_button(
            shell,
            item.icon_name,
            item.label,
            item.route.clone(),
        ));
    }
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    shell.compact_nav.append(&spacer);
    shell.compact_nav.append(&rail_button(
        shell,
        "audio-x-generic-symbolic",
        "Rufin",
        Route::Settings,
    ));
}

#[derive(Clone)]
struct NavItem {
    icon_name: &'static str,
    label: &'static str,
    route: Route,
}

fn nav_items() -> Vec<NavItem> {
    vec![
        NavItem {
            icon_name: "go-home-symbolic",
            label: "Home",
            route: Route::Home,
        },
        NavItem {
            icon_name: "emblem-favorite-symbolic",
            label: "Favorites",
            route: Route::Favorites,
        },
        NavItem {
            icon_name: "media-optical-symbolic",
            label: "Albums",
            route: Route::Albums,
        },
        NavItem {
            icon_name: "audio-x-generic-symbolic",
            label: "Tracks",
            route: Route::Tracks,
        },
        NavItem {
            icon_name: "avatar-default-symbolic",
            label: "Album Artists",
            route: Route::AlbumArtists,
        },
        NavItem {
            icon_name: "system-users-symbolic",
            label: "Artists",
            route: Route::Artists,
        },
        NavItem {
            icon_name: "flag-symbolic",
            label: "Genres",
            route: Route::Genres,
        },
        NavItem {
            icon_name: "folder-music-symbolic",
            label: "Playlists",
            route: Route::Playlists,
        },
    ]
}

fn nav_button(
    shell: &Rc<Shell>,
    icon_name: &str,
    label: &str,
    route: Route,
    compact: bool,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("nav-button");
    button.set_tooltip_text(Some(&tr(label)));

    let content = gtk::Box::new(
        if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        },
        8,
    );
    content.append(&gtk::Image::from_icon_name(icon_name));
    let text = gtk::Label::new(Some(&tr(label)));
    text.set_xalign(0.0);
    if compact {
        text.add_css_class("rail-label");
    }
    content.append(&text);
    button.set_child(Some(&content));

    let shell = Rc::clone(shell);
    button.connect_clicked(move |_| shell.navigate(route.clone()));
    button
}

fn rail_button(shell: &Rc<Shell>, icon_name: &str, label: &str, route: Route) -> gtk::Button {
    nav_button(shell, icon_name, label, route, true)
}

fn build_right_panel() -> gtk::Box {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add_css_class("right-panel");
    panel.set_width_request(340);
    panel.set_vexpand(true);

    let queue = gtk::Box::new(gtk::Orientation::Vertical, 8);
    queue.add_css_class("queue-panel");
    queue.set_vexpand(true);
    queue.set_margin_top(16);
    queue.set_margin_start(16);
    queue.set_margin_end(16);
    queue.set_margin_bottom(12);

    let queue_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let queue_title = gtk::Label::new(Some(&tr("Queue")));
    queue_title.add_css_class("panel-title");
    queue_title.set_xalign(0.0);
    queue_title.set_hexpand(true);
    queue_header.append(&queue_title);
    queue_header.append(&icon_button("media-playlist-shuffle-symbolic", "Shuffle"));
    queue_header.append(&icon_button("edit-clear-symbolic", "Clear queue"));
    queue.append(&queue_header);

    let queue_list = gtk::ListBox::new();
    queue_list.add_css_class("queue-list");
    for index in 1..=12 {
        queue_list.append(&queue_row(index));
    }
    queue.append(&queue_list);

    let lyrics = gtk::Box::new(gtk::Orientation::Vertical, 10);
    lyrics.add_css_class("lyrics-panel");
    lyrics.set_vexpand(true);
    lyrics.set_margin_top(12);
    lyrics.set_margin_start(16);
    lyrics.set_margin_end(16);
    lyrics.set_margin_bottom(18);

    let lyrics_title = gtk::Label::new(Some(&tr("Lyrics")));
    lyrics_title.add_css_class("panel-title");
    lyrics_title.set_xalign(0.0);
    lyrics.append(&lyrics_title);

    let lyrics_scroll = gtk::ScrolledWindow::new();
    lyrics_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    lyrics_scroll.set_vexpand(true);
    let lyrics_lines = gtk::Box::new(gtk::Orientation::Vertical, 14);
    lyrics_lines.add_css_class("lyrics-lines");
    for (index, line) in [
        "I keep the signal close",
        "A quiet room starts glowing",
        "The chorus moves in time",
        "This line is the live lyric",
        "Every echo folds back home",
        "The last light stays on",
    ]
    .iter()
    .enumerate()
    {
        let label = gtk::Label::new(Some(line));
        label.set_wrap(true);
        label.set_justify(gtk::Justification::Center);
        label.add_css_class(if index == 3 {
            "lyric-current"
        } else {
            "lyric-line"
        });
        lyrics_lines.append(&label);
    }
    lyrics_scroll.set_child(Some(&lyrics_lines));
    lyrics.append(&lyrics_scroll);

    panel.append(&queue);
    panel.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    panel.append(&lyrics);
    panel
}

fn queue_row(index: u32) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("queue-row");
    let number = gtk::Label::new(Some(&index.to_string()));
    number.add_css_class("muted");
    number.set_width_chars(2);
    let cover = cover_tile(index * 7, 42);
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let title = gtk::Label::new(Some(&format!("Queue Track {index}")));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let artist = gtk::Label::new(Some("Signal Park"));
    artist.add_css_class("muted");
    artist.set_xalign(0.0);
    labels.append(&title);
    labels.append(&artist);
    let duration = gtk::Label::new(Some("3:24"));
    duration.add_css_class("muted");
    row.append(&number);
    row.append(&cover);
    row.append(&labels);
    row.append(&duration);
    row.upcast()
}

fn build_bottom_player() -> gtk::Box {
    let player = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    player.add_css_class("bottom-player");
    player.set_height_request(90);

    let cover = cover_tile(42, 58);
    player.append(&cover);

    let identity = gtk::Box::new(gtk::Orientation::Vertical, 2);
    identity.set_width_request(210);
    let title = gtk::Label::new(Some("First Motion 1"));
    title.add_css_class("player-title");
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let artist = gtk::Label::new(Some("Astral Kin"));
    artist.set_xalign(0.0);
    artist.add_css_class("muted");
    let album = gtk::Label::new(Some("Blue Rooms 1"));
    album.set_xalign(0.0);
    album.add_css_class("muted");
    album.set_ellipsize(gtk::pango::EllipsizeMode::End);
    identity.append(&title);
    identity.append(&artist);
    identity.append(&album);
    player.append(&identity);

    let transport = gtk::Box::new(gtk::Orientation::Vertical, 6);
    transport.set_hexpand(true);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::Center);
    for (icon, label) in [
        ("media-playback-stop-symbolic", "Stop"),
        ("media-playlist-shuffle-symbolic", "Shuffle"),
        ("media-skip-backward-symbolic", "Previous"),
        ("media-playback-pause-symbolic", "Pause"),
        ("media-skip-forward-symbolic", "Next"),
        ("media-playlist-repeat-symbolic", "Repeat"),
    ] {
        buttons.append(&icon_button(icon, label));
    }
    let progress = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 205.0, 1.0);
    progress.set_value(82.0);
    progress.set_draw_value(false);
    progress.set_hexpand(true);
    transport.append(&buttons);
    transport.append(&progress);
    player.append(&transport);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.append(&icon_button("view-list-symbolic", "Queue"));
    actions.append(&icon_button("insert-text-symbolic", "Lyrics"));
    actions.append(&icon_button("emblem-favorite-symbolic", "Favorite"));
    actions.append(&icon_button("audio-volume-high-symbolic", "Volume"));
    player.append(&actions);

    player
}

fn album_model(albums: &[Album]) -> gio::ListStore {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    for album in albums {
        model.append(&glib::BoxedAnyObject::new(album.clone()));
    }
    model
}

fn track_model(tracks: &[Track]) -> gio::ListStore {
    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    for track in tracks {
        model.append(&glib::BoxedAnyObject::new(track.clone()));
    }
    model
}

fn track_column<F>(title: &str, width: i32, value: F) -> gtk::ColumnViewColumn
where
    F: Fn(&Track) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);

    factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        list_item.set_child(Some(&label));
    });

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(child) = list_item.child() else {
            return;
        };
        let Ok(label) = child.downcast::<gtk::Label>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let track = boxed.borrow::<Track>();
        label.set_text(&value(&track));
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(false);
    column
}

fn album_card_widget(album: &Album, density: EffectiveDensity, strip: bool) -> gtk::Widget {
    let size = match (density, strip) {
        (_, true) => 118,
        (EffectiveDensity::Compact, false) => 146,
        (EffectiveDensity::Normal, false) => 112,
    };

    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("album-card");
    card.set_width_request(size);
    let cover = cover_tile(album.color_seed, size);
    card.append(&cover);

    let title = gtk::Label::new(Some(&album.title));
    title.add_css_class("album-title");
    title.set_xalign(0.0);
    title.set_lines(2);
    title.set_wrap(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let artist = gtk::Label::new(Some(&album.artist));
    artist.add_css_class("muted");
    artist.set_xalign(0.0);
    artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let year = gtk::Label::new(Some(&album.year.to_string()));
    year.add_css_class("muted");
    year.set_xalign(0.0);

    card.append(&title);
    card.append(&artist);
    card.append(&year);
    card.upcast()
}

fn cover_tile(seed: u32, size: i32) -> gtk::Widget {
    let area = gtk::DrawingArea::new();
    area.add_css_class("cover-tile");
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_width_request(size);
    area.set_height_request(size);
    area.set_draw_func(move |_, context, width, height| {
        let red = f64::from((seed & 0xff) as u8) / 255.0;
        let green = f64::from(((seed >> 8) & 0xff) as u8) / 255.0;
        let blue = f64::from(((seed >> 16) & 0xff) as u8) / 255.0;
        context.set_source_rgb(red * 0.7 + 0.18, green * 0.7 + 0.18, blue * 0.7 + 0.18);
        context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
        let _paint = context.fill();

        context.set_source_rgba(1.0, 1.0, 1.0, 0.18);
        context.move_to(0.0, f64::from(height) * 0.2);
        context.line_to(f64::from(width) * 0.8, 0.0);
        context.line_to(f64::from(width), f64::from(height) * 0.8);
        context.line_to(f64::from(width) * 0.2, f64::from(height));
        context.close_path();
        let _fill = context.fill();
    });
    area.upcast()
}

fn icon_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon_name);
    button.add_css_class("icon-button");
    button.set_tooltip_text(Some(&tr(label)));
    button
}

fn text_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("pill-button");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&gtk::Image::from_icon_name(icon_name));
    content.append(&gtk::Label::new(Some(&tr(label))));
    button.set_child(Some(&content));
    button
}

fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use rufin_core::{
    Album, AlbumId, DensityMode, EffectiveDensity, HomeSection, Route, RouteStack, ServerIdentity,
    Track, format_duration,
};
use rufin_provider::{MusicProvider, PagedRequest};
use rufin_test_support::{FakeProvider, FakeScale};
use tracing::{debug, info};

use crate::i18n::tr;

#[derive(Clone, Debug)]
pub struct AppOptions {
    pub fake_scale: FakeScale,
    pub smoke_exit_ms: Option<u64>,
}

struct AppState {
    routes: RefCell<RouteStack>,
    density_mode: Cell<DensityMode>,
    effective_density: Cell<EffectiveDensity>,
    library: ProviderSnapshot,
}

#[derive(Clone, Debug)]
struct ProviderSnapshot {
    server: ServerIdentity,
    home_sections: Vec<HomeSection>,
    albums: Vec<Album>,
    tracks: Vec<Track>,
}

struct Shell {
    state: AppState,
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
    let provider = FakeProvider::new(options.fake_scale);
    let library = load_provider_snapshot(&provider);
    info!(
        albums = library.albums.len(),
        tracks = library.tracks.len(),
        elapsed_ms = loaded_at.elapsed().as_millis(),
        "loaded fake music library through provider boundary"
    );

    let state = AppState {
        routes: RefCell::new(RouteStack::new(Route::Home)),
        density_mode: Cell::new(DensityMode::Auto),
        effective_density: Cell::new(DensityMode::Auto.resolve(1_400)),
        library,
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

    if let Some(delay_ms) = options.smoke_exit_ms {
        let app = app.clone();
        glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
            info!(delay_ms, "smoke exit requested");
            app.quit();
        });
    }

    shell.window.present();
}

fn load_provider_snapshot(provider: &FakeProvider) -> ProviderSnapshot {
    let context = glib::MainContext::default();
    let home_sections = match context.block_on(provider.home_sections()) {
        Ok(sections) => sections,
        Err(error) => panic!("failed to load fake home sections: {error}"),
    };
    let albums =
        match context.block_on(provider.albums(PagedRequest::new(0, provider.album_count()))) {
            Ok(response) => response.items,
            Err(error) => panic!("failed to load fake albums: {error}"),
        };
    let tracks =
        match context.block_on(provider.tracks(PagedRequest::new(0, provider.track_count()))) {
            Ok(response) => response.items,
            Err(error) => panic!("failed to load fake tracks: {error}"),
        };

    ProviderSnapshot {
        server: provider.identity().server.clone(),
        home_sections,
        albums,
        tracks,
    }
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
            Route::Tracks => self.tracks_view(self.state.library.tracks.clone(), &tr("Tracks")),
            Route::Settings => self.settings_view(),
            Route::Favorites => self.placeholder_view(
                "Favorites",
                "Favorite tracks, albums, and artists will be grouped here.",
            ),
            Route::Artists => {
                self.placeholder_view("Artists", "Artist browsing uses fake rows in M0.")
            }
            Route::ArtistDetail(_) => self.placeholder_view(
                "Artist",
                "Artist detail is represented by this native route.",
            ),
            Route::AlbumArtists => self.placeholder_view(
                "Album Artists",
                "Album artist browsing uses fake rows in M0.",
            ),
            Route::Genres => {
                self.placeholder_view("Genres", "Genre chips and counts will live here.")
            }
            Route::GenreDetail(_) => {
                self.placeholder_view("Genre", "Genre detail keeps albums above tracks.")
            }
            Route::Playlists => self.placeholder_view(
                "Playlists",
                "Playlist shells are native placeholders in M0.",
            ),
            Route::PlaylistDetail(_) => {
                self.placeholder_view("Playlist", "Playlist detail will use the track table.")
            }
            Route::Search { query, .. } => self.placeholder_view(
                "Search",
                &format!("Search route is wired. Current query: {query}"),
            ),
        };

        self.route_host.append(&view);
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

        for section in &self.state.library.home_sections {
            content.append(&self.home_album_section(section));
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

        let model = album_model(&self.state.library.albums);
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
            if let Some(album) = shell.state.library.albums.get(position as usize) {
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
            .albums
            .iter()
            .find(|album| album.id.as_str() == album_id.as_str())
        else {
            return self.placeholder_view("Album", "The selected fake album was not found.");
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

        let status = gtk::Label::new(Some(&format!(
            "{}: {} {} / {} {}",
            self.state.library.server.name,
            self.state.library.albums.len(),
            tr("albums"),
            self.state.library.tracks.len(),
            tr("tracks")
        )));
        status.add_css_class("muted");
        status.set_xalign(0.0);
        wrapper.append(&status);

        wrapper.upcast()
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

fn build_normal_navigation(shell: &Rc<Shell>) {
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&tr("Search")));
    search.set_margin_top(18);
    search.set_margin_start(16);
    search.set_margin_end(16);
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
    let subtitle = gtk::Label::new(Some(&tr("Fake library")));
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

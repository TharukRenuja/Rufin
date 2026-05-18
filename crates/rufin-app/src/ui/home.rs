use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{
    Album, Genre, HomeBlockKind, HomeSection, HomeSectionKind, Route, format_duration,
};

use crate::controller::LibrarySnapshot;
use crate::i18n::tr;

use super::cards::{render_home_album_page, render_home_track_page};
use super::{
    GRID_COVER_SIZE, HOME_ALBUM_GAP, HomeSectionState, PRIMARY_ROUTE_MARGIN_END,
    PRIMARY_ROUTE_MARGIN_START, Shell, add_album_seed_gradient_class, icon_button,
};

fn showcase_album(library: &LibrarySnapshot) -> Option<Album> {
    let explore_first_id = library
        .home_sections
        .iter()
        .find(|section| section.kind == HomeSectionKind::Explore)
        .and_then(|section| section.albums.first())
        .map(|album| album.id.clone());

    library
        .home_sections
        .iter()
        .filter(|section| section.kind != HomeSectionKind::Explore)
        .flat_map(|section| section.albums.iter())
        .find(|album| explore_first_id.as_ref() != Some(&album.id))
        .cloned()
        .or_else(|| {
            library
                .home_sections
                .iter()
                .find(|section| section.kind == HomeSectionKind::Explore)
                .and_then(|section| section.albums.get(1))
                .cloned()
        })
        .or_else(|| {
            library
                .albums
                .iter()
                .find(|album| explore_first_id.as_ref() != Some(&album.id))
                .cloned()
        })
        .or_else(|| {
            library
                .home_sections
                .iter()
                .find(|section| section.kind == HomeSectionKind::Explore)
                .and_then(|section| section.albums.first())
                .cloned()
        })
        .or_else(|| library.albums.first().cloned())
}

fn home_showcase_facts(album: &Album) -> String {
    let mut parts = Vec::new();
    if album.year > 0 {
        parts.push(album.year.to_string());
    }
    if album.track_count > 0 {
        parts.push(format!("{} {}", album.track_count, tr("tracks")));
    }
    if album.duration_seconds > 0 {
        parts.push(format_duration(album.duration_seconds));
    }
    parts.join(" • ")
}

impl Shell {
    pub(super) fn home_view(self: &Rc<Self>) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.add_css_class("route-content");
        content.set_margin_top(24);
        content.set_margin_bottom(36);
        content.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        content.set_margin_end(PRIMARY_ROUTE_MARGIN_END);

        let blocks = self.state.settings.borrow().home_blocks.clone();
        let library = self.state.library.borrow().clone();
        let mut appended = false;
        for block in blocks {
            let child = match block {
                HomeBlockKind::Showcase => self.home_showcase_block(&library),
                HomeBlockKind::Genres => self.home_genres_block(&library.genres),
                _ => block
                    .section_kind()
                    .and_then(|kind| {
                        library
                            .home_sections
                            .iter()
                            .find(|section| section.kind == kind)
                    })
                    .map(|section| self.home_section(section)),
            };
            if let Some(child) = child {
                content.append(&child);
                appended = true;
            }
        }

        if !appended {
            content
                .append(&self.route_empty_view(
                    "Cached library data will appear here as sync pages finish.",
                ));
        }

        scroller.set_child(Some(&content));
        scroller.upcast()
    }

    fn home_showcase_block(self: &Rc<Self>, library: &LibrarySnapshot) -> Option<gtk::Widget> {
        let album = showcase_album(library)?;

        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);

        let heading = gtk::Label::new(Some(&tr(HomeBlockKind::Showcase.title())));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        section.append(&heading);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        body.add_css_class("home-showcase");
        add_album_seed_gradient_class(&body, album.color_seed);
        body.set_hexpand(true);
        body.set_valign(gtk::Align::Start);
        let cover = self.cover_tile_for(
            album.image_ref.as_ref(),
            album.color_seed,
            196,
            GRID_COVER_SIZE,
        );
        cover.add_css_class("home-showcase-cover");
        body.append(&cover);

        let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
        metadata.set_hexpand(true);
        metadata.set_valign(gtk::Align::Center);

        let title = gtk::Label::new(Some(&album.title));
        title.add_css_class("home-showcase-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        metadata.append(&title);

        let artist = gtk::Label::new(Some(&album.artist));
        artist.add_css_class("muted");
        artist.set_xalign(0.0);
        artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
        metadata.append(&artist);

        let facts = gtk::Label::new(Some(&home_showcase_facts(&album)));
        facts.add_css_class("muted");
        facts.set_xalign(0.0);
        metadata.append(&facts);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.add_css_class("home-showcase-actions");
        let play = icon_button("media-playback-start-symbolic", "Play album");
        play.add_css_class("home-showcase-action-button");
        play.add_css_class("home-showcase-play-button");
        let controller = self.controller.clone();
        let album_id = album.id.clone();
        play.connect_clicked(move |_| controller.play_album_now(album_id.clone()));
        actions.append(&play);

        let play_next = icon_button("media-skip-forward-symbolic", "Play next");
        play_next.add_css_class("home-showcase-action-button");
        let controller = self.controller.clone();
        let album_id = album.id.clone();
        play_next.connect_clicked(move |_| {
            if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
                for track in tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        });
        actions.append(&play_next);
        metadata.append(&actions);

        body.append(&metadata);
        section.append(&body);
        Some(section.upcast())
    }

    fn home_genres_block(self: &Rc<Self>, genres: &[Genre]) -> Option<gtk::Widget> {
        if genres.is_empty() {
            return None;
        }

        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);

        let heading = gtk::Label::new(Some(&tr(HomeBlockKind::Genres.title())));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        section.append(&heading);

        let flow = gtk::FlowBox::new();
        flow.add_css_class("home-genre-flow");
        flow.set_column_spacing(8);
        flow.set_row_spacing(8);
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(6);
        flow.set_min_children_per_line(2);

        for genre in genres.iter().take(12) {
            flow.insert(&self.home_genre_chip(genre), -1);
        }

        section.append(&flow);
        Some(section.upcast())
    }

    fn home_genre_chip(self: &Rc<Self>, genre: &Genre) -> gtk::Widget {
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("home-genre-chip");
        button.set_hexpand(true);
        button.set_halign(gtk::Align::Fill);

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_margin_top(8);
        labels.set_margin_bottom(8);
        labels.set_margin_start(10);
        labels.set_margin_end(10);

        let name = gtk::Label::new(Some(&genre.name));
        name.add_css_class("album-title");
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&name);

        let counts = gtk::Label::new(Some(&format!(
            "{} {} • {} {}",
            genre.album_count,
            tr("albums"),
            genre.track_count,
            tr("tracks")
        )));
        counts.add_css_class("muted");
        counts.set_xalign(0.0);
        counts.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&counts);

        button.set_child(Some(&labels));
        let shell = Rc::clone(self);
        let genre_id = genre.id.clone();
        button.connect_clicked(move |_| shell.navigate(Route::GenreDetail(genre_id.clone())));
        button.upcast()
    }

    fn home_section(self: &Rc<Self>, section_data: &HomeSection) -> gtk::Widget {
        if !section_data.tracks.is_empty() {
            self.home_track_section(section_data)
        } else {
            self.home_album_section(section_data)
        }
    }

    pub(super) fn home_album_section(self: &Rc<Self>, section_data: &HomeSection) -> gtk::Widget {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);
        let section_kind = section_data.kind;
        let albums = section_data.albums.clone();

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let heading = gtk::Label::new(Some(&tr(section_data.kind.title())));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        heading.set_hexpand(true);
        header.append(&heading);

        let previous = icon_button("go-previous-symbolic", "Previous page");
        let next = icon_button("go-next-symbolic", "Next page");
        let refresh = icon_button("view-refresh-symbolic", "Refresh section");
        header.append(&previous);
        header.append(&next);
        header.append(&refresh);
        section.append(&header);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, HOME_ALBUM_GAP);
        row.add_css_class("album-strip");
        row.set_hexpand(true);
        section.append(&row);

        let shell = Rc::clone(self);
        previous.connect_clicked(move |_| {
            let mut states = shell.state.home_section_state.borrow_mut();
            let state = states.entry(section_kind).or_insert(HomeSectionState {
                page_start: 0,
                page_size: 2,
            });
            state.page_start = state.page_start.saturating_sub(state.page_size);
            drop(states);
            shell.render_current_route();
        });

        let shell = Rc::clone(self);
        let albums_for_next = albums.clone();
        next.connect_clicked(move |_| {
            let mut states = shell.state.home_section_state.borrow_mut();
            let state = states.entry(section_kind).or_insert(HomeSectionState {
                page_start: 0,
                page_size: 2,
            });
            let next_page = state.page_start.saturating_add(state.page_size);
            if next_page < albums_for_next.len() {
                state.page_start = next_page;
            }
            drop(states);
            shell.render_current_route();
        });

        let shell = Rc::clone(self);
        refresh.connect_clicked(move |_| {
            shell.refresh_home_section(section_kind);
        });

        render_home_album_page(self, &row, &previous, &next, section_kind, &albums);
        section.upcast()
    }

    fn home_track_section(self: &Rc<Self>, section_data: &HomeSection) -> gtk::Widget {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);
        let section_kind = section_data.kind;
        let tracks = section_data.tracks.clone();

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let heading = gtk::Label::new(Some(&tr(section_data.kind.title())));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        heading.set_hexpand(true);
        header.append(&heading);

        let previous = icon_button("go-previous-symbolic", "Previous page");
        let next = icon_button("go-next-symbolic", "Next page");
        let refresh = icon_button("view-refresh-symbolic", "Refresh section");
        header.append(&previous);
        header.append(&next);
        header.append(&refresh);
        section.append(&header);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, HOME_ALBUM_GAP);
        row.add_css_class("album-strip");
        row.set_hexpand(true);
        section.append(&row);

        let shell = Rc::clone(self);
        previous.connect_clicked(move |_| {
            let mut states = shell.state.home_section_state.borrow_mut();
            let state = states.entry(section_kind).or_insert(HomeSectionState {
                page_start: 0,
                page_size: 2,
            });
            state.page_start = state.page_start.saturating_sub(state.page_size);
            drop(states);
            shell.render_current_route();
        });

        let shell = Rc::clone(self);
        let tracks_for_next = tracks.clone();
        next.connect_clicked(move |_| {
            let mut states = shell.state.home_section_state.borrow_mut();
            let state = states.entry(section_kind).or_insert(HomeSectionState {
                page_start: 0,
                page_size: 2,
            });
            let next_page = state.page_start.saturating_add(state.page_size);
            if next_page < tracks_for_next.len() {
                state.page_start = next_page;
            }
            drop(states);
            shell.render_current_route();
        });

        let shell = Rc::clone(self);
        refresh.connect_clicked(move |_| {
            shell.refresh_home_section(section_kind);
        });

        render_home_track_page(self, &row, &previous, &next, section_kind, &tracks);
        section.upcast()
    }
}

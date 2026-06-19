use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;
use domain::{Album, Genre, HomeBlockKind, HomeSection, HomeSectionKind, Route, format_duration};

use crate::controller::LibrarySnapshot;
use crate::i18n::tr;

use super::cards::{album_cover_tile, render_home_album_page, render_home_track_page};
use super::{
    DETAIL_GRADIENT_MARGIN_END, HOME_ALBUM_GAP, PRIMARY_ROUTE_MARGIN_START, ROUTE_TOP_MARGIN,
    Shell, add_album_seed_gradient_class, add_card_label_link, album_artist_route,
    album_count_text, configure_fill_width_clip, icon_button, mark_route_scroll_owner,
    track_count_text,
};

pub(super) fn showcase_album(library: &LibrarySnapshot, seed: u64) -> Option<Album> {
    let explore_first_id = library
        .home_sections
        .iter()
        .find(|section| section.kind == HomeSectionKind::Explore)
        .and_then(|section| section.albums.first())
        .map(|album| album.id.clone());

    let mut seen = HashSet::new();
    let section_candidates = library
        .home_sections
        .iter()
        .filter(|section| section.kind != HomeSectionKind::Explore)
        .flat_map(|section| section.albums.iter())
        .filter(|album| explore_first_id.as_ref() != Some(&album.id))
        .filter(|album| seen.insert(album.id.clone()))
        .collect::<Vec<_>>();

    if !section_candidates.is_empty() {
        return section_candidates
            .get((seed as usize) % section_candidates.len())
            .map(|album| (*album).clone());
    }

    if !library.albums.is_empty() {
        let mut album_index = (seed as usize) % library.albums.len();
        if explore_first_id.as_ref() == Some(&library.albums[album_index].id) {
            album_index = (album_index + 1) % library.albums.len();
        }
        if explore_first_id.as_ref() != Some(&library.albums[album_index].id) {
            return library.albums.get(album_index).cloned();
        }
    }

    library
        .home_sections
        .iter()
        .find(|section| section.kind == HomeSectionKind::Explore)
        .and_then(|section| section.albums.first())
        .cloned()
}

fn home_showcase_facts(album: &Album) -> String {
    let mut parts = Vec::new();
    if album.year > 0 {
        parts.push(album.year.to_string());
    }
    if album.track_count > 0 {
        parts.push(track_count_text(album.track_count.into()));
    }
    if album.duration_seconds > 0 {
        parts.push(format_duration(album.duration_seconds));
    }
    parts.join(" • ")
}

impl Shell {
    pub(super) fn home_view(self: &Rc<Self>) -> gtk::Widget {
        let scroller = gtk::ScrolledWindow::new();
        mark_route_scroll_owner(&scroller);
        configure_fill_width_clip(&scroller, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.add_css_class("route-content");
        content.set_margin_top(ROUTE_TOP_MARGIN);
        content.set_margin_bottom(36);
        content.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        content.set_margin_end(0);

        let blocks = self.state.settings.borrow().home_blocks.clone();
        let library = self.state.library.borrow();
        let mut appended = false;
        for block in blocks {
            let child = match block {
                HomeBlockKind::Showcase => {
                    self.home_showcase_block(&library, self.state.home_showcase_seed.get())
                }
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

    fn home_showcase_block(
        self: &Rc<Self>,
        library: &LibrarySnapshot,
        seed: u64,
    ) -> Option<gtk::Widget> {
        let album = showcase_album(library, seed)?;

        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        body.add_css_class("home-showcase");
        add_album_seed_gradient_class(&body, album.color_seed);
        body.set_hexpand(true);
        body.set_valign(gtk::Align::Start);
        body.set_margin_end(DETAIL_GRADIENT_MARGIN_END);
        let cover = album_cover_tile(self, &album, 196, Some(&self.controller));
        cover.add_css_class("home-showcase-cover");
        let facts = gtk::Label::new(Some(&home_showcase_facts(&album)));
        facts.add_css_class("muted");
        facts.add_css_class("home-showcase-facts");
        facts.set_xalign(0.0);
        facts.set_halign(gtk::Align::Start);
        facts.set_justify(gtk::Justification::Left);
        facts.set_wrap(true);
        facts.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        facts.set_width_chars(1);
        facts.set_max_width_chars(22);
        let cover_column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        cover_column.set_width_request(196);
        cover_column.set_halign(gtk::Align::Start);
        cover_column.append(&cover);
        body.append(&cover_column);

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
        add_card_label_link(
            self,
            artist.upcast_ref(),
            &artist,
            &album.artist,
            album_artist_route(&album),
        );
        metadata.append(&artist);
        metadata.append(&facts);

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
            "{} • {}",
            album_count_text(genre.album_count.into()),
            track_count_text(genre.track_count.into())
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

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.set_hexpand(true);
        header.set_halign(gtk::Align::Fill);
        header.set_width_request(1);
        let heading = gtk::Label::new(Some(&tr(section_data.kind.title())));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        heading.set_hexpand(true);
        header.append(&heading);

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        controls.set_margin_end(DETAIL_GRADIENT_MARGIN_END);
        let previous = icon_button("go-previous-symbolic", "Previous page");
        let next = icon_button("go-next-symbolic", "Next page");
        let refresh = icon_button("view-refresh-symbolic", "Refresh section");
        next.add_css_class("home-section-control-button");
        refresh.add_css_class("home-section-control-button");
        controls.append(&previous);
        controls.append(&next);
        controls.append(&refresh);
        header.append(&controls);
        section.append(&header);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, HOME_ALBUM_GAP);
        row.add_css_class("album-strip");
        row.set_hexpand(true);
        section.append(&row);

        let shell = Rc::clone(self);
        previous.connect_clicked(move |_| {
            shell.show_previous_home_section_page(section_kind);
        });

        let shell = Rc::clone(self);
        next.connect_clicked(move |_| {
            shell.show_next_home_section_page(section_kind);
        });

        let shell = Rc::clone(self);
        refresh.connect_clicked(move |_| {
            shell.refresh_home_section(section_kind);
        });

        self.register_home_section_view(section_kind, &section, &row, &previous, &next);
        render_home_album_page(
            self,
            &row,
            &previous,
            &next,
            section_kind,
            &section_data.albums,
        );
        section.upcast()
    }

    fn home_track_section(self: &Rc<Self>, section_data: &HomeSection) -> gtk::Widget {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);
        let section_kind = section_data.kind;

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.set_hexpand(true);
        header.set_halign(gtk::Align::Fill);
        header.set_width_request(1);
        let heading = gtk::Label::new(Some(&tr(section_data.kind.title())));
        heading.add_css_class("section-heading");
        heading.set_xalign(0.0);
        heading.set_hexpand(true);
        header.append(&heading);

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        controls.set_margin_end(DETAIL_GRADIENT_MARGIN_END);
        let previous = icon_button("go-previous-symbolic", "Previous page");
        let next = icon_button("go-next-symbolic", "Next page");
        let refresh = icon_button("view-refresh-symbolic", "Refresh section");
        next.add_css_class("home-section-control-button");
        refresh.add_css_class("home-section-control-button");
        controls.append(&previous);
        controls.append(&next);
        controls.append(&refresh);
        header.append(&controls);
        section.append(&header);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, HOME_ALBUM_GAP);
        row.add_css_class("album-strip");
        row.set_hexpand(true);
        section.append(&row);

        let shell = Rc::clone(self);
        previous.connect_clicked(move |_| {
            shell.show_previous_home_section_page(section_kind);
        });

        let shell = Rc::clone(self);
        next.connect_clicked(move |_| {
            shell.show_next_home_section_page(section_kind);
        });

        let shell = Rc::clone(self);
        refresh.connect_clicked(move |_| {
            shell.refresh_home_section(section_kind);
        });

        self.register_home_section_view(section_kind, &section, &row, &previous, &next);
        render_home_track_page(
            self,
            &row,
            &previous,
            &next,
            section_kind,
            &section_data.tracks,
        );
        section.upcast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::LocalAccessStatus;
    use domain::AlbumId;
    use source::SearchResults;

    #[test]
    fn home_use_candidate() {
        let library = snapshot_with_albums(vec![album(1), album(2), album(3)]);

        let first = showcase_album(&library, 0).expect("first showcase album");
        let second = showcase_album(&library, 1).expect("second showcase album");

        assert_eq!(first.id, AlbumId::fake(1));
        assert_eq!(second.id, AlbumId::fake(2));
    }

    #[test]
    fn home_showcase_possible() {
        let mut library = snapshot_with_albums(vec![album(1), album(2)]);
        library.home_sections = vec![HomeSection {
            kind: HomeSectionKind::Explore,
            albums: vec![album(1)],
            tracks: Vec::new(),
        }];

        let selected = showcase_album(&library, 0).expect("showcase album");

        assert_eq!(selected.id, AlbumId::fake(2));
    }

    fn snapshot_with_albums(albums: Vec<Album>) -> LibrarySnapshot {
        LibrarySnapshot {
            server: None,
            servers: Vec::new(),
            selected_source: None,
            local_folders: Vec::new(),
            server_local_access: Vec::new(),
            local_access: None,
            local_access_status: LocalAccessStatus::default(),
            music_folders: Vec::new(),
            selected_music_folder_id: None,
            username: None,
            first_run: false,
            sync_status: String::new(),
            last_error: None,
            cached_album_count: albums.len(),
            cached_track_count: 0,
            cached_artist_count: 0,
            cached_album_artist_count: 0,
            cached_genre_count: 0,
            cached_playlist_count: 0,
            home_sections: Vec::new(),
            prefetched_explore: None,
            albums,
            tracks: Vec::new(),
            artists: Vec::new(),
            album_artists: Vec::new(),
            genres: Vec::new(),
            playlists: Vec::new(),
            playlist_entry_keys: std::collections::HashMap::new(),
            favorites: Vec::new(),
            search: SearchResults::default(),
        }
    }

    fn album(number: u32) -> Album {
        Album {
            id: AlbumId::fake(number),
            title: format!("Album {number}"),
            artist: "Artist".to_string(),
            artist_id: None,
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 1,
            duration_seconds: 60,
            favorite: false,
            color_seed: number,
            image_ref: None,
            genres: Vec::new(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
        }
    }
}

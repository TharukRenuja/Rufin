use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use ::library::{Album, Artist, Track};
use adw::prelude::*;
use artwork::ArtworkBinding;
use tracing::warn;

use crate::interactions::RADIO_ICON;
use crate::interactions::add_widget_click;
use crate::layout::width_allocation_owner;
use crate::localization::bind_label_text_with;
use crate::shell::Shell;
use crate::shell::actions::{ActionButtonVariant, configure_action_button, icon_button};
use crate::shell::actions::{PLAY_LATER_ICON, PLAY_NEXT_ICON, REMOVE_ICON};
use crate::shell::cover::cover_fetch_size_for_display;
use crate::shell::cover::presentation::add_album_seed_gradient_class;
use crate::shell::cover::{ArtworkTile, CoverGroupProjection};
use localization::{msgid, tr};

use super::detail_links::{DetailEntityKind, DetailExternalLink, server_entity_link};
use super::route_layout::detail_showcase_cover_only;

const DETAIL_HEADER_SPACING: i32 = 18;

pub(crate) struct MediaDetailShowcase {
    pub(crate) route_class: &'static str,
    pub(crate) seed: u32,
    pub(crate) initial_width: i32,
    pub(crate) cover: DetailCoverProjection,
    pub(crate) external_links: Option<gtk::Widget>,
    pub(crate) external_links_class: Option<&'static str>,
    pub(crate) text_stack: gtk::Widget,
    pub(crate) actions: gtk::Widget,
}

pub(crate) struct CollectionDetailShowcase {
    pub(crate) seed: u32,
    pub(crate) initial_width: i32,
    pub(crate) compact_spacing: i32,
    pub(crate) wide_spacing: i32,
    pub(crate) cover: CoverGroupProjection,
    pub(crate) metadata: Vec<gtk::Widget>,
}

pub(crate) struct PlaylistDetailShowcase {
    pub(crate) seed: u32,
    pub(crate) initial_width: i32,
    pub(crate) cover: CoverGroupProjection,
    pub(crate) kind_row: gtk::Widget,
    pub(crate) title: gtk::Widget,
    pub(crate) summary: gtk::Widget,
    pub(crate) actions: gtk::Widget,
}

pub(crate) fn media_detail_showcase(shell: &Rc<Shell>, config: MediaDetailShowcase) -> gtk::Widget {
    let header = gtk::Box::new(gtk::Orientation::Vertical, 12);
    header.add_css_class("detail-showcase");
    header.add_css_class(config.route_class);
    header.add_css_class("detail-showcase-horizontal");
    add_album_seed_gradient_class(&header, config.seed);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, DETAIL_HEADER_SPACING);
    body.set_hexpand(true);
    body.set_halign(gtk::Align::Fill);
    body.set_width_request(1);

    let cover_column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    cover_column.set_halign(gtk::Align::Start);
    cover_column.append(&config.cover.button());

    let link_stack = gtk::Box::new(gtk::Orientation::Vertical, 6);
    if let Some(class) = config.external_links_class {
        link_stack.add_css_class(class);
    }
    link_stack.set_halign(gtk::Align::Center);
    if let Some(external_links) = config.external_links {
        external_links.set_halign(gtk::Align::Center);
        link_stack.append(&external_links);
    }
    if link_stack.first_child().is_some() {
        cover_column.append(&link_stack);
    }
    body.append(&cover_column);

    let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
    metadata.set_hexpand(true);
    metadata.set_valign(gtk::Align::Start);
    metadata.set_halign(gtk::Align::Fill);
    metadata.set_width_request(1);
    metadata.append(&config.text_stack);
    metadata.append(&config.actions);
    body.append(&metadata);

    header.append(&body);
    let presentation = MediaShowcasePresentation {
        viewport_width: Rc::new(Cell::new(0)),
        cover_width: Rc::new(Cell::new(0)),
        header: header.clone(),
        cover_column,
        cover: config.cover,
        link_stack,
        metadata,
    };
    presentation.apply_viewport_width(config.initial_width);
    presentation.resize_cover(config.initial_width);
    let frame = detail_showcase_frame_with_back(shell, header.upcast());
    width_allocation_owner(&frame, move |width| {
        presentation.apply_viewport_width(width);
        presentation.resize_cover(width);
    })
    .upcast()
}

#[derive(Clone)]
struct MediaShowcasePresentation {
    viewport_width: Rc<Cell<i32>>,
    cover_width: Rc<Cell<i32>>,
    header: gtk::Box,
    cover_column: gtk::Box,
    cover: DetailCoverProjection,
    link_stack: gtk::Box,
    metadata: gtk::Box,
}

impl MediaShowcasePresentation {
    fn apply_viewport_width(&self, width: i32) {
        if width <= 1 || self.viewport_width.replace(width) == width {
            return;
        }
        let cover_only = detail_showcase_cover_only(width);
        update_tiny_detail_showcase(&self.header, width);
        self.link_stack.set_visible(!cover_only);
        self.metadata.set_visible(!cover_only);
    }

    fn resize_cover(&self, width: i32) {
        if width <= 1 || self.cover_width.replace(width) == width {
            return;
        }
        let cover_size = super::route_layout::detail_showcase_cover_size(width);
        self.cover_column.set_width_request(cover_size);
        self.cover.resize(cover_size);
    }
}

pub(crate) fn collection_detail_showcase(
    shell: &Rc<Shell>,
    config: CollectionDetailShowcase,
) -> gtk::Widget {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, config.wide_spacing);
    header.add_css_class("playlist-detail-showcase");
    add_album_seed_gradient_class(&header, config.seed);
    header.set_hexpand(true);
    header.set_halign(gtk::Align::Fill);
    header.set_width_request(1);
    header.append(&config.cover.widget());

    let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
    metadata.set_valign(gtk::Align::Center);
    metadata.set_hexpand(true);
    metadata.set_halign(gtk::Align::Fill);
    metadata.set_width_request(1);
    for child in config.metadata {
        metadata.append(&child);
    }
    header.append(&metadata);

    let presentation = CollectionShowcasePresentation {
        viewport_width: Rc::new(Cell::new(0)),
        cover_width: Rc::new(Cell::new(0)),
        header: header.clone(),
        cover: config.cover,
        metadata,
        compact_spacing: config.compact_spacing,
        wide_spacing: config.wide_spacing,
    };
    presentation.apply_viewport_width(config.initial_width);
    presentation.resize_cover(config.initial_width);
    let frame = detail_showcase_frame_with_back(shell, header.upcast());
    width_allocation_owner(&frame, move |width| {
        presentation.apply_viewport_width(width);
        presentation.resize_cover(width);
    })
    .upcast()
}

#[derive(Clone)]
struct CollectionShowcasePresentation {
    viewport_width: Rc<Cell<i32>>,
    cover_width: Rc<Cell<i32>>,
    header: gtk::Box,
    cover: CoverGroupProjection,
    metadata: gtk::Box,
    compact_spacing: i32,
    wide_spacing: i32,
}

impl CollectionShowcasePresentation {
    fn apply_viewport_width(&self, width: i32) {
        if width <= 1 || self.viewport_width.replace(width) == width {
            return;
        }
        let cover_only = detail_showcase_cover_only(width);
        update_tiny_detail_showcase(&self.header, width);
        self.header.set_spacing(
            if super::playlist_detail::playlist_detail_compact_for_width(width) {
                self.compact_spacing
            } else {
                self.wide_spacing
            },
        );
        self.metadata.set_visible(!cover_only);
    }

    fn resize_cover(&self, width: i32) {
        if width <= 1 || self.cover_width.replace(width) == width {
            return;
        }
        let cover_size = super::playlist_detail::playlist_cover_size(width);
        self.cover.resize(cover_size);
    }
}

pub(crate) fn playlist_detail_showcase(
    shell: &Rc<Shell>,
    config: PlaylistDetailShowcase,
) -> gtk::Widget {
    collection_detail_showcase(
        shell,
        CollectionDetailShowcase {
            seed: config.seed,
            initial_width: config.initial_width,
            compact_spacing: 20,
            wide_spacing: 28,
            cover: config.cover,
            metadata: vec![
                config.kind_row,
                config.title,
                config.summary,
                config.actions,
            ],
        },
    )
}

#[derive(Clone)]
pub(crate) struct DetailSummaryProjection {
    root: gtk::Box,
    items: Rc<Vec<(gtk::Box, gtk::Image, gtk::Label)>>,
}

impl DetailSummaryProjection {
    pub(crate) fn new(items: &[(&str, String)]) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.add_css_class("detail-summary-row");
        root.set_halign(gtk::Align::Start);
        let slots = Rc::new(
            (0..3)
                .map(|_| {
                    let item = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                    let icon = gtk::Image::new();
                    icon.add_css_class("muted");
                    icon.set_pixel_size(14);
                    item.append(&icon);
                    let label = gtk::Label::new(None);
                    label.add_css_class("muted");
                    label.set_xalign(0.0);
                    item.append(&label);
                    root.append(&item);
                    (item, icon, label)
                })
                .collect::<Vec<_>>(),
        );
        let projection = Self { root, items: slots };
        projection.replace(items);
        projection
    }

    pub(crate) fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub(crate) fn replace(&self, values: &[(&str, String)]) {
        for (index, (item, icon, label)) in self.items.iter().enumerate() {
            if let Some((icon_name, text)) = values.get(index) {
                icon.set_icon_name(Some(icon_name));
                label.set_text(text);
                item.set_visible(true);
            } else {
                item.set_visible(false);
            }
        }
    }

    pub(crate) fn bind_text_with(&self, index: usize, text: impl Fn() -> String + 'static) {
        if let Some((_, _, label)) = self.items.get(index) {
            bind_label_text_with(label, text);
        }
    }
}

pub(crate) fn detail_action_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = icon_button(icon_name, label);
    configure_action_button(&button, ActionButtonVariant::DetailAction, Some(icon_name));
    button
}

pub(crate) fn detail_primary_action_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = icon_button(icon_name, label);
    configure_action_button(&button, ActionButtonVariant::DetailPrimary, Some(icon_name));
    button
}

pub(crate) fn detail_radio_button() -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("detail-kind-radio-button");
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some(&tr("Play radio")));
    let icon = gtk::Image::from_icon_name(RADIO_ICON);
    icon.set_pixel_size(16);
    button.set_child(Some(&icon));
    button
}

pub(crate) fn append_track_query_batch_queue_actions(
    actions: &gtk::Box,
    controller: &playback::QueueHandle,
    tracks: Rc<dyn Fn() -> Vec<Track>>,
) {
    let play_next = detail_action_button(PLAY_NEXT_ICON, "Next");
    let next_controller = controller.clone();
    let next_tracks = Rc::clone(&tracks);
    play_next.connect_clicked(move |_| {
        for track in next_tracks().into_iter().rev() {
            next_controller.play_next(track);
        }
    });
    actions.append(&play_next);

    let play_later = detail_action_button(PLAY_LATER_ICON, "Play Later");
    let later_controller = controller.clone();
    play_later.connect_clicked(move |_| later_controller.play_last(tracks()));
    actions.append(&play_later);
}

pub(crate) fn detail_title_label(text: &str) -> gtk::Label {
    let title = gtk::Label::new(Some(text));
    title.add_css_class("detail-title");
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_width_request(1);
    title.set_width_chars(1);
    title.set_max_width_chars(32);
    title
}

pub(crate) fn fitted_detail_title_label(text: &str) -> gtk::Label {
    let title = detail_title_label(text);
    title.set_justify(gtk::Justification::Left);
    fit_detail_text(&title, text);
    title
}

pub(crate) fn detail_genre_pill_button(label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("album-detail-genre-pill");
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Center);
    button.set_hexpand(false);
    button.set_tooltip_text(Some(label));
    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_halign(gtk::Align::Start);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.set_width_chars(1);
    text.set_max_width_chars(28);
    button.set_child(Some(&text));
    button
}

pub(crate) fn detail_delete_button(label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some(&tr(label)));
    button.set_child(Some(&gtk::Image::from_icon_name(REMOVE_ICON)));
    configure_action_button(
        &button,
        ActionButtonVariant::DetailAction,
        Some(REMOVE_ICON),
    );
    button
}

pub(crate) fn detail_action_row() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("detail-showcase-actions");
    row.set_halign(gtk::Align::Center);
    row
}

#[derive(Clone)]
pub(crate) struct DetailCoverProjection {
    button: gtk::Button,
    tile: ArtworkTile,
    candidates: Rc<RefCell<ArtworkBinding>>,
    seed: Rc<Cell<u32>>,
    size: Rc<Cell<i32>>,
}

impl DetailCoverProjection {
    pub(crate) fn button(&self) -> gtk::Button {
        self.button.clone()
    }

    pub(crate) fn replace(&self, shell: &Rc<Shell>, candidates: ArtworkBinding, seed: u32) {
        *self.candidates.borrow_mut() = candidates.clone();
        self.seed.set(seed);
        let size = self.size.get();
        shell.bind_artwork_tile(
            &self.tile,
            candidates,
            seed,
            size,
            cover_fetch_size_for_display(size),
        );
    }

    pub(crate) fn resize(&self, size: i32) {
        let size = size.max(1);
        if self.size.replace(size) == size {
            return;
        }
        self.button.set_size_request(size, size);
        self.tile.set_square_size(size);
    }
}

pub(crate) fn detail_cover_projection(
    shell: &Rc<Shell>,
    candidates: ArtworkBinding,
    seed: u32,
    size: i32,
    fetch_size: u32,
    cover_class: &str,
) -> DetailCoverProjection {
    let tile = ArtworkTile::new_sized(size, size, seed);
    shell.bind_artwork_tile(&tile, candidates.clone(), seed, size, fetch_size);
    let cover = tile.widget();
    cover.add_css_class("detail-showcase-cover");
    cover.add_css_class(cover_class);

    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("detail-cover-button");
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Start);
    button.set_cursor_from_name(Some("pointer"));
    button.set_child(Some(&cover));

    let candidates = Rc::new(RefCell::new(candidates));
    let seed = Rc::new(Cell::new(seed));
    let open_candidates = Rc::clone(&candidates);
    let open_seed = Rc::clone(&seed);
    let shell = Rc::clone(shell);
    button.connect_clicked(move |_| {
        shell.present_full_artwork(open_candidates.borrow().clone(), open_seed.get());
    });
    DetailCoverProjection {
        button,
        tile,
        candidates,
        seed,
        size: Rc::new(Cell::new(size)),
    }
}

impl Shell {
    fn present_full_artwork(self: &Rc<Self>, candidates: ArtworkBinding, seed: u32) {
        let size = full_artwork_size(self.chrome.window.width(), self.chrome.window.height());
        let fetch_size = cover_fetch_size_for_display(size);
        let tile = ArtworkTile::new_sized(size, size, seed);
        let cover = tile.widget();
        self.bind_artwork_tile(&tile, candidates, seed, size, fetch_size);
        cover.add_css_class("full-artwork-cover");
        cover.set_halign(gtk::Align::Center);
        cover.set_valign(gtk::Align::Center);

        let root = gtk::Overlay::new();
        root.add_css_class("full-artwork-window");
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_child(Some(&cover));

        self.chrome.app_root_overlay.add_overlay(&root);
        self.chrome
            .app_root_overlay
            .set_measure_overlay(&root, false);

        let overlay = self.chrome.app_root_overlay.downgrade();
        let root_for_close = root.downgrade();
        let tile_for_close = tile.downgrade();
        let shell_for_close = Rc::downgrade(self);
        add_widget_click(root.upcast_ref(), move || {
            if let (Some(shell), Some(tile)) = (shell_for_close.upgrade(), tile_for_close.upgrade())
            {
                shell.clear_artwork_tile(&tile);
            }
            if let (Some(overlay), Some(root)) = (overlay.upgrade(), root_for_close.upgrade()) {
                overlay.remove_overlay(&root);
            }
        });
    }
}

fn full_artwork_size(width: i32, height: i32) -> i32 {
    (width.min(height) - 80).clamp(240, 720)
}

pub(crate) fn detail_showcase_frame(header: gtk::Widget) -> gtk::Widget {
    header.set_hexpand(true);
    header.set_halign(gtk::Align::Fill);
    header.set_width_request(1);
    header
}

pub(crate) fn detail_showcase_frame_with_back(
    shell: &Rc<Shell>,
    header: gtk::Widget,
) -> gtk::Widget {
    let frame = detail_showcase_frame(header);
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_halign(gtk::Align::Fill);
    overlay.set_width_request(1);
    overlay.set_child(Some(&frame));

    let back = icon_button("go-previous-symbolic", "Back");
    back.add_css_class("detail-back-button");
    back.set_halign(gtk::Align::Start);
    back.set_valign(gtk::Align::Start);
    back.set_margin_top(1);
    back.set_margin_start(4);
    back.set_sensitive(shell.navigation.routes.borrow().can_back());
    {
        let shell = Rc::clone(shell);
        back.connect_clicked(move |_| shell.go_back());
    }
    overlay.add_overlay(&back);
    overlay.set_measure_overlay(&back, false);
    overlay.upcast()
}

pub(crate) fn mark_tiny_detail_showcase(widget: &impl IsA<gtk::Widget>, width: i32) {
    update_tiny_detail_showcase(widget, width);
}

fn update_tiny_detail_showcase(widget: &impl IsA<gtk::Widget>, width: i32) {
    if width < 520 {
        widget.add_css_class("detail-showcase-tiny");
    } else {
        widget.remove_css_class("detail-showcase-tiny");
    }
    if detail_showcase_cover_only(width) {
        widget.add_css_class("detail-showcase-cover-only");
    } else {
        widget.remove_css_class("detail-showcase-cover-only");
    }
}

pub(crate) fn fit_detail_text(label: &gtk::Label, text: &str) {
    let count = text.chars().count();
    if count >= 42 {
        label.add_css_class("detail-text-very-long");
    } else if count >= 24 {
        label.add_css_class("detail-text-long");
    }
}

pub(crate) fn album_external_links(shell: &Rc<Shell>, album: &Album) -> Option<gtk::Widget> {
    let settings = shell.settings.current.borrow();
    let link_settings = &settings.external_site_links;
    if !settings.allows_external_site_links() {
        return None;
    }

    let row = detail_external_link_row();
    if link_settings.lastfm
        && let Some(url) = lastfm_album_url(&album.artist, &album.title)
    {
        row.append(&detail_external_link_button(
            shell,
            "io.github.screwys.Rufin.external.lastfm",
            msgid("Open on Last.fm"),
            url,
        ));
    }
    if link_settings.musicbrainz
        && let Some(url) = musicbrainz_album_url(album)
    {
        row.append(&detail_external_link_button(
            shell,
            "io.github.screwys.Rufin.external.musicbrainz",
            msgid("Open on MusicBrainz"),
            url,
        ));
    }
    if link_settings.server
        && let Some(link) = server_entity_url(shell, DetailEntityKind::Album, album.id.as_str())
    {
        row.append(&detail_external_link_button(
            shell,
            link.icon_name,
            link.label,
            link.url,
        ));
    }

    row.first_child().is_some().then(|| row.upcast())
}

pub(crate) fn artist_external_links(
    shell: &Rc<Shell>,
    artist: &Artist,
    tracks: &[Track],
) -> Option<gtk::Widget> {
    let settings = shell.settings.current.borrow();
    let link_settings = &settings.external_site_links;
    if !settings.allows_external_site_links() {
        return None;
    }

    let row = detail_external_link_row();
    if link_settings.lastfm
        && let Some(url) = lastfm_artist_url(&artist.name)
    {
        row.append(&detail_external_link_button(
            shell,
            "io.github.screwys.Rufin.external.lastfm",
            msgid("Open on Last.fm"),
            url,
        ));
    }
    if link_settings.musicbrainz
        && let Some(url) = musicbrainz_artist_url(artist, tracks)
    {
        row.append(&detail_external_link_button(
            shell,
            "io.github.screwys.Rufin.external.musicbrainz",
            msgid("Open on MusicBrainz"),
            url,
        ));
    }
    if link_settings.server
        && let Some(link) = server_entity_url(shell, DetailEntityKind::Artist, artist.id.as_str())
    {
        row.append(&detail_external_link_button(
            shell,
            link.icon_name,
            link.label,
            link.url,
        ));
    }

    row.first_child().is_some().then(|| row.upcast())
}

fn detail_external_link_row() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.add_css_class("detail-external-link-row");
    row
}

fn detail_external_link_button(
    shell: &Rc<Shell>,
    icon_name: &str,
    label: &str,
    url: String,
) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("icon-button");
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.add_css_class("detail-external-link-button");
    button.set_tooltip_text(Some(&tr(label)));
    let image = gtk::Image::from_icon_name(icon_name);
    image.set_pixel_size(18);
    button.set_child(Some(&image));
    let window = shell.chrome.window.clone();
    button.connect_clicked(move |_| {
        let launcher = gtk::UriLauncher::new(&url);
        let window = window.clone();
        gtk::glib::spawn_future_local(async move {
            if let Err(error) = launcher.launch_future(Some(&window)).await {
                warn!(%error, "failed to open external detail link");
            }
        });
    });
    button
}

fn lastfm_album_url(artist: &str, album: &str) -> Option<String> {
    let artist = clean_url_label(artist)?;
    let album = clean_url_label(album)?;
    Some(format!(
        "https://www.last.fm/music/{}/{}",
        percent_encode_path_segment(artist),
        percent_encode_path_segment(album)
    ))
}

fn lastfm_artist_url(artist: &str) -> Option<String> {
    let artist = clean_url_label(artist)?;
    Some(format!(
        "https://www.last.fm/music/{}",
        percent_encode_path_segment(artist)
    ))
}

fn musicbrainz_album_url(album: &Album) -> Option<String> {
    if let Some(group_id) = album
        .musicbrainz_release_group_id
        .as_deref()
        .and_then(clean_url_label)
    {
        return Some(format!("https://musicbrainz.org/release-group/{group_id}"));
    }
    let release_id = album
        .musicbrainz_album_id
        .as_deref()
        .and_then(clean_url_label)?;
    Some(format!("https://musicbrainz.org/release/{release_id}"))
}

fn musicbrainz_artist_url(artist: &Artist, tracks: &[Track]) -> Option<String> {
    let artist_id = tracks
        .iter()
        .flat_map(|track| {
            track
                .artist_credits
                .iter()
                .chain(track.album_artist_credits.iter())
        })
        .find(|credit| {
            credit.id == artist.id || credit.name.eq_ignore_ascii_case(artist.name.as_str())
        })
        .and_then(|credit| credit.musicbrainz_artist_id.as_deref())
        .and_then(clean_url_label)?;
    Some(format!("https://musicbrainz.org/artist/{artist_id}"))
}

fn server_entity_url(
    shell: &Shell,
    kind: DetailEntityKind,
    entity_id: &str,
) -> Option<DetailExternalLink> {
    let library = shell.source.presentation.borrow();
    let server = library.source.as_ref()?;
    server_entity_link(server, kind, entity_id)
}

fn clean_url_label(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{byte:02X}"));
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use ::library::{Album, AlbumId};

    use super::{full_artwork_size, lastfm_album_url, lastfm_artist_url, musicbrainz_album_url};

    #[test]
    fn full_artwork_size_fits_window() {
        assert_eq!(full_artwork_size(1440, 900), 720);
        assert_eq!(full_artwork_size(640, 480), 400);
        assert_eq!(full_artwork_size(300, 260), 240);
    }

    #[test]
    fn lastfm_urls_escape_path_segments() {
        assert_eq!(
            lastfm_album_url("Test Artist", "A/B").as_deref(),
            Some("https://www.last.fm/music/Test%20Artist/A%2FB")
        );
        assert_eq!(
            lastfm_artist_url("青葉市子").as_deref(),
            Some("https://www.last.fm/music/%E9%9D%92%E8%91%89%E5%B8%82%E5%AD%90")
        );
    }

    #[test]
    fn musicbrainz_album_url_prefers_release_group() {
        let mut album = Album {
            id: AlbumId::fake(1),
            title: "Album".to_string(),
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
            color_seed: 1,
            image_ref: None,
            genres: Vec::new(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: Some("release-one".to_string()),
            musicbrainz_release_group_id: Some("group-one".to_string()),
        };

        assert_eq!(
            musicbrainz_album_url(&album).as_deref(),
            Some("https://musicbrainz.org/release-group/group-one")
        );

        album.musicbrainz_release_group_id = None;
        assert_eq!(
            musicbrainz_album_url(&album).as_deref(),
            Some("https://musicbrainz.org/release/release-one")
        );
    }
}

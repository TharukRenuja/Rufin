use super::*;

use crate::i18n::msgid;
use crate::ui::root::detail_links::{DetailEntityKind, DetailExternalLink, server_entity_link};

const DETAIL_HEADER_SPACING: i32 = 18;

pub(in crate::ui) struct MediaDetailShowcase {
    pub(in crate::ui) route_class: &'static str,
    pub(in crate::ui) seed: u32,
    pub(in crate::ui) content_width: i32,
    pub(in crate::ui) cover_size: i32,
    pub(in crate::ui) cover: gtk::Widget,
    pub(in crate::ui) external_links: Option<gtk::Widget>,
    pub(in crate::ui) external_links_class: Option<&'static str>,
    pub(in crate::ui) text_stack: gtk::Widget,
    pub(in crate::ui) actions: gtk::Widget,
}

pub(in crate::ui) struct CollectionDetailShowcase {
    pub(in crate::ui) seed: u32,
    pub(in crate::ui) content_width: i32,
    pub(in crate::ui) orientation: gtk::Orientation,
    pub(in crate::ui) spacing: i32,
    pub(in crate::ui) cover: gtk::Widget,
    pub(in crate::ui) metadata: Vec<gtk::Widget>,
}

pub(in crate::ui) struct PlaylistDetailShowcase {
    pub(in crate::ui) seed: u32,
    pub(in crate::ui) content_width: i32,
    pub(in crate::ui) compact: bool,
    pub(in crate::ui) cover: gtk::Widget,
    pub(in crate::ui) kind_row: gtk::Widget,
    pub(in crate::ui) title: gtk::Widget,
    pub(in crate::ui) summary: gtk::Widget,
    pub(in crate::ui) actions: gtk::Widget,
}

pub(in crate::ui) fn media_detail_showcase(
    shell: &Rc<Shell>,
    config: MediaDetailShowcase,
) -> gtk::Widget {
    let cover_only = detail_showcase_cover_only(config.content_width);
    let header = gtk::Box::new(gtk::Orientation::Vertical, 12);
    header.add_css_class("detail-showcase");
    header.add_css_class(config.route_class);
    header.add_css_class("detail-showcase-horizontal");
    mark_tiny_detail_showcase(&header, config.content_width);
    add_album_seed_gradient_class(&header, config.seed);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, DETAIL_HEADER_SPACING);
    body.set_hexpand(true);
    body.set_halign(gtk::Align::Fill);
    body.set_width_request(1);

    let cover_column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    cover_column.set_halign(gtk::Align::Start);
    cover_column.set_width_request(config.cover_size);
    cover_column.append(&config.cover);

    let link_stack = gtk::Box::new(gtk::Orientation::Vertical, 6);
    if let Some(class) = config.external_links_class {
        link_stack.add_css_class(class);
    }
    link_stack.set_halign(gtk::Align::Center);
    link_stack.set_visible(!cover_only);
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
    metadata.set_visible(!cover_only);
    metadata.append(&config.text_stack);
    metadata.append(&config.actions);
    body.append(&metadata);

    header.append(&body);
    detail_showcase_frame_with_back(shell, header.upcast())
}

pub(in crate::ui) fn collection_detail_showcase(
    shell: &Rc<Shell>,
    config: CollectionDetailShowcase,
) -> gtk::Widget {
    let cover_only = detail_showcase_cover_only(config.content_width);
    let header = gtk::Box::new(config.orientation, config.spacing);
    header.add_css_class("playlist-detail-showcase");
    mark_tiny_detail_showcase(&header, config.content_width);
    add_album_seed_gradient_class(&header, config.seed);
    header.set_hexpand(true);
    header.set_halign(gtk::Align::Fill);
    header.set_width_request(1);
    header.append(&config.cover);

    let metadata = gtk::Box::new(gtk::Orientation::Vertical, 10);
    metadata.set_valign(gtk::Align::Center);
    metadata.set_hexpand(true);
    metadata.set_halign(gtk::Align::Fill);
    metadata.set_width_request(1);
    metadata.set_visible(!cover_only);
    for child in config.metadata {
        metadata.append(&child);
    }
    header.append(&metadata);

    detail_showcase_frame_with_back(shell, header.upcast())
}

pub(in crate::ui) fn playlist_detail_showcase(
    shell: &Rc<Shell>,
    config: PlaylistDetailShowcase,
) -> gtk::Widget {
    collection_detail_showcase(
        shell,
        CollectionDetailShowcase {
            seed: config.seed,
            content_width: config.content_width,
            orientation: gtk::Orientation::Horizontal,
            spacing: if config.compact { 20 } else { 28 },
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

pub(in crate::ui) fn detail_summary_row(items: &[(&str, String)]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("detail-summary-row");
    row.set_halign(gtk::Align::Start);
    for (icon_name, text) in items {
        let item = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.add_css_class("muted");
        icon.set_pixel_size(14);
        item.append(&icon);

        let label = gtk::Label::new(Some(text));
        label.add_css_class("muted");
        label.set_xalign(0.0);
        item.append(&label);
        row.append(&item);
    }
    row
}

pub(in crate::ui) fn detail_action_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = icon_button(icon_name, label);
    configure_action_button(&button, ActionButtonVariant::DetailAction, Some(icon_name));
    button
}

pub(in crate::ui) fn detail_primary_action_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = icon_button(icon_name, label);
    configure_action_button(&button, ActionButtonVariant::DetailPrimary, Some(icon_name));
    button
}

pub(in crate::ui) fn detail_radio_button() -> gtk::Button {
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

pub(in crate::ui) fn append_track_batch_queue_actions(
    actions: &gtk::Box,
    controller: &AppController,
    tracks: Rc<Vec<Track>>,
) {
    let play_next = detail_action_button(PLAY_NEXT_ICON, "Next");
    let next_controller = controller.clone();
    let next_tracks = Rc::clone(&tracks);
    play_next.connect_clicked(move |_| {
        for track in next_tracks.as_ref().iter().rev() {
            next_controller.play_next(track.clone());
        }
    });
    actions.append(&play_next);

    let play_later = detail_action_button(PLAY_LATER_ICON, "Play Later");
    let later_controller = controller.clone();
    play_later.connect_clicked(move |_| later_controller.play_last(tracks.as_ref().clone()));
    actions.append(&play_later);
}

pub(in crate::ui) fn detail_title_label(text: &str) -> gtk::Label {
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

pub(in crate::ui) fn fitted_detail_title_label(text: &str) -> gtk::Label {
    let title = detail_title_label(text);
    title.set_justify(gtk::Justification::Left);
    fit_detail_text(&title, text);
    title
}

pub(in crate::ui) fn detail_genre_pill_button(label: &str) -> gtk::Button {
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

pub(in crate::ui) fn detail_delete_button(label: &str) -> gtk::Button {
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

pub(in crate::ui) fn detail_action_row() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("detail-showcase-actions");
    row.set_halign(gtk::Align::Center);
    row
}

pub(in crate::ui) fn detail_cover_button(
    shell: &Rc<Shell>,
    image_ref: Option<&ImageRef>,
    seed: u32,
    size: i32,
    fetch_size: u32,
    cover_class: &str,
) -> gtk::Button {
    shell.prime_cached_cover(image_ref, fetch_size, size);
    let cover = shell.cover_tile_for(image_ref, seed, size, fetch_size);
    cover.add_css_class("detail-showcase-cover");
    cover.add_css_class(cover_class);

    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("detail-cover-button");
    button.set_halign(gtk::Align::Start);
    button.set_valign(gtk::Align::Start);
    button.set_cursor_from_name(Some("pointer"));
    button.set_child(Some(&cover));

    let shell = Rc::clone(shell);
    let image_ref = image_ref.cloned();
    button.connect_clicked(move |_| {
        shell.present_full_artwork(image_ref.as_ref(), seed);
    });
    button
}

impl Shell {
    fn present_full_artwork(self: &Rc<Self>, image_ref: Option<&ImageRef>, seed: u32) {
        let size = full_artwork_size(self.window.width(), self.window.height());
        let fetch_size = cover_fetch_size_for_display(size);
        let tile = ArtworkTile::new_sized(size, size, seed);
        let cover = tile.widget();
        self.bind_cover_tile_for_dimensions(
            &tile,
            image_ref,
            seed,
            GRID_COVER_SIZE as i32,
            GRID_COVER_SIZE,
        );
        self.bind_cover_tile_for_dimensions(&tile, image_ref, seed, size, fetch_size);
        cover.add_css_class("full-artwork-cover");
        cover.set_halign(gtk::Align::Center);
        cover.set_valign(gtk::Align::Center);

        let root = gtk::Overlay::new();
        root.add_css_class("full-artwork-window");
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_child(Some(&cover));

        self.app_root_overlay.add_overlay(&root);
        self.app_root_overlay.set_measure_overlay(&root, false);

        let overlay = self.app_root_overlay.clone();
        let root_for_close = root.clone();
        add_widget_click(root.upcast_ref(), move || {
            overlay.remove_overlay(&root_for_close)
        });
    }
}

fn full_artwork_size(width: i32, height: i32) -> i32 {
    (width.min(height) - 80).clamp(240, 720)
}

pub(in crate::ui) fn detail_showcase_frame(header: gtk::Widget) -> gtk::Widget {
    header.set_hexpand(true);
    header.set_halign(gtk::Align::Fill);
    header.set_width_request(1);
    header
}

pub(in crate::ui) fn detail_showcase_frame_with_back(
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
    back.set_sensitive(shell.state.routes.borrow().can_back());
    {
        let shell = Rc::clone(shell);
        back.connect_clicked(move |_| shell.go_back());
    }
    overlay.add_overlay(&back);
    overlay.set_measure_overlay(&back, false);
    overlay.upcast()
}

pub(in crate::ui) fn mark_tiny_detail_showcase(widget: &impl IsA<gtk::Widget>, width: i32) {
    if width < 520 {
        widget.add_css_class("detail-showcase-tiny");
    }
    if detail_showcase_cover_only(width) {
        widget.add_css_class("detail-showcase-cover-only");
    }
}

pub(in crate::ui) fn fit_detail_text(label: &gtk::Label, text: &str) {
    let count = text.chars().count();
    if count >= 42 {
        label.add_css_class("detail-text-very-long");
    } else if count >= 24 {
        label.add_css_class("detail-text-long");
    }
}

pub(in crate::ui) fn album_external_links(shell: &Rc<Shell>, album: &Album) -> Option<gtk::Widget> {
    let settings = shell.state.settings.borrow();
    let link_settings = &settings.external_site_links;
    if !crate::external_activity::external_site_links(&settings) {
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

pub(in crate::ui) fn artist_external_links(
    shell: &Rc<Shell>,
    artist: &Artist,
    tracks: &[Track],
) -> Option<gtk::Widget> {
    let settings = shell.state.settings.borrow();
    let link_settings = &settings.external_site_links;
    if !crate::external_activity::external_site_links(&settings) {
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
    let window = shell.window.clone();
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
    let library = shell.state.library.borrow();
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
    use super::*;

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

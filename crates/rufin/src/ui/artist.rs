use std::rc::Rc;

use ::library::CachedArtistDetail;
use adw::prelude::*;
use domain::{
    Album, AlbumId, Artist, ArtistId, ArtistTrackScope, PlaySourceDescriptor, Route, Track,
};

use crate::i18n::msgid;

use super::release_kind::{AlbumReleaseKind, album_release_kind};
use super::*;

const ARTIST_COUNT_ICON_SIZE: i32 = 16;

impl Shell {
    pub(super) fn artist_detail_view(self: &Rc<Self>, artist_id: ArtistId) -> gtk::Widget {
        let detail = self.artist_detail_data(&artist_id);
        let Some(detail) = detail else {
            let active_server_id = self
                .state
                .library
                .borrow()
                .server
                .as_ref()
                .map(|server| server.id.to_string());
            let queue_server_id = self
                .state
                .queue
                .borrow()
                .as_ref()
                .map(|queue| queue.server_id.to_string());
            let player_server_id = self
                .state
                .player
                .borrow()
                .current_server_id
                .as_ref()
                .map(ToString::to_string);
            warn!(
                artist_id = artist_id.as_str(),
                active_server_id, queue_server_id, player_server_id, "cached artist route missing"
            );
            return self.placeholder_view("Artist", "The selected cached artist was not found.");
        };
        let artist = detail.artist;
        let albums = detail.albums;
        let appears_on = detail.appears_on;
        let tracks = detail.tracks;
        let favorite_tracks = favorite_artist_tracks(&tracks);
        let has_favorite_tracks = !favorite_tracks.is_empty();

        let wrapper = detail_route_wrapper(0);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(ROUTE_TOP_MARGIN);
        content.set_margin_bottom(36);
        content.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        content.set_margin_end(0);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        content.set_width_request(1);

        let track_count = artist
            .track_count
            .max(tracks.len().min(u32::MAX as usize) as u32);
        content.append(&self.artist_detail_header(
            &artist,
            &tracks,
            albums.len(),
            appears_on.len(),
            track_count,
        ));

        if has_favorite_tracks {
            content.append(&section_heading(msgid("Favorite tracks")));
            let favorite_artist_id = artist.id.clone();
            let selected_music_folder_id = selected_music_folder_id(self);
            let source_descriptor = PlaySourceDescriptor::HomeCollection {
                section_id: "artist-favorites".to_string(),
                source: Box::new(PlaySourceDescriptor::ArtistTracks {
                    artist_id: favorite_artist_id,
                    scope: ArtistTrackScope::AllCredits,
                    selected_music_folder_id,
                }),
            };
            content.append(&self.compact_artist_tracks_table(
                favorite_tracks,
                "artist-favorites",
                Some(source_descriptor),
            ));
        }

        if !albums.is_empty() {
            self.append_artist_release_sections(&content, &albums);
        }

        if !appears_on.is_empty() {
            content.append(&self.artist_album_section(msgid("Appears On"), &appears_on));
        }

        if !has_favorite_tracks && albums.is_empty() && appears_on.is_empty() {
            content.append(&self.placeholder_view(
                "Artist",
                "No cached albums or tracks are linked to this artist yet.",
            ));
        }

        wrapper.append(&detail_route_scroller(self, content.upcast()));
        wrapper.upcast()
    }

    pub(super) fn artist_discography_view(self: &Rc<Self>, artist_id: ArtistId) -> gtk::Widget {
        let Some(detail) = self.artist_detail_data(&artist_id) else {
            return self
                .placeholder_view("Discography", "The selected cached artist was not found.");
        };

        let wrapper = detail_route_wrapper(0);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(ROUTE_TOP_MARGIN);
        content.set_margin_bottom(36);
        content.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        content.set_margin_end(0);
        content.set_hexpand(true);
        content.set_halign(gtk::Align::Fill);
        content.set_width_request(1);

        let summary = artist_summary_text(
            detail.albums.len(),
            detail.appears_on.len(),
            detail
                .artist
                .track_count
                .max(detail.tracks.len().min(u32::MAX as usize) as u32),
        );
        content.append(&self.artist_subroute_header(&detail.artist, "Discography", &summary));

        if !detail.albums.is_empty() {
            self.append_artist_release_sections(&content, &detail.albums);
        }
        if !detail.appears_on.is_empty() {
            content.append(&self.artist_album_section(msgid("Appears On"), &detail.appears_on));
        }
        if detail.albums.is_empty() && detail.appears_on.is_empty() {
            content.append(&self.placeholder_view(
                "Discography",
                "No cached albums are linked to this artist yet.",
            ));
        }

        wrapper.append(&detail_route_scroller(self, content.upcast()));
        wrapper.upcast()
    }

    pub(super) fn artist_tracks_view(self: &Rc<Self>, artist_id: ArtistId) -> gtk::Widget {
        let Some(detail) = self.artist_detail_data(&artist_id) else {
            return self.placeholder_view("Tracks", "The selected cached artist was not found.");
        };

        if detail.tracks.is_empty() {
            return self
                .placeholder_view("Tracks", "No cached tracks are linked to this artist yet.");
        }

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 14);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(ROUTE_TOP_MARGIN);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(0);
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);
        wrapper.set_width_request(1);

        let summary = artist_summary_text(
            detail.albums.len(),
            detail.appears_on.len(),
            detail
                .artist
                .track_count
                .max(detail.tracks.len().min(u32::MAX as usize) as u32),
        );
        wrapper.append(&self.artist_subroute_header(&detail.artist, "Tracks", &summary));

        wrapper.append(&self.library_tracks_scrolling_panel(
            detail.tracks,
            LibraryListKey::ArtistTracks,
            "artist-tracks",
            0,
            Some(PlaySourceDescriptor::ArtistTracks {
                artist_id: detail.artist.id,
                scope: ArtistTrackScope::AllCredits,
                selected_music_folder_id: selected_music_folder_id(self),
            }),
        ));

        wrapper.upcast()
    }

    fn artist_detail_header(
        self: &Rc<Self>,
        artist: &Artist,
        tracks: &[Track],
        album_count: usize,
        appears_on_count: usize,
        track_count: u32,
    ) -> gtk::Widget {
        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let cover_size = detail_showcase_cover_size(content_width);
        let seed = stable_seed(artist.id.as_str());
        let external_links = artist_external_links(self, artist, tracks);

        let image_ref = super::library::artist_cover_image_ref(self, artist);
        let cover_fetch_size = cover_fetch_size_for_display(cover_size);
        let cover = detail_cover_button(
            self,
            image_ref.as_ref(),
            seed,
            cover_size,
            cover_fetch_size,
            "artist-detail-cover",
        );
        let counts = self.artist_count_buttons(artist, album_count + appears_on_count, track_count);
        let text_stack = gtk::Box::new(gtk::Orientation::Vertical, 8);
        text_stack.set_hexpand(true);
        text_stack.set_halign(gtk::Align::Fill);
        text_stack.set_width_request(1);
        let kind = gtk::Label::new(Some(&tr("Artist")));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);
        kind.set_halign(gtk::Align::Start);

        let title = fitted_detail_title_label(&artist.name);

        let actions = detail_action_row();
        actions.add_css_class("artist-detail-actions");
        actions.set_halign(gtk::Align::Start);

        let action_tracks = Rc::new(tracks.to_vec());

        let play = detail_action_button("media-playback-start-symbolic", "Play");
        play.add_css_class("detail-showcase-play-button");
        let controller = self.controller.clone();
        let play_tracks = Rc::clone(&action_tracks);
        let artist_id = artist.id.clone();
        play.connect_clicked(move |_| {
            controller.play_artist_tracks_window(
                artist_id.clone(),
                ArtistTrackScope::AllCredits,
                play_tracks.len(),
                0,
                |index| play_tracks.as_ref().get(index).cloned(),
            );
        });
        actions.append(&play);

        append_track_batch_queue_actions(&actions, &self.controller, Rc::clone(&action_tracks));

        let favorite = favorite_icon_button("Favorite");
        favorite.add_css_class("detail-showcase-action-button");
        set_favorite_button_active(&favorite, artist.favorite);
        self.register_favorite_button(artist_favorite_key(&artist.id), &favorite);
        let shell = Rc::clone(self);
        let artist_id = artist.id.clone();
        favorite.connect_clicked(move |button| {
            let favorite = !favorite_button_is_active(button);
            shell.set_favorite_with_feedback(
                FavoriteItemId::Artist(artist_id.clone()),
                favorite,
                Some(button),
            );
        });
        actions.append(&favorite);

        text_stack.append(&kind);
        text_stack.append(&title);
        text_stack.append(&counts);
        media_detail_showcase(
            self,
            MediaDetailShowcase {
                route_class: "artist-detail-showcase",
                seed,
                content_width,
                cover_size,
                cover: cover.upcast(),
                external_links,
                external_links_class: None,
                text_stack: text_stack.upcast(),
                actions: actions.upcast(),
            },
        )
    }

    fn artist_subroute_header(
        self: &Rc<Self>,
        artist: &Artist,
        kind: &str,
        summary: &str,
    ) -> gtk::Widget {
        let content_width = detail_route_inner_width(self, PRIMARY_ROUTE_MARGIN_START);
        let seed = stable_seed(artist.id.as_str());
        let header = gtk::Box::new(gtk::Orientation::Vertical, 8);
        header.add_css_class("detail-showcase");
        header.add_css_class("artist-detail-showcase");
        mark_tiny_detail_showcase(&header, content_width);
        add_album_seed_gradient_class(&header, seed);

        let kind = gtk::Label::new(Some(&tr(kind)));
        kind.add_css_class("eyebrow");
        kind.set_xalign(0.0);

        let title = gtk::Label::new(Some(&artist.name));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        fit_detail_text(&title, &artist.name);

        let summary = gtk::Label::new(Some(summary));
        summary.add_css_class("muted");
        summary.set_xalign(0.0);

        header.append(&kind);
        header.append(&title);
        header.append(&summary);
        let showcase = detail_showcase_frame_with_back(self, header.upcast());
        showcase.set_margin_end(DETAIL_GRADIENT_MARGIN_END);
        showcase
    }

    fn artist_count_buttons(
        self: &Rc<Self>,
        artist: &Artist,
        album_count: usize,
        track_count: u32,
    ) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("artist-count-row");
        row.set_halign(gtk::Align::Start);

        let albums = artist_count_button(
            "route-albums-symbolic",
            &album_count_text(album_count as u64),
        );
        let shell = Rc::clone(self);
        let artist_id = artist.id.clone();
        albums.connect_clicked(move |_| {
            shell.navigate(Route::ArtistDiscography(artist_id.clone()));
        });
        row.append(&albums);

        let tracks = artist_count_button(
            "route-tracks-symbolic",
            &track_count_text(track_count.into()),
        );
        let shell = Rc::clone(self);
        let artist_id = artist.id.clone();
        tracks.connect_clicked(move |_| {
            shell.navigate(Route::ArtistTracks(artist_id.clone()));
        });
        row.append(&tracks);

        row
    }

    fn artist_detail_data(&self, artist_id: &ArtistId) -> Option<CachedArtistDetail> {
        self.controller
            .cached_artist_detail(artist_id)
            .ok()
            .flatten()
            .or_else(|| {
                let library = self.state.library.borrow();
                let artist = library
                    .artists
                    .iter()
                    .chain(library.album_artists.iter())
                    .find(|artist| artist.id.as_str() == artist_id.as_str())
                    .cloned()?;
                let artist_name_lower = artist_name_lower(&artist.name);
                let albums = library
                    .albums
                    .iter()
                    .filter(|album| {
                        album.artist_id.as_ref().map(ArtistId::as_str) == Some(artist_id.as_str())
                            || album
                                .album_artist_credits
                                .iter()
                                .any(|artist| artist.id.as_str() == artist_id.as_str())
                            || (album.artist_id.is_none()
                                && artist_name_lower
                                    .as_deref()
                                    .map(|name| album.artist.to_lowercase() == name)
                                    .unwrap_or(false))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let tracks = library
                    .tracks
                    .iter()
                    .filter(|track| {
                        track_matches_artist(track, artist_id, artist_name_lower.as_deref())
                            || albums.iter().any(|album| album.id == track.album_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let appears_on = artist_appears_on_from_tracks(
                    &library.albums,
                    &albums,
                    &tracks,
                    artist_id,
                    artist_name_lower.as_deref(),
                );
                Some(CachedArtistDetail {
                    artist,
                    albums,
                    appears_on,
                    tracks,
                })
            })
    }

    fn artist_album_section(self: &Rc<Self>, title: &str, albums: &[Album]) -> gtk::Widget {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
        section.set_hexpand(true);
        section.set_halign(gtk::Align::Fill);
        section.set_width_request(1);
        section.append(&section_heading(title));
        section.append(&self.library_album_collection_panel(
            albums,
            LibraryListKey::ArtistAlbums,
            "artist-albums",
        ));
        section.upcast()
    }

    fn append_artist_release_sections(self: &Rc<Self>, content: &gtk::Box, albums: &[Album]) {
        for section in artist_release_sections(albums) {
            content.append(&self.artist_album_section(section.title, &section.albums));
        }
    }
}

fn section_heading(title: &str) -> gtk::Widget {
    let heading = gtk::Label::new(Some(&tr(title)));
    heading.add_css_class("section-heading");
    heading.set_xalign(0.0);
    heading.upcast()
}

fn artist_summary_text(album_count: usize, appears_on_count: usize, track_count: u32) -> String {
    format!(
        "{} / {}",
        album_count_text((album_count + appears_on_count) as u64),
        track_count_text(track_count.into())
    )
}

fn artist_count_button(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("artist-count-button");

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(ARTIST_COUNT_ICON_SIZE);
    icon.set_size_request(ARTIST_COUNT_ICON_SIZE, ARTIST_COUNT_ICON_SIZE);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    content.append(&icon);
    let label = gtk::Label::new(Some(label));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&label);
    button.set_child(Some(&content));
    button
}

struct ArtistReleaseSection {
    title: &'static str,
    albums: Vec<Album>,
}

fn artist_release_sections(albums: &[Album]) -> Vec<ArtistReleaseSection> {
    let mut grouped = [
        (AlbumReleaseKind::Album, Vec::new()),
        (AlbumReleaseKind::Ep, Vec::new()),
        (AlbumReleaseKind::Single, Vec::new()),
        (AlbumReleaseKind::Collection, Vec::new()),
        (AlbumReleaseKind::Other, Vec::new()),
    ];
    for album in albums {
        let group = album_release_kind(album);
        if let Some((_, bucket)) = grouped
            .iter_mut()
            .find(|(candidate, _)| candidate == &group)
        {
            bucket.push(album.clone());
        }
    }
    grouped
        .into_iter()
        .filter_map(|(group, albums)| {
            (!albums.is_empty()).then_some(ArtistReleaseSection {
                title: group.section_title(),
                albums,
            })
        })
        .collect()
}

fn favorite_artist_tracks(tracks: &[Track]) -> Vec<Track> {
    let mut favorites = tracks
        .iter()
        .filter(|track| track.favorite)
        .cloned()
        .collect::<Vec<_>>();
    favorites.sort_by(|left, right| {
        left.album
            .to_lowercase()
            .cmp(&right.album.to_lowercase())
            .then(left.disc_number.cmp(&right.disc_number))
            .then(left.track_number.cmp(&right.track_number))
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    favorites
}

fn artist_appears_on_from_tracks(
    all_albums: &[Album],
    albums: &[Album],
    tracks: &[Track],
    artist_id: &ArtistId,
    artist_name_lower: Option<&str>,
) -> Vec<Album> {
    let mut appears_on = Vec::new();
    let mut seen_album_ids = Vec::new();
    for track in tracks
        .iter()
        .filter(|track| track_matches_artist(track, artist_id, artist_name_lower))
    {
        if albums.iter().any(|album| album.id == track.album_id)
            || seen_album_ids.contains(&track.album_id)
        {
            continue;
        }
        seen_album_ids.push(track.album_id.clone());
        if let Some(album) = all_albums.iter().find(|album| album.id == track.album_id) {
            appears_on.push(album.clone());
        } else {
            let album_tracks = tracks
                .iter()
                .filter(|candidate| candidate.album_id == track.album_id)
                .cloned()
                .collect::<Vec<_>>();
            if let Some(album) = synthesize_album_from_tracks(&track.album_id, &album_tracks) {
                appears_on.push(album);
            }
        }
    }
    appears_on.sort_by(|left, right| {
        left.year
            .cmp(&right.year)
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    appears_on
}

fn synthesize_album_from_tracks(album_id: &AlbumId, tracks: &[Track]) -> Option<Album> {
    let first = tracks.first()?;
    Some(Album {
        id: album_id.clone(),
        title: first.album.clone(),
        artist: first.artist.clone(),
        artist_id: first.artist_id.clone(),
        album_artist_credits: Vec::new(),
        artist_credits: Vec::new(),
        year: first.year,
        release_date: first.release_date.clone(),
        date_added: first.date_added.clone(),
        last_played: first.last_played.clone(),
        play_count: first.play_count,
        user_rating: first.user_rating,
        track_count: tracks.len().min(usize::from(u16::MAX)) as u16,
        duration_seconds: tracks
            .iter()
            .map(|track| track.duration_seconds)
            .fold(0_u32, u32::saturating_add),
        favorite: tracks.iter().any(|track| track.favorite),
        color_seed: stable_seed(album_id.as_str()),
        image_ref: first.image_ref.clone(),
        genres: first.genres.clone(),
        release_types: Vec::new(),
        is_compilation: None,
        musicbrainz_album_id: None,
        musicbrainz_release_group_id: None,
    })
}

fn track_matches_artist(
    track: &Track,
    artist_id: &ArtistId,
    artist_name_lower: Option<&str>,
) -> bool {
    if track.artist_id.as_ref() == Some(artist_id) {
        return true;
    }
    if track
        .artist_credits
        .iter()
        .any(|artist| &artist.id == artist_id)
    {
        return true;
    }

    track.artist_id.is_none()
        && artist_name_lower
            .map(|artist_name| track.artist.to_lowercase() == artist_name)
            .unwrap_or(false)
}

fn artist_name_lower(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artist_summary_merges_appears_on() {
        let summary = artist_summary_text(0, 2, 3);

        assert_eq!(summary, "2 albums / 3 tracks");
    }

    #[test]
    fn artist_exclude_album() {
        let artist_id = ArtistId::fake(7);
        let mut primary = test_album("Artist", Some(artist_id.clone()));
        primary.id = AlbumId::fake(1);
        let mut appears_on = test_album("Other Artist", Some(ArtistId::fake(8)));
        appears_on.id = AlbumId::fake(2);
        appears_on.title = "Compilation".to_string();

        let mut primary_track = test_track("Artist", Some(artist_id.clone()));
        primary_track.album_id = primary.id.clone();
        let mut featured_track = test_track("Artist", Some(artist_id.clone()));
        featured_track.id = domain::TrackId::fake(2);
        featured_track.album_id = appears_on.id.clone();
        featured_track.album = appears_on.title.clone();

        assert_eq!(
            artist_appears_on_from_tracks(
                &[primary.clone(), appears_on.clone()],
                &[primary],
                &[primary_track, featured_track],
                &artist_id,
                Some("artist"),
            ),
            vec![appears_on]
        );
    }

    #[test]
    fn artist_use_missing() {
        let artist_id = ArtistId::fake(7);
        let appears_on = test_album("Other Artist", Some(ArtistId::fake(8)));
        let mut featured_track = test_track("Artist", None);
        featured_track.album_id = appears_on.id.clone();
        featured_track.album = appears_on.title.clone();

        assert_eq!(
            artist_appears_on_from_tracks(
                std::slice::from_ref(&appears_on),
                &[],
                &[featured_track],
                &artist_id,
                Some("artist"),
            ),
            vec![appears_on]
        );
    }

    #[test]
    fn artist_release_sections_group_album_types() {
        let mut album = test_album("Artist", Some(ArtistId::fake(1)));
        album.title = "Album".to_string();
        let mut ep = test_album("Artist", Some(ArtistId::fake(1)));
        ep.id = AlbumId::fake(2);
        ep.title = "Short".to_string();
        ep.release_types = vec!["EP".to_string()];
        let mut single = test_album("Artist", Some(ArtistId::fake(1)));
        single.id = AlbumId::fake(3);
        single.title = "One Track".to_string();
        single.release_types = vec!["single".to_string()];
        let mut collection = test_album("Artist", Some(ArtistId::fake(1)));
        collection.id = AlbumId::fake(4);
        collection.title = "Archive".to_string();
        collection.is_compilation = Some(true);
        let mut other = test_album("Artist", Some(ArtistId::fake(1)));
        other.id = AlbumId::fake(5);
        other.title = "Live Set".to_string();
        other.release_types = vec!["live".to_string()];

        let sections = artist_release_sections(&[
            ep.clone(),
            other.clone(),
            album.clone(),
            collection.clone(),
            single.clone(),
        ]);
        let labels = sections
            .iter()
            .map(|section| section.title)
            .collect::<Vec<_>>();
        let titles = sections
            .iter()
            .map(|section| {
                section
                    .albums
                    .iter()
                    .map(|album| album.title.as_str())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec!["Albums", "EPs", "Singles", "Collections", "Other"]
        );
        assert_eq!(
            titles,
            vec![
                vec!["Album"],
                vec!["Short"],
                vec!["One Track"],
                vec!["Archive"],
                vec!["Live Set"]
            ]
        );
    }

    fn test_album(artist: &str, artist_id: Option<ArtistId>) -> Album {
        Album {
            id: AlbumId::fake(1),
            title: "Album".to_string(),
            artist: artist.to_string(),
            artist_id,
            album_artist_credits: Vec::new(),
            artist_credits: Vec::new(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            track_count: 1,
            duration_seconds: 180,
            favorite: false,
            color_seed: 1,
            image_ref: None,
            genres: Vec::new(),
            release_types: Vec::new(),
            is_compilation: None,
            musicbrainz_album_id: None,
            musicbrainz_release_group_id: None,
        }
    }

    fn test_track(artist: &str, artist_id: Option<ArtistId>) -> Track {
        Track {
            id: domain::TrackId::fake(1),
            album_id: AlbumId::fake(1),
            title: "Track".to_string(),
            artist: artist.to_string(),
            artist_id,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2026,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: 1,
            image_ref: None,
            genres: Vec::new(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
        }
    }
}

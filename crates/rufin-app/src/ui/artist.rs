use std::rc::Rc;

use adw::prelude::*;
use rufin_core::{Album, AlbumId, ArtistId, Route, Track};
use rufin_store::CachedArtistDetail;

use super::*;

impl Shell {
    pub(super) fn artist_detail_view(self: &Rc<Self>, artist_id: ArtistId) -> gtk::Widget {
        let detail = self.artist_detail_data(&artist_id);
        let Some(detail) = detail else {
            return self.placeholder_view("Artist", "The selected cached artist was not found.");
        };
        let artist = detail.artist;
        let albums = detail.albums;
        let appears_on = detail.appears_on;
        let tracks = detail.tracks;
        let favorite_tracks = favorite_artist_tracks(&tracks);
        let has_favorite_tracks = !favorite_tracks.is_empty();

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let title = gtk::Label::new(Some(&artist.name));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        wrapper.append(&title);

        let summary = gtk::Label::new(Some(&artist_summary_text(
            albums.len(),
            appears_on.len(),
            artist
                .track_count
                .max(tracks.len().min(u32::MAX as usize) as u32),
        )));
        summary.add_css_class("muted");
        summary.set_xalign(0.0);
        wrapper.append(&summary);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let favorite = favorite_icon_button("Favorite");
        set_favorite_button_active(&favorite, artist.favorite);
        self.register_favorite_button(artist_favorite_key(&artist.id), &favorite);
        let controller = self.controller.clone();
        let artist_id = artist.id.clone();
        favorite.connect_clicked(move |button| {
            controller.set_artist_favorite(artist_id.clone(), !favorite_button_is_active(button));
        });
        actions.append(&favorite);

        let discography = text_button("media-optical-symbolic", "Discography");
        let shell = Rc::clone(self);
        let artist_id = artist.id.clone();
        discography.connect_clicked(move |_| {
            shell.navigate(Route::ArtistDiscography(artist_id.clone()));
        });
        actions.append(&discography);

        let all_tracks = text_button("audio-x-generic-symbolic", "View all tracks");
        let shell = Rc::clone(self);
        let artist_id = artist.id.clone();
        all_tracks.connect_clicked(move |_| {
            shell.navigate(Route::ArtistTracks(artist_id.clone()));
        });
        actions.append(&all_tracks);
        wrapper.append(&actions);

        if has_favorite_tracks {
            wrapper.append(&section_heading("Favorite tracks"));
            wrapper.append(&self.compact_artist_tracks_table(favorite_tracks, "artist-favorites"));
        }

        if !albums.is_empty() {
            wrapper.append(&self.artist_album_section("Albums", &albums));
        }

        if !appears_on.is_empty() {
            wrapper.append(&self.artist_album_section("Appears on", &appears_on));
        }

        if !has_favorite_tracks && albums.is_empty() && appears_on.is_empty() {
            wrapper.append(&self.placeholder_view(
                "Artist",
                "No cached albums or tracks are linked to this artist yet.",
            ));
        }

        scroller.set_child(Some(&wrapper));
        scroller.upcast()
    }

    pub(super) fn artist_discography_view(self: &Rc<Self>, artist_id: ArtistId) -> gtk::Widget {
        let Some(detail) = self.artist_detail_data(&artist_id) else {
            return self
                .placeholder_view("Discography", "The selected cached artist was not found.");
        };

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_min_content_width(0);
        scroller.set_vexpand(true);

        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 18);
        wrapper.add_css_class("route-content");
        wrapper.set_margin_top(28);
        wrapper.set_margin_bottom(36);
        wrapper.set_margin_start(32);
        wrapper.set_margin_end(32);

        let title = gtk::Label::new(Some(&detail.artist.name));
        title.add_css_class("detail-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        wrapper.append(&title);

        let summary = gtk::Label::new(Some(&artist_summary_text(
            detail.albums.len(),
            detail.appears_on.len(),
            detail
                .artist
                .track_count
                .max(detail.tracks.len().min(u32::MAX as usize) as u32),
        )));
        summary.add_css_class("muted");
        summary.set_xalign(0.0);
        wrapper.append(&summary);

        if !detail.albums.is_empty() {
            wrapper.append(&self.artist_album_section("Albums", &detail.albums));
        }
        if !detail.appears_on.is_empty() {
            wrapper.append(&self.artist_album_section("Appears on", &detail.appears_on));
        }
        if detail.albums.is_empty() && detail.appears_on.is_empty() {
            wrapper.append(&self.placeholder_view(
                "Discography",
                "No cached albums are linked to this artist yet.",
            ));
        }

        scroller.set_child(Some(&wrapper));
        scroller.upcast()
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
        wrapper.set_margin_top(24);
        wrapper.set_margin_bottom(28);
        wrapper.set_margin_start(PRIMARY_ROUTE_MARGIN_START);
        wrapper.set_margin_end(PRIMARY_ROUTE_MARGIN_END);
        wrapper.set_vexpand(true);

        let title = gtk::Label::new(Some(&detail.artist.name));
        title.add_css_class("section-heading");
        title.set_xalign(0.0);
        wrapper.append(&title);
        wrapper.append(&self.library_tracks_panel(
            detail.tracks,
            LibraryListKey::ArtistTracks,
            "artist-tracks",
        ));

        wrapper.upcast()
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
        section.append(&section_heading(title));
        section.append(&self.library_album_collection_panel(
            albums,
            LibraryListKey::ArtistAlbums,
            "artist-albums",
        ));
        section.upcast()
    }
}

fn section_heading(title: &str) -> gtk::Widget {
    let heading = gtk::Label::new(Some(&tr(title)));
    heading.add_css_class("section-heading");
    heading.set_xalign(0.0);
    heading.upcast()
}

fn artist_summary_text(album_count: usize, appears_on_count: usize, track_count: u32) -> String {
    if appears_on_count == 0 {
        format!(
            "{} {} / {} {}",
            album_count,
            tr("albums"),
            track_count,
            tr("tracks")
        )
    } else {
        format!(
            "{} {} / {} {} / {} {}",
            album_count,
            tr("albums"),
            appears_on_count,
            tr("appears on"),
            track_count,
            tr("tracks")
        )
    }
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
    fn artist_summary_counts_appears_on_albums() {
        let summary = artist_summary_text(0, 1, 3);

        assert!(summary.contains("0 albums"));
        assert!(summary.contains("1 appears on"));
        assert!(summary.contains("3 tracks"));
    }

    #[test]
    fn artist_appears_on_from_tracks_excludes_primary_albums() {
        let artist_id = ArtistId::fake(7);
        let mut primary = test_album("Artist", Some(artist_id.clone()));
        primary.id = AlbumId::fake(1);
        let mut appears_on = test_album("Other Artist", Some(ArtistId::fake(8)));
        appears_on.id = AlbumId::fake(2);
        appears_on.title = "Compilation".to_string();

        let mut primary_track = test_track("Artist", Some(artist_id.clone()));
        primary_track.album_id = primary.id.clone();
        let mut featured_track = test_track("Artist", Some(artist_id.clone()));
        featured_track.id = rufin_core::TrackId::fake(2);
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
    fn artist_appears_on_from_tracks_uses_name_when_artist_id_is_missing() {
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
        }
    }

    fn test_track(artist: &str, artist_id: Option<ArtistId>) -> Track {
        Track {
            id: rufin_core::TrackId::fake(1),
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
        }
    }
}

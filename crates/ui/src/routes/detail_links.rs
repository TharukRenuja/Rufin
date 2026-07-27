use crate::preferences::source::login::source_kind_icon_name;
use ::library::{Album, Track};
use localization::msgid;

use super::route::Route;

pub(crate) fn track_artist_route(track: &Track) -> Option<Route> {
    track.primary_artist_id().cloned().map(Route::ArtistDetail)
}

pub(crate) fn album_artist_route(album: &Album) -> Option<Route> {
    album.primary_artist_id().cloned().map(Route::ArtistDetail)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DetailEntityKind {
    Album,
    Artist,
}

impl DetailEntityKind {
    fn id_prefix(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Artist => "artist",
        }
    }
}

pub(crate) struct DetailExternalLink {
    pub(crate) label: &'static str,
    pub(crate) icon_name: &'static str,
    pub(crate) url: String,
}

pub(crate) fn server_entity_link(
    source_kind: &str,
    base_url: &str,
    kind: DetailEntityKind,
    entity_id: &str,
) -> Option<DetailExternalLink> {
    let base_url = clean_source_base_url(base_url)?;
    match source_kind {
        "jellyfin" => {
            let item_id = raw_source_entity_id(entity_id, "jellyfin", kind)?;
            Some(DetailExternalLink {
                label: msgid("Open on Jellyfin"),
                icon_name: source_kind_icon_name("jellyfin")?,
                url: format!("{base_url}/web/index.html#!/details?id={item_id}"),
            })
        }
        "navidrome" => {
            let item_id = raw_source_entity_id(entity_id, "navidrome", kind)?;
            Some(DetailExternalLink {
                label: msgid("Open on Navidrome"),
                icon_name: source_kind_icon_name("navidrome")?,
                url: format!(
                    "{base_url}/app/#/{}/{}/show",
                    kind.id_prefix(),
                    percent_encode_path_segment(item_id)
                ),
            })
        }
        _ => None,
    }
}

fn raw_source_entity_id<'a>(
    entity_id: &'a str,
    source_kind: &str,
    kind: DetailEntityKind,
) -> Option<&'a str> {
    let raw_id = entity_id.strip_prefix(&format!("{source_kind}:{}:", kind.id_prefix()))?;
    let raw_id = raw_id.trim();
    (!raw_id.is_empty()).then_some(raw_id)
}

fn clean_source_base_url(base_url: &str) -> Option<&str> {
    let base_url = base_url.trim().trim_end_matches('/');
    (!base_url.is_empty()).then_some(base_url)
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
    use library::{ArtistCredit, ArtistId};

    use super::{DetailEntityKind, album_artist_route, server_entity_link, track_artist_route};
    use crate::routes::route::Route;

    #[test]
    fn server_links_use_only_known_web_routes() {
        let jellyfin = server_entity_link(
            "jellyfin",
            "https://music.example/",
            DetailEntityKind::Album,
            "jellyfin:album:abc123",
        )
        .expect("jellyfin album link");
        assert_eq!(
            jellyfin.url,
            "https://music.example/web/index.html#!/details?id=abc123"
        );

        let navidrome = server_entity_link(
            "navidrome",
            "https://music.example/library/",
            DetailEntityKind::Artist,
            "navidrome:artist:artist/one",
        )
        .expect("navidrome artist link");
        assert_eq!(
            navidrome.url,
            "https://music.example/library/app/#/artist/artist%2Fone/show"
        );

        assert!(
            server_entity_link(
                "subsonic",
                "https://music.example",
                DetailEntityKind::Album,
                "subsonic:album:album-one",
            )
            .is_none()
        );
    }

    #[test]
    fn track_artist_links_follow_canonical_relations() {
        let mut track = crate::test_support::track(1, "Track");
        track.artist = "A label without a relationship".to_string();
        assert_eq!(track_artist_route(&track), None);

        track.relations.artists = vec![credit(3, "Track Artist")];
        assert_eq!(
            track_artist_route(&track),
            Some(Route::ArtistDetail(ArtistId::fake(3)))
        );

        track.relations.artists.clear();
        track.relations.album_artists = vec![credit(4, "Album Artist")];
        assert_eq!(
            track_artist_route(&track),
            Some(Route::ArtistDetail(ArtistId::fake(4)))
        );
    }

    #[test]
    fn album_artist_links_follow_canonical_relations() {
        let mut album = crate::test_support::album(1, "Album");
        album.artist = "A label without a relationship".to_string();
        assert_eq!(album_artist_route(&album), None);

        album.relations.album_artists = vec![credit(5, "Album Artist")];
        assert_eq!(
            album_artist_route(&album),
            Some(Route::ArtistDetail(ArtistId::fake(5)))
        );
    }

    fn credit(id: u32, name: &str) -> ArtistCredit {
        ArtistCredit {
            id: ArtistId::fake(id),
            name: name.to_string(),
            musicbrainz_artist_id: None,
        }
    }
}

use domain::SourceIdentity;

use crate::i18n::msgid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum DetailEntityKind {
    Album,
    Artist,
}

pub(in crate::ui) struct DetailExternalLink {
    pub(in crate::ui) label: &'static str,
    pub(in crate::ui) icon_name: &'static str,
    pub(in crate::ui) url: String,
}

pub(in crate::ui) fn server_entity_link(
    server: &SourceIdentity,
    kind: DetailEntityKind,
    entity_id: &str,
) -> Option<DetailExternalLink> {
    let base_url = clean_base_url(&server.base_url)?;
    match server.kind.as_str() {
        "jellyfin" => jellyfin_entity_link(base_url, kind, entity_id),
        "navidrome" => navidrome_entity_link(base_url, kind, entity_id),
        _ => None,
    }
}

fn jellyfin_entity_link(
    base_url: &str,
    kind: DetailEntityKind,
    entity_id: &str,
) -> Option<DetailExternalLink> {
    let item_id = raw_entity_id(entity_id, "jellyfin", kind)?;
    Some(DetailExternalLink {
        label: msgid("Open on Jellyfin"),
        icon_name: "io.github.screwys.Rufin.source.jellyfin",
        url: format!("{base_url}/web/index.html#!/details?id={item_id}"),
    })
}

fn navidrome_entity_link(
    base_url: &str,
    kind: DetailEntityKind,
    entity_id: &str,
) -> Option<DetailExternalLink> {
    let item_id = raw_entity_id(entity_id, "navidrome", kind)?;
    let route = match kind {
        DetailEntityKind::Album => "album",
        DetailEntityKind::Artist => "artist",
    };
    Some(DetailExternalLink {
        label: msgid("Open on Navidrome"),
        icon_name: "io.github.screwys.Rufin.source.navidrome",
        url: format!(
            "{base_url}/app/#/{route}/{}/show",
            percent_encode_path_segment(item_id)
        ),
    })
}

fn raw_entity_id<'a>(
    entity_id: &'a str,
    source_kind: &str,
    kind: DetailEntityKind,
) -> Option<&'a str> {
    let prefix = match kind {
        DetailEntityKind::Album => "album",
        DetailEntityKind::Artist => "artist",
    };
    let raw_id = entity_id.strip_prefix(&format!("{source_kind}:{prefix}:"))?;
    let raw_id = raw_id.trim();
    (!raw_id.is_empty()).then_some(raw_id)
}

fn clean_base_url(base_url: &str) -> Option<&str> {
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
    use domain::{SourceId, SourceIdentity};

    use super::*;

    fn server(kind: &str, base_url: &str) -> SourceIdentity {
        SourceIdentity {
            id: SourceId::new("test:server"),
            kind: kind.to_string(),
            name: "Test".to_string(),
            base_url: base_url.to_string(),
        }
    }

    #[test]
    fn server_links_use_only_known_web_routes() {
        let jellyfin = server_entity_link(
            &server("jellyfin", "https://music.example/"),
            DetailEntityKind::Album,
            "jellyfin:album:abc123",
        )
        .expect("jellyfin album link");
        assert_eq!(
            jellyfin.url,
            "https://music.example/web/index.html#!/details?id=abc123"
        );

        let navidrome = server_entity_link(
            &server("navidrome", "https://music.example/library/"),
            DetailEntityKind::Artist,
            "navidrome:artist:artist/one",
        )
        .expect("navidrome artist link");
        assert_eq!(
            navidrome.url,
            "https://music.example/library/app/#/artist/artist%2Fone/show"
        );

        for provider in ["subsonic", "opensubsonic"] {
            assert!(
                server_entity_link(
                    &server(provider, "https://music.example"),
                    DetailEntityKind::Album,
                    &format!("{provider}:album:album-one"),
                )
                .is_none()
            );
        }
    }
}

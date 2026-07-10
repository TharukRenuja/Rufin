use domain::SourceIdentity;

use crate::sources::resolve_source_registration;
pub(in crate::ui) use crate::sources::{
    SourceEntityKind as DetailEntityKind, SourceEntityLink as DetailExternalLink,
};

pub(in crate::ui) fn server_entity_link(
    server: &SourceIdentity,
    kind: DetailEntityKind,
    entity_id: &str,
) -> Option<DetailExternalLink> {
    let registration = resolve_source_registration(&server.kind)?;
    (registration.entity_link?)(server, kind, entity_id)
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

        assert!(
            server_entity_link(
                &server("subsonic", "https://music.example"),
                DetailEntityKind::Album,
                "subsonic:album:album-one",
            )
            .is_none()
        );
    }
}

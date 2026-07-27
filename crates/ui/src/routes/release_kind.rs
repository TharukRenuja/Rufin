use ::library::{Album, normalize_release_types};

use localization::msgid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AlbumReleaseKind {
    Album,
    Ep,
    Single,
    Collection,
    Other,
}

pub(super) fn album_release_kind(album: &Album) -> AlbumReleaseKind {
    if album.is_compilation == Some(true) {
        return AlbumReleaseKind::Collection;
    }

    let types = normalize_release_types(&album.release_types);
    if types.is_empty() || types.iter().any(|value| value == "album") {
        return AlbumReleaseKind::Album;
    }
    if types.iter().any(|value| {
        matches!(
            value.as_str(),
            "compilation" | "compilations" | "collection" | "collections"
        )
    }) {
        return AlbumReleaseKind::Collection;
    }
    if types
        .iter()
        .any(|value| matches!(value.as_str(), "ep" | "e.p."))
    {
        return AlbumReleaseKind::Ep;
    }
    if types.iter().any(|value| value == "single") {
        return AlbumReleaseKind::Single;
    }

    AlbumReleaseKind::Other
}

pub(super) fn album_release_kind_label(album: &Album) -> &'static str {
    album_release_kind(album).detail_label()
}

impl AlbumReleaseKind {
    pub(super) fn section_title(self) -> &'static str {
        match self {
            AlbumReleaseKind::Album => msgid("Albums"),
            AlbumReleaseKind::Ep => msgid("EPs"),
            AlbumReleaseKind::Single => msgid("Singles"),
            AlbumReleaseKind::Collection => msgid("Collections"),
            AlbumReleaseKind::Other => msgid("Other"),
        }
    }

    fn detail_label(self) -> &'static str {
        match self {
            AlbumReleaseKind::Album => msgid("Album"),
            AlbumReleaseKind::Ep => msgid("EP"),
            AlbumReleaseKind::Single => msgid("Single"),
            AlbumReleaseKind::Collection => msgid("Collection"),
            AlbumReleaseKind::Other => msgid("Release"),
        }
    }
}

#[cfg(test)]
mod tests {
    use ::library::Album;

    use super::{AlbumReleaseKind, album_release_kind_label};

    #[test]
    fn release_kind_detail_labels_follow_release_metadata() {
        let mut album = test_album();
        assert_eq!(album_release_kind_label(&album), "Album");

        album.release_types = vec!["EP".to_string()];
        assert_eq!(album_release_kind_label(&album), "EP");

        album.release_types = vec!["single".to_string()];
        assert_eq!(album_release_kind_label(&album), "Single");

        album.release_types = vec!["live".to_string()];
        assert_eq!(album_release_kind_label(&album), "Release");

        album.release_types = vec!["compilation".to_string()];
        assert_eq!(album_release_kind_label(&album), "Collection");

        album.release_types = vec!["album".to_string(), "ep".to_string()];
        assert_eq!(album_release_kind_label(&album), "Album");

        album.release_types.clear();
        album.is_compilation = Some(true);
        assert_eq!(album_release_kind_label(&album), "Collection");
    }

    #[test]
    fn release_kind_section_titles_match_artist_groups() {
        assert_eq!(AlbumReleaseKind::Album.section_title(), "Albums");
        assert_eq!(AlbumReleaseKind::Ep.section_title(), "EPs");
        assert_eq!(AlbumReleaseKind::Single.section_title(), "Singles");
        assert_eq!(AlbumReleaseKind::Collection.section_title(), "Collections");
        assert_eq!(AlbumReleaseKind::Other.section_title(), "Other");
    }

    fn test_album() -> Album {
        crate::test_support::album(1, "Title")
    }
}

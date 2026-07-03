use super::*;

/// set of features Rufin should expose for this source.
///
/// this is based only on saved source facts plus Rufin's own policy. it must be
/// deterministic, and must not be based on live state. if a server connection
/// dies, trying to favorite fails, but the favorite button does not disappear.
///
/// `Native` means the source owns this operation and Rufin supports it. `Store`
/// means Rufin owns the feature for that source.
pub(in crate::controller) fn source_capabilities_for_saved(
    saved: &SavedServer,
) -> SourceCapabilities {
    let source_kind = saved.server.provider.as_str();
    let native_playlists = source_kind_has_native_playlists(source_kind);
    let native_playlist_mutations = source_kind_has_native_playlist_mutations(source_kind);
    let native_favorites = source_kind_has_native_favorites(source_kind);
    let native_favorite_mutations = source_kind_has_native_favorite_mutations(source_kind);
    let native_music_folders = source_kind_has_native_music_folders(source_kind);
    let native_folder_browsing = source_kind_has_native_folder_browsing(source_kind);

    SourceCapabilities {
        playlists: SourcePlaylistCapabilities {
            read_native: native_playlists,
            read_store: true,
            create: if native_playlist_mutations {
                SourceFeatureSupport::native()
            } else {
                SourceFeatureSupport::store()
            },
            mutate_native: native_playlist_mutations,
            mutate_store: true,
        },
        smart_playlists: SourceFeatureSupport::store(),
        favorites: if source_kind == "fake" || source_kind == LOCAL_SOURCE_ID {
            SourceFeatureSupport::store()
        } else if native_favorites {
            SourceFeatureSupport::native()
        } else {
            SourceFeatureSupport::Unsupported
        },
        favorite_mutations: if source_kind == "fake" || source_kind == LOCAL_SOURCE_ID {
            SourceFeatureSupport::store()
        } else if native_favorite_mutations {
            SourceFeatureSupport::native()
        } else {
            SourceFeatureSupport::Unsupported
        },
        music_folders: if native_music_folders {
            SourceFeatureSupport::native()
        } else {
            SourceFeatureSupport::Unsupported
        },
        folder_browsing: if native_folder_browsing {
            SourceFeatureSupport::native()
        } else {
            SourceFeatureSupport::Unsupported
        },
    }
}

fn source_kind_has_native_playlists(source_kind: &str) -> bool {
    !matches!(source_kind, "fake" | LOCAL_SOURCE_ID)
}

fn source_kind_has_native_playlist_mutations(source_kind: &str) -> bool {
    matches!(
        source_kind,
        "jellyfin" | "navidrome" | "subsonic" | "opensubsonic"
    )
}

fn source_kind_has_native_favorites(source_kind: &str) -> bool {
    matches!(
        source_kind,
        "jellyfin" | "navidrome" | "subsonic" | "opensubsonic"
    )
}

fn source_kind_has_native_favorite_mutations(source_kind: &str) -> bool {
    matches!(
        source_kind,
        "jellyfin" | "navidrome" | "subsonic" | "opensubsonic"
    )
}

fn source_kind_has_native_music_folders(source_kind: &str) -> bool {
    matches!(
        source_kind,
        "jellyfin" | "navidrome" | "subsonic" | "opensubsonic"
    )
}

fn source_kind_has_native_folder_browsing(source_kind: &str) -> bool {
    matches!(
        source_kind,
        LOCAL_SOURCE_ID | "jellyfin" | "navidrome" | "subsonic" | "opensubsonic"
    )
}

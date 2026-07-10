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
    saved: &SavedSource,
) -> SourceCapabilities {
    let source_kind = saved.source.kind.as_str();
    let native_playlists = source_kind_has_native_playlists(source_kind);
    let native_playlist_mutations = source_kind_has_native_playlist_mutations(source_kind);
    let native_favorites = source_kind_has_native_favorites(source_kind);
    let native_favorite_mutations = source_kind_has_native_favorite_mutations(source_kind);
    let native_music_folders = source_kind_has_native_music_folders(source_kind);
    let native_folder_browsing = source_kind_has_native_folder_browsing(source_kind);

    let playlist_mutations = SourcePlaylistOperationSupport {
        native: native_playlist_mutations,
        store: true,
    };
    SourceCapabilities {
        playlists: SourcePlaylistCapabilities {
            read_native: native_playlists,
            read_store: true,
            create: if native_playlist_mutations {
                SourceFeatureOwner::Native
            } else {
                SourceFeatureOwner::Store
            },
            rename: playlist_mutations,
            delete: playlist_mutations,
            add_tracks: playlist_mutations,
            remove_entries: playlist_mutations,
            reorder_entries: playlist_mutations,
        },
        smart_playlists: SourceFeatureSupport::store(),
        favorites: if native_favorites {
            SourceFeatureSupport::native()
        } else {
            SourceFeatureSupport::store()
        },
        favorite_mutations: if native_favorite_mutations {
            SourceFeatureOwner::Native
        } else {
            SourceFeatureOwner::Store
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
    source_kind != LOCAL_SOURCE_ID
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

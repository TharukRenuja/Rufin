use std::cell::RefCell;
use std::rc::Rc;

use ::library::{
    AlbumSummary, ArtistSummary, FavoriteItemId, GenreSummary, MetadataItemId, PlaylistEdit,
    PlaylistSummary, RadioSeed, SmartPlaylistSummary, Track, TrackList,
};
use adw::prelude::*;
use downloads::DownloadSubject;
use gtk::glib;
use playback::{QueuePlacement, RadioPlayRequest};

use crate::SidebarPin;
use crate::favorites::{FAVORITE_ADD_ICON, FAVORITE_REMOVE_ICON};
use crate::interactions::{
    ADD_TO_PLAYLIST_ICON, ALBUM_ICON, ARTIST_ICON, ContextMenuSurface, DOWNLOAD_ICON, RADIO_ICON,
    install_context_menu_openers, radio_context_submenu,
};
use crate::player::state::current_playback_track;
use crate::preferences::dialogs::metadata::present_metadata_dialog;
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::settings::ContextMenuItem;
use crate::shell::Shell;
use crate::shell::actions::{
    ADD_ICON, EDIT_ICON, PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON, REMOVE_ICON, TRASH_ICON,
};
use localization::{msgid, tr};

use super::collections::{CollectionPlay, PlaybackTarget};
use super::detail_links::{album_artist_route, track_artist_route};
use super::playlist_entries::{
    PlaylistEntryContextMenuAction, PlaylistEntryContextMenuState, confirm_remove_playlist_entry,
};
use super::playlist_picker::{
    PlaylistTrackSource, context_menu_can_add_to_playlist, install_context_menu_picker_action,
};
use super::route::Route;

pub(crate) fn install_track_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    track: Track,
) {
    install_dynamic_track_context_menu(target, shell, Rc::new(RefCell::new(Some(track))));
}

pub(crate) fn install_dynamic_playlist_entry_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    state: Rc<RefCell<Option<PlaylistEntryContextMenuState>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(state) = state.borrow().clone() else {
                return;
            };
            present_track_context_menu_inner(
                target,
                &shell,
                state.track.clone(),
                position,
                state.remove_action,
                None,
            );
        }),
    );
}

pub(crate) fn install_dynamic_track_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    track: Rc<RefCell<Option<Track>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(track) = track.borrow().clone() else {
                return;
            };
            present_track_context_menu(target, &shell, track, position);
        }),
    );
}

pub(crate) fn install_album_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    album: AlbumSummary,
    tracks: TrackList,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let playback_target = PlaybackTarget::prepared(
                tracks.clone(),
                format!("album:{}", album.album.id.as_str()),
                DownloadSubject::Album(album.album.id.clone()),
            );
            present_album_context_menu_inner(
                target,
                &shell,
                album.clone(),
                playback_target,
                None,
                position,
            );
        }),
    );
}

pub(crate) fn install_dynamic_album_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    album: Rc<RefCell<Option<AlbumSummary>>>,
    playback_context: Option<String>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(album) = album.borrow().clone() else {
                return;
            };
            present_album_context_menu(
                target,
                &shell,
                album,
                playback_context.clone(),
                None,
                position,
            );
        }),
    );
}

pub(crate) fn install_genre_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    genre: GenreSummary,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            present_genre_context_menu(target, &shell, genre.clone(), None, position);
        }),
    );
}

pub(crate) fn install_current_track_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            if let Some(track) = current_playback_track(&shell.playback.player.borrow()) {
                present_track_context_menu(target, &shell, track, position);
            }
        }),
    );
}

pub(crate) fn present_current_track_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
) {
    if let Some(track) = current_playback_track(&shell.playback.player.borrow()) {
        present_track_context_menu_above(target.as_ref(), shell, track, None);
    }
}

pub(crate) fn present_track_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    position: Option<(f64, f64)>,
) {
    present_track_context_menu_inner(target, shell, track, position, None, None);
}

pub(crate) fn present_track_context_menu_above(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    position: Option<(f64, f64)>,
) {
    present_track_context_menu_inner(
        target,
        shell,
        track,
        position,
        None,
        Some(gtk::PositionType::Top),
    );
}

fn present_track_context_menu_inner(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    position: Option<(f64, f64)>,
    remove_action: Option<PlaylistEntryContextMenuAction>,
    popover_position: Option<gtk::PositionType>,
) {
    let metadata_editable =
        shell.metadata_editing_available(MetadataItemId::Track(track.id.clone()));
    present_resolved_track_context_menu(
        target,
        shell,
        track,
        position,
        remove_action,
        popover_position,
        metadata_editable,
    );
}

fn present_resolved_track_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    position: Option<(f64, f64)>,
    remove_action: Option<PlaylistEntryContextMenuAction>,
    popover_position: Option<gtk::PositionType>,
    metadata_editable: bool,
) {
    let playback_target = PlaybackTarget::Track(track.id.clone());
    let surface = ContextMenuSurface::new(target, "track", position);
    surface.append_configurable_action(ContextMenuItem::Play, msgid("Play"), "play", PLAY_ICON);
    surface.append_configurable_action(
        ContextMenuItem::PlayNext,
        msgid("Play Next"),
        "play-next",
        PLAY_NEXT_ICON,
    );
    surface.append_configurable_action(
        ContextMenuItem::PlayLater,
        msgid("Play Later"),
        "play-last",
        PLAY_LATER_ICON,
    );
    surface.append_configurable_submenu(
        ContextMenuItem::PlayRadio,
        msgid("Track radio"),
        &radio_context_submenu("track"),
        RADIO_ICON,
    );
    let playlist_source = shell.library.selected.borrow().as_ref().map(|selected| {
        PlaylistTrackSource::ready(
            selected,
            DownloadSubject::Track(track.id.clone()),
            vec![track.id.clone()].into(),
        )
    });
    if playlist_source.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::AddToPlaylist,
            msgid("Add to Playlist"),
            "add-to-playlist",
            ADD_TO_PLAYLIST_ICON,
        );
    }
    surface.append_configurable_action(
        ContextMenuItem::Favorites,
        if track.favorite {
            msgid("Remove from Favorites")
        } else {
            msgid("Add to Favorites")
        },
        "favorite",
        if track.favorite {
            FAVORITE_REMOVE_ICON
        } else {
            FAVORITE_ADD_ICON
        },
    );
    if metadata_editable {
        surface.append_configurable_action(
            ContextMenuItem::EditMetadata,
            msgid("Edit metadata"),
            "edit-metadata",
            EDIT_ICON,
        );
    }
    let artist_route = track_artist_route(&track);
    if artist_route.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::GoToArtist,
            msgid("Go to Artist"),
            "go-artist",
            ARTIST_ICON,
        );
    }
    if track.album_id.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::GoToAlbum,
            msgid("Go to Album"),
            "go-album",
            ALBUM_ICON,
        );
    }
    if remove_action.is_some() {
        surface.append_fixed_action(
            msgid("Remove from playlist"),
            "remove-from-playlist",
            REMOVE_ICON,
        );
    }

    if let Some(popover_position) = popover_position {
        surface.popover().set_position(popover_position);
    }
    install_loaded_actions(&surface, shell, playback_target, false, None);
    install_radio_actions(&surface, shell, RadioSeed::Track(track.id.clone()));
    if let Some(playlist_source) = playlist_source {
        install_context_menu_picker_action(&surface, shell, playlist_source);
    }

    surface.add_action("favorite", {
        let shell = Rc::clone(shell);
        let track_id = track.id.clone();
        let favorite = !track.favorite;
        move || {
            shell.set_favorite_with_feedback(
                FavoriteItemId::Track(track_id.clone()),
                favorite,
                None,
            );
        }
    });
    if let Some(route) = artist_route {
        surface.add_action("go-artist", {
            let shell = Rc::clone(shell);
            move || {
                let shell = Rc::clone(&shell);
                let route = route.clone();
                glib::idle_add_local_once(move || shell.navigate(route));
            }
        });
    }
    if let Some(album_id) = track.album_id.clone() {
        surface.add_action("go-album", {
            let shell = Rc::clone(shell);
            move || {
                let shell = Rc::clone(&shell);
                let album_id = album_id.clone();
                glib::idle_add_local_once(move || shell.navigate(Route::AlbumDetail(album_id)));
            }
        });
    }
    if metadata_editable {
        surface.add_action("edit-metadata", {
            let shell = Rc::clone(shell);
            let track = track.clone();
            move || present_metadata_dialog(&shell, MetadataItemId::Track(track.id.clone()))
        });
    }
    if let Some(remove_action) = remove_action {
        surface.add_action("remove-from-playlist", {
            let shell = Rc::clone(shell);
            move || {
                confirm_remove_playlist_entry(
                    &shell,
                    remove_action.playlist_id.clone(),
                    remove_action.occurrence_id.clone(),
                    remove_action.title.clone(),
                );
            }
        });
    }
    surface.popup(&shell.settings.current.borrow().context_menu);
}

pub(crate) fn present_album_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    album: AlbumSummary,
    playback_context: Option<String>,
    play: Option<CollectionPlay>,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::Album(album.album.id.clone());
    let playback_target = playback_context
        .map(|context| playback_target.clone().in_context(context))
        .unwrap_or(playback_target);
    present_album_context_menu_inner(target, shell, album, playback_target, play, position);
}

fn present_album_context_menu_inner(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    album: AlbumSummary,
    playback_target: PlaybackTarget,
    play: Option<CollectionPlay>,
    position: Option<(f64, f64)>,
) {
    let metadata_editable =
        shell.metadata_editing_available(MetadataItemId::Album(album.album.id.clone()));
    present_resolved_album_context_menu_inner(
        target,
        shell,
        album,
        playback_target,
        play,
        position,
        metadata_editable,
    );
}

fn present_resolved_album_context_menu_inner(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    album: AlbumSummary,
    playback_target: PlaybackTarget,
    play: Option<CollectionPlay>,
    position: Option<(f64, f64)>,
    metadata_editable: bool,
) {
    let surface = ContextMenuSurface::new(target, "album", position);
    surface.append_configurable_action(ContextMenuItem::Play, msgid("Play"), "play", PLAY_ICON);
    surface.append_configurable_action(
        ContextMenuItem::PlayNext,
        msgid("Play Next"),
        "play-next",
        PLAY_NEXT_ICON,
    );
    surface.append_configurable_action(
        ContextMenuItem::PlayLater,
        msgid("Play Later"),
        "play-last",
        PLAY_LATER_ICON,
    );
    surface.append_configurable_submenu(
        ContextMenuItem::PlayRadio,
        msgid("Album radio"),
        &radio_context_submenu("album"),
        RADIO_ICON,
    );
    let playlist_source = context_menu_can_add_to_playlist(shell)
        .then(|| playback_target.playlist_tracks(shell))
        .flatten();
    if playlist_source.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::AddToPlaylist,
            msgid("Add to Playlist"),
            "add-to-playlist",
            ADD_TO_PLAYLIST_ICON,
        );
    }
    surface.append_configurable_action(
        ContextMenuItem::Favorites,
        if album.album.favorite {
            msgid("Remove from Favorites")
        } else {
            msgid("Add to Favorites")
        },
        "favorite",
        if album.album.favorite {
            FAVORITE_REMOVE_ICON
        } else {
            FAVORITE_ADD_ICON
        },
    );
    if metadata_editable {
        surface.append_configurable_action(
            ContextMenuItem::EditMetadata,
            msgid("Edit metadata"),
            "edit-metadata",
            EDIT_ICON,
        );
    }
    install_sidebar_pin_action(
        &surface,
        shell,
        shell
            .library
            .selected
            .borrow()
            .as_ref()
            .map(|selected| SidebarPin::Album {
                source_id: selected.source_id.clone(),
                album_id: album.album.id.clone(),
            }),
    );
    let artist_route = album_artist_route(&album.album);
    if artist_route.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::GoToArtist,
            msgid("Go to Artist"),
            "go-artist",
            ARTIST_ICON,
        );
    }
    surface.append_configurable_action(
        ContextMenuItem::GoToAlbum,
        msgid("Go to Album"),
        "go-album",
        ALBUM_ICON,
    );

    install_loaded_actions(&surface, shell, playback_target, true, play);
    install_radio_actions(&surface, shell, RadioSeed::Album(album.album.id.clone()));
    if let Some(playlist_source) = playlist_source {
        install_context_menu_picker_action(&surface, shell, playlist_source);
    }
    surface.add_action("favorite", {
        let shell = Rc::clone(shell);
        let album_id = album.album.id.clone();
        let favorite = !album.album.favorite;
        move || {
            shell.set_favorite_with_feedback(
                FavoriteItemId::Album(album_id.clone()),
                favorite,
                None,
            );
        }
    });
    if metadata_editable {
        surface.add_action("edit-metadata", {
            let shell = Rc::clone(shell);
            let album_id = album.album.id.clone();
            move || {
                present_metadata_dialog(&shell, MetadataItemId::Album(album_id.clone()));
            }
        });
    }
    if let Some(route) = artist_route {
        surface.add_action("go-artist", {
            let shell = Rc::clone(shell);
            move || {
                let shell = Rc::clone(&shell);
                let route = route.clone();
                glib::idle_add_local_once(move || shell.navigate(route));
            }
        });
    }
    surface.add_action("go-album", {
        let shell = Rc::clone(shell);
        let album_id = album.album.id.clone();
        move || {
            let shell = Rc::clone(&shell);
            let album_id = album_id.clone();
            glib::idle_add_local_once(move || shell.navigate(Route::AlbumDetail(album_id)));
        }
    });
    surface.popup(&shell.settings.current.borrow().context_menu);
}

pub(crate) fn present_artist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    artist: ArtistSummary,
    play: Option<CollectionPlay>,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::Artist(artist.artist.id.clone());
    let metadata_editable =
        shell.metadata_editing_available(MetadataItemId::Artist(artist.artist.id.clone()));
    present_resolved_artist_context_menu(
        target,
        shell,
        artist,
        playback_target,
        play,
        position,
        metadata_editable,
    );
}

fn present_resolved_artist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    artist: ArtistSummary,
    playback_target: PlaybackTarget,
    play: Option<CollectionPlay>,
    position: Option<(f64, f64)>,
    metadata_editable: bool,
) {
    let surface = ContextMenuSurface::new(target, "artist", position);
    surface.append_configurable_action(ContextMenuItem::Play, msgid("Play"), "play", PLAY_ICON);
    surface.append_configurable_action(
        ContextMenuItem::PlayNext,
        msgid("Play Next"),
        "play-next",
        PLAY_NEXT_ICON,
    );
    surface.append_configurable_action(
        ContextMenuItem::PlayLater,
        msgid("Play Later"),
        "play-last",
        PLAY_LATER_ICON,
    );
    surface.append_configurable_submenu(
        ContextMenuItem::PlayRadio,
        msgid("Artist radio"),
        &radio_context_submenu("artist"),
        RADIO_ICON,
    );
    let playlist_source = context_menu_can_add_to_playlist(shell)
        .then(|| playback_target.playlist_tracks(shell))
        .flatten();
    if playlist_source.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::AddToPlaylist,
            msgid("Add to Playlist"),
            "add-to-playlist",
            ADD_TO_PLAYLIST_ICON,
        );
    }
    surface.append_configurable_action(
        ContextMenuItem::Favorites,
        if artist.artist.favorite {
            msgid("Remove from Favorites")
        } else {
            msgid("Add to Favorites")
        },
        "favorite",
        if artist.artist.favorite {
            FAVORITE_REMOVE_ICON
        } else {
            FAVORITE_ADD_ICON
        },
    );
    if metadata_editable {
        surface.append_configurable_action(
            ContextMenuItem::EditMetadata,
            msgid("Edit metadata"),
            "edit-metadata",
            EDIT_ICON,
        );
    }
    install_sidebar_pin_action(
        &surface,
        shell,
        shell
            .library
            .selected
            .borrow()
            .as_ref()
            .map(|selected| SidebarPin::Artist {
                source_id: selected.source_id.clone(),
                artist_id: artist.artist.id.clone(),
            }),
    );
    surface.append_configurable_action(
        ContextMenuItem::GoToArtist,
        msgid("Go to Artist"),
        "go-artist",
        ARTIST_ICON,
    );

    install_loaded_actions(&surface, shell, playback_target, true, play);
    install_radio_actions(&surface, shell, RadioSeed::Artist(artist.artist.id.clone()));
    if let Some(playlist_source) = playlist_source {
        install_context_menu_picker_action(&surface, shell, playlist_source);
    }
    surface.add_action("favorite", {
        let shell = Rc::clone(shell);
        let artist_id = artist.artist.id.clone();
        let favorite = !artist.artist.favorite;
        move || {
            shell.set_favorite_with_feedback(
                FavoriteItemId::Artist(artist_id.clone()),
                favorite,
                None,
            );
        }
    });
    if metadata_editable {
        surface.add_action("edit-metadata", {
            let shell = Rc::clone(shell);
            let artist_id = artist.artist.id.clone();
            move || {
                present_metadata_dialog(&shell, MetadataItemId::Artist(artist_id.clone()));
            }
        });
    }
    surface.add_action("go-artist", {
        let shell = Rc::clone(shell);
        let artist_id = artist.artist.id.clone();
        move || shell.navigate(Route::ArtistDetail(artist_id.clone()))
    });
    surface.popup(&shell.settings.current.borrow().context_menu);
}

pub(crate) fn present_genre_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    genre: GenreSummary,
    play: Option<CollectionPlay>,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::Genre(genre.genre.id.clone());
    let surface = ContextMenuSurface::new(target, "genre", position);
    surface.append_configurable_action(ContextMenuItem::Play, msgid("Play"), "play", PLAY_ICON);
    surface.append_configurable_action(
        ContextMenuItem::PlayNext,
        msgid("Play Next"),
        "play-next",
        PLAY_NEXT_ICON,
    );
    surface.append_configurable_action(
        ContextMenuItem::PlayLater,
        msgid("Play Later"),
        "play-last",
        PLAY_LATER_ICON,
    );
    surface.append_configurable_submenu(
        ContextMenuItem::PlayRadio,
        msgid("Genre radio"),
        &radio_context_submenu("genre"),
        RADIO_ICON,
    );
    let playlist_source = context_menu_can_add_to_playlist(shell)
        .then(|| playback_target.playlist_tracks(shell))
        .flatten();
    if playlist_source.is_some() {
        surface.append_configurable_action(
            ContextMenuItem::AddToPlaylist,
            msgid("Add to Playlist"),
            "add-to-playlist",
            ADD_TO_PLAYLIST_ICON,
        );
    }
    install_sidebar_pin_action(
        &surface,
        shell,
        shell
            .library
            .selected
            .borrow()
            .as_ref()
            .map(|selected| SidebarPin::Genre {
                source_id: selected.source_id.clone(),
                genre_id: genre.genre.id.clone(),
            }),
    );
    install_loaded_actions(&surface, shell, playback_target, true, play);
    install_radio_actions(
        &surface,
        shell,
        RadioSeed::Genre {
            id: genre.genre.id.clone(),
            name: genre.genre.name.clone(),
        },
    );
    if let Some(playlist_source) = playlist_source {
        install_context_menu_picker_action(&surface, shell, playlist_source);
    }
    surface.popup(&shell.settings.current.borrow().context_menu);
}

pub(crate) fn present_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: PlaylistSummary,
    play: Option<CollectionPlay>,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::Playlist(playlist.playlist.id.clone());
    let surface = ContextMenuSurface::new(target, "playlist", position);
    surface.append_configurable_action(ContextMenuItem::Play, msgid("Play"), "play", PLAY_ICON);
    surface.append_configurable_action(
        ContextMenuItem::PlayNext,
        msgid("Play Next"),
        "play-next",
        PLAY_NEXT_ICON,
    );
    surface.append_configurable_action(
        ContextMenuItem::PlayLater,
        msgid("Play Later"),
        "play-last",
        PLAY_LATER_ICON,
    );
    surface.append_configurable_submenu(
        ContextMenuItem::PlayRadio,
        msgid("Playlist radio"),
        &radio_context_submenu("playlist"),
        RADIO_ICON,
    );
    install_sidebar_pin_action(
        &surface,
        shell,
        shell
            .library
            .selected
            .borrow()
            .as_ref()
            .map(|selected| SidebarPin::Playlist {
                source_id: selected.source_id.clone(),
                playlist_id: playlist.playlist.id.clone(),
            }),
    );
    surface.append_fixed_action(msgid("Rename"), "rename", EDIT_ICON);
    surface.append_fixed_action(msgid("Add current"), "add-current", ADD_ICON);
    surface.append_fixed_action(msgid("Delete"), "delete", REMOVE_ICON);
    install_loaded_actions(&surface, shell, playback_target, true, play);
    install_radio_actions(
        &surface,
        shell,
        RadioSeed::Playlist(playlist.playlist.id.clone()),
    );
    surface.add_action("rename", {
        let shell = Rc::clone(shell);
        let playlist_id = playlist.playlist.id.clone();
        let playlist_name = playlist.playlist.name.clone();
        move || shell.rename_playlist_dialog(playlist_id.clone(), playlist_name.clone())
    });
    let current_track_id = {
        let player = shell.playback.player.borrow();
        current_playback_track(&player).map(|track| track.id.clone())
    };
    surface.add_action_enabled("add-current", current_track_id.is_some(), {
        let source = shell.products.source.clone();
        let playlist_id = playlist.playlist.id.clone();
        move || {
            let Some(track_id) = current_track_id.clone() else {
                return;
            };
            source.edit_playlist(PlaylistEdit::AddTracks {
                playlist_id: playlist_id.clone(),
                track_ids: vec![track_id],
            });
        }
    });
    surface.add_action("delete", {
        let source = shell.products.source.clone();
        let shell = Rc::clone(shell);
        let window = shell.chrome.window.clone();
        let playlist_id = playlist.playlist.id.clone();
        let playlist_name = playlist.playlist.name.clone();
        move || {
            let dialog = adw::AlertDialog::builder()
                .heading(tr("Delete Playlist"))
                .body(format!("Delete \"{playlist_name}\"?"))
                .build();
            dialog.add_response("cancel", &tr("Cancel"));
            dialog.add_response("delete", &tr("Delete"));
            dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            let source = source.clone();
            let shell = Rc::clone(&shell);
            let playlist_id = playlist_id.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "delete" {
                    source.edit_playlist(PlaylistEdit::Delete {
                        playlist_id: playlist_id.clone(),
                    });
                    shell.navigate(Route::Playlists);
                }
            });
            present_light_dismiss_dialog(&dialog, &window);
        }
    });
    surface.popup(&shell.settings.current.borrow().context_menu);
}

pub(crate) fn present_smart_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: SmartPlaylistSummary,
    play: Option<CollectionPlay>,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::SmartPlaylist(playlist.smart_playlist.id.clone());
    let surface = ContextMenuSurface::new(target, "smart-playlist", position);
    surface.append_configurable_action(ContextMenuItem::Play, msgid("Play"), "play", PLAY_ICON);
    install_sidebar_pin_action(
        &surface,
        shell,
        shell
            .library
            .selected
            .borrow()
            .as_ref()
            .map(|selected| SidebarPin::SmartPlaylist {
                source_id: selected.source_id.clone(),
                playlist_id: playlist.smart_playlist.id.clone(),
            }),
    );
    surface.append_fixed_action(msgid("Rename"), "rename", EDIT_ICON);
    surface.append_fixed_action(msgid("Delete"), "delete", REMOVE_ICON);
    install_loaded_actions(&surface, shell, playback_target, true, play);
    surface.add_action("rename", {
        let shell = Rc::clone(shell);
        let playlist = (*playlist.smart_playlist).clone();
        move || shell.rename_smart_playlist_dialog(playlist.clone())
    });
    surface.add_action("delete", {
        let shell = Rc::clone(shell);
        let playlist_id = playlist.smart_playlist.id.clone();
        move || {
            shell.products.smart_playlists.delete(playlist_id.clone());
            shell.navigate(Route::SmartPlaylists);
        }
    });
    surface.popup(&shell.settings.current.borrow().context_menu);
}

fn install_sidebar_pin_action(
    surface: &ContextMenuSurface,
    shell: &Rc<Shell>,
    pin: Option<SidebarPin>,
) {
    let Some(pin) = pin else {
        return;
    };
    let settings = shell.settings.current.borrow();
    if !sidebar_pin_action_available(&settings) {
        return;
    }
    let pinned = settings.sidebar.is_pinned(&pin);
    drop(settings);
    surface.append_configurable_action(
        ContextMenuItem::Pins,
        if pinned {
            msgid("Remove from Pins")
        } else {
            msgid("Add to Pins")
        },
        "pin",
        if pinned { REMOVE_ICON } else { ADD_ICON },
    );
    let shell = Rc::clone(shell);
    surface.add_action("pin", move || {
        shell.set_sidebar_pin(pin.clone(), !pinned);
    });
}

fn sidebar_pin_action_available(settings: &crate::Settings) -> bool {
    settings.sidebar.pins_visible
}

fn install_loaded_actions(
    surface: &ContextMenuSurface,
    shell: &Rc<Shell>,
    target: PlaybackTarget,
    shuffled_start: bool,
    play: Option<CollectionPlay>,
) {
    install_download_actions(surface, shell, &target);
    for (action, placement) in [
        ("play", QueuePlacement::Now),
        ("play-next", QueuePlacement::Next),
        ("play-last", QueuePlacement::Last),
    ] {
        let queue = shell.products.playback.queue.clone();
        let shell = Rc::clone(shell);
        let target = target.clone();
        let play = play.clone();
        surface.add_action(action, move || {
            if let Some(play) = play.as_ref() {
                play(
                    placement,
                    shuffled_start && placement == QueuePlacement::Now,
                );
            } else if let Some(request) = target.play_request(&shell, placement, shuffled_start) {
                queue.play_loaded(request);
            }
        });
    }
}

pub(crate) fn install_download_actions(
    surface: &ContextMenuSurface,
    shell: &Rc<Shell>,
    target: &PlaybackTarget,
) {
    let Some(selected) = shell.library.selected.borrow().as_ref().cloned() else {
        return;
    };
    let remote = shell
        .source
        .configured
        .borrow()
        .sources
        .iter()
        .any(|source| source.id == selected.source_id && source.kind != "local");
    if !remote {
        return;
    }
    let status = target.download_status(&selected).unwrap_or_default();
    let collection = !matches!(target, PlaybackTarget::Track(_));
    if !status.complete {
        let source = shell.products.source.clone();
        let shell = Rc::clone(shell);
        let target = target.clone();
        surface.append_configurable_action(
            ContextMenuItem::Download,
            msgid("Download"),
            "download",
            DOWNLOAD_ICON,
        );
        surface.add_action("download", move || {
            if let Some(request) = target.download_request(&shell) {
                source.download(request);
            }
        });
    }
    if status.any {
        let source = shell.products.source.clone();
        let shell = Rc::clone(shell);
        let target = target.clone();
        surface.append_configurable_action(
            ContextMenuItem::Download,
            remove_download_label(collection),
            "remove-downloads",
            TRASH_ICON,
        );
        surface.add_action("remove-downloads", move || {
            if let Some(request) = target.remove_download_request(&shell) {
                source.remove_download(request);
            }
        });
    }
}

fn remove_download_label(collection: bool) -> &'static str {
    if collection {
        msgid("Remove Downloads")
    } else {
        msgid("Remove Download")
    }
}

fn install_radio_actions(surface: &ContextMenuSurface, shell: &Shell, seed: RadioSeed) {
    for (action, request) in [
        ("play-radio", RadioPlayRequest::now(seed.clone())),
        ("play-radio-next", RadioPlayRequest::next(seed.clone())),
        ("play-radio-last", RadioPlayRequest::last(seed.clone())),
    ] {
        let radio = shell.products.playback.radio.clone();
        surface.add_action(action, move || radio.play_radio(request.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::sidebar_pin_action_available;
    use crate::Settings;

    #[test]
    fn disabling_sidebar_pins_also_removes_context_menu_pin_actions() {
        let mut settings = Settings::default();
        settings.sidebar.pins_visible = false;

        assert!(!sidebar_pin_action_available(&settings));
    }
}

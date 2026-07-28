use std::cell::RefCell;
use std::rc::Rc;

use ::library::{
    AlbumSummary, ArtistSummary, FavoriteItemId, GenreSummary, PlaylistEdit, PlaylistSummary,
    RadioSeed, SmartPlaylistSummary, Track, TrackList,
};
use adw::prelude::*;
use gtk::glib;
use playback::{QueuePlacement, RadioPlayRequest};

use crate::interactions::{
    ContextMenuSurface, install_context_menu_openers, radio_context_submenu,
};
use crate::player::state::current_playback_track;
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::shell::Shell;
use localization::{msgid, tr};

use super::collections::PlaybackTarget;
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
            );
            present_album_context_menu_inner(
                target,
                &shell,
                album.clone(),
                playback_target,
                position,
            );
        }),
    );
}

pub(crate) fn install_dynamic_album_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    album: Rc<RefCell<Option<AlbumSummary>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(album) = album.borrow().clone() else {
                return;
            };
            present_album_context_menu(target, &shell, album, position);
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
            present_genre_context_menu(target, &shell, genre.clone(), position);
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
    let playback_target = PlaybackTarget::Track(track.id.clone());
    let surface = ContextMenuSurface::new(target, "track", position);
    surface.append_action(msgid("Play"), "play");
    surface.append_action(msgid("Play Next"), "play-next");
    surface.append_action(msgid("Play Later"), "play-last");
    surface.append_submenu(msgid("Track radio"), &radio_context_submenu("track"));
    let playlist_source = shell
        .library
        .selected
        .borrow()
        .as_ref()
        .map(|selected| PlaylistTrackSource::ready(selected, vec![track.id.clone()].into()));
    if playlist_source.is_some() {
        surface.append_action(msgid("Add to Playlist"), "add-to-playlist");
    }
    surface.append_action(
        if track.favorite {
            msgid("Remove from Favorites")
        } else {
            msgid("Add to Favorites")
        },
        "favorite",
    );
    let artist_route = track_artist_route(&track);
    if artist_route.is_some() {
        surface.append_action(msgid("Go to Artist"), "go-artist");
    }
    if track.album_id.is_some() {
        surface.append_action(msgid("Go to Album"), "go-album");
    }
    if remove_action.is_some() {
        surface.append_action(msgid("Remove from playlist"), "remove-from-playlist");
    }

    if let Some(popover_position) = popover_position {
        surface.popover().set_position(popover_position);
    }
    install_loaded_actions(&surface, shell, playback_target, false);
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
    surface.popup();
}

pub(crate) fn present_album_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    album: AlbumSummary,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::Album(album.album.id.clone());
    present_album_context_menu_inner(target, shell, album, playback_target, position);
}

fn present_album_context_menu_inner(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    album: AlbumSummary,
    playback_target: PlaybackTarget,
    position: Option<(f64, f64)>,
) {
    let surface = ContextMenuSurface::new(target, "album", position);
    surface.append_action(msgid("Play"), "play");
    surface.append_action(msgid("Play Next"), "play-next");
    surface.append_action(msgid("Play Later"), "play-last");
    surface.append_submenu(msgid("Album radio"), &radio_context_submenu("album"));
    let playlist_source = context_menu_can_add_to_playlist(shell)
        .then(|| playback_target.playlist_tracks(shell))
        .flatten();
    if playlist_source.is_some() {
        surface.append_action(msgid("Add to Playlist"), "add-to-playlist");
    }
    surface.append_action(
        if album.album.favorite {
            msgid("Remove from Favorites")
        } else {
            msgid("Add to Favorites")
        },
        "favorite",
    );
    let artist_route = album_artist_route(&album.album);
    if artist_route.is_some() {
        surface.append_action(msgid("Go to Artist"), "go-artist");
    }
    surface.append_action(msgid("Go to Album"), "go-album");

    install_loaded_actions(&surface, shell, playback_target, true);
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
    surface.popup();
}

pub(crate) fn present_artist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    artist: ArtistSummary,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::Artist(artist.artist.id.clone());
    let surface = ContextMenuSurface::new(target, "artist", position);
    surface.append_action(msgid("Play"), "play");
    surface.append_action(msgid("Play Next"), "play-next");
    surface.append_action(msgid("Play Later"), "play-last");
    surface.append_submenu(msgid("Artist radio"), &radio_context_submenu("artist"));
    let playlist_source = context_menu_can_add_to_playlist(shell)
        .then(|| playback_target.playlist_tracks(shell))
        .flatten();
    if playlist_source.is_some() {
        surface.append_action(msgid("Add to Playlist"), "add-to-playlist");
    }
    surface.append_action(
        if artist.artist.favorite {
            msgid("Remove from Favorites")
        } else {
            msgid("Add to Favorites")
        },
        "favorite",
    );
    surface.append_action(msgid("Go to Artist"), "go-artist");

    install_loaded_actions(&surface, shell, playback_target, true);
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
    surface.add_action("go-artist", {
        let shell = Rc::clone(shell);
        let artist_id = artist.artist.id.clone();
        move || shell.navigate(Route::ArtistDetail(artist_id.clone()))
    });
    surface.popup();
}

pub(crate) fn present_genre_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    genre: GenreSummary,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::Genre(genre.genre.id.clone());
    let surface = ContextMenuSurface::new(target, "genre", position);
    surface.append_action(msgid("Play"), "play");
    surface.append_action(msgid("Play Next"), "play-next");
    surface.append_action(msgid("Play Later"), "play-last");
    surface.append_submenu(msgid("Genre radio"), &radio_context_submenu("genre"));
    let playlist_source = context_menu_can_add_to_playlist(shell)
        .then(|| playback_target.playlist_tracks(shell))
        .flatten();
    if playlist_source.is_some() {
        surface.append_action(msgid("Add to Playlist"), "add-to-playlist");
    }
    install_loaded_actions(&surface, shell, playback_target, true);
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
    surface.popup();
}

pub(crate) fn present_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: PlaylistSummary,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::Playlist(playlist.playlist.id.clone());
    let surface = ContextMenuSurface::new(target, "playlist", position);
    surface.append_action(msgid("Play"), "play");
    surface.append_action(msgid("Play Next"), "play-next");
    surface.append_action(msgid("Play Later"), "play-last");
    surface.append_submenu(msgid("Playlist radio"), &radio_context_submenu("playlist"));
    surface.append_action(msgid("Delete"), "delete");
    install_loaded_actions(&surface, shell, playback_target, true);
    install_radio_actions(
        &surface,
        shell,
        RadioSeed::Playlist(playlist.playlist.id.clone()),
    );
    surface.add_action("delete", {
        let source = shell.products.source.clone();
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
            let playlist_id = playlist_id.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "delete" {
                    source.edit_playlist(PlaylistEdit::Delete {
                        playlist_id: playlist_id.clone(),
                    });
                }
            });
            present_light_dismiss_dialog(&dialog, &window);
        }
    });
    surface.popup();
}

pub(crate) fn present_smart_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: SmartPlaylistSummary,
    position: Option<(f64, f64)>,
) {
    let playback_target = PlaybackTarget::SmartPlaylist(playlist.smart_playlist.id.clone());
    let surface = ContextMenuSurface::new(target, "smart-playlist", position);
    surface.append_action(msgid("Play"), "play");
    surface.append_action(msgid("Delete"), "delete");
    install_loaded_actions(&surface, shell, playback_target, true);
    surface.add_action("delete", {
        let smart_playlists = shell.products.smart_playlists.clone();
        let playlist_id = playlist.smart_playlist.id.clone();
        move || smart_playlists.delete(playlist_id.clone())
    });
    surface.popup();
}

fn install_loaded_actions(
    surface: &ContextMenuSurface,
    shell: &Rc<Shell>,
    target: PlaybackTarget,
    shuffled_start: bool,
) {
    for (action, placement) in [
        ("play", QueuePlacement::Now),
        ("play-next", QueuePlacement::Next),
        ("play-last", QueuePlacement::Last),
    ] {
        let queue = shell.products.playback.queue.clone();
        let shell = Rc::clone(shell);
        let target = target.clone();
        surface.add_action(action, move || {
            if let Some(request) = target.play_request(&shell, placement, shuffled_start) {
                queue.play_loaded(request);
            }
        });
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

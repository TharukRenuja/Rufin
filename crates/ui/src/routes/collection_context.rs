use std::{cell::RefCell, rc::Rc};

use ::library::{
    ActiveLibraryQuery, Album, Artist, ArtistId, FavoriteItemId, Genre, Playlist, SmartPlaylist,
    Track, play_context::ArtistTrackScope,
};
use adw::prelude::*;
use gtk::glib;
use sources::SourcePlaylistOperation;

use crate::favorites::{FAVORITE_ADD_ICON, FAVORITE_REMOVE_ICON};
use crate::interactions::{
    ADD_TO_PLAYLIST_ICON, ALBUM_ICON, ARTIST_ICON, ContextMenuSurface, RADIO_ICON,
    context_menu_action, context_menu_box, context_menu_submenu_action,
    install_context_menu_openers, radio_context_submenu,
};
use crate::player::state::current_playback_track;
use crate::preferences::dialogs::popup::present_light_dismiss_dialog;
use crate::shell::Shell;
use crate::shell::actions::{PLAY_ICON, PLAY_LATER_ICON, PLAY_NEXT_ICON, REMOVE_ICON};
use localization::{msgid, tr};
use playback::{
    AlbumPlayRequest, ArtistWindowPlayRequest, CachedPlaylistPlayRequest, ContextPlayRequest,
    QueuePlacement, RadioPlayRequest, RadioSeed, SmartPlaylistPlayRequest,
};

use super::detail_links::{album_artist_route, track_artist_route};
use super::play_context::selected_music_folder_id;
use super::playlist_entries::playlist_operation_supported;
use super::playlist_entries::{
    PlaylistEntryContextMenuAction, PlaylistEntryContextMenuState, confirm_remove_playlist_entry,
};
use super::playlist_picker::{
    context_menu_can_add_to_playlist, context_menu_context_picker_button,
    context_menu_picker_button,
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
            let track = context_track(&shell, &state.track);
            present_track_context_menu_inner(
                target,
                &shell,
                track,
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
            let track = context_track(&shell, &track);
            present_track_context_menu(target, &shell, track, position);
        }),
    );
}

pub(crate) fn install_album_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    album: Album,
) {
    install_dynamic_album_context_menu(target, shell, Rc::new(RefCell::new(Some(album))));
}

pub(crate) fn install_dynamic_album_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    album: Rc<RefCell<Option<Album>>>,
) {
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(album) = album.borrow().clone() else {
                return;
            };
            present_album_context_menu(target, &shell, context_album(&shell, &album), position);
        }),
    );
}

pub(crate) fn install_genre_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    genre: Genre,
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
    let target = target.as_ref();
    if let Some(track) = current_playback_track(&shell.playback.player.borrow()) {
        present_track_context_menu_above(target, shell, track, None);
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
    let main_menu = context_menu_box();
    main_menu.append(&context_menu_action("Play", "track.play", PLAY_ICON));
    main_menu.append(&context_menu_action(
        "Play Next",
        "track.play-next",
        PLAY_NEXT_ICON,
    ));
    main_menu.append(&context_menu_action(
        "Play Later",
        "track.play-last",
        PLAY_LATER_ICON,
    ));
    main_menu.append(&context_menu_submenu_action(
        msgid("Track radio"),
        "track.play-radio",
        RADIO_ICON,
        &radio_context_submenu("track"),
    ));

    if context_menu_can_add_to_playlist(shell) {
        let track_source: Rc<dyn Fn() -> Vec<Track>> = Rc::new({
            let track = track.clone();
            move || vec![track.clone()]
        });
        main_menu.append(&context_menu_picker_button(
            "Add to Playlist",
            ADD_TO_PLAYLIST_ICON,
            shell,
            track_source,
        ));
    }

    if track.favorite {
        main_menu.append(&context_menu_action(
            "Remove from Favorites",
            "track.favorite",
            FAVORITE_REMOVE_ICON,
        ));
    } else {
        main_menu.append(&context_menu_action(
            "Add to Favorites",
            "track.favorite",
            FAVORITE_ADD_ICON,
        ));
    }
    let artist_route = track_artist_route(&track);
    if artist_route.is_some() {
        main_menu.append(&context_menu_action(
            "Go to Artist",
            "track.go-artist",
            ARTIST_ICON,
        ));
    }
    main_menu.append(&context_menu_action(
        "Go to Album",
        "track.go-album",
        ALBUM_ICON,
    ));
    if remove_action.is_some() {
        main_menu.append(&context_menu_action(
            "Remove from playlist",
            "track.remove-from-playlist",
            REMOVE_ICON,
        ));
    }

    let surface =
        ContextMenuSurface::new(target, "track", "track-context-menu", position, &main_menu);
    if let Some(popover_position) = popover_position {
        surface.popover().set_position(popover_position);
    }

    surface.add_action("play", {
        let controller = shell.products.playback.queue.clone();
        let action_track = track.clone();
        move || {
            controller.play_now(action_track.clone());
        }
    });

    surface.add_action("play-next", {
        let controller = shell.products.playback.queue.clone();
        let action_track = track.clone();
        move || {
            controller.play_next(action_track.clone());
        }
    });

    surface.add_action("play-last", {
        let controller = shell.products.playback.queue.clone();
        let action_track = track.clone();
        move || {
            controller.play_last(vec![action_track.clone()]);
        }
    });

    surface.add_action("play-radio", {
        let controller = shell.products.playback.radio.clone();
        let action_track = track.clone();
        move || {
            controller.play_radio(RadioPlayRequest::now(RadioSeed::Track(
                action_track.clone(),
            )));
        }
    });

    surface.add_action("play-radio-next", {
        let controller = shell.products.playback.radio.clone();
        let action_track = track.clone();
        move || {
            controller.play_radio(RadioPlayRequest::next(RadioSeed::Track(
                action_track.clone(),
            )));
        }
    });

    surface.add_action("play-radio-last", {
        let controller = shell.products.playback.radio.clone();
        let action_track = track.clone();
        move || {
            controller.play_radio(RadioPlayRequest::last(RadioSeed::Track(
                action_track.clone(),
            )));
        }
    });

    surface.add_action("favorite", {
        let favorite_shell = Rc::clone(shell);
        let track_id = track.id.clone();
        let favorite = !track.favorite;
        move || {
            favorite_shell.set_favorite_with_feedback(
                FavoriteItemId::Track(track_id.clone()),
                favorite,
                None,
            );
        }
    });

    if let Some(artist_route) = artist_route {
        surface.add_action("go-artist", {
            let action_shell = Rc::clone(shell);
            move || {
                let shell = Rc::clone(&action_shell);
                let route = artist_route.clone();
                glib::idle_add_local_once(move || shell.navigate(route));
            }
        });
    }

    surface.add_action("go-album", {
        let go_album_shell = Rc::clone(shell);
        let album_id = track.album_id.clone();
        move || {
            let shell = Rc::clone(&go_album_shell);
            let album_id = album_id.clone();
            glib::idle_add_local_once(move || shell.navigate(Route::AlbumDetail(album_id)));
        }
    });

    if let Some(remove_action) = remove_action {
        surface.add_action("remove-from-playlist", {
            let shell = Rc::clone(shell);
            move || {
                confirm_remove_playlist_entry(
                    &shell,
                    remove_action.playlist_id.clone(),
                    remove_action.entry_id.clone(),
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
    album: Album,
    position: Option<(f64, f64)>,
) {
    let library_query = shell.library.query.borrow().clone();
    let main_menu = context_menu_box();
    main_menu.append(&context_menu_action("Play", "album.play", PLAY_ICON));
    main_menu.append(&context_menu_action(
        "Play Next",
        "album.play-next",
        PLAY_NEXT_ICON,
    ));
    main_menu.append(&context_menu_action(
        "Play Later",
        "album.play-last",
        PLAY_LATER_ICON,
    ));
    main_menu.append(&context_menu_submenu_action(
        msgid("Album radio"),
        "album.play-radio",
        RADIO_ICON,
        &radio_context_submenu("album"),
    ));

    if context_menu_can_add_to_playlist(shell) {
        let track_source: Rc<dyn Fn() -> Vec<Track>> = Rc::new({
            let library_query = library_query.clone();
            let album_id = album.id.clone();
            move || {
                library_query
                    .as_ref()
                    .and_then(|query| query.album_detail(&album_id).ok().flatten())
                    .map(|(_, tracks)| tracks)
                    .unwrap_or_default()
            }
        });
        main_menu.append(&context_menu_picker_button(
            "Add to Playlist",
            ADD_TO_PLAYLIST_ICON,
            shell,
            track_source,
        ));
    }

    if album.favorite {
        main_menu.append(&context_menu_action(
            "Remove from Favorites",
            "album.favorite",
            FAVORITE_REMOVE_ICON,
        ));
    } else {
        main_menu.append(&context_menu_action(
            "Add to Favorites",
            "album.favorite",
            FAVORITE_ADD_ICON,
        ));
    }
    let artist_route = album_artist_route(&album);
    if artist_route.is_some() {
        main_menu.append(&context_menu_action(
            "Go to Artist",
            "album.go-artist",
            ARTIST_ICON,
        ));
    }
    main_menu.append(&context_menu_action(
        "Go to Album",
        "album.go-album",
        ALBUM_ICON,
    ));

    let surface =
        ContextMenuSurface::new(target, "album", "album-context-menu", position, &main_menu);

    surface.add_action("play", {
        let controller = shell.products.playback.queue.clone();
        let library_query = library_query.clone();
        let album_id = album.id.clone();
        move || {
            if let Some((album, tracks)) = library_query
                .as_ref()
                .and_then(|query| query.album_detail(&album_id).ok().flatten())
            {
                controller.play_album(AlbumPlayRequest {
                    album_id: album.id,
                    tracks,
                    anchor_index: 0,
                    shuffled_start: true,
                });
            }
        }
    });

    surface.add_action("play-next", {
        let controller = shell.products.playback.queue.clone();
        let library_query = library_query.clone();
        let album_id = album.id.clone();
        move || {
            if let Some((_, tracks)) = library_query
                .as_ref()
                .and_then(|query| query.album_detail(&album_id).ok().flatten())
            {
                for track in tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        }
    });

    surface.add_action("play-last", {
        let controller = shell.products.playback.queue.clone();
        let library_query = library_query.clone();
        let album_id = album.id.clone();
        move || {
            if let Some((_, tracks)) = library_query
                .as_ref()
                .and_then(|query| query.album_detail(&album_id).ok().flatten())
            {
                controller.play_last(tracks);
            }
        }
    });

    surface.add_action("play-radio", {
        let controller = shell.products.playback.radio.clone();
        let album = album.clone();
        move || {
            controller.play_radio(RadioPlayRequest::now(RadioSeed::Album(album.clone())));
        }
    });

    surface.add_action("play-radio-next", {
        let controller = shell.products.playback.radio.clone();
        let album = album.clone();
        move || {
            controller.play_radio(RadioPlayRequest::next(RadioSeed::Album(album.clone())));
        }
    });

    surface.add_action("play-radio-last", {
        let controller = shell.products.playback.radio.clone();
        let album = album.clone();
        move || {
            controller.play_radio(RadioPlayRequest::last(RadioSeed::Album(album.clone())));
        }
    });

    surface.add_action("favorite", {
        let favorite_shell = Rc::clone(shell);
        let album_id = album.id.clone();
        let favorite = !album.favorite;
        move || {
            favorite_shell.set_favorite_with_feedback(
                FavoriteItemId::Album(album_id.clone()),
                favorite,
                None,
            );
        }
    });

    if let Some(artist_route) = artist_route {
        surface.add_action("go-artist", {
            let action_shell = Rc::clone(shell);
            move || {
                let shell = Rc::clone(&action_shell);
                let route = artist_route.clone();
                glib::idle_add_local_once(move || shell.navigate(route));
            }
        });
    }

    surface.add_action("go-album", {
        let shell = Rc::clone(shell);
        let album_id = album.id.clone();
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
    artist: Artist,
    position: Option<(f64, f64)>,
) {
    let library_query = shell.library.query.borrow().clone();
    let main_menu = context_menu_box();
    main_menu.append(&context_menu_action("Play", "artist.play", PLAY_ICON));
    main_menu.append(&context_menu_action(
        "Play Next",
        "artist.play-next",
        PLAY_NEXT_ICON,
    ));
    main_menu.append(&context_menu_action(
        "Play Later",
        "artist.play-last",
        PLAY_LATER_ICON,
    ));
    main_menu.append(&context_menu_submenu_action(
        msgid("Artist radio"),
        "artist.play-radio",
        RADIO_ICON,
        &radio_context_submenu("artist"),
    ));

    if context_menu_can_add_to_playlist(shell) {
        let track_source: Rc<dyn Fn() -> Vec<Track>> = Rc::new({
            let library_query = library_query.clone();
            let artist_id = artist.id.clone();
            move || {
                library_query
                    .as_ref()
                    .and_then(|query| artist_tracks_for_context(query, &artist_id))
                    .unwrap_or_default()
            }
        });
        main_menu.append(&context_menu_picker_button(
            "Add to Playlist",
            ADD_TO_PLAYLIST_ICON,
            shell,
            track_source,
        ));
    }

    if artist.favorite {
        main_menu.append(&context_menu_action(
            "Remove from Favorites",
            "artist.favorite",
            FAVORITE_REMOVE_ICON,
        ));
    } else {
        main_menu.append(&context_menu_action(
            "Add to Favorites",
            "artist.favorite",
            FAVORITE_ADD_ICON,
        ));
    }
    main_menu.append(&context_menu_action(
        "Go to Artist",
        "artist.go-artist",
        ARTIST_ICON,
    ));

    let surface = ContextMenuSurface::new(
        target,
        "artist",
        "artist-context-menu",
        position,
        &main_menu,
    );

    surface.add_action("play", {
        let controller = shell.products.playback.queue.clone();
        let library_query = library_query.clone();
        let artist_id = artist.id.clone();
        move || {
            if let Some(tracks) = library_query
                .as_ref()
                .and_then(|query| artist_tracks_for_context(query, &artist_id))
            {
                let total_items = tracks.len();
                controller.play_artist_window(ArtistWindowPlayRequest {
                    artist_id: artist_id.clone(),
                    scope: ArtistTrackScope::AllCredits,
                    total_items,
                    anchor_index: 0,
                    track_at: Box::new(move |index| tracks.get(index).cloned()),
                });
            }
        }
    });

    surface.add_action("play-next", {
        let controller = shell.products.playback.queue.clone();
        let library_query = library_query.clone();
        let artist_id = artist.id.clone();
        move || {
            if let Some(tracks) = library_query
                .as_ref()
                .and_then(|query| artist_tracks_for_context(query, &artist_id))
            {
                for track in tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        }
    });

    surface.add_action("play-last", {
        let controller = shell.products.playback.queue.clone();
        let library_query = library_query.clone();
        let artist_id = artist.id.clone();
        move || {
            if let Some(tracks) = library_query
                .as_ref()
                .and_then(|query| artist_tracks_for_context(query, &artist_id))
            {
                controller.play_last(tracks);
            }
        }
    });

    surface.add_action("play-radio", {
        let controller = shell.products.playback.radio.clone();
        let artist = artist.clone();
        move || {
            controller.play_radio(RadioPlayRequest::now(RadioSeed::Artist(artist.clone())));
        }
    });

    surface.add_action("play-radio-next", {
        let controller = shell.products.playback.radio.clone();
        let artist = artist.clone();
        move || {
            controller.play_radio(RadioPlayRequest::next(RadioSeed::Artist(artist.clone())));
        }
    });

    surface.add_action("play-radio-last", {
        let controller = shell.products.playback.radio.clone();
        let artist = artist.clone();
        move || {
            controller.play_radio(RadioPlayRequest::last(RadioSeed::Artist(artist.clone())));
        }
    });

    surface.add_action("favorite", {
        let favorite_shell = Rc::clone(shell);
        let artist_id = artist.id.clone();
        let favorite = !artist.favorite;
        move || {
            favorite_shell.set_favorite_with_feedback(
                FavoriteItemId::Artist(artist_id.clone()),
                favorite,
                None,
            );
        }
    });

    surface.add_action("go-artist", {
        let shell = Rc::clone(shell);
        let artist_id = artist.id.clone();
        move || {
            shell.navigate(Route::ArtistDetail(artist_id.clone()));
        }
    });

    surface.popup();
}
fn artist_tracks_for_context(
    query: &ActiveLibraryQuery,
    artist_id: &ArtistId,
) -> Option<Vec<Track>> {
    query
        .artist_detail(artist_id)
        .ok()
        .flatten()
        .map(|detail| detail.tracks)
        .filter(|tracks| !tracks.is_empty())
}

pub(crate) fn present_genre_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    genre: Genre,
    position: Option<(f64, f64)>,
) {
    let main_menu = context_menu_box();
    main_menu.append(&context_menu_action("Play", "genre.play", PLAY_ICON));
    main_menu.append(&context_menu_action(
        "Play Next",
        "genre.play-next",
        PLAY_NEXT_ICON,
    ));
    main_menu.append(&context_menu_action(
        "Play Later",
        "genre.play-last",
        PLAY_LATER_ICON,
    ));
    main_menu.append(&context_menu_submenu_action(
        msgid("Genre radio"),
        "genre.play-radio",
        RADIO_ICON,
        &radio_context_submenu("genre"),
    ));

    if context_menu_can_add_to_playlist(shell) {
        main_menu.append(&context_menu_context_picker_button(
            "Add to Playlist",
            ADD_TO_PLAYLIST_ICON,
            shell,
            ::library::play_context::PlayContextDescriptor::Genre {
                genre_id: genre.id.clone(),
                music_folder_id: selected_music_folder_id(shell),
            },
        ));
    }

    let surface =
        ContextMenuSurface::new(target, "genre", "genre-context-menu", position, &main_menu);

    surface.add_action("play", {
        let controller = shell.products.playback.queue.clone();
        let genre_id = genre.id.clone();
        let music_folder_id = selected_music_folder_id(shell);
        move || {
            controller.play_context(ContextPlayRequest::store(
                ::library::play_context::PlayContextDescriptor::Genre {
                    genre_id: genre_id.clone(),
                    music_folder_id: music_folder_id.clone(),
                },
                QueuePlacement::Now,
            ));
        }
    });

    surface.add_action("play-radio", {
        let controller = shell.products.playback.radio.clone();
        let genre = genre.clone();
        move || {
            controller.play_radio(RadioPlayRequest::now(RadioSeed::Genre(genre.clone())));
        }
    });

    surface.add_action("play-radio-next", {
        let controller = shell.products.playback.radio.clone();
        let genre = genre.clone();
        move || {
            controller.play_radio(RadioPlayRequest::next(RadioSeed::Genre(genre.clone())));
        }
    });

    surface.add_action("play-radio-last", {
        let controller = shell.products.playback.radio.clone();
        let genre = genre.clone();
        move || {
            controller.play_radio(RadioPlayRequest::last(RadioSeed::Genre(genre.clone())));
        }
    });

    surface.add_action("play-next", {
        let controller = shell.products.playback.queue.clone();
        let genre_id = genre.id.clone();
        let music_folder_id = selected_music_folder_id(shell);
        move || {
            controller.play_context(ContextPlayRequest::store(
                ::library::play_context::PlayContextDescriptor::Genre {
                    genre_id: genre_id.clone(),
                    music_folder_id: music_folder_id.clone(),
                },
                QueuePlacement::Next,
            ));
        }
    });

    surface.add_action("play-last", {
        let controller = shell.products.playback.queue.clone();
        let genre_id = genre.id.clone();
        let music_folder_id = selected_music_folder_id(shell);
        move || {
            controller.play_context(ContextPlayRequest::store(
                ::library::play_context::PlayContextDescriptor::Genre {
                    genre_id: genre_id.clone(),
                    music_folder_id: music_folder_id.clone(),
                },
                QueuePlacement::Last,
            ));
        }
    });

    surface.popup();
}

pub(crate) fn present_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: Playlist,
    position: Option<(f64, f64)>,
) {
    let menu = context_menu_box();
    menu.append(&context_menu_action("Play", "playlist.play", PLAY_ICON));
    menu.append(&context_menu_action(
        "Play Next",
        "playlist.play-next",
        PLAY_NEXT_ICON,
    ));
    menu.append(&context_menu_action(
        "Play Later",
        "playlist.play-last",
        PLAY_LATER_ICON,
    ));
    let radio_supported = shell
        .products
        .playback
        .radio
        .manual_radio_supported(sources::GeneratedTrackSeedKind::Playlist);
    if radio_supported {
        menu.append(&context_menu_submenu_action(
            msgid("Playlist radio"),
            "playlist.play-radio",
            RADIO_ICON,
            &radio_context_submenu("playlist"),
        ));
    }
    let can_delete =
        playlist_operation_supported(shell, &playlist, SourcePlaylistOperation::Delete);
    if can_delete {
        menu.append(&context_menu_action(
            "Delete",
            "playlist.delete",
            REMOVE_ICON,
        ));
    }

    let surface =
        ContextMenuSurface::new(target, "playlist", "playlist-context-menu", position, &menu);

    surface.add_action("play", {
        let controller = shell.products.playback.queue.clone();
        let playlist_id = playlist.id.clone();
        move || {
            controller.play_cached_playlist(CachedPlaylistPlayRequest::new(
                playlist_id.clone(),
                QueuePlacement::Now,
            ));
        }
    });

    surface.add_action("play-next", {
        let controller = shell.products.playback.queue.clone();
        let playlist_id = playlist.id.clone();
        move || {
            controller.play_cached_playlist(CachedPlaylistPlayRequest::new(
                playlist_id.clone(),
                QueuePlacement::Next,
            ));
        }
    });

    surface.add_action("play-last", {
        let controller = shell.products.playback.queue.clone();
        let playlist_id = playlist.id.clone();
        move || {
            controller.play_cached_playlist(CachedPlaylistPlayRequest::new(
                playlist_id.clone(),
                QueuePlacement::Last,
            ));
        }
    });

    if radio_supported {
        surface.add_action("play-radio", {
            let controller = shell.products.playback.radio.clone();
            let playlist = playlist.clone();
            move || {
                controller.play_radio(RadioPlayRequest::now(RadioSeed::Playlist(playlist.clone())));
            }
        });

        surface.add_action("play-radio-next", {
            let controller = shell.products.playback.radio.clone();
            let playlist = playlist.clone();
            move || {
                controller.play_radio(RadioPlayRequest::next(RadioSeed::Playlist(
                    playlist.clone(),
                )));
            }
        });

        surface.add_action("play-radio-last", {
            let controller = shell.products.playback.radio.clone();
            let playlist = playlist.clone();
            move || {
                controller.play_radio(RadioPlayRequest::last(RadioSeed::Playlist(
                    playlist.clone(),
                )));
            }
        });
    }

    if can_delete {
        surface.add_action("delete", {
            let library = shell.products.library.clone();
            let window = shell.chrome.window.clone();
            let playlist_id = playlist.id.clone();
            let playlist_name = playlist.name.clone();
            move || {
                let dialog = adw::AlertDialog::builder()
                    .heading(tr("Delete Playlist"))
                    .body(format!("Delete \"{playlist_name}\"?"))
                    .build();
                dialog.add_response("cancel", &tr("Cancel"));
                dialog.add_response("delete", &tr("Delete"));
                dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                let library = library.clone();
                let playlist_id = playlist_id.clone();
                dialog.connect_response(None, move |_, response| {
                    if response == "delete" {
                        library.delete_playlist(playlist_id.clone());
                    }
                });
                present_light_dismiss_dialog(&dialog, &window);
            }
        });
    }

    surface.popup();
}
pub(crate) fn present_smart_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: SmartPlaylist,
    position: Option<(f64, f64)>,
) {
    let library_query = shell.library.query.borrow().clone();
    let menu = context_menu_box();
    menu.append(&context_menu_action(
        "Play",
        "smart-playlist.play",
        PLAY_ICON,
    ));
    menu.append(&context_menu_action(
        "Delete",
        "smart-playlist.delete",
        REMOVE_ICON,
    ));

    let surface = ContextMenuSurface::new(
        target,
        "smart-playlist",
        "playlist-context-menu",
        position,
        &menu,
    );

    surface.add_action("play", {
        let controller = shell.products.playback.queue.clone();
        let shell = Rc::clone(shell);
        let library_query = library_query.clone();
        let playlist_id = playlist.id.clone();
        move || {
            if let Some(detail) = library_query
                .as_ref()
                .and_then(|query| query.smart_playlist_detail(&playlist_id).ok().flatten())
            {
                let first_track_id = detail.tracks.first().map(|track| track.id.clone());
                controller.play_smart_playlist(SmartPlaylistPlayRequest {
                    playlist: detail.smart_playlist,
                    anchor_track_id: first_track_id,
                    music_folder_id: selected_music_folder_id(&shell),
                });
            }
        }
    });

    surface.add_action("delete", {
        let library = shell.products.library.clone();
        let playlist_id = playlist.id.clone();
        move || {
            library.delete_smart_playlist(playlist_id.clone());
        }
    });

    surface.popup();
}
pub(crate) fn context_track(shell: &Rc<Shell>, fallback: &Track) -> Track {
    shell
        .library
        .query
        .borrow()
        .clone()
        .and_then(|query| query.track(&fallback.id).ok().flatten())
        .unwrap_or_else(|| fallback.clone())
}
pub(crate) fn context_album(shell: &Rc<Shell>, fallback: &Album) -> Album {
    shell
        .library
        .query
        .borrow()
        .clone()
        .and_then(|query| query.album_detail(&fallback.id).ok().flatten())
        .map(|(album, _)| album)
        .unwrap_or_else(|| fallback.clone())
}
pub(crate) fn context_artist(shell: &Rc<Shell>, fallback: &Artist) -> Artist {
    shell
        .library
        .query
        .borrow()
        .clone()
        .and_then(|query| query.artist_detail(&fallback.id).ok().flatten())
        .map(|detail| detail.artist)
        .unwrap_or_else(|| fallback.clone())
}

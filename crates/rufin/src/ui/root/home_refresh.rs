use std::cell::{Cell, RefCell};
use std::time::Duration;

use crate::i18n::{msgid, tr_with};

use super::*;

const CONTEXT_MENU_PLAYLIST_MAX_HEIGHT: i32 = 320;
const CONTEXT_MENU_PLAYLIST_MIN_WIDTH: i32 = 380;
const CONTEXT_PLAYLIST_ROW_COVER_SIZE: i32 = 48;
const CONTEXT_SUBMENU_CLOSE_DELAY_MS: u64 = 120;
const ADD_TO_PLAYLIST_DIALOG_WIDTH: i32 = 700;
const ADD_TO_PLAYLIST_DIALOG_HEIGHT: i32 = 510;
pub(in crate::ui) const ADD_TO_PLAYLIST_ICON: &str = "route-playlists-symbolic";
pub(in crate::ui) const ALBUM_ICON: &str = "route-albums-symbolic";
pub(in crate::ui) const ARTIST_ICON: &str = "route-artists-symbolic";
pub(in crate::ui) const FAVORITE_ADD_ICON: &str = "favorite-add";
pub(in crate::ui) const FAVORITE_REMOVE_ICON: &str = "favorite-remove";
pub(in crate::ui) const RADIO_ICON: &str = "audio-radio-symbolic";

thread_local! {
    static OPEN_CONTEXT_SUBMENU: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
}

#[derive(Clone, Debug)]
pub(in crate::ui) struct PlaylistEntryContextMenuAction {
    pub(in crate::ui) playlist_id: PlaylistId,
    pub(in crate::ui) entry_id: String,
    pub(in crate::ui) title: String,
}

#[derive(Clone, Debug)]
pub(in crate::ui) struct PlaylistEntryContextMenuState {
    pub(in crate::ui) track: Track,
    pub(in crate::ui) remove_action: PlaylistEntryContextMenuAction,
}

pub(in crate::ui) fn present_track_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    position: Option<(f64, f64)>,
) {
    present_track_context_menu_inner(target, shell, track, position, None, None);
}
pub(in crate::ui) fn present_track_context_menu_above(
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
pub(in crate::ui) fn present_track_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    remove_action: PlaylistEntryContextMenuAction,
    position: Option<(f64, f64)>,
) {
    present_track_context_menu_inner(target, shell, track, position, Some(remove_action), None);
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
    main_menu.append(&context_menu_action(
        "Play",
        "track.play",
        "media-playback-start-symbolic",
    ));
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
            "remove-minus",
        ));
    }

    let surface =
        ContextMenuSurface::new(target, "track", "track-context-menu", position, &main_menu);
    if let Some(popover_position) = popover_position {
        surface.popover().set_position(popover_position);
    }

    surface.add_action("play", {
        let controller = shell.controller.clone();
        let action_track = track.clone();
        move || {
            controller.play_now(action_track.clone());
        }
    });

    surface.add_action("play-next", {
        let controller = shell.controller.clone();
        let action_track = track.clone();
        move || {
            controller.play_next(action_track.clone());
        }
    });

    surface.add_action("play-last", {
        let controller = shell.controller.clone();
        let action_track = track.clone();
        move || {
            controller.play_last(vec![action_track.clone()]);
        }
    });

    surface.add_action("play-radio", {
        let controller = shell.controller.clone();
        let action_track = track.clone();
        move || {
            controller.play_track_radio(action_track.clone());
        }
    });

    surface.add_action("play-radio-next", {
        let controller = shell.controller.clone();
        let action_track = track.clone();
        move || {
            controller.play_track_radio_next(action_track.clone());
        }
    });

    surface.add_action("play-radio-last", {
        let controller = shell.controller.clone();
        let action_track = track.clone();
        move || {
            controller.play_track_radio_last(action_track.clone());
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
pub(in crate::ui) fn present_album_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    album: Album,
    position: Option<(f64, f64)>,
) {
    let main_menu = context_menu_box();
    main_menu.append(&context_menu_action(
        "Play",
        "album.play",
        "media-playback-start-symbolic",
    ));
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
            let controller = shell.controller.clone();
            let album_id = album.id.clone();
            move || {
                controller
                    .cached_album_detail(&album_id)
                    .ok()
                    .flatten()
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
        let controller = shell.controller.clone();
        let album_id = album.id.clone();
        move || {
            controller.play_album_now(album_id.clone());
        }
    });

    surface.add_action("play-next", {
        let controller = shell.controller.clone();
        let album_id = album.id.clone();
        move || {
            if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
                for track in tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        }
    });

    surface.add_action("play-last", {
        let controller = shell.controller.clone();
        let album_id = album.id.clone();
        move || {
            if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
                controller.play_last(tracks);
            }
        }
    });

    surface.add_action("play-radio", {
        let controller = shell.controller.clone();
        let album = album.clone();
        move || {
            controller.play_album_radio(album.clone());
        }
    });

    surface.add_action("play-radio-next", {
        let controller = shell.controller.clone();
        let album = album.clone();
        move || {
            controller.play_album_radio_next(album.clone());
        }
    });

    surface.add_action("play-radio-last", {
        let controller = shell.controller.clone();
        let album = album.clone();
        move || {
            controller.play_album_radio_last(album.clone());
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
pub(in crate::ui) fn present_artist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    artist: Artist,
    position: Option<(f64, f64)>,
) {
    let main_menu = context_menu_box();
    main_menu.append(&context_menu_action(
        "Play",
        "artist.play",
        "media-playback-start-symbolic",
    ));
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
            let controller = shell.controller.clone();
            let artist_id = artist.id.clone();
            move || artist_tracks_for_context(&controller, &artist_id).unwrap_or_default()
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
        let controller = shell.controller.clone();
        let artist_id = artist.id.clone();
        move || {
            if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
                controller.play_artist_tracks_window(
                    artist_id.clone(),
                    ArtistTrackScope::AllCredits,
                    tracks.len(),
                    0,
                    |index| tracks.get(index).cloned(),
                );
            }
        }
    });

    surface.add_action("play-next", {
        let controller = shell.controller.clone();
        let artist_id = artist.id.clone();
        move || {
            if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
                for track in tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        }
    });

    surface.add_action("play-last", {
        let controller = shell.controller.clone();
        let artist_id = artist.id.clone();
        move || {
            if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
                controller.play_last(tracks);
            }
        }
    });

    surface.add_action("play-radio", {
        let controller = shell.controller.clone();
        let artist = artist.clone();
        move || {
            controller.play_artist_radio(artist.clone());
        }
    });

    surface.add_action("play-radio-next", {
        let controller = shell.controller.clone();
        let artist = artist.clone();
        move || {
            controller.play_artist_radio_next(artist.clone());
        }
    });

    surface.add_action("play-radio-last", {
        let controller = shell.controller.clone();
        let artist = artist.clone();
        move || {
            controller.play_artist_radio_last(artist.clone());
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
pub(in crate::ui) fn artist_tracks_for_context(
    controller: &AppController,
    artist_id: &ArtistId,
) -> Option<Vec<Track>> {
    controller
        .cached_artist_detail(artist_id)
        .ok()
        .flatten()
        .map(|detail| detail.tracks)
        .filter(|tracks| !tracks.is_empty())
}

pub(in crate::ui) fn present_genre_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    genre: Genre,
    position: Option<(f64, f64)>,
) {
    let main_menu = context_menu_box();
    main_menu.append(&context_menu_action(
        "Play",
        "genre.play",
        "media-playback-start-symbolic",
    ));
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
        let controller = shell.controller.clone();
        let genre_id = genre.id.clone();
        let track_source: Rc<dyn Fn() -> Vec<Track>> = Rc::new(move || {
            controller
                .cached_genre_detail(&genre_id)
                .ok()
                .flatten()
                .map(|detail| detail.tracks)
                .unwrap_or_default()
        });
        main_menu.append(&context_menu_picker_button(
            "Add to Playlist",
            ADD_TO_PLAYLIST_ICON,
            shell,
            track_source,
        ));
    }

    let surface =
        ContextMenuSurface::new(target, "genre", "genre-context-menu", position, &main_menu);

    surface.add_action("play", {
        let controller = shell.controller.clone();
        let genre_id = genre.id.clone();
        move || {
            if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
                let tracks = detail.tracks;
                controller.play_genre_tracks_window(genre_id.clone(), tracks.len(), 0, |index| {
                    tracks.get(index).cloned()
                });
            }
        }
    });

    surface.add_action("play-radio", {
        let controller = shell.controller.clone();
        let genre = genre.clone();
        move || {
            controller.play_genre_radio(genre.clone());
        }
    });

    surface.add_action("play-radio-next", {
        let controller = shell.controller.clone();
        let genre = genre.clone();
        move || {
            controller.play_genre_radio_next(genre.clone());
        }
    });

    surface.add_action("play-radio-last", {
        let controller = shell.controller.clone();
        let genre = genre.clone();
        move || {
            controller.play_genre_radio_last(genre.clone());
        }
    });

    surface.add_action("play-next", {
        let controller = shell.controller.clone();
        let genre_id = genre.id.clone();
        move || {
            if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
                for track in detail.tracks.iter().rev() {
                    controller.play_next(track.clone());
                }
            }
        }
    });

    surface.add_action("play-last", {
        let controller = shell.controller.clone();
        let genre_id = genre.id.clone();
        move || {
            if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
                controller.play_last(detail.tracks);
            }
        }
    });

    surface.popup();
}

pub(in crate::ui) fn present_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: Playlist,
    position: Option<(f64, f64)>,
) {
    let menu = context_menu_box();
    menu.append(&context_menu_action(
        "Play",
        "playlist.play",
        "media-playback-start-symbolic",
    ));
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
    menu.append(&context_menu_submenu_action(
        msgid("Playlist radio"),
        "playlist.play-radio",
        RADIO_ICON,
        &radio_context_submenu("playlist"),
    ));
    menu.append(&context_menu_action(
        "Delete",
        "playlist.delete",
        "window-close-symbolic",
    ));

    let surface =
        ContextMenuSurface::new(target, "playlist", "playlist-context-menu", position, &menu);

    surface.add_action("play", {
        let controller = shell.controller.clone();
        let playlist_id = playlist.id.clone();
        move || {
            controller.play_cached_playlist(playlist_id.clone());
        }
    });

    surface.add_action("play-next", {
        let controller = shell.controller.clone();
        let playlist_id = playlist.id.clone();
        move || {
            controller.play_cached_playlist_next(playlist_id.clone());
        }
    });

    surface.add_action("play-last", {
        let controller = shell.controller.clone();
        let playlist_id = playlist.id.clone();
        move || {
            controller.play_cached_playlist_last(playlist_id.clone());
        }
    });

    surface.add_action("play-radio", {
        let controller = shell.controller.clone();
        let playlist = playlist.clone();
        move || {
            controller.play_playlist_radio(playlist.clone());
        }
    });

    surface.add_action("play-radio-next", {
        let controller = shell.controller.clone();
        let playlist = playlist.clone();
        move || {
            controller.play_playlist_radio_next(playlist.clone());
        }
    });

    surface.add_action("play-radio-last", {
        let controller = shell.controller.clone();
        let playlist = playlist.clone();
        move || {
            controller.play_playlist_radio_last(playlist.clone());
        }
    });

    surface.add_action("delete", {
        let controller = shell.controller.clone();
        let window = shell.window.clone();
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
            let controller = controller.clone();
            let playlist_id = playlist_id.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "delete" {
                    controller.delete_playlist(playlist_id.clone());
                }
            });
            present_light_dismiss_dialog(&dialog, &window);
        }
    });

    surface.popup();
}
pub(in crate::ui) fn present_smart_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: SmartPlaylist,
    position: Option<(f64, f64)>,
) {
    let menu = context_menu_box();
    menu.append(&context_menu_action(
        "Play",
        "smart-playlist.play",
        "media-playback-start-symbolic",
    ));
    menu.append(&context_menu_action(
        "Delete",
        "smart-playlist.delete",
        "window-close-symbolic",
    ));

    let surface = ContextMenuSurface::new(
        target,
        "smart-playlist",
        "playlist-context-menu",
        position,
        &menu,
    );

    surface.add_action("play", {
        let controller = shell.controller.clone();
        let playlist_id = playlist.id.clone();
        move || {
            if let Ok(Some(detail)) = controller.cached_smart_playlist_detail(&playlist_id) {
                controller.play_smart_playlist_detail(detail);
            }
        }
    });

    surface.add_action("delete", {
        let controller = shell.controller.clone();
        let playlist_id = playlist.id.clone();
        move || {
            controller.delete_smart_playlist(playlist_id.clone());
        }
    });

    surface.popup();
}
pub(in crate::ui) struct ContextMenuSurface {
    target: gtk::Widget,
    group_name: &'static str,
    popover: gtk::Popover,
    actions: gio::SimpleActionGroup,
}

impl ContextMenuSurface {
    pub(in crate::ui) fn new(
        target: &gtk::Widget,
        group_name: &'static str,
        css_class: &str,
        position: Option<(f64, f64)>,
        child: &impl IsA<gtk::Widget>,
    ) -> Self {
        Self {
            target: target.clone(),
            group_name,
            popover: context_popover(target, css_class, position, child),
            actions: gio::SimpleActionGroup::new(),
        }
    }

    pub(in crate::ui) fn popover(&self) -> &gtk::Popover {
        &self.popover
    }

    pub(in crate::ui) fn add_action(&self, name: &str, run: impl Fn() + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let popover = self.popover.downgrade();
        action.connect_activate(move |_, _| {
            popdown_current_context_submenu();
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
            run();
        });
        self.actions.add_action(&action);
    }

    pub(in crate::ui) fn popup(self) {
        self.target
            .insert_action_group(self.group_name, Some(&self.actions));
        self.popover.connect_closed(move |popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || {
                popdown_current_context_submenu();
                popover.unparent();
            });
        });
        self.popover.popup();
    }
}

pub(in crate::ui) fn context_menu_box() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, 0)
}
pub(in crate::ui) fn context_menu_scroll_page(
    child: &impl IsA<gtk::Widget>,
) -> gtk::ScrolledWindow {
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_min_content_width(CONTEXT_MENU_PLAYLIST_MIN_WIDTH);
    scroller.set_propagate_natural_width(true);
    scroller.set_propagate_natural_height(false);
    scroller.set_max_content_height(CONTEXT_MENU_PLAYLIST_MAX_HEIGHT);
    scroller.set_vexpand(true);
    scroller.set_child(Some(child));
    scroller
}
#[derive(Clone)]
pub(in crate::ui) struct PlaylistPickerRow {
    playlist: Playlist,
    row: gtk::Widget,
    check: gtk::CheckButton,
    haystack: String,
}
#[derive(Clone)]
pub(in crate::ui) struct PlaylistPickerHandle {
    list: gtk::Box,
    rows: Rc<RefCell<Vec<PlaylistPickerRow>>>,
    create: gtk::Button,
    search: gtk::SearchEntry,
    add_button: gtk::Button,
}
fn present_context_playlist_picker_dialog(
    shell: &Rc<Shell>,
    track_source: Rc<dyn Fn() -> Vec<Track>>,
) {
    let content = context_playlist_picker(shell, track_source);
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(&tr("Add to Playlist"), "")));
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));

    let dialog = adw::Dialog::builder()
        .title(tr("Add to Playlist"))
        .content_width(ADD_TO_PLAYLIST_DIALOG_WIDTH)
        .content_height(ADD_TO_PLAYLIST_DIALOG_HEIGHT)
        .child(&toolbar)
        .build();
    let shell_for_close = Rc::clone(shell);
    dialog.connect_closed(move |_| {
        *shell_for_close.state.context_playlist_picker.borrow_mut() = None;
    });
    present_light_dismiss_dialog(&dialog, &shell.window);
}
fn context_playlist_picker(
    shell: &Rc<Shell>,
    track_source: Rc<dyn Fn() -> Vec<Track>>,
) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.add_css_class("context-playlist-picker");
    root.set_margin_top(12);
    root.set_margin_bottom(14);
    root.set_margin_start(18);
    root.set_margin_end(18);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&tr("Type to search or create a new playlist")));
    root.append(&search);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let rows = Rc::new(RefCell::new(Vec::<PlaylistPickerRow>::new()));
    let create = playlist_create_row("");
    create.set_visible(false);
    list.append(&create);
    let add_button = gtk::Button::with_label(&tr("Add"));
    add_button.add_css_class("suggested-action");
    add_button.set_sensitive(false);
    let scroller = context_menu_scroll_page(&list);
    root.append(&scroller);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let skip = gtk::CheckButton::with_label(&tr("Don't duplicate"));
    skip.set_active(true);
    footer.append(&skip);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    cancel.connect_clicked(close_context_surface);
    footer.append(&cancel);
    footer.append(&add_button);
    root.append(&footer);

    let handle = PlaylistPickerHandle {
        list: list.clone(),
        rows: Rc::clone(&rows),
        create: create.clone(),
        search: search.clone(),
        add_button: add_button.clone(),
    };
    refresh_playlist_picker_rows(shell, &handle, &context_menu_playlists(shell));
    *shell.state.context_playlist_picker.borrow_mut() = Some(handle.clone());

    let handle_for_search = handle.clone();
    search.connect_search_changed(move |entry| {
        let text = entry.text();
        let label = create_playlist_label(text.trim());
        let query = text.trim().to_lowercase();
        handle_for_search.create.set_label(&label);
        sync_playlist_picker_filter(&handle_for_search, &query);
    });

    let controller = shell.controller.clone();
    let track_source_for_create = Rc::clone(&track_source);
    create.connect_clicked(move |_| {
        let name = search.text().trim().to_string();
        if !name.is_empty() {
            controller.create_playlist(name, track_source_for_create());
            search.set_text("");
        }
    });

    let rows_for_add = Rc::clone(&rows);
    let controller = shell.controller.clone();
    let toast_overlay = shell.quick_toast_overlay.clone();
    add_button.connect_clicked(move |button| {
        let tracks = track_source();
        if tracks.is_empty() {
            close_context_surface(button);
            return;
        }
        let mut added_tracks = 0;
        let mut changed_playlists = 0;
        for row in rows_for_add
            .borrow()
            .iter()
            .filter(|row| row.check.is_active())
        {
            let tracks =
                playlist_tracks_to_add(&controller, &row.playlist.id, &tracks, skip.is_active());
            if !tracks.is_empty() {
                added_tracks += tracks.len();
                changed_playlists += 1;
                controller.add_tracks_to_playlist(row.playlist.id.clone(), tracks);
            }
        }
        let toast = adw::Toast::new(&playlist_add_toast(added_tracks, changed_playlists));
        toast.set_timeout(2);
        toast_overlay.add_toast(toast);
        close_context_surface(button);
    });

    root
}
pub(in crate::ui) fn refresh_context_playlist_picker(shell: &Rc<Shell>) {
    let Some(handle) = shell.state.context_playlist_picker.borrow().clone() else {
        return;
    };
    refresh_playlist_picker_rows(shell, &handle, &context_menu_playlists(shell));
}
fn refresh_playlist_picker_rows(
    shell: &Rc<Shell>,
    handle: &PlaylistPickerHandle,
    playlists: &[Playlist],
) {
    while let Some(child) = handle.list.first_child() {
        handle.list.remove(&child);
    }
    handle.list.append(&handle.create);
    handle.rows.borrow_mut().clear();
    for playlist in playlists {
        let (row, check, haystack) = playlist_picker_row(shell, playlist);
        handle.list.append(&row);
        handle.rows.borrow_mut().push(PlaylistPickerRow {
            playlist: playlist.clone(),
            row: row.upcast::<gtk::Widget>(),
            check: check.clone(),
            haystack,
        });
        let rows_for_check = Rc::clone(&handle.rows);
        let add_for_check = handle.add_button.clone();
        check.connect_toggled(move |_| {
            update_playlist_picker_add_button(&rows_for_check, &add_for_check)
        });
    }
    let query = handle.search.text().trim().to_lowercase();
    sync_playlist_picker_filter(handle, &query);
}
fn sync_playlist_picker_filter(handle: &PlaylistPickerHandle, query: &str) {
    handle.create.set_visible(show_create_playlist_row(query));
    for row in handle.rows.borrow().iter() {
        row.row
            .set_visible(query.is_empty() || row.haystack.contains(query));
    }
    update_playlist_picker_add_button(&handle.rows, &handle.add_button);
}
fn playlist_create_row(name: &str) -> gtk::Button {
    let button = gtk::Button::with_label(&create_playlist_label(name));
    button.add_css_class("flat");
    button.add_css_class("context-playlist-row");
    button.add_css_class("context-playlist-create-row");
    button.set_halign(gtk::Align::Fill);
    button
}
fn create_playlist_label(name: &str) -> String {
    format!("+ {} {}", tr("Create"), name)
}
fn show_create_playlist_row(query: &str) -> bool {
    !query.trim().is_empty()
}
fn playlist_picker_row(
    shell: &Rc<Shell>,
    playlist: &Playlist,
) -> (gtk::Box, gtk::CheckButton, String) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("context-playlist-row");
    row.set_margin_top(4);
    row.set_margin_bottom(4);

    let check = gtk::CheckButton::new();
    row.append(&check);
    row.append(&playlist_picker_cover(shell, playlist));

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let title = gtk::Label::new(Some(&playlist.name));
    title.add_css_class("context-playlist-title");
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&title);

    let meta = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    meta.add_css_class("context-playlist-meta");
    meta.append(&playlist_picker_meta(
        "route-tracks-symbolic",
        &track_count_text(playlist.track_count.into()),
    ));
    meta.append(&playlist_picker_meta(
        "appointment-soon-symbolic",
        &format_duration_units(playlist.duration_seconds),
    ));
    let genres = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    genres.add_css_class("context-playlist-genres");
    for genre in playlist.top_genres.iter().take(2) {
        genres.append(&playlist_genre_pill(genre));
    }
    genres.set_visible(genres.first_child().is_some());
    meta.append(&genres);
    text.append(&meta);
    row.append(&text);

    let haystack = format!(
        "{} {} {}",
        playlist.name,
        playlist.track_count,
        format_duration_units(playlist.duration_seconds)
    )
    .to_lowercase();
    (row, check, haystack)
}
fn playlist_genre_pill(name: &str) -> gtk::Label {
    let pill = gtk::Label::new(Some(name));
    pill.add_css_class("album-detail-genre-pill");
    pill
}
fn playlist_picker_cover(shell: &Rc<Shell>, playlist: &Playlist) -> gtk::Widget {
    let settings = shell.state.settings.borrow();
    let image_refs = crate::cover_art_policy::selected_collection_refs(
        &playlist.image_refs,
        playlist.image_ref.as_ref(),
        settings.prefer_server_playlist_covers,
    );
    let cover = shell.cover_collection_tile_for(
        image_refs.first(),
        stable_seed(playlist.id.as_str()),
        CONTEXT_PLAYLIST_ROW_COVER_SIZE,
        THUMB_COVER_SIZE,
    );
    cover.add_css_class("context-playlist-cover");
    cover
}
fn playlist_picker_meta(icon_name: &str, text: &str) -> gtk::Box {
    let item = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.add_css_class("muted");
    icon.set_pixel_size(13);
    item.append(&icon);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.set_xalign(0.0);
    item.append(&label);
    item
}
fn playlist_add_toast(added_tracks: usize, playlist_count: usize) -> String {
    if added_tracks == 0 {
        return tr("No songs added");
    }
    let track_count = added_tracks.to_string();
    let playlist_count_text = playlist_count.to_string();
    let args = [
        ("track_count", track_count.as_str()),
        ("playlist_count", playlist_count_text.as_str()),
    ];
    match (added_tracks == 1, playlist_count == 1) {
        (true, true) => tr_with(
            "{track_count} song added to {playlist_count} playlist",
            &args,
        ),
        (true, false) => tr_with(
            "{track_count} song added to {playlist_count} playlists",
            &args,
        ),
        (false, true) => tr_with(
            "{track_count} songs added to {playlist_count} playlist",
            &args,
        ),
        (false, false) => tr_with(
            "{track_count} songs added to {playlist_count} playlists",
            &args,
        ),
    }
}
fn playlist_tracks_to_add(
    controller: &AppController,
    playlist_id: &PlaylistId,
    tracks: &[Track],
    skip_duplicates: bool,
) -> Vec<Track> {
    if !skip_duplicates {
        return tracks.to_vec();
    }
    let Ok(Some(detail)) = controller.cached_playlist_detail(playlist_id) else {
        return tracks.to_vec();
    };
    if detail.entries.is_empty() {
        filter_existing_tracks(tracks, &detail.tracks)
    } else {
        filter_duplicate_tracks(tracks, &detail.entries)
    }
}
fn filter_duplicate_tracks(tracks: &[Track], entries: &[source::PlaylistEntry]) -> Vec<Track> {
    filter_existing_tracks(
        tracks,
        &entries
            .iter()
            .map(|entry| entry.track.clone())
            .collect::<Vec<_>>(),
    )
}
fn filter_existing_tracks(tracks: &[Track], existing: &[Track]) -> Vec<Track> {
    tracks
        .iter()
        .filter(|track| !existing.iter().any(|existing| existing.id == track.id))
        .cloned()
        .collect()
}
fn update_playlist_picker_add_button(
    rows: &Rc<RefCell<Vec<PlaylistPickerRow>>>,
    button: &gtk::Button,
) {
    button.set_sensitive(rows.borrow().iter().any(|row| row.check.is_active()));
}
fn close_context_surface(widget: &impl IsA<gtk::Widget>) {
    if let Some(popover) = widget
        .as_ref()
        .ancestor(gtk::Popover::static_type())
        .and_then(|widget| widget.downcast::<gtk::Popover>().ok())
    {
        popover.popdown();
        return;
    }
    if let Some(dialog) = widget
        .as_ref()
        .ancestor(adw::Dialog::static_type())
        .and_then(|widget| widget.downcast::<adw::Dialog>().ok())
    {
        dialog.close();
    }
}
pub(in crate::ui) fn context_menu_action(
    label: &str,
    action: &str,
    icon_name: &str,
) -> gtk::Button {
    context_menu_action_with_label(&tr(label), action, icon_name)
}
pub(in crate::ui) fn context_menu_action_with_label(
    label: &str,
    action: &str,
    icon_name: &str,
) -> gtk::Button {
    let button = context_menu_button(label, icon_name);
    button.set_action_name(Some(action));
    button
}
pub(in crate::ui) fn context_menu_submenu_action(
    label: &str,
    action: &str,
    icon_name: &str,
    submenu: &impl IsA<gtk::Widget>,
) -> gtk::Button {
    let button = context_menu_disclosure_button(&tr(label), icon_name);
    button.set_action_name(Some(action));

    let popover = gtk::Popover::new();
    popover.add_css_class("context-submenu");
    popover.set_autohide(false);
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Right);
    popover.set_child(Some(submenu));
    popover.set_parent(&button);

    let button_hovered = Rc::new(Cell::new(false));
    let submenu_hovered = Rc::new(Cell::new(false));

    let motion = gtk::EventControllerMotion::new();
    let popover_for_enter = popover.clone();
    let button_hovered_for_enter = Rc::clone(&button_hovered);
    motion.connect_enter(move |_, _, _| {
        button_hovered_for_enter.set(true);
        popup_context_submenu(&popover_for_enter);
    });
    let popover_for_leave = popover.clone();
    let button_hovered_for_leave = Rc::clone(&button_hovered);
    let submenu_hovered_for_leave = Rc::clone(&submenu_hovered);
    motion.connect_leave(move |_| {
        button_hovered_for_leave.set(false);
        schedule_context_submenu_popdown(
            &popover_for_leave,
            Rc::clone(&button_hovered_for_leave),
            Rc::clone(&submenu_hovered_for_leave),
        );
    });
    button.add_controller(motion);

    let submenu_motion = gtk::EventControllerMotion::new();
    let submenu_hovered_for_enter = Rc::clone(&submenu_hovered);
    submenu_motion.connect_enter(move |_, _, _| {
        submenu_hovered_for_enter.set(true);
    });
    let popover_for_leave = popover.clone();
    let button_hovered_for_leave = Rc::clone(&button_hovered);
    let submenu_hovered_for_leave = Rc::clone(&submenu_hovered);
    submenu_motion.connect_leave(move |_| {
        submenu_hovered_for_leave.set(false);
        schedule_context_submenu_popdown(
            &popover_for_leave,
            Rc::clone(&button_hovered_for_leave),
            Rc::clone(&submenu_hovered_for_leave),
        );
    });
    popover.add_controller(submenu_motion);

    button.connect_unrealize(move |_| {
        forget_context_submenu(&popover);
        popover.unparent();
    });
    button
}
pub(in crate::ui) fn radio_context_submenu(group: &str) -> gtk::Box {
    let menu = context_menu_box();
    menu.append(&context_menu_action(
        "Play",
        &format!("{group}.play-radio"),
        "media-playback-start-symbolic",
    ));
    menu.append(&context_menu_action(
        "Play Next",
        &format!("{group}.play-radio-next"),
        PLAY_NEXT_ICON,
    ));
    menu.append(&context_menu_action(
        "Play Later",
        &format!("{group}.play-radio-last"),
        PLAY_LATER_ICON,
    ));
    menu
}
pub(in crate::ui) fn context_menu_picker_button(
    label: &str,
    icon_name: &str,
    shell: &Rc<Shell>,
    track_source: Rc<dyn Fn() -> Vec<Track>>,
) -> gtk::Button {
    let button = context_menu_button(&tr(label), icon_name);
    let shell = Rc::clone(shell);
    button.connect_clicked(move |button| {
        close_context_surface(button);
        present_context_playlist_picker_dialog(&shell, Rc::clone(&track_source));
    });
    button
}
pub(in crate::ui) fn context_popover(
    target: &gtk::Widget,
    css_class: &str,
    position: Option<(f64, f64)>,
    child: &impl IsA<gtk::Widget>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_autohide(true);
    popover.add_css_class(css_class);
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_parent(target);
    popover.set_child(Some(child));
    if let Some((x, y)) = position {
        popover.add_css_class("context-menu-opening");
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }
    let motion = gtk::EventControllerMotion::new();
    let popover_for_motion = popover.clone();
    motion.connect_motion(move |_, _, _| {
        popover_for_motion.remove_css_class("context-menu-opening");
    });
    popover.add_controller(motion);
    popover
}
fn context_menu_button(label: &str, icon_name: &str) -> gtk::Button {
    let row = context_menu_button_content(label, icon_name);
    let button = gtk::Button::builder()
        .child(&row)
        .tooltip_text(label)
        .halign(gtk::Align::Fill)
        .hexpand(true)
        .build();
    button.add_css_class("flat");
    button.add_css_class("context-menu-button");
    button
}
fn context_menu_disclosure_button(label: &str, icon_name: &str) -> gtk::Button {
    let row = context_menu_button_content(label, icon_name);
    let arrow = gtk::Image::from_icon_name("go-next-symbolic");
    arrow.add_css_class("context-submenu-arrow");
    arrow.set_pixel_size(14);
    row.append(&arrow);

    let button = gtk::Button::builder()
        .child(&row)
        .tooltip_text(label)
        .halign(gtk::Align::Fill)
        .hexpand(true)
        .build();
    button.add_css_class("flat");
    button.add_css_class("context-menu-button");
    button
}
fn context_menu_button_content(label: &str, icon_name: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::Fill);
    row.set_hexpand(true);

    let icon = context_menu_icon(icon_name);
    row.append(&icon);

    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&text);
    row
}
fn context_menu_icon(icon_name: &str) -> gtk::Widget {
    let icon = if icon_name == FAVORITE_REMOVE_ICON {
        gtk::Label::new(Some(FAVORITE_FILLED_GLYPH)).upcast::<gtk::Widget>()
    } else if icon_name == FAVORITE_ADD_ICON {
        gtk::Label::new(Some(FAVORITE_EMPTY_GLYPH)).upcast::<gtk::Widget>()
    } else if icon_name == "remove-minus" {
        gtk::Label::new(Some("−")).upcast::<gtk::Widget>()
    } else {
        let image = gtk::Image::from_icon_name(icon_name);
        let pixel_size = if icon_name == "media-playback-start-symbolic" {
            12
        } else if icon_name == ADD_TO_PLAYLIST_ICON {
            16
        } else if matches!(
            icon_name,
            PLAY_NEXT_ICON | PLAY_LATER_ICON | ARTIST_ICON | ALBUM_ICON | RADIO_ICON
        ) {
            20
        } else {
            18
        };
        image.set_pixel_size(pixel_size);
        image.upcast::<gtk::Widget>()
    };
    icon.add_css_class("context-menu-icon");
    icon.set_size_request(20, 20);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon
}
fn popup_context_submenu(popover: &gtk::Popover) {
    OPEN_CONTEXT_SUBMENU.with(|current| {
        let previous = current.borrow().clone();
        if let Some(previous) = previous
            && previous != *popover
        {
            previous.popdown();
        }
        *current.borrow_mut() = Some(popover.clone());
    });
    popover.popup();
}
fn schedule_context_submenu_popdown(
    popover: &gtk::Popover,
    button_hovered: Rc<Cell<bool>>,
    submenu_hovered: Rc<Cell<bool>>,
) {
    let popover = popover.clone();
    glib::timeout_add_local_once(
        Duration::from_millis(CONTEXT_SUBMENU_CLOSE_DELAY_MS),
        move || {
            if !button_hovered.get() && !submenu_hovered.get() {
                forget_context_submenu(&popover);
                popover.popdown();
            }
        },
    );
}
fn popdown_current_context_submenu() {
    OPEN_CONTEXT_SUBMENU.with(|current| {
        if let Some(popover) = current.borrow_mut().take() {
            popover.popdown();
        }
    });
}
fn forget_context_submenu(popover: &gtk::Popover) {
    OPEN_CONTEXT_SUBMENU.with(|current| {
        let is_current = current
            .borrow()
            .as_ref()
            .is_some_and(|current| current == popover);
        if is_current {
            current.borrow_mut().take();
        }
    });
}
pub(in crate::ui) fn context_menu_can_add_to_playlist(shell: &Rc<Shell>) -> bool {
    shell.state.library.borrow().server.is_some()
}
pub(in crate::ui) fn context_menu_playlists(shell: &Rc<Shell>) -> Vec<Playlist> {
    context_menu_playlist_items(&shell.state.library.borrow().playlists)
}
fn context_menu_playlist_items(playlists: &[Playlist]) -> Vec<Playlist> {
    playlists.to_vec()
}
pub(in crate::ui) fn context_track(shell: &Rc<Shell>, fallback: &Track) -> Track {
    let library = shell.state.library.borrow();
    library_track(&library, &fallback.id).unwrap_or_else(|| fallback.clone())
}
pub(in crate::ui) fn library_track(library: &LibrarySnapshot, track_id: &TrackId) -> Option<Track> {
    library
        .tracks
        .iter()
        .chain(library.favorites.iter())
        .chain(library.search.tracks.iter())
        .chain(
            library
                .home_sections
                .iter()
                .flat_map(|section| section.tracks.iter()),
        )
        .find(|track| track.id == *track_id)
        .cloned()
}
pub(in crate::ui) fn context_album(shell: &Rc<Shell>, fallback: &Album) -> Album {
    {
        let library = shell.state.library.borrow();
        library_album(&library, &fallback.id)
    }
    .unwrap_or_else(|| fallback.clone())
}
pub(in crate::ui) fn context_artist(shell: &Rc<Shell>, fallback: &Artist) -> Artist {
    {
        let library = shell.state.library.borrow();
        library_artist(&library, &fallback.id)
    }
    .unwrap_or_else(|| fallback.clone())
}
pub(in crate::ui) fn library_album(library: &LibrarySnapshot, album_id: &AlbumId) -> Option<Album> {
    library
        .albums
        .iter()
        .chain(library.search.albums.iter())
        .chain(
            library
                .home_sections
                .iter()
                .flat_map(|section| section.albums.iter()),
        )
        .find(|album| album.id == *album_id)
        .cloned()
}
pub(in crate::ui) fn library_artist(
    library: &LibrarySnapshot,
    artist_id: &ArtistId,
) -> Option<Artist> {
    library
        .artists
        .iter()
        .chain(library.album_artists.iter())
        .chain(library.search.artists.iter())
        .find(|artist| artist.id == *artist_id)
        .cloned()
}
pub(in crate::ui) fn current_player_track(shell: &Rc<Shell>) -> Option<Track> {
    let entry = shell.state.player.borrow().current.clone()?;
    let library = shell.state.library.borrow();
    library_track(&library, &entry.track_id).or_else(|| track_from_queue_entry(&entry))
}
pub(in crate::ui) fn track_from_queue_entry(entry: &QueueEntry) -> Option<Track> {
    Some(Track {
        id: entry.track_id.clone(),
        album_id: entry.album_id.clone()?,
        title: entry.title.clone(),
        artist: entry.artist.clone(),
        artist_id: entry.artist_id.clone(),
        artist_credits: Vec::new(),
        album_artist_credits: Vec::new(),
        album: entry.album.clone(),
        year: entry.year,
        release_date: None,
        date_added: None,
        last_played: None,
        play_count: None,
        user_rating: None,
        duration_seconds: entry.duration_seconds,
        favorite: entry.favorite,
        disc_number: 0,
        track_number: 0,
        image_ref: entry.image_ref.clone(),
        genres: Vec::new(),
        musicbrainz_recording_id: None,
        musicbrainz_release_track_id: None,
        local_path: entry.local_path.clone(),
        source_format: entry.source_format.clone(),
        comment: None,
        skip_count: None,
    })
}
pub(in crate::ui) fn add_link_hover(target: &gtk::Widget, label: &gtk::Label, text: &str) {
    let escaped_text = glib::markup_escape_text(text);
    let enter_label = label.clone();
    let enter_markup = format!("<u>{escaped_text}</u>");
    let leave_label = label.clone();
    let leave_text = text.to_string();
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        enter_label.add_css_class("hovered-link");
        enter_label.set_markup(&enter_markup);
    });
    motion.connect_leave(move |_| {
        leave_label.remove_css_class("hovered-link");
        leave_label.set_text(&leave_text);
    });
    target.add_controller(motion);
}
pub(in crate::ui) fn add_stateful_link_hover(
    target: &gtk::Widget,
    label: &gtk::Label,
    text: Rc<RefCell<String>>,
) {
    let enter_label = label.clone();
    let enter_text = Rc::clone(&text);
    let leave_label = label.clone();
    let leave_text = text;
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        let escaped_text = glib::markup_escape_text(enter_text.borrow().as_str());
        enter_label.add_css_class("hovered-link");
        enter_label.set_markup(&format!("<u>{escaped_text}</u>"));
    });
    motion.connect_leave(move |_| {
        leave_label.remove_css_class("hovered-link");
        leave_label.set_text(leave_text.borrow().as_str());
    });
    target.add_controller(motion);
}
pub(in crate::ui) fn add_dynamic_link_hover(target: &gtk::Widget, label: &gtk::Label) {
    let enter_label = label.clone();
    let leave_label = label.clone();
    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter(move |_, _, _| {
        let text = enter_label.text();
        let escaped_text = glib::markup_escape_text(text.as_str());
        enter_label.add_css_class("hovered-link");
        enter_label.set_markup(&format!("<u>{escaped_text}</u>"));
    });
    motion.connect_leave(move |_| {
        let text = leave_label.text().to_string();
        leave_label.remove_css_class("hovered-link");
        leave_label.set_text(&text);
    });
    target.add_controller(motion);
}
impl ArtworkTile {
    pub(in crate::ui) fn new(size: i32, seed: u32) -> Self {
        Self::new_sized(size, size, seed)
    }

    pub(in crate::ui) fn new_sized(width: i32, height: i32, seed: u32) -> Self {
        let area = gtk::DrawingArea::new();
        area.add_css_class("cover-tile");
        area.add_css_class("card");
        area.set_content_width(width);
        area.set_content_height(height);
        area.set_width_request(width);
        area.set_height_request(height);
        area.set_size_request(width, height);
        area.set_hexpand(false);
        area.set_vexpand(false);
        area.set_halign(gtk::Align::Start);
        area.set_valign(gtk::Align::Start);

        let seed = Rc::new(Cell::new(seed));
        let size = Rc::new(Cell::new(width.max(height)));
        let pixbuf = Rc::new(RefCell::new(None::<Pixbuf>));
        let expects_image = Rc::new(Cell::new(false));
        let artwork_id = Rc::new(RefCell::new(None::<String>));
        let request_key = Rc::new(RefCell::new(None::<String>));
        let generation = Rc::new(Cell::new(0));
        let draw_seed = Rc::clone(&seed);
        let draw_pixbuf = Rc::clone(&pixbuf);
        area.set_draw_func(move |_, context, width, height| {
            clip_rounded_rect(context, width, height, 12.0);
            if let Some(pixbuf) = draw_pixbuf.borrow().as_ref() {
                draw_pixbuf_cover(context, pixbuf, width, height);
            } else {
                draw_fallback_cover(context, draw_seed.get(), width, height);
            }
        });

        Self {
            area,
            size,
            seed,
            pixbuf,
            expects_image,
            artwork_id,
            request_key,
            generation,
        }
    }

    pub(in crate::ui) fn widget(&self) -> gtk::Widget {
        self.area.clone().upcast()
    }

    pub(in crate::ui) fn downgrade(&self) -> ArtworkTileWeak {
        ArtworkTileWeak {
            area: self.area.downgrade(),
            size: Rc::clone(&self.size),
            seed: Rc::clone(&self.seed),
            pixbuf: Rc::clone(&self.pixbuf),
            expects_image: Rc::clone(&self.expects_image),
            artwork_id: Rc::clone(&self.artwork_id),
            request_key: Rc::clone(&self.request_key),
            generation: Rc::clone(&self.generation),
        }
    }

    pub(in crate::ui) fn generation(&self) -> u64 {
        self.generation.get()
    }

    pub(in crate::ui) fn is_live_generation(&self, generation: u64) -> bool {
        self.generation.get() == generation && self.area.root().is_some()
    }

    pub(in crate::ui) fn is_current_generation(&self, generation: u64) -> bool {
        self.generation.get() == generation
    }

    pub(in crate::ui) fn advance_generation(&self) {
        self.generation.set(self.generation.get().saturating_add(1));
    }

    pub(in crate::ui) fn bind_selected_cover(
        &self,
        seed: u32,
        artwork_id: String,
        request_key: String,
        pixbuf: Option<Pixbuf>,
    ) -> ArtworkBindOutcome {
        let same_artwork = self.artwork_id.borrow().as_deref() == Some(artwork_id.as_str());
        let same_request = self.request_key.borrow().as_deref() == Some(request_key.as_str());
        let has_pixbuf = self.pixbuf.borrow().is_some();
        let action = artwork_bind_action(same_artwork, same_request, has_pixbuf, pixbuf.is_some());

        let request_changed = !same_artwork || !same_request;
        if request_changed {
            self.advance_generation();
            *self.artwork_id.borrow_mut() = Some(artwork_id);
            *self.request_key.borrow_mut() = Some(request_key);
        }

        self.seed.set(seed);
        if let Some(pixbuf) = pixbuf {
            *self.pixbuf.borrow_mut() = Some(pixbuf);
        } else if !same_artwork {
            *self.pixbuf.borrow_mut() = None;
        }
        self.expects_image.set(true);

        let has_pixbuf = self.pixbuf.borrow().is_some();
        self.sync_cover_state_classes(true, has_pixbuf);
        self.area.queue_draw();

        ArtworkBindOutcome {
            generation: self.generation.get(),
            request_needed: matches!(
                action,
                ArtworkBindAction::Request | ArtworkBindAction::RetainAndRequest
            ),
        }
    }

    pub(in crate::ui) fn set_seed(&self, seed: u32) {
        self.seed.set(seed);
        self.area.queue_draw();
    }

    pub(in crate::ui) fn set_square_size(&self, size: i32) {
        let size = size.max(1);
        if self.size.replace(size) == size {
            return;
        }
        self.area.set_content_width(size);
        self.area.set_content_height(size);
        self.area.set_width_request(size);
        self.area.set_height_request(size);
        self.area.set_size_request(size, size);
        self.area.queue_resize();
        self.area.queue_draw();
    }

    pub(in crate::ui) fn bind_image(&self, seed: u32, pixbuf: Option<Pixbuf>) -> u64 {
        self.bind_image_state(seed, pixbuf, false)
    }

    fn bind_image_state(&self, seed: u32, pixbuf: Option<Pixbuf>, expects_image: bool) -> u64 {
        let generation = self.generation.get().saturating_add(1);
        self.generation.set(generation);
        self.seed.set(seed);
        let has_pixbuf = pixbuf.is_some();
        *self.pixbuf.borrow_mut() = pixbuf;
        self.expects_image.set(expects_image);
        *self.artwork_id.borrow_mut() = None;
        *self.request_key.borrow_mut() = None;
        self.sync_cover_state_classes(expects_image, has_pixbuf);
        self.area.queue_draw();
        generation
    }

    pub(in crate::ui) fn set_pixbuf_if_current(&self, generation: u64, pixbuf: Pixbuf) -> bool {
        if self.generation.get() != generation {
            return false;
        }
        *self.pixbuf.borrow_mut() = Some(pixbuf);
        self.sync_cover_state_classes(self.expects_image.get(), true);
        self.area.queue_draw();
        true
    }

    pub(in crate::ui) fn clear_image(&self) {
        self.advance_generation();
        *self.pixbuf.borrow_mut() = None;
        self.expects_image.set(false);
        *self.artwork_id.borrow_mut() = None;
        *self.request_key.borrow_mut() = None;
        self.sync_cover_state_classes(false, false);
        self.area.queue_draw();
    }

    pub(in crate::ui) fn clear_image_if_current(&self, generation: u64) -> bool {
        if self.generation.get() != generation {
            return false;
        }
        self.generation.set(self.generation.get().saturating_add(1));
        *self.pixbuf.borrow_mut() = None;
        self.expects_image.set(false);
        *self.artwork_id.borrow_mut() = None;
        *self.request_key.borrow_mut() = None;
        self.sync_cover_state_classes(false, false);
        self.area.queue_draw();
        true
    }

    fn sync_cover_state_classes(&self, expects_image: bool, has_pixbuf: bool) {
        if expects_image {
            self.area.add_css_class("cover-tile-expected");
        } else {
            self.area.remove_css_class("cover-tile-expected");
        }

        if has_pixbuf {
            self.area.add_css_class("cover-tile-resolved");
            self.area.remove_css_class("cover-tile-final-missing");
            self.area.remove_css_class("cover-tile-fallback");
        } else {
            self.area.remove_css_class("cover-tile-resolved");
            if expects_image {
                self.area.remove_css_class("cover-tile-final-missing");
                self.area.add_css_class("cover-tile-fallback");
            } else {
                self.area.add_css_class("cover-tile-final-missing");
                self.area.remove_css_class("cover-tile-fallback");
            }
        }
    }
}
impl ArtworkTileWeak {
    pub(in crate::ui) fn upgrade(&self) -> Option<ArtworkTile> {
        Some(ArtworkTile {
            area: self.area.upgrade()?,
            size: Rc::clone(&self.size),
            seed: Rc::clone(&self.seed),
            pixbuf: Rc::clone(&self.pixbuf),
            expects_image: Rc::clone(&self.expects_image),
            artwork_id: Rc::clone(&self.artwork_id),
            request_key: Rc::clone(&self.request_key),
            generation: Rc::clone(&self.generation),
        })
    }

    pub(in crate::ui) fn size(&self) -> i32 {
        self.size.get()
    }

    pub(in crate::ui) fn is_current_generation(&self, generation: u64) -> bool {
        self.upgrade()
            .is_some_and(|tile| tile.is_current_generation(generation))
    }
}

pub(in crate::ui) fn artwork_bind_action(
    same_artwork: bool,
    same_request: bool,
    has_pixbuf: bool,
    decoded_ready: bool,
) -> ArtworkBindAction {
    if decoded_ready {
        ArtworkBindAction::Replace
    } else if !same_artwork || !has_pixbuf {
        ArtworkBindAction::Request
    } else if same_request {
        ArtworkBindAction::Retain
    } else {
        ArtworkBindAction::RetainAndRequest
    }
}
pub(in crate::ui) async fn load_cover_pixbuf(
    path: PathBuf,
    size: i32,
    _priority: glib::Priority,
) -> Result<Pixbuf, glib::Error> {
    let decode_size = cover_pixbuf_decode_size(size);
    let pixels = gio::spawn_blocking(move || decode_cover_pixels(path, decode_size))
        .await
        .map_err(|_| glib::Error::new(glib::FileError::Failed, "cover decode task failed"))??;
    let bytes = glib::Bytes::from_owned(pixels.data);
    Ok(Pixbuf::from_bytes(
        &bytes,
        pixels.colorspace,
        pixels.has_alpha,
        pixels.bits_per_sample,
        pixels.width,
        pixels.height,
        pixels.rowstride,
    ))
}
struct CoverPixelData {
    data: Vec<u8>,
    colorspace: gdk_pixbuf::Colorspace,
    has_alpha: bool,
    bits_per_sample: i32,
    width: i32,
    height: i32,
    rowstride: i32,
}
fn decode_cover_pixels(path: PathBuf, decode_size: i32) -> Result<CoverPixelData, glib::Error> {
    let pixbuf = Pixbuf::from_file_at_scale(&path, decode_size, decode_size, true)?;
    Ok(CoverPixelData {
        data: pixbuf.read_pixel_bytes().as_ref().to_vec(),
        colorspace: pixbuf.colorspace(),
        has_alpha: pixbuf.has_alpha(),
        bits_per_sample: pixbuf.bits_per_sample(),
        width: pixbuf.width(),
        height: pixbuf.height(),
        rowstride: pixbuf.rowstride(),
    })
}
pub(in crate::ui) fn cover_pixbuf_decode_size(size: i32) -> i32 {
    let size = size.max(1);
    if size >= GRID_COVER_SIZE as i32 {
        size
    } else {
        size.saturating_mul(2).min(GRID_COVER_SIZE as i32)
    }
}
pub(in crate::ui) fn apply_pixbuf_to_bindings(bindings: Vec<CoverBinding>, pixbuf: Pixbuf) {
    let apply_started = Instant::now();
    let binding_count = bindings.len();
    let mut applied = 0_usize;
    for binding in bindings {
        if let Some(tile) = binding.tile.upgrade()
            && tile.set_pixbuf_if_current(binding.generation, pixbuf.clone())
        {
            applied = applied.saturating_add(1);
        }
    }
    let apply_ms = apply_started.elapsed().as_millis() as u64;
    if apply_ms >= SLOW_COVER_CALLBACK_MS {
        warn!(
            bindings = binding_count,
            applied, apply_ms, "slow cover pixbuf binding"
        );
    }
}
pub(in crate::ui) fn draw_fallback_cover(
    context: &gtk::cairo::Context,
    seed: u32,
    width: i32,
    height: i32,
) {
    let red = f64::from((seed & 0xff) as u8) / 255.0;
    let green = f64::from(((seed >> 8) & 0xff) as u8) / 255.0;
    let blue = f64::from(((seed >> 16) & 0xff) as u8) / 255.0;
    context.set_source_rgb(red * 0.7 + 0.18, green * 0.7 + 0.18, blue * 0.7 + 0.18);
    context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
    let _paint = context.fill();

    context.set_source_rgba(1.0, 1.0, 1.0, 0.18);
    context.move_to(0.0, f64::from(height) * 0.2);
    context.line_to(f64::from(width) * 0.8, 0.0);
    context.line_to(f64::from(width), f64::from(height) * 0.8);
    context.line_to(f64::from(width) * 0.2, f64::from(height));
    context.close_path();
    let _fill = context.fill();
}
pub(in crate::ui) fn draw_pixbuf_cover(
    context: &gtk::cairo::Context,
    pixbuf: &Pixbuf,
    width: i32,
    height: i32,
) {
    let rect = cover_draw_rect(pixbuf.width(), pixbuf.height(), width, height);
    let _save = context.save();
    context.translate(rect.x, rect.y);
    context.scale(rect.scale, rect.scale);
    context.set_source_pixbuf(pixbuf, 0.0, 0.0);
    let _paint = context.paint();
    let _restore = context.restore();
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) struct CoverDrawRect {
    pub(in crate::ui) x: f64,
    pub(in crate::ui) y: f64,
    pub(in crate::ui) scale: f64,
}
pub(in crate::ui) fn cover_draw_rect(
    image_width: i32,
    image_height: i32,
    target_width: i32,
    target_height: i32,
) -> CoverDrawRect {
    let image_width = image_width.max(1);
    let image_height = image_height.max(1);
    let target_width = target_width.max(1);
    let target_height = target_height.max(1);
    let scale = (f64::from(target_width) / f64::from(image_width))
        .max(f64::from(target_height) / f64::from(image_height));
    let drawn_width = f64::from(image_width) * scale;
    let drawn_height = f64::from(image_height) * scale;
    CoverDrawRect {
        x: (f64::from(target_width) - drawn_width) / 2.0,
        y: (f64::from(target_height) - drawn_height) / 2.0,
        scale,
    }
}
pub(in crate::ui) fn clip_rounded_rect(
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    radius: f64,
) {
    let width = f64::from(width);
    let height = f64::from(height);
    let radius = radius.min(width / 2.0).min(height / 2.0);
    context.new_sub_path();
    context.arc(
        width - radius,
        radius,
        radius,
        (-90.0_f64).to_radians(),
        0.0,
    );
    context.arc(
        width - radius,
        height - radius,
        radius,
        0.0,
        90.0_f64.to_radians(),
    );
    context.arc(
        radius,
        height - radius,
        radius,
        90.0_f64.to_radians(),
        180.0_f64.to_radians(),
    );
    context.arc(
        radius,
        radius,
        radius,
        180.0_f64.to_radians(),
        270.0_f64.to_radians(),
    );
    context.close_path();
    context.clip();
}
pub(in crate::ui) fn add_label_click(label: &gtk::Label, callback: impl Fn() + 'static) {
    add_widget_click(label.upcast_ref(), callback);
}
pub(in crate::ui) fn add_widget_click(target: &gtk::Widget, callback: impl Fn() + 'static) {
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released(move |gesture, press_count, _, _| {
        if press_count == 1 {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            callback();
        }
    });
    target.add_controller(click);
}
pub(in crate::ui) fn add_card_label_link(
    shell: &Rc<Shell>,
    target: &gtk::Widget,
    label: &gtk::Label,
    text: &str,
    route: Option<Route>,
) {
    let Some(route) = route else {
        return;
    };
    target.set_cursor_from_name(Some("pointer"));
    label.set_cursor_from_name(Some("pointer"));
    add_link_hover(target, label, text);
    let shell = Rc::clone(shell);
    add_widget_click(target, move || shell.navigate(route.clone()));
}
pub(in crate::ui) fn current_playback_track_id(
    snapshot: &PlaybackSnapshot,
) -> Option<domain::TrackId> {
    snapshot
        .current
        .as_ref()
        .map(|entry| entry.track_id.clone())
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum PlaylistEntrySort {
    Order,
    Title,
    Artist,
    Album,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_menu_playlist_items_use_snapshot_order_without_limit() {
        let playlists = (0..102).map(test_playlist).collect::<Vec<_>>();

        let items = context_menu_playlist_items(&playlists);

        assert_eq!(items.len(), playlists.len());
        assert_eq!(
            items.first().map(|playlist| playlist.name.as_str()),
            Some("List 0")
        );
        assert_eq!(
            items.last().map(|playlist| playlist.name.as_str()),
            Some("List 101")
        );
    }

    #[test]
    fn filter_duplicate_tracks_skips_existing_playlist_entries() {
        let tracks = vec![test_track(1, &[]), test_track(2, &[])];
        let entries = vec![source::PlaylistEntry {
            entry_id: "entry-1".to_string(),
            track: test_track(1, &[]),
        }];

        let filtered = filter_duplicate_tracks(&tracks, &entries);

        assert_eq!(filtered, vec![test_track(2, &[])]);
    }

    #[test]
    fn playlist_add_toast_summarizes_added_tracks_and_playlists() {
        assert_eq!(playlist_add_toast(24, 3), "24 songs added to 3 playlists");
        assert_eq!(playlist_add_toast(1, 1), "1 song added to 1 playlist");
        assert_eq!(playlist_add_toast(0, 0), "No songs added");
    }

    #[test]
    fn playlist_create_row_uses_search_text() {
        assert!(!show_create_playlist_row(""));
        assert!(!show_create_playlist_row("   "));
        assert!(show_create_playlist_row("driving"));
        assert_eq!(create_playlist_label("Driving"), "+ Create Driving");
    }

    #[test]
    fn playlist_picker_duration_uses_units() {
        assert_eq!(format_duration_units(41), "41s");
        assert_eq!(format_duration_units(743), "12m 23s");
        assert_eq!(format_duration_units(4_421), "1h 13m 41s");
    }

    fn test_playlist(index: usize) -> Playlist {
        Playlist {
            id: PlaylistId::fake(index + 1),
            name: format!("List {index}"),
            track_count: index as u32,
            duration_seconds: index as u32 * 60,
            top_genres: Vec::new(),
            image_refs: Vec::new(),
            image_ref: None,
        }
    }
    fn test_track(index: usize, genres: &[&str]) -> Track {
        Track {
            id: TrackId::fake(index),
            album_id: AlbumId::fake(1),
            title: format!("Track {index}"),
            artist: "Artist".to_string(),
            artist_id: None,
            artist_credits: Vec::new(),
            album_artist_credits: Vec::new(),
            album: "Album".to_string(),
            year: 2024,
            release_date: None,
            date_added: None,
            last_played: None,
            play_count: None,
            user_rating: None,
            duration_seconds: 180,
            favorite: false,
            disc_number: 1,
            track_number: index as u16,
            image_ref: None,
            genres: genres.iter().map(|genre| genre.to_string()).collect(),
            musicbrainz_recording_id: None,
            musicbrainz_release_track_id: None,
            local_path: None,
            source_format: None,
            comment: None,
            skip_count: None,
        }
    }
}

use super::*;

#[derive(Clone, Debug)]
pub(in crate::ui) struct PlaylistEntryContextMenuAction {
    pub(in crate::ui) playlist_id: PlaylistId,
    pub(in crate::ui) entry_id: String,
    pub(in crate::ui) title: String,
}

pub(in crate::ui) fn present_track_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    position: Option<(f64, f64)>,
) {
    present_track_context_menu_inner(target, shell, track, position, None);
}
pub(in crate::ui) fn present_track_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    remove_action: PlaylistEntryContextMenuAction,
    position: Option<(f64, f64)>,
) {
    present_track_context_menu_inner(target, shell, track, position, Some(remove_action));
}
fn present_track_context_menu_inner(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    track: Track,
    position: Option<(f64, f64)>,
    remove_action: Option<PlaylistEntryContextMenuAction>,
) {
    let menu = gio::Menu::new();
    menu.append_item(&menu_item(
        "Play",
        "track.play",
        "media-playback-start-symbolic",
    ));
    menu.append_item(&menu_item("Play Next", "track.play-next", PLAY_NEXT_ICON));
    menu.append_item(&menu_item("Play Later", "track.play-last", PLAY_LATER_ICON));

    let playlists = context_menu_playlists(shell);
    if !playlists.is_empty() {
        let playlist_menu = gio::Menu::new();
        for (index, playlist) in playlists.iter().enumerate() {
            playlist_menu.append(
                Some(&playlist.name),
                Some(&format!("track.add-to-playlist-{index}")),
            );
        }
        menu.append_submenu(Some(&tr("Add to Playlist")), &playlist_menu);
    }

    menu.append(
        Some(&tr(if track.favorite {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        })),
        Some("track.favorite"),
    );
    let artist_route = track_artist_route(&track);
    if artist_route.is_some() {
        menu.append(Some(&tr("Go to Artist")), Some("track.go-artist"));
    }
    menu.append(Some(&tr("Go to Album")), Some("track.go-album"));
    if remove_action.is_some() {
        menu.append(
            Some(&tr("Remove from playlist")),
            Some("track.remove-from-playlist"),
        );
    }

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("track-context-menu");
    popover.set_parent(target);
    if let Some((x, y)) = position {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }

    let actions = gio::SimpleActionGroup::new();

    let play = gio::SimpleAction::new("play", None);
    let controller = shell.controller.clone();
    let action_track = track.clone();
    let action_popover = popover.downgrade();
    play.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.play_now(action_track.clone());
    });
    actions.add_action(&play);

    let play_next = gio::SimpleAction::new("play-next", None);
    let controller = shell.controller.clone();
    let action_track = track.clone();
    let action_popover = popover.downgrade();
    play_next.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.play_next(action_track.clone());
    });
    actions.add_action(&play_next);

    let play_last = gio::SimpleAction::new("play-last", None);
    let controller = shell.controller.clone();
    let action_track = track.clone();
    let action_popover = popover.downgrade();
    play_last.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.play_last(vec![action_track.clone()]);
    });
    actions.add_action(&play_last);

    for (index, playlist) in playlists.into_iter().enumerate() {
        let action_name = format!("add-to-playlist-{index}");
        let add = gio::SimpleAction::new(&action_name, None);
        let controller = shell.controller.clone();
        let playlist_id = playlist.id;
        let action_track = track.clone();
        let action_popover = popover.downgrade();
        add.connect_activate(move |_, _| {
            if let Some(popover) = action_popover.upgrade() {
                popover.popdown();
            }
            controller.add_tracks_to_playlist(playlist_id.clone(), vec![action_track.clone()]);
        });
        actions.add_action(&add);
    }

    let favorite_action = gio::SimpleAction::new("favorite", None);
    let controller = shell.controller.clone();
    let track_id = track.id.clone();
    let favorite = !track.favorite;
    let action_popover = popover.downgrade();
    favorite_action.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.set_track_favorite(track_id.clone(), favorite);
    });
    actions.add_action(&favorite_action);

    if let Some(artist_route) = artist_route {
        let go_artist = gio::SimpleAction::new("go-artist", None);
        let action_shell = Rc::clone(shell);
        let action_popover = popover.downgrade();
        go_artist.connect_activate(move |_, _| {
            if let Some(popover) = action_popover.upgrade() {
                popover.popdown();
            }
            let shell = Rc::clone(&action_shell);
            let route = artist_route.clone();
            glib::idle_add_local_once(move || shell.navigate(route));
        });
        actions.add_action(&go_artist);
    }

    let go_album = gio::SimpleAction::new("go-album", None);
    let go_album_shell = Rc::clone(shell);
    let album_id = track.album_id.clone();
    let action_popover = popover.downgrade();
    go_album.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        let shell = Rc::clone(&go_album_shell);
        let album_id = album_id.clone();
        glib::idle_add_local_once(move || shell.navigate(Route::AlbumDetail(album_id)));
    });
    actions.add_action(&go_album);

    if let Some(remove_action) = remove_action {
        let remove_from_playlist = gio::SimpleAction::new("remove-from-playlist", None);
        let shell = Rc::clone(shell);
        let action_popover = popover.downgrade();
        remove_from_playlist.connect_activate(move |_, _| {
            if let Some(popover) = action_popover.upgrade() {
                popover.popdown();
            }
            confirm_remove_playlist_entry(
                &shell,
                remove_action.playlist_id.clone(),
                remove_action.entry_id.clone(),
                remove_action.title.clone(),
            );
        });
        actions.add_action(&remove_from_playlist);
    }

    target.insert_action_group("track", Some(&actions));
    popover.connect_closed(move |popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            popover.unparent();
        });
    });
    popover.popup();
}
pub(in crate::ui) fn present_album_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    album: Album,
    position: Option<(f64, f64)>,
) {
    let menu = gio::Menu::new();
    menu.append_item(&menu_item(
        "Play",
        "album.play",
        "media-playback-start-symbolic",
    ));
    menu.append_item(&menu_item("Play Next", "album.play-next", PLAY_NEXT_ICON));
    menu.append_item(&menu_item("Play Later", "album.play-last", PLAY_LATER_ICON));

    let playlists = context_menu_playlists(shell);
    if !playlists.is_empty() {
        let playlist_menu = gio::Menu::new();
        for (index, playlist) in playlists.iter().enumerate() {
            playlist_menu.append(
                Some(&playlist.name),
                Some(&format!("album.add-to-playlist-{index}")),
            );
        }
        menu.append_submenu(Some(&tr("Add to Playlist")), &playlist_menu);
    }

    menu.append(
        Some(&tr(if album.favorite {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        })),
        Some("album.favorite"),
    );
    let artist_route = album_artist_route(&album);
    if artist_route.is_some() {
        menu.append(Some(&tr("Go to Artist")), Some("album.go-artist"));
    }
    menu.append(Some(&tr("Go to Album")), Some("album.go-album"));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("album-context-menu");
    popover.set_parent(target);
    if let Some((x, y)) = position {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }

    let actions = gio::SimpleActionGroup::new();

    let play = gio::SimpleAction::new("play", None);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    let action_popover = popover.downgrade();
    play.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.play_album_now(album_id.clone());
    });
    actions.add_action(&play);

    let play_next = gio::SimpleAction::new("play-next", None);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    let action_popover = popover.downgrade();
    play_next.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
            for track in tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });
    actions.add_action(&play_next);

    let play_last = gio::SimpleAction::new("play-last", None);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    let action_popover = popover.downgrade();
    play_last.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
            controller.play_last(tracks);
        }
    });
    actions.add_action(&play_last);

    for (index, playlist) in playlists.into_iter().enumerate() {
        let action_name = format!("add-to-playlist-{index}");
        let add = gio::SimpleAction::new(&action_name, None);
        let controller = shell.controller.clone();
        let playlist_id = playlist.id;
        let album_id = album.id.clone();
        let action_popover = popover.downgrade();
        add.connect_activate(move |_, _| {
            if let Some(popover) = action_popover.upgrade() {
                popover.popdown();
            }
            if let Ok(Some((_, tracks))) = controller.cached_album_detail(&album_id) {
                controller.add_tracks_to_playlist(playlist_id.clone(), tracks);
            }
        });
        actions.add_action(&add);
    }

    let favorite_action = gio::SimpleAction::new("favorite", None);
    let controller = shell.controller.clone();
    let album_id = album.id.clone();
    let favorite = !album.favorite;
    let action_popover = popover.downgrade();
    favorite_action.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.set_album_favorite(album_id.clone(), favorite);
    });
    actions.add_action(&favorite_action);

    if let Some(artist_route) = artist_route {
        let go_artist = gio::SimpleAction::new("go-artist", None);
        let action_shell = Rc::clone(shell);
        let action_popover = popover.downgrade();
        go_artist.connect_activate(move |_, _| {
            if let Some(popover) = action_popover.upgrade() {
                popover.popdown();
            }
            let shell = Rc::clone(&action_shell);
            let route = artist_route.clone();
            glib::idle_add_local_once(move || shell.navigate(route));
        });
        actions.add_action(&go_artist);
    }

    let go_album = gio::SimpleAction::new("go-album", None);
    let shell = Rc::clone(shell);
    let album_id = album.id.clone();
    let action_popover = popover.downgrade();
    go_album.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        let shell = Rc::clone(&shell);
        let album_id = album_id.clone();
        glib::idle_add_local_once(move || shell.navigate(Route::AlbumDetail(album_id)));
    });
    actions.add_action(&go_album);

    target.insert_action_group("album", Some(&actions));
    popover.connect_closed(move |popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            popover.unparent();
        });
    });
    popover.popup();
}
pub(in crate::ui) fn present_artist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    artist: Artist,
    position: Option<(f64, f64)>,
) {
    let menu = gio::Menu::new();
    menu.append_item(&menu_item(
        "Play",
        "artist.play",
        "media-playback-start-symbolic",
    ));
    menu.append_item(&menu_item("Play Next", "artist.play-next", PLAY_NEXT_ICON));
    menu.append_item(&menu_item(
        "Play Later",
        "artist.play-last",
        PLAY_LATER_ICON,
    ));

    let playlists = context_menu_playlists(shell);
    if !playlists.is_empty() {
        let playlist_menu = gio::Menu::new();
        for (index, playlist) in playlists.iter().enumerate() {
            playlist_menu.append(
                Some(&playlist.name),
                Some(&format!("artist.add-to-playlist-{index}")),
            );
        }
        menu.append_submenu(Some(&tr("Add to Playlist")), &playlist_menu);
    }

    menu.append(
        Some(&tr(if artist.favorite {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        })),
        Some("artist.favorite"),
    );
    menu.append(Some(&tr("Go to Artist")), Some("artist.go-artist"));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("artist-context-menu");
    popover.set_parent(target);
    if let Some((x, y)) = position {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }

    let actions = gio::SimpleActionGroup::new();

    let play = gio::SimpleAction::new("play", None);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    let selected_music_folder_id = selected_music_folder_id(shell);
    let action_popover = popover.downgrade();
    play.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id)
            && let Some(activation) = loaded_tracks_window_play_activation(
                PlaySourceKey {
                    descriptor: PlaySourceDescriptor::ArtistTracks {
                        artist_id: artist_id.clone(),
                        scope: ArtistTrackScope::AllCredits,
                        selected_music_folder_id: selected_music_folder_id.clone(),
                    },
                    order: SourceOrder::Canonical,
                },
                tracks.len(),
                0,
                |index| tracks.get(index).cloned(),
            )
        {
            controller.play_activation(activation);
        }
    });
    actions.add_action(&play);

    let play_next = gio::SimpleAction::new("play-next", None);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    let action_popover = popover.downgrade();
    play_next.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
            for track in tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    });
    actions.add_action(&play_next);

    let play_last = gio::SimpleAction::new("play-last", None);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    let action_popover = popover.downgrade();
    play_last.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
            controller.play_last(tracks);
        }
    });
    actions.add_action(&play_last);

    for (index, playlist) in playlists.into_iter().enumerate() {
        let action_name = format!("add-to-playlist-{index}");
        let add = gio::SimpleAction::new(&action_name, None);
        let controller = shell.controller.clone();
        let playlist_id = playlist.id;
        let artist_id = artist.id.clone();
        let action_popover = popover.downgrade();
        add.connect_activate(move |_, _| {
            if let Some(popover) = action_popover.upgrade() {
                popover.popdown();
            }
            if let Some(tracks) = artist_tracks_for_context(&controller, &artist_id) {
                controller.add_tracks_to_playlist(playlist_id.clone(), tracks);
            }
        });
        actions.add_action(&add);
    }

    let favorite_action = gio::SimpleAction::new("favorite", None);
    let controller = shell.controller.clone();
    let artist_id = artist.id.clone();
    let favorite = !artist.favorite;
    let action_popover = popover.downgrade();
    favorite_action.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.set_artist_favorite(artist_id.clone(), favorite);
    });
    actions.add_action(&favorite_action);

    let go_artist = gio::SimpleAction::new("go-artist", None);
    let shell = Rc::clone(shell);
    let artist_id = artist.id.clone();
    let action_popover = popover.downgrade();
    go_artist.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        shell.navigate(Route::ArtistDetail(artist_id.clone()));
    });
    actions.add_action(&go_artist);

    target.insert_action_group("artist", Some(&actions));
    popover.connect_closed(move |popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            popover.unparent();
        });
    });
    popover.popup();
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
pub(in crate::ui) fn present_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: Playlist,
    position: Option<(f64, f64)>,
) {
    let menu = gio::Menu::new();
    menu.append_item(&menu_item(
        "Play",
        "playlist.play",
        "media-playback-start-symbolic",
    ));
    menu.append_item(&menu_item(
        "Delete",
        "playlist.delete",
        "user-trash-symbolic",
    ));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("playlist-context-menu");
    popover.set_parent(target);
    if let Some((x, y)) = position {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }

    let actions = gio::SimpleActionGroup::new();
    let play = gio::SimpleAction::new("play", None);
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    let action_popover = popover.downgrade();
    play.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Ok(Some(detail)) = controller.cached_playlist_detail(&playlist_id) {
            let state = PlaylistEntryListState::default();
            let activation = if detail.entries.is_empty() {
                loaded_tracks_window_play_activation(
                    playlist_play_source_key(playlist_id.clone(), &state),
                    detail.tracks.len(),
                    0,
                    |index| detail.tracks.get(index).cloned(),
                )
            } else {
                playlist_entry_play_activation(playlist_id.clone(), &detail.entries[0], 0, &state)
            };
            if let Some(activation) = activation {
                controller.play_activation(activation);
            }
        }
    });
    actions.add_action(&play);

    let delete = gio::SimpleAction::new("delete", None);
    let controller = shell.controller.clone();
    let window = shell.window.clone();
    let playlist_id = playlist.id.clone();
    let playlist_name = playlist.name.clone();
    let action_popover = popover.downgrade();
    delete.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
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
        dialog.present(Some(&window));
    });
    actions.add_action(&delete);
    target.insert_action_group("playlist", Some(&actions));
    popover.connect_closed(move |popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            popover.unparent();
        });
    });
    popover.popup();
}
pub(in crate::ui) fn present_smart_playlist_context_menu(
    target: &gtk::Widget,
    shell: &Rc<Shell>,
    playlist: SmartPlaylist,
    position: Option<(f64, f64)>,
) {
    let menu = gio::Menu::new();
    menu.append_item(&menu_item(
        "Play",
        "smart-playlist.play",
        "media-playback-start-symbolic",
    ));
    menu.append_item(&menu_item(
        "Delete",
        "smart-playlist.delete",
        "user-trash-symbolic",
    ));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.add_css_class("playlist-context-menu");
    popover.set_parent(target);
    if let Some((x, y)) = position {
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }

    let actions = gio::SimpleActionGroup::new();
    let play = gio::SimpleAction::new("play", None);
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    let selected_music_folder_id = selected_music_folder_id(shell);
    let action_popover = popover.downgrade();
    play.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        if let Ok(Some(detail)) = controller.cached_smart_playlist_detail(&playlist_id)
            && let Some(activation) = loaded_tracks_window_play_activation(
                smart_playlist_play_source_key(
                    &detail.smart_playlist,
                    selected_music_folder_id.clone(),
                ),
                detail.tracks.len(),
                0,
                |index| detail.tracks.get(index).cloned(),
            )
        {
            controller.play_activation(activation);
        }
    });
    actions.add_action(&play);

    let delete = gio::SimpleAction::new("delete", None);
    let controller = shell.controller.clone();
    let playlist_id = playlist.id.clone();
    let action_popover = popover.downgrade();
    delete.connect_activate(move |_, _| {
        if let Some(popover) = action_popover.upgrade() {
            popover.popdown();
        }
        controller.delete_smart_playlist(playlist_id.clone());
    });
    actions.add_action(&delete);
    target.insert_action_group("smart-playlist", Some(&actions));
    popover.connect_closed(move |popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || {
            popover.unparent();
        });
    });
    popover.popup();
}
pub(in crate::ui) fn menu_item(label: &str, action: &str, icon_name: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(&tr(label)), Some(action));
    item.set_icon(&gio::ThemedIcon::new(icon_name));
    item
}
pub(in crate::ui) fn context_menu_playlists(shell: &Rc<Shell>) -> Vec<Playlist> {
    shell
        .controller
        .cached_playlists_page(0, CONTEXT_MENU_PLAYLIST_LIMIT)
        .map(|page| page.items)
        .unwrap_or_else(|_| shell.state.library.borrow().playlists.clone())
}
pub(in crate::ui) fn context_track(shell: &Rc<Shell>, fallback: &Track) -> Track {
    shell
        .controller
        .cached_track(&fallback.id)
        .ok()
        .flatten()
        .or_else(|| {
            let library = shell.state.library.borrow();
            library_track(&library, &fallback.id)
        })
        .unwrap_or_else(|| fallback.clone())
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
    shell
        .controller
        .cached_track(&entry.track_id)
        .ok()
        .flatten()
        .or_else(|| track_from_queue_entry(&entry))
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
#[derive(Clone)]
pub(in crate::ui) struct TrackFavoriteCell {
    button: gtk::Button,
    current_track: Rc<RefCell<Option<Track>>>,
    current_key: Rc<RefCell<Option<FavoriteControlKey>>>,
}

thread_local! {
    static TRACK_FAVORITE_CELLS: RefCell<HashMap<usize, TrackFavoriteCell>> = RefCell::new(HashMap::new());
}

pub(in crate::ui) fn store_track_favorite_cell(list_item: &gtk::ListItem, cell: TrackFavoriteCell) {
    let key = list_item_storage_key(list_item);
    TRACK_FAVORITE_CELLS.with(|cells| {
        cells.borrow_mut().insert(key, cell);
    });
}

pub(in crate::ui) fn track_favorite_cell(list_item: &gtk::ListItem) -> Option<TrackFavoriteCell> {
    let key = list_item_storage_key(list_item);
    TRACK_FAVORITE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(in crate::ui) fn remove_track_favorite_cell(list_item: &gtk::ListItem) {
    let key = list_item_storage_key(list_item);
    TRACK_FAVORITE_CELLS.with(|cells| {
        cells.borrow_mut().remove(&key);
    });
}

pub(in crate::ui) fn track_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let current_key = Rc::new(RefCell::new(None::<FavoriteControlKey>));
        let button = favorite_icon_button("Favorite");
        install_dynamic_track_context_menu(&button, &setup_shell, Rc::clone(&current_track));

        let controller = setup_shell.controller.clone();
        let click_track = Rc::clone(&current_track);
        button.connect_clicked(move |button| {
            let Some(track) = click_track.borrow().as_ref().cloned() else {
                return;
            };
            controller.set_track_favorite(track.id, !favorite_button_is_active(button));
        });

        list_item.set_child(Some(&button));
        store_track_favorite_cell(
            list_item,
            TrackFavoriteCell {
                button,
                current_track,
                current_key,
            },
        );
    });

    let bind_shell = Rc::clone(&shell);
    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let track = boxed.borrow::<Track>().clone();
        let Some(cell) = track_favorite_cell(list_item) else {
            return;
        };
        let next_key = track_favorite_key(&track.id);
        let mut current_key = cell.current_key.borrow_mut();
        if current_key.as_ref() != Some(&next_key) {
            if let Some(previous_key) = current_key.as_ref() {
                bind_shell.unregister_favorite_button(previous_key, &cell.button);
            }
            bind_shell.register_favorite_button(next_key.clone(), &cell.button);
            *current_key = Some(next_key);
        }
        drop(current_key);

        set_favorite_button_active(&cell.button, track.favorite);
        *cell.current_track.borrow_mut() = Some(track);
    });

    let unbind_shell = Rc::clone(&shell);
    factory.connect_unbind(move |_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_favorite_cell(list_item)
        {
            if let Some(previous_key) = cell.current_key.borrow_mut().take() {
                unbind_shell.unregister_favorite_button(&previous_key, &cell.button);
            }
            *cell.current_track.borrow_mut() = None;
        }
    });

    let teardown_shell = Rc::clone(&shell);
    factory.connect_teardown(move |_, list_item| {
        if let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() {
            if let Some(cell) = track_favorite_cell(list_item)
                && let Some(previous_key) = cell.current_key.borrow_mut().take()
            {
                teardown_shell.unregister_favorite_button(&previous_key, &cell.button);
            }
            remove_track_favorite_cell(list_item);
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(&tr("Favorite")), Some(factory));
    column.set_fixed_width(76);
    column.set_resizable(false);
    column
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

    pub(in crate::ui) fn retain_cover_image(&self, seed: u32) -> u64 {
        let generation = self.generation.get().saturating_add(1);
        self.generation.set(generation);
        self.seed.set(seed);
        self.expects_image.set(true);
        let has_pixbuf = self.pixbuf.borrow().is_some();
        self.sync_cover_state_classes(true, has_pixbuf);
        self.area.queue_draw();
        generation
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

    pub(in crate::ui) fn bind_cover_image(&self, seed: u32, pixbuf: Option<Pixbuf>) -> u64 {
        self.bind_image_state(seed, pixbuf, true)
    }

    fn bind_image_state(&self, seed: u32, pixbuf: Option<Pixbuf>, expects_image: bool) -> u64 {
        let generation = self.generation.get().saturating_add(1);
        self.generation.set(generation);
        self.seed.set(seed);
        let has_pixbuf = pixbuf.is_some();
        *self.pixbuf.borrow_mut() = pixbuf;
        self.expects_image.set(expects_image);
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
    for binding in bindings {
        if let Some(tile) = binding.tile.upgrade() {
            tile.set_pixbuf_if_current(binding.generation, pixbuf.clone());
        }
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
) -> Option<rufin_core::TrackId> {
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

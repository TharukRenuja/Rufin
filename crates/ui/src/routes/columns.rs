use std::{cell::RefCell, collections::HashMap, rc::Rc};

use ::library::{AlbumSummary, ArtistSummary, PlaylistSummary, SmartPlaylistSummary, Track};
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::glib;

use super::collection_context::{
    install_dynamic_album_context_menu, install_dynamic_track_context_menu,
    present_album_context_menu, present_artist_context_menu,
};
use crate::favorites::{
    album_favorite_key, artist_favorite_key, favorite_button_is_active, favorite_icon_button,
    set_favorite_button_active, track_favorite_key,
};
use crate::interactions::{add_dynamic_link_hover, add_label_click, install_context_menu_openers};
use crate::localization::localized_column;
use crate::shell::Shell;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::cover::{ArtworkTile, THUMB_COVER_SIZE};
use crate::{LibraryField, LibraryListKey};

use super::detail_links::{album_artist_route, track_artist_route};
use super::library_fields::{
    album_field, artist_field, column_width, item_at_from_item, play_count_column_width,
    playlist_field, smart_playlist_display_name, smart_playlist_field, track_artwork_at_from_item,
    track_field,
};
use super::route::Route;
use super::table_links::track_link_column;

pub(crate) const ROW_INDEX_COLUMN_TITLE: &str = "\u{2003}\u{a0}#";

pub(crate) fn album_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => {
            album_image_column(shell, "Image", column_width(LibraryField::Image))
        }
        LibraryField::TitleMerged => {
            album_merged_column(shell, "Title", column_width(LibraryField::TitleMerged))
        }
        LibraryField::Title => {
            album_text_column(shell, "Title", 220, |album| album.album.title.clone())
        }
        LibraryField::Favorite => album_favorite_column(shell),
        _ => album_text_column(shell, field.title(), column_width(field), move |album| {
            album_field(album, field)
        }),
    }
}
pub(crate) fn artist_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => artist_image_column(shell),
        LibraryField::TitleMerged | LibraryField::Title => {
            artist_text_column(shell, "Title", 220, |artist| artist.artist.name.clone())
        }
        LibraryField::Favorite => artist_favorite_column(shell),
        _ => artist_text_column(shell, field.title(), column_width(field), move |artist| {
            artist_field(artist, field)
        }),
    }
}
pub(crate) fn playlist_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => {
            let settings_shell = Rc::clone(shell);
            artwork_column::<PlaylistSummary, _, _>(
                shell,
                "Image",
                column_width(LibraryField::Image),
                move |playlist| {
                    let prefer_server_cover = settings_shell
                        .settings
                        .current
                        .borrow()
                        .prefer_server_playlist_covers;
                    ArtworkBinding::playlist(
                        &playlist.playlist,
                        &playlist.representative_albums,
                        prefer_server_cover,
                    )
                },
                |playlist| stable_seed(playlist.playlist.id.as_str()),
            )
        }
        LibraryField::Title | LibraryField::TitleMerged => {
            text_column::<PlaylistSummary, _>("Title", 220, |playlist| {
                playlist.playlist.name.clone()
            })
        }
        _ => {
            text_column::<PlaylistSummary, _>(field.title(), column_width(field), move |playlist| {
                playlist_field(playlist, field)
            })
        }
    }
}
pub(crate) fn smart_playlist_column(
    shell: &Rc<Shell>,
    field: LibraryField,
) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Image => artwork_column::<SmartPlaylistSummary, _, _>(
            shell,
            "Image",
            column_width(LibraryField::Image),
            |playlist| {
                ArtworkBinding::smart_playlist(
                    &playlist.smart_playlist,
                    &playlist.representative_albums,
                )
            },
            |playlist| stable_seed(playlist.smart_playlist.id.as_str()),
        ),
        LibraryField::Title | LibraryField::TitleMerged => {
            text_column::<SmartPlaylistSummary, _>("Title", 220, |playlist| {
                smart_playlist_display_name(&playlist.smart_playlist)
            })
        }
        _ => text_column::<SmartPlaylistSummary, _>(
            field.title(),
            column_width(field),
            move |playlist| smart_playlist_field(playlist, field),
        ),
    }
}
pub(crate) fn track_column_for_key(
    shell: &Rc<Shell>,
    key: LibraryListKey,
    field: LibraryField,
    playing: &TrackRowPlayingIndicator,
) -> gtk::ColumnViewColumn {
    let width = track_column_width(key, field);
    match field {
        LibraryField::RowIndex => track_row_index_column_with_width(width, playing.clone()),
        LibraryField::Image => track_image_column(shell, "Image", width),
        LibraryField::TitleMerged => track_merged_column(
            shell,
            "Title",
            width,
            TrackMergedColumnValues {
                title: |track: &Track| track.title.clone(),
                subtitle: |track: &Track| track.artist.clone(),
                seed: |track: &Track| stable_seed(track.id.as_str()),
            },
        ),
        LibraryField::Title => {
            track_text_column(shell, "Title", width, 0.0, |track| track.title.clone())
        }
        LibraryField::Favorite => track_favorite_column(shell),
        LibraryField::Artist => track_link_column(shell, "Artist", width, |track| {
            (track.artist.clone(), track_artist_route(track))
        }),
        LibraryField::AlbumArtist => {
            track_link_column(shell, LibraryField::AlbumArtist.title(), width, |track| {
                (
                    track_field(track, LibraryField::AlbumArtist),
                    track_album_artist_route(track),
                )
            })
        }
        LibraryField::Album => track_link_column(shell, "Album", width, |track| {
            (
                track.album.clone(),
                track.album_id.clone().map(Route::AlbumDetail),
            )
        }),
        LibraryField::Duration => track_text_column(shell, "◷", width, 0.0, |track| {
            track_field(track, LibraryField::Duration)
        }),
        _ => track_text_column(shell, field.title(), width, 0.0, move |track| {
            track_field(track, field)
        }),
    }
}
pub(crate) fn track_column_fit_width(key: LibraryListKey, field: LibraryField) -> i32 {
    column_fit_width(field, track_column_width(key, field))
}
pub(crate) fn track_column_width(key: LibraryListKey, field: LibraryField) -> i32 {
    match key {
        LibraryListKey::ArtistTracks
        | LibraryListKey::GenreTracks
        | LibraryListKey::PlaylistTracks => return track_list_column_width(field),
        LibraryListKey::SmartPlaylistTracks => {}
        _ => return column_width(field),
    }

    match field {
        LibraryField::RowIndex => 44,
        LibraryField::Title | LibraryField::TitleMerged => 212,
        LibraryField::Album
        | LibraryField::Artist
        | LibraryField::AlbumArtist
        | LibraryField::Genre => 180,
        LibraryField::PlayCount => play_count_column_width(),
        LibraryField::UserRating | LibraryField::SongCount | LibraryField::AlbumCount => 82,
        LibraryField::ReleaseDate | LibraryField::DateAdded | LibraryField::LastPlayed => 108,
        LibraryField::Year
        | LibraryField::DiscNumber
        | LibraryField::TrackNumber
        | LibraryField::Bpm => 62,
        LibraryField::Duration => 70,
        LibraryField::Image => column_width(LibraryField::Image),
        LibraryField::Favorite => 48,
    }
}
pub(crate) fn column_fit_width(field: LibraryField, width: i32) -> i32 {
    if field == LibraryField::TitleMerged {
        width.saturating_add(72)
    } else {
        width
    }
}
fn track_album_artist_route(track: &Track) -> Option<Route> {
    track
        .album_artist_credits()
        .first()
        .map(|artist| Route::ArtistDetail(artist.id.clone()))
}
fn track_list_column_width(field: LibraryField) -> i32 {
    match field {
        LibraryField::RowIndex => 54,
        LibraryField::Title | LibraryField::TitleMerged => 320,
        LibraryField::Album => 260,
        LibraryField::Artist | LibraryField::AlbumArtist | LibraryField::Genre => 220,
        LibraryField::Year
        | LibraryField::DiscNumber
        | LibraryField::TrackNumber
        | LibraryField::Bpm => 70,
        LibraryField::Duration => 90,
        LibraryField::Favorite => 76,
        _ => column_width(field),
    }
}
pub(crate) fn text_column<T, F>(title: &str, width: i32, value: F) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let value = Rc::new(value);
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::new(None);
            label.set_xalign(0.0);
            label.set_halign(gtk::Align::Fill);
            label.set_hexpand(true);
            label.set_wrap(false);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_single_line_mode(true);
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let data = boxed.borrow::<T>();
        label.set_text(&(value)(&data));
    });
    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}
pub(crate) fn row_index_column() -> gtk::ColumnViewColumn {
    row_index_column_with_width(column_width(LibraryField::RowIndex))
}
pub(crate) fn row_index_column_with_width(width: i32) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::new(None);
            label.add_css_class("muted");
            label.set_xalign(0.5);
            label.set_halign(gtk::Align::Fill);
            label.set_hexpand(true);
            label.set_wrap(false);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_single_line_mode(true);
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        label.set_text(&(item.position() + 1).to_string());
    });
    let column = gtk::ColumnViewColumn::new(Some(ROW_INDEX_COLUMN_TITLE), Some(factory));
    column.set_fixed_width(width);
    column
}

fn track_row_index_column_with_width(
    width: i32,
    playing: TrackRowPlayingIndicator,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(Some(&track_row_index_cell("")));
        }
    });
    let bind_playing = playing.clone();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(cell) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Overlay>().ok())
        else {
            return;
        };
        set_track_row_index_text(&cell, &(item.position() + 1).to_string());
        bind_playing.bind(&cell, item.position());
    });
    factory.connect_unbind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(cell) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Overlay>().ok())
        else {
            return;
        };
        playing.unbind(&cell);
        set_track_row_index_text(&cell, "");
    });
    let column = gtk::ColumnViewColumn::new(Some(ROW_INDEX_COLUMN_TITLE), Some(factory));
    column.set_fixed_width(width);
    column
}

#[derive(Clone)]
pub(crate) struct TrackRowPlayingIndicator {
    inner: Rc<TrackRowPlayingIndicatorInner>,
}

struct TrackRowPlayingIndicatorInner {
    position: std::cell::Cell<u32>,
    cells: RefCell<HashMap<usize, (glib::WeakRef<gtk::Overlay>, u32)>>,
}

impl TrackRowPlayingIndicator {
    pub(crate) fn new() -> Self {
        Self {
            inner: Rc::new(TrackRowPlayingIndicatorInner {
                position: std::cell::Cell::new(gtk::INVALID_LIST_POSITION),
                cells: RefCell::new(HashMap::new()),
            }),
        }
    }

    pub(crate) fn bind(&self, cell: &gtk::Overlay, position: u32) {
        apply_track_row_playing(cell, position == self.inner.position.get());
        self.inner
            .cells
            .borrow_mut()
            .insert(cell.as_ptr() as usize, (cell.downgrade(), position));
    }

    pub(crate) fn unbind(&self, cell: &gtk::Overlay) {
        cell.remove_css_class("track-row-playing");
        self.inner
            .cells
            .borrow_mut()
            .remove(&(cell.as_ptr() as usize));
    }

    pub(crate) fn set_position(&self, position: u32) {
        self.inner.position.set(position);
        self.inner
            .cells
            .borrow_mut()
            .retain(|_, (cell, bound_position)| {
                let Some(cell) = cell.upgrade() else {
                    return false;
                };
                apply_track_row_playing(&cell, *bound_position == position);
                true
            });
    }
}

fn apply_track_row_playing(cell: &gtk::Overlay, playing: bool) {
    if playing {
        cell.add_css_class("track-row-playing");
    } else {
        cell.remove_css_class("track-row-playing");
    }
}

pub(crate) fn track_row_index_cell(text: &str) -> gtk::Overlay {
    let cell = gtk::Overlay::new();
    cell.add_css_class("track-row-index-cell");
    cell.set_hexpand(true);
    cell.set_halign(gtk::Align::Fill);

    let label = gtk::Label::new(Some(text));
    label.add_css_class("muted");
    label.add_css_class("track-row-index-number");
    label.set_xalign(0.5);
    label.set_halign(gtk::Align::Fill);
    label.set_hexpand(true);
    label.set_single_line_mode(true);
    cell.set_child(Some(&label));

    let playing = gtk::Image::from_icon_name("media-playback-start-symbolic");
    playing.add_css_class("track-row-index-playing");
    playing.set_pixel_size(14);
    playing.set_halign(gtk::Align::Center);
    playing.set_valign(gtk::Align::Center);
    playing.set_margin_start(2);
    cell.add_overlay(&playing);
    cell
}

pub(crate) fn set_track_row_index_text(cell: &gtk::Overlay, text: &str) {
    let Some(label) = cell
        .child()
        .and_then(|child| child.downcast::<gtk::Label>().ok())
    else {
        return;
    };
    label.set_text(text);
}

#[derive(Clone)]
struct LibraryArtworkCell {
    cover: ArtworkTile,
}

#[derive(Clone)]
pub(crate) struct LibraryAlbumImageCell {
    pub(crate) cover: ArtworkTile,
    pub(crate) current_album: Rc<RefCell<Option<AlbumSummary>>>,
}

#[derive(Clone)]
pub(crate) struct LibraryAlbumTextCell {
    pub(crate) label: gtk::Label,
    pub(crate) current_album: Rc<RefCell<Option<AlbumSummary>>>,
}

#[derive(Clone)]
pub(crate) struct LibraryAlbumMergedCell {
    pub(crate) cover: ArtworkTile,
    pub(crate) title: gtk::Label,
    pub(crate) subtitle: gtk::Label,
    pub(crate) subtitle_route: Rc<RefCell<Option<Route>>>,
    pub(crate) current_album: Rc<RefCell<Option<AlbumSummary>>>,
}

thread_local! {
    static LIBRARY_ARTWORK_CELLS: RefCell<HashMap<usize, LibraryArtworkCell>> = RefCell::new(HashMap::new());
    static LIBRARY_ALBUM_IMAGE_CELLS: RefCell<HashMap<usize, LibraryAlbumImageCell>> = RefCell::new(HashMap::new());
    static LIBRARY_ALBUM_TEXT_CELLS: RefCell<HashMap<usize, LibraryAlbumTextCell>> = RefCell::new(HashMap::new());
    static LIBRARY_ALBUM_MERGED_CELLS: RefCell<HashMap<usize, LibraryAlbumMergedCell>> = RefCell::new(HashMap::new());
}

fn artwork_cell(item: &gtk::ListItem) -> Option<LibraryArtworkCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ARTWORK_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(crate) fn album_image_cell(item: &gtk::ListItem) -> Option<LibraryAlbumImageCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ALBUM_IMAGE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(crate) fn album_text_cell(item: &gtk::ListItem) -> Option<LibraryAlbumTextCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ALBUM_TEXT_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(crate) fn album_merged_cell(item: &gtk::ListItem) -> Option<LibraryAlbumMergedCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_ALBUM_MERGED_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(crate) fn album_image_column(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_album = Rc::new(RefCell::new(None::<AlbumSummary>));
        let cover = ArtworkTile::new(48, 0);
        let widget = cover.widget();
        install_dynamic_album_context_menu(&widget, &setup_shell, Rc::clone(&current_album));
        item.set_child(Some(&widget));
        let key = library_list_item_storage_key(item);
        LIBRARY_ALBUM_IMAGE_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryAlbumImageCell {
                    cover,
                    current_album,
                },
            );
        });
    });

    let bind_shell = Rc::clone(&shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(album) = item_at_from_item::<AlbumSummary>(item) else {
            return;
        };
        let Some(cell) = album_image_cell(item) else {
            return;
        };
        bind_shell.bind_artwork_tile(
            &cell.cover,
            ArtworkBinding::album(&album.album),
            album.album.color_seed,
            48,
            THUMB_COVER_SIZE,
        );
        *cell.current_album.borrow_mut() = Some(album);
    });

    let unbind_shell = Rc::clone(&shell);
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = album_image_cell(item)
        {
            unbind_shell.clear_artwork_tile(&cell.cover);
            *cell.current_album.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_ALBUM_IMAGE_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}

pub(crate) fn album_text_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&AlbumSummary) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let value = Rc::new(value);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_album = Rc::new(RefCell::new(None::<AlbumSummary>));
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        install_dynamic_album_context_menu(&label, &setup_shell, Rc::clone(&current_album));
        item.set_child(Some(&label));
        let key = library_list_item_storage_key(item);
        LIBRARY_ALBUM_TEXT_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryAlbumTextCell {
                    label,
                    current_album,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(album) = item_at_from_item::<AlbumSummary>(item) else {
            return;
        };
        let Some(cell) = album_text_cell(item) else {
            return;
        };
        cell.label.set_text(&(value)(&album));
        *cell.current_album.borrow_mut() = Some(album);
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = album_text_cell(item)
        {
            cell.label.set_text("");
            *cell.current_album.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_ALBUM_TEXT_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}

pub(crate) fn album_merged_column(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_album = Rc::new(RefCell::new(None::<AlbumSummary>));
        let subtitle_route = Rc::new(RefCell::new(None::<Route>));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_valign(gtk::Align::Center);

        let cover = ArtworkTile::new(48, 0);
        row.append(&cover.widget());

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_single_line_mode(true);
        labels.append(&title);

        let subtitle = gtk::Label::new(None);
        subtitle.add_css_class("artist-label");
        subtitle.set_xalign(0.0);
        subtitle.set_halign(gtk::Align::Start);
        subtitle.set_hexpand(false);
        subtitle.set_wrap(false);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.set_single_line_mode(true);
        subtitle.set_visible(false);
        subtitle.set_cursor_from_name(Some("pointer"));
        add_dynamic_link_hover(subtitle.upcast_ref(), &subtitle);
        let click_shell = Rc::clone(&setup_shell);
        let route_for_click = Rc::clone(&subtitle_route);
        add_label_click(&subtitle, move || {
            let route = route_for_click.borrow().clone();
            if let Some(route) = route {
                click_shell.navigate(route);
            }
        });
        labels.append(&subtitle);

        row.append(&labels);
        install_dynamic_album_context_menu(&row, &setup_shell, Rc::clone(&current_album));
        item.set_child(Some(&row));
        let key = library_list_item_storage_key(item);
        LIBRARY_ALBUM_MERGED_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryAlbumMergedCell {
                    cover,
                    title,
                    subtitle,
                    subtitle_route,
                    current_album,
                },
            );
        });
    });

    let bind_shell = Rc::clone(&shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(album) = item_at_from_item::<AlbumSummary>(item) else {
            return;
        };
        let Some(cell) = album_merged_cell(item) else {
            return;
        };
        bind_shell.bind_artwork_tile(
            &cell.cover,
            ArtworkBinding::album(&album.album),
            album.album.color_seed,
            48,
            THUMB_COVER_SIZE,
        );
        cell.title.set_text(&album.album.title);
        cell.subtitle.set_text(&album.album.artist);
        let route = album_artist_route(&album.album);
        *cell.subtitle_route.borrow_mut() = route;
        cell.subtitle
            .set_visible(!album.album.artist.trim().is_empty());
        *cell.current_album.borrow_mut() = Some(album);
    });

    let unbind_shell = Rc::clone(&shell);
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = album_merged_cell(item)
        {
            cell.title.set_text("");
            cell.subtitle.set_text("");
            cell.subtitle.set_visible(false);
            *cell.subtitle_route.borrow_mut() = None;
            unbind_shell.clear_artwork_tile(&cell.cover);
            *cell.current_album.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_ALBUM_MERGED_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}

fn install_album_list_item_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    item: &gtk::ListItem,
) {
    let item = item.downgrade();
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(album) = item.upgrade().and_then(|item| item_at_from_item(&item)) else {
                return;
            };
            present_album_context_menu(target, &shell, album, position);
        }),
    );
}

fn install_artist_list_item_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    item: &gtk::ListItem,
) {
    let item = item.downgrade();
    let shell = Rc::clone(shell);
    install_context_menu_openers(
        target,
        Rc::new(move |target, position| {
            let Some(artist) = item.upgrade().and_then(|item| item_at_from_item(&item)) else {
                return;
            };
            present_artist_context_menu(target, &shell, artist, position);
        }),
    );
}

pub(crate) fn artwork_column<T, F, S>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    candidates: F,
    seed: S,
) -> gtk::ColumnViewColumn
where
    T: Clone + 'static,
    F: Fn(&T) -> ArtworkBinding + 'static,
    S: Fn(&T) -> u32 + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let candidates = Rc::new(candidates);
    let seed = Rc::new(seed);

    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let cover = ArtworkTile::new(48, 0);
        item.set_child(Some(&cover.widget()));
        let key = library_list_item_storage_key(item);
        LIBRARY_ARTWORK_CELLS.with(|cells| {
            cells.borrow_mut().insert(key, LibraryArtworkCell { cover });
        });
    });

    let bind_shell = Rc::clone(&shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(boxed) = item
            .item()
            .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
        else {
            return;
        };
        let data = boxed.borrow::<T>();
        let Some(cell) = artwork_cell(item) else {
            return;
        };
        bind_shell.bind_artwork_tile(
            &cell.cover,
            candidates(&data),
            seed(&data),
            48,
            THUMB_COVER_SIZE,
        );
    });
    let unbind_shell = Rc::clone(&shell);
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = artwork_cell(item)
        {
            unbind_shell.clear_artwork_tile(&cell.cover);
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_ARTWORK_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });
    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}
pub(crate) fn artist_image_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let cover = ArtworkTile::new(48, 0);
        let widget = cover.widget();
        install_artist_list_item_context_menu(&widget, &setup_shell, item);
        item.set_child(Some(&widget));
        let key = library_list_item_storage_key(item);
        LIBRARY_ARTWORK_CELLS.with(|cells| {
            cells.borrow_mut().insert(key, LibraryArtworkCell { cover });
        });
    });

    let bind_shell = Rc::clone(&shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(artist) = item_at_from_item::<ArtistSummary>(item) else {
            return;
        };
        let Some(cell) = artwork_cell(item) else {
            return;
        };
        bind_shell.bind_artwork_tile(
            &cell.cover,
            ArtworkBinding::artist(&artist.artist, &artist.representative_albums),
            stable_seed(artist.artist.id.as_str()),
            48,
            THUMB_COVER_SIZE,
        );
    });
    let unbind_shell = Rc::clone(&shell);
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = artwork_cell(item)
        {
            unbind_shell.clear_artwork_tile(&cell.cover);
        }
    });
    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_ARTWORK_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });
    let column = localized_column("Image", &factory);
    column.set_fixed_width(column_width(LibraryField::Image));
    column
}
pub(crate) fn artist_text_column<F>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&ArtistSummary) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let value = Rc::new(value);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        install_artist_list_item_context_menu(&label, &setup_shell, item);
        item.set_child(Some(&label));
    });

    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(artist) = item_at_from_item::<ArtistSummary>(item) else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        label.set_text(&(value)(&artist));
    });
    factory.connect_unbind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        {
            label.set_text("");
        }
    });
    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}
#[derive(Clone)]
pub(crate) struct LibraryTrackImageCell {
    pub(crate) cover: ArtworkTile,
    pub(crate) current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
pub(crate) struct LibraryTrackTextCell {
    pub(crate) label: gtk::Label,
    pub(crate) current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
pub(crate) struct LibraryTrackMergedCell {
    pub(crate) cover: ArtworkTile,
    pub(crate) title: gtk::Label,
    pub(crate) subtitle: gtk::Label,
    pub(crate) subtitle_route: Rc<RefCell<Option<Route>>>,
    pub(crate) current_track: Rc<RefCell<Option<Track>>>,
}

#[derive(Clone)]
pub(crate) struct LibraryTrackFavoriteCell {
    pub(crate) button: gtk::Button,
    pub(crate) current_track: Rc<RefCell<Option<Track>>>,
}

thread_local! {
    static LIBRARY_TRACK_IMAGE_CELLS: RefCell<HashMap<usize, LibraryTrackImageCell>> = RefCell::new(HashMap::new());
    static LIBRARY_TRACK_TEXT_CELLS: RefCell<HashMap<usize, LibraryTrackTextCell>> = RefCell::new(HashMap::new());
    static LIBRARY_TRACK_MERGED_CELLS: RefCell<HashMap<usize, LibraryTrackMergedCell>> = RefCell::new(HashMap::new());
    static LIBRARY_TRACK_FAVORITE_CELLS: RefCell<HashMap<usize, LibraryTrackFavoriteCell>> = RefCell::new(HashMap::new());
}

pub(crate) fn library_list_item_storage_key(list_item: &gtk::ListItem) -> usize {
    list_item.as_ptr() as usize
}

pub(crate) fn track_image_cell(item: &gtk::ListItem) -> Option<LibraryTrackImageCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_IMAGE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(crate) fn track_text_cell(item: &gtk::ListItem) -> Option<LibraryTrackTextCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_TEXT_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(crate) fn track_merged_cell(item: &gtk::ListItem) -> Option<LibraryTrackMergedCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_MERGED_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

pub(crate) fn track_favorite_cell(item: &gtk::ListItem) -> Option<LibraryTrackFavoriteCell> {
    let key = library_list_item_storage_key(item);
    LIBRARY_TRACK_FAVORITE_CELLS.with(|cells| cells.borrow().get(&key).cloned())
}

fn install_track_cell_context_menu(
    target: &impl IsA<gtk::Widget>,
    shell: &Rc<Shell>,
    current_track: Rc<RefCell<Option<Track>>>,
) {
    install_dynamic_track_context_menu(target, shell, current_track);
}

pub(crate) fn track_image_column(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let cover = ArtworkTile::new(48, 0);
        let widget = cover.widget();
        install_track_cell_context_menu(&widget, &setup_shell, Rc::clone(&current_track));
        item.set_child(Some(&widget));
        let key = library_list_item_storage_key(item);
        LIBRARY_TRACK_IMAGE_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryTrackImageCell {
                    cover,
                    current_track,
                },
            );
        });
    });

    let bind_shell = Rc::clone(&shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let Some(artwork) = track_artwork_at_from_item(item) else {
            return;
        };
        let Some(cell) = track_image_cell(item) else {
            return;
        };
        bind_shell.bind_artwork_tile(
            &cell.cover,
            artwork,
            stable_seed(track.id.as_str()),
            48,
            THUMB_COVER_SIZE,
        );
        *cell.current_track.borrow_mut() = Some(track);
    });

    let unbind_shell = Rc::clone(&shell);
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_image_cell(item)
        {
            *cell.current_track.borrow_mut() = None;
            unbind_shell.clear_artwork_tile(&cell.cover);
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_TRACK_IMAGE_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}
pub(crate) fn track_text_column<F>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    xalign: f32,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&Track) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let value = Rc::new(value);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let label = gtk::Label::new(None);
        if title == "Title" {
            label.add_css_class("track-list-title");
        }
        label.set_xalign(xalign);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        install_track_cell_context_menu(&label, &setup_shell, Rc::clone(&current_track));
        item.set_child(Some(&label));
        let key = library_list_item_storage_key(item);
        LIBRARY_TRACK_TEXT_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryTrackTextCell {
                    label,
                    current_track,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let Some(cell) = track_text_cell(item) else {
            return;
        };
        cell.label.set_text(&(value)(&track));
        *cell.current_track.borrow_mut() = Some(track);
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_text_cell(item)
        {
            cell.label.set_text("");
            *cell.current_track.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_TRACK_TEXT_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}

pub(crate) struct TrackMergedColumnValues<Title, Subtitle, Seed> {
    pub(crate) title: Title,
    pub(crate) subtitle: Subtitle,
    pub(crate) seed: Seed,
}

pub(crate) fn track_merged_column<Title, Subtitle, Seed>(
    shell: &Rc<Shell>,
    title: &'static str,
    width: i32,
    values: TrackMergedColumnValues<Title, Subtitle, Seed>,
) -> gtk::ColumnViewColumn
where
    Title: Fn(&Track) -> String + 'static,
    Subtitle: Fn(&Track) -> String + 'static,
    Seed: Fn(&Track) -> u32 + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let TrackMergedColumnValues {
        title: title_value,
        subtitle: subtitle_value,
        seed,
    } = values;
    let title_value = Rc::new(title_value);
    let subtitle_value = Rc::new(subtitle_value);
    let seed = Rc::new(seed);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let subtitle_route = Rc::new(RefCell::new(None::<Route>));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.set_valign(gtk::Align::Center);

        let cover = ArtworkTile::new(48, 0);
        row.append(&cover.widget());

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(None);
        title.add_css_class("track-list-title");
        title.set_xalign(0.0);
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_single_line_mode(true);
        labels.append(&title);

        let subtitle = gtk::Label::new(None);
        subtitle.add_css_class("artist-label");
        subtitle.add_css_class("table-link-label");
        subtitle.set_xalign(0.0);
        subtitle.set_halign(gtk::Align::Start);
        subtitle.set_hexpand(false);
        subtitle.set_wrap(false);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.set_single_line_mode(true);
        subtitle.set_width_chars(1);
        subtitle.set_max_width_chars(28);
        subtitle.set_visible(false);
        subtitle.set_cursor_from_name(Some("pointer"));
        add_dynamic_link_hover(subtitle.upcast_ref(), &subtitle);
        labels.append(&subtitle);

        let click_shell = Rc::clone(&setup_shell);
        let route_for_click = Rc::clone(&subtitle_route);
        add_label_click(&subtitle, move || {
            let route = route_for_click.borrow().clone();
            if let Some(route) = route {
                click_shell.navigate(route);
            }
        });

        row.append(&labels);
        install_track_cell_context_menu(&row, &setup_shell, Rc::clone(&current_track));
        item.set_child(Some(&row));
        let key = library_list_item_storage_key(item);
        LIBRARY_TRACK_MERGED_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryTrackMergedCell {
                    cover,
                    title,
                    subtitle,
                    subtitle_route,
                    current_track,
                },
            );
        });
    });

    let bind_shell = Rc::clone(&shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let Some(artwork) = track_artwork_at_from_item(item) else {
            return;
        };
        let Some(cell) = track_merged_cell(item) else {
            return;
        };
        bind_shell.bind_artwork_tile(&cell.cover, artwork, seed(&track), 48, THUMB_COVER_SIZE);
        cell.title.set_text(&title_value(&track));
        let subtitle = subtitle_value(&track);
        let subtitle_route = track_artist_route(&track);
        *cell.current_track.borrow_mut() = Some(track);
        if subtitle.trim().is_empty() {
            *cell.subtitle_route.borrow_mut() = None;
            cell.subtitle.set_visible(false);
        } else if let Some(route) = subtitle_route {
            *cell.subtitle_route.borrow_mut() = Some(route);
            cell.subtitle.set_text(&subtitle);
            cell.subtitle.set_visible(true);
        } else {
            *cell.subtitle_route.borrow_mut() = None;
            cell.subtitle.set_text(&subtitle);
            cell.subtitle.set_visible(true);
        }
    });

    let unbind_shell = Rc::clone(&shell);
    factory.connect_unbind(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_merged_cell(item)
        {
            cell.title.set_text("");
            cell.subtitle.set_text("");
            cell.subtitle.set_visible(false);
            *cell.subtitle_route.borrow_mut() = None;
            unbind_shell.clear_artwork_tile(&cell.cover);
            *cell.current_track.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_TRACK_MERGED_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}
pub(crate) fn album_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let button = favorite_icon_button("Favorite album");
        let favorite_item = item.downgrade();
        shell.favorites.register_dynamic_button(
            Rc::new(move || {
                favorite_item
                    .upgrade()
                    .and_then(|item| item_at_from_item::<AlbumSummary>(&item))
                    .map(|album| album_favorite_key(&album.album.id))
            }),
            &button,
        );
        install_album_list_item_context_menu(&button, &shell, item);
        let favorite_shell = Rc::clone(&shell);
        let click_item = item.downgrade();
        button.connect_clicked(move |button| {
            let Some(album) = click_item
                .upgrade()
                .and_then(|item| item_at_from_item::<AlbumSummary>(&item))
            else {
                return;
            };
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                library::FavoriteItemId::Album(album.album.id.clone()),
                favorite,
                Some(button),
            );
        });
        item.set_child(Some(&button));
    });

    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(album) = item_at_from_item::<AlbumSummary>(item) else {
            return;
        };
        let Some(button) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Button>().ok())
        else {
            return;
        };
        set_favorite_button_active(&button, album.album.favorite);
    });

    factory.connect_unbind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(button) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Button>().ok())
        {
            set_favorite_button_active(&button, false);
        }
    });
    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(column_width(LibraryField::Favorite));
    column
}
pub(crate) fn artist_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let button = favorite_icon_button("Favorite artist");
        let favorite_item = item.downgrade();
        shell.favorites.register_dynamic_button(
            Rc::new(move || {
                favorite_item
                    .upgrade()
                    .and_then(|item| item_at_from_item::<ArtistSummary>(&item))
                    .map(|artist| artist_favorite_key(&artist.artist.id))
            }),
            &button,
        );
        install_artist_list_item_context_menu(&button, &shell, item);
        let favorite_shell = Rc::clone(&shell);
        let click_item = item.downgrade();
        button.connect_clicked(move |button| {
            let Some(artist) = click_item
                .upgrade()
                .and_then(|item| item_at_from_item::<ArtistSummary>(&item))
            else {
                return;
            };
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                library::FavoriteItemId::Artist(artist.artist.id.clone()),
                favorite,
                Some(button),
            );
        });
        item.set_child(Some(&button));
    });

    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(artist) = item_at_from_item::<ArtistSummary>(item) else {
            return;
        };
        let Some(button) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Button>().ok())
        else {
            return;
        };
        set_favorite_button_active(&button, artist.artist.favorite);
    });

    factory.connect_unbind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(button) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Button>().ok())
        {
            set_favorite_button_active(&button, false);
        }
    });
    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(column_width(LibraryField::Favorite));
    column
}
pub(crate) fn track_favorite_column(shell: &Rc<Shell>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);

    let setup_shell = Rc::clone(&shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let current_track = Rc::new(RefCell::new(None::<Track>));
        let button = favorite_icon_button("Favorite track");
        install_track_cell_context_menu(&button, &setup_shell, Rc::clone(&current_track));
        let favorite_key_track = Rc::clone(&current_track);
        setup_shell.favorites.register_dynamic_button(
            Rc::new(move || {
                favorite_key_track
                    .borrow()
                    .as_ref()
                    .map(|track| track_favorite_key(&track.id))
            }),
            &button,
        );
        let favorite_shell = Rc::clone(&setup_shell);
        let click_track = Rc::clone(&current_track);
        button.connect_clicked(move |button| {
            let Some(track) = click_track.borrow().as_ref().cloned() else {
                return;
            };
            let favorite = !favorite_button_is_active(button);
            favorite_shell.set_favorite_with_feedback(
                library::FavoriteItemId::Track(track.id.clone()),
                favorite,
                Some(button),
            );
        });
        item.set_child(Some(&button));
        let key = library_list_item_storage_key(item);
        LIBRARY_TRACK_FAVORITE_CELLS.with(|cells| {
            cells.borrow_mut().insert(
                key,
                LibraryTrackFavoriteCell {
                    button,
                    current_track,
                },
            );
        });
    });

    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(track) = item_at_from_item::<Track>(item) else {
            return;
        };
        let Some(cell) = track_favorite_cell(item) else {
            return;
        };
        set_favorite_button_active(&cell.button, track.favorite);
        *cell.current_track.borrow_mut() = Some(track);
    });

    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>()
            && let Some(cell) = track_favorite_cell(item)
        {
            *cell.current_track.borrow_mut() = None;
        }
    });

    factory.connect_teardown(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let key = library_list_item_storage_key(item);
            LIBRARY_TRACK_FAVORITE_CELLS.with(|cells| {
                cells.borrow_mut().remove(&key);
            });
        }
    });

    let column = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    column.set_fixed_width(column_width(LibraryField::Favorite));
    column
}

use std::{cell::RefCell, cmp::Ordering, rc::Rc, sync::Arc};

use ::library::{GenreSummary, Library, MoodSummary, MusicFolderId};
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::{gio, glib};
use playback::QueuePlacement;

use super::collection_context::{install_genre_context_menu, present_genre_context_menu};
use crate::format_duration_units;
use crate::interactions::install_context_menu_openers;
use crate::localization::localized_column;
use crate::runtime::SelectedLibrary;
use crate::shell::Shell;
use crate::shell::cover::THUMB_COVER_SIZE;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::route::{LatestMountedRouteRead, MountedRoute, SelectedRouteIdentity};
use crate::{LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings};
use localization::msgid;
use localization::{album_count_text, track_count_text};

use super::cards;
use super::collections::{
    CollectionTableProjection, LibraryCollectionProjection, LibraryPresentationProjection,
    PlaybackTarget, collection_column_width, dynamic_collection_table,
};
use super::columns::{column_fit_width, row_index_column};
use super::grid_cells::{
    CollectionGridCardCell, CollectionGridProjection, ReusableCollectionGridCell, collection_grid,
    collection_grid_cover_shell, install_dynamic_genre_context_menu,
};
use super::library_fields::{
    apply_desc, clear_list_item_child, cmp_string, column_width, item_at_from_item,
};
use super::route::Route;
use super::route_shell::LibraryPageShellOptions;
use super::table_sizing::route_column_view_initial_width;

#[derive(Clone, Copy)]
pub(crate) enum NamedCollectionKind {
    Genres,
    Moods,
}

#[derive(Clone)]
pub(crate) enum NamedCollectionItem {
    Genre(GenreSummary),
    Mood(MoodSummary),
}

#[derive(Clone)]
struct NamedCollectionReadRequest {
    identity: SelectedRouteIdentity,
    query: String,
    settings: LibraryListSettings,
}

pub(crate) struct PreparedNamedCollection {
    pub(crate) source: Arc<[NamedCollectionItem]>,
    pub(crate) visible: Arc<[NamedCollectionItem]>,
}

impl NamedCollectionKind {
    fn key(self) -> LibraryListKey {
        match self {
            Self::Genres => LibraryListKey::Genres,
            Self::Moods => LibraryListKey::Moods,
        }
    }

    fn fallback_warning(self) -> &'static str {
        match self {
            Self::Genres => msgid("Nothing here yet"),
            Self::Moods => {
                msgid("Files need Mood/BPM tags written on them. Not supported for Jellyfin")
            }
        }
    }

    fn play_label(self) -> &'static str {
        match self {
            Self::Genres => msgid("Play genre"),
            Self::Moods => msgid("Play mood"),
        }
    }
}

impl NamedCollectionItem {
    fn name(&self) -> &str {
        match self {
            Self::Genre(genre) => &genre.genre.name,
            Self::Mood(mood) => &mood.mood.name,
        }
    }

    fn seed(&self) -> u32 {
        match self {
            Self::Genre(genre) => stable_seed(genre.genre.id.as_str()),
            Self::Mood(mood) => stable_seed(mood.mood.id.as_str()),
        }
    }

    fn route(&self) -> Route {
        match self {
            Self::Genre(genre) => Route::GenreDetail(genre.genre.id.clone()),
            Self::Mood(mood) => Route::MoodDetail(mood.mood.id.clone()),
        }
    }

    fn artwork(&self) -> Vec<ArtworkBinding> {
        match self {
            Self::Genre(genre) => {
                ArtworkBinding::genre_slots(&genre.genre, &genre.representative_albums)
            }
            Self::Mood(mood) => ArtworkBinding::mood_slots(&mood.mood, &mood.representative_albums),
        }
    }

    fn field(&self, field: LibraryField) -> String {
        match (self, field) {
            (_, LibraryField::Title | LibraryField::TitleMerged) => self.name().to_string(),
            (Self::Genre(genre), LibraryField::AlbumCount) => {
                album_count_text(genre.album_count.into())
            }
            (Self::Genre(genre), LibraryField::SongCount) => {
                track_count_text(genre.track_count.into())
            }
            (Self::Mood(mood), LibraryField::SongCount) => {
                track_count_text(mood.track_count.into())
            }
            (Self::Genre(genre), LibraryField::Duration) => {
                format_duration_units(genre.duration_seconds)
            }
            (Self::Mood(mood), LibraryField::Duration) => {
                format_duration_units(mood.duration_seconds)
            }
            _ => String::new(),
        }
    }

    fn matches_query(&self, query: &str) -> bool {
        self.name().to_lowercase().contains(query)
    }

    fn compare(&self, other: &Self, field: LibraryField) -> Ordering {
        match field {
            LibraryField::AlbumCount => self.album_count().cmp(&other.album_count()),
            LibraryField::SongCount => self.track_count().cmp(&other.track_count()),
            LibraryField::Duration => self.duration_seconds().cmp(&other.duration_seconds()),
            _ => cmp_string(self.name(), other.name()),
        }
        .then_with(|| cmp_string(self.name(), other.name()))
    }

    fn album_count(&self) -> u32 {
        match self {
            Self::Genre(genre) => genre.album_count,
            Self::Mood(_) => 0,
        }
    }

    fn track_count(&self) -> u32 {
        match self {
            Self::Genre(genre) => genre.track_count,
            Self::Mood(mood) => mood.track_count,
        }
    }

    fn duration_seconds(&self) -> u32 {
        match self {
            Self::Genre(genre) => genre.duration_seconds,
            Self::Mood(mood) => mood.duration_seconds,
        }
    }

    fn play(&self, shell: &Shell, placement: QueuePlacement) {
        let target = match self {
            Self::Genre(genre) => PlaybackTarget::Genre(genre.genre.id.clone()),
            Self::Mood(mood) => PlaybackTarget::Mood(mood.mood.id.clone()),
        };
        if let Some(request) = target.play_request(shell, placement, true) {
            shell.products.playback.queue.play_loaded(request);
        }
    }

    fn install_context_menu(&self, widget: &impl IsA<gtk::Widget>, shell: &Rc<Shell>) {
        if let Self::Genre(genre) = self {
            install_genre_context_menu(widget, shell, genre.clone());
        }
    }

    fn is_downloaded(&self, selected: &SelectedLibrary) -> bool {
        match self {
            Self::Genre(genre) => selected
                .library
                .is_genre_downloaded(&genre.genre.id, selected.music_folder_id.as_ref())
                .unwrap_or(false),
            Self::Mood(mood) => selected
                .library
                .is_mood_downloaded(&mood.mood.id, selected.music_folder_id.as_ref())
                .unwrap_or(false),
        }
    }
}

pub(crate) fn sort_named_collection_items(
    items: &mut [NamedCollectionItem],
    settings: &LibraryListSettings,
) {
    items.sort_by(|left, right| {
        apply_desc(left.compare(right, settings.sort_key), settings.descending)
    });
}

pub(crate) fn replace_named_collection_items(
    model: &gio::ListStore,
    items: impl IntoIterator<Item = NamedCollectionItem>,
) {
    let additions = items
        .into_iter()
        .map(glib::BoxedAnyObject::new)
        .collect::<Vec<_>>();
    model.splice(0, model.n_items(), &additions);
}

fn populate_named_collection_model(
    model: &gio::ListStore,
    items: &[NamedCollectionItem],
    settings: &LibraryListSettings,
) {
    let mut values = items.to_vec();
    sort_named_collection_items(&mut values, settings);
    replace_named_collection_items(model, values);
}

fn prepare_named_collection(
    mut source: Arc<[NamedCollectionItem]>,
    query: &str,
    settings: &LibraryListSettings,
) -> PreparedNamedCollection {
    sort_named_collection_items(Arc::make_mut(&mut source), settings);
    let query = query.trim().to_lowercase();
    let visible = if query.is_empty() {
        Arc::clone(&source)
    } else {
        source
            .iter()
            .filter(|item| item.matches_query(&query))
            .cloned()
            .collect::<Vec<_>>()
            .into()
    };
    PreparedNamedCollection { source, visible }
}

pub(crate) fn named_collection_projection(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    kind: NamedCollectionKind,
) -> LibraryCollectionProjection {
    let key = kind.key();
    let settings = shell.settings.current.borrow().library_list(key);
    let shell = Rc::clone(shell);
    LibraryCollectionProjection::new(
        settings,
        Rc::new(move |layout| match layout {
            LibraryLayout::Row => LibraryPresentationProjection::Row(named_collection_table(
                &shell,
                model.clone(),
                kind,
            )),
            LibraryLayout::Grid | LibraryLayout::Detail => LibraryPresentationProjection::Grid(
                named_collection_grid(&shell, model.clone(), kind),
            ),
        }),
    )
}

fn named_collection_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    kind: NamedCollectionKind,
) -> CollectionGridProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(kind.key())
        .grid_fields;
    let cell_shell = Rc::clone(shell);
    let activate_shell = Rc::clone(shell);
    collection_grid(
        model,
        &fields,
        move |fields| NamedCollectionGridCell::new(Rc::clone(&cell_shell), kind, fields),
        move |_, item: NamedCollectionItem| activate_shell.navigate(item.route()),
    )
}

fn named_collection_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    kind: NamedCollectionKind,
) -> CollectionTableProjection {
    let fields = shell
        .settings
        .current
        .borrow()
        .library_list(kind.key())
        .row_fields;
    let activate_shell = Rc::clone(shell);
    let column_shell = Rc::clone(shell);
    dynamic_collection_table(
        shell,
        kind.key(),
        model,
        &fields,
        Vec::new(),
        move |field| named_collection_column(&column_shell, field),
        |field| {
            column_fit_width(field, {
                if matches!(field, LibraryField::Title | LibraryField::TitleMerged) {
                    180
                } else {
                    collection_column_width(field)
                }
            })
        },
        true,
        Some(Box::new(move |_, item: NamedCollectionItem| {
            activate_shell.navigate(item.route());
        })),
        None,
        route_column_view_initial_width(shell),
    )
}

fn named_collection_column(shell: &Rc<Shell>, field: LibraryField) -> gtk::ColumnViewColumn {
    match field {
        LibraryField::RowIndex => row_index_column(),
        LibraryField::Title | LibraryField::TitleMerged => {
            named_collection_title_column(shell, "Title", 180)
        }
        _ => named_collection_text_column(shell, field.title(), column_width(field), {
            move |item| item.field(field)
        }),
    }
}

fn named_collection_title_column(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let setup_shell = Rc::clone(shell);
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        row.set_hexpand(true);
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        row.append(&label);
        let weak_item = item.downgrade();
        let downloaded = setup_shell.download_badge(true, move |selected| {
            weak_item
                .upgrade()
                .and_then(|item| item_at_from_item::<NamedCollectionItem>(&item))
                .is_some_and(|item| item.is_downloaded(selected))
        });
        row.append(&downloaded);

        let menu_item = item.downgrade();
        let menu_shell = Rc::clone(&setup_shell);
        install_context_menu_openers(
            &row,
            Rc::new(move |target, position| {
                let Some(NamedCollectionItem::Genre(genre)) = menu_item
                    .upgrade()
                    .and_then(|item| item_at_from_item::<NamedCollectionItem>(&item))
                else {
                    return;
                };
                present_genre_context_menu(target, &menu_shell, genre, None, position);
            }),
        );
        item.set_child(Some(&row));
    });

    let bind_shell = Rc::clone(shell);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(collection) = item_at_from_item::<NamedCollectionItem>(item) else {
            return;
        };
        let Some(row) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        else {
            return;
        };
        let Some(label) = row
            .first_child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        else {
            return;
        };
        let Some(downloaded) = row
            .last_child()
            .and_then(|child| child.downcast::<gtk::Image>().ok())
        else {
            return;
        };
        label.set_text(collection.name());
        bind_shell.set_download_badge_visible(
            &downloaded,
            bind_shell
                .library
                .selected
                .borrow()
                .as_ref()
                .is_some_and(|selected| collection.is_downloaded(selected)),
        );
    });
    factory.connect_unbind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item
            .child()
            .and_then(|child| child.downcast::<gtk::Box>().ok())
        else {
            return;
        };
        if let Some(label) = row
            .first_child()
            .and_then(|child| child.downcast::<gtk::Label>().ok())
        {
            label.set_text("");
        }
        if let Some(downloaded) = row
            .last_child()
            .and_then(|child| child.downcast::<gtk::Image>().ok())
        {
            downloaded.set_visible(false);
        }
    });
    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}

fn named_collection_text_column<F>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    value: F,
) -> gtk::ColumnViewColumn
where
    F: Fn(&NamedCollectionItem) -> String + 'static,
{
    let factory = gtk::SignalListItemFactory::new();
    let shell = Rc::clone(shell);
    let value = Rc::new(value);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(collection_item) = item_at_from_item::<NamedCollectionItem>(item) else {
            return;
        };
        let label = gtk::Label::new(Some(&(value)(&collection_item)));
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Fill);
        label.set_hexpand(true);
        label.set_wrap(false);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_single_line_mode(true);
        collection_item.install_context_menu(&label, &shell);
        item.set_child(Some(&label));
    });
    factory.connect_unbind(clear_list_item_child);
    let column = localized_column(title, &factory);
    column.set_fixed_width(width);
    column
}

pub(super) struct NamedCollectionGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_button: gtk::Button,
    current_item: Rc<RefCell<Option<NamedCollectionItem>>>,
    current_genre: Rc<RefCell<Option<GenreSummary>>>,
}

impl NamedCollectionGridCell {
    pub(super) fn new(
        shell: Rc<Shell>,
        kind: NamedCollectionKind,
        fields: &[LibraryField],
    ) -> Self {
        let current_item = Rc::new(RefCell::new(None::<NamedCollectionItem>));
        let current_genre = Rc::new(RefCell::new(None::<GenreSummary>));

        let overlay = cards::elastic_cover_overlay();
        let cover_button = collection_grid_cover_shell();
        let open_shell = Rc::clone(&shell);
        let open_item = Rc::clone(&current_item);
        cover_button.connect_clicked(move |_| {
            let Some(item) = open_item.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(item.route());
        });
        overlay.set_child(Some(&cover_button));

        let controls = cards::cover_play_hover_controls(0, kind.play_label());
        let play_shell = Rc::clone(&shell);
        let play_item = Rc::clone(&current_item);
        controls.play.connect_clicked(move |_| {
            if let Some(item) = play_item.borrow().as_ref() {
                item.play(&play_shell, QueuePlacement::Now);
            }
        });

        let next_shell = Rc::clone(&shell);
        let next_item = Rc::clone(&current_item);
        controls.play_next.connect_clicked(move |_| {
            if let Some(item) = next_item.borrow().as_ref() {
                item.play(&next_shell, QueuePlacement::Next);
            }
        });

        let last_shell = Rc::clone(&shell);
        let last_item = Rc::clone(&current_item);
        controls.play_last.connect_clicked(move |_| {
            if let Some(item) = last_item.borrow().as_ref() {
                item.play(&last_shell, QueuePlacement::Last);
            }
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let cover = cards::square_cover_frame(&overlay);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
        let downloaded_item = Rc::clone(&current_item);
        body.set_download_badge(shell.download_badge(true, move |selected| {
            downloaded_item
                .borrow()
                .as_ref()
                .is_some_and(|item| item.is_downloaded(selected))
        }));
        install_dynamic_genre_context_menu(&body.card, &shell, Rc::clone(&current_genre));

        Self {
            body,
            shell,
            cover_button,
            current_item,
            current_genre,
        }
    }
}

impl ReusableCollectionGridCell<NamedCollectionItem> for NamedCollectionGridCell {
    fn widget(&self) -> gtk::Widget {
        self.body.widget()
    }

    fn bind(&self, _: u32, item: NamedCollectionItem) {
        self.cover_button
            .set_child(Some(&self.shell.elastic_cover_group_tile_for_artwork(
                &item.artwork(),
                item.seed(),
                THUMB_COVER_SIZE,
            )));
        self.body
            .bind(item.name(), |field| (item.field(field), None));
        self.body.set_downloaded(
            &self.shell,
            self.shell
                .library
                .selected
                .borrow()
                .as_ref()
                .is_some_and(|selected| item.is_downloaded(selected)),
        );
        *self.current_genre.borrow_mut() = match &item {
            NamedCollectionItem::Genre(genre) => Some(genre.clone()),
            NamedCollectionItem::Mood(_) => None,
        };
        *self.current_item.borrow_mut() = Some(item);
    }

    fn clear(&self) {
        self.cover_button.set_child(None::<&gtk::Widget>);
        self.body.clear();
        *self.current_genre.borrow_mut() = None;
        *self.current_item.borrow_mut() = None;
    }

    fn apply_fields(&self, fields: &[LibraryField]) {
        self.body.replace_fields(&self.shell, fields);
        if let Some(item) = self.current_item.borrow().as_ref().cloned() {
            self.body
                .bind(item.name(), |field| (item.field(field), None));
        }
    }
}

impl Shell {
    pub(crate) fn library_named_collection_route(
        self: &Rc<Self>,
        kind: NamedCollectionKind,
        source_items: Arc<[NamedCollectionItem]>,
        loaded: Arc<Library>,
        music_folder_id: Option<MusicFolderId>,
    ) -> MountedRoute {
        let key = kind.key();
        let settings = self.settings.current.borrow().library_list(key);
        let applied_settings = Rc::new(RefCell::new(settings.clone()));
        let source_items = Rc::new(RefCell::new(source_items));
        let visible = Rc::new(RefCell::new(Arc::clone(&source_items.borrow())));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        replace_named_collection_items(&model, visible.borrow().iter().cloned());

        let search = gtk::SearchEntry::new();
        crate::localization::bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        {
            let shell = Rc::clone(self);
            let model = model.clone();
            let source_items = Rc::clone(&source_items);
            let visible = Rc::clone(&visible);
            search.connect_search_changed(move |entry| {
                let query = entry.text().trim().to_lowercase();
                let items = if query.is_empty() {
                    Arc::clone(&source_items.borrow())
                } else {
                    source_items
                        .borrow()
                        .iter()
                        .filter(|item| item.matches_query(&query))
                        .cloned()
                        .collect::<Vec<_>>()
                        .into()
                };
                *visible.borrow_mut() = items;
                populate_named_collection_model(
                    &model,
                    &visible.borrow(),
                    &shell.settings.current.borrow().library_list(key),
                );
            });
        }

        let content = named_collection_projection(self, model.clone(), kind);
        let visible_results = Rc::clone(&visible);
        let page_shell = self.library_page_shell(LibraryPageShellOptions {
            key,
            empty: source_items.borrow().is_empty(),
            empty_body: kind.fallback_warning(),
            search: search.clone(),
            has_visible_results: Rc::new(move || !visible_results.borrow().is_empty()),
            content: content.scrolling_widget(),
        });
        let route = match kind {
            NamedCollectionKind::Genres => Route::Genres,
            NamedCollectionKind::Moods => Route::Moods,
        };
        let identity = self.mounted_route_read_identity(route, &loaded, music_folder_id.clone());
        let apply = {
            let shell = Rc::clone(self);
            let source_items = Rc::clone(&source_items);
            let visible = Rc::clone(&visible);
            let model = model.clone();
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            Rc::new(
                move |request: NamedCollectionReadRequest,
                      result: Result<PreparedNamedCollection, String>| {
                    if !shell.mounted_route_read_is_current(&request.identity) {
                        return;
                    }
                    let prepared = match result {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                "failed to refresh the mounted collection route"
                            );
                            return;
                        }
                    };
                    source_items.replace(prepared.source);
                    visible.replace(prepared.visible);
                    replace_named_collection_items(&model, visible.borrow().iter().cloned());
                    content.apply_settings(&request.settings);
                    page_shell.apply_library_list_settings(key, &request.settings);
                    page_shell.set_empty(source_items.borrow().is_empty());
                    applied_settings.replace(request.settings);
                },
            )
        };
        let load = {
            let loaded = Arc::clone(&loaded);
            let music_folder_id = music_folder_id.clone();
            Arc::new(move |request: &NamedCollectionReadRequest| {
                load_named_collection(
                    &loaded,
                    music_folder_id.as_ref(),
                    kind,
                    &request.query,
                    &request.settings,
                )
            })
        };
        let read =
            LatestMountedRouteRead::new_with_request(apply, load, "mounted collection route");
        {
            let read = Rc::downgrade(&read);
            let identity = identity.clone();
            let shell = Rc::clone(self);
            search.connect_search_changed(move |entry| {
                let Some(read) = read.upgrade() else {
                    return;
                };
                read.request_with_if_running(NamedCollectionReadRequest {
                    identity: identity.clone(),
                    query: entry.text().trim().to_string(),
                    settings: shell.settings.current.borrow().library_list(key),
                });
            });
        }
        let resume = {
            let shell = Rc::clone(self);
            let model = model.clone();
            let visible = Rc::clone(&visible);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let applied_settings = Rc::clone(&applied_settings);
            let search = search.clone();
            let read = Rc::clone(&read);
            let identity = identity.clone();
            Rc::new(move || {
                let settings = shell.settings.current.borrow().library_list(key);
                let previous = applied_settings.borrow().clone();
                if previous.sort_key != settings.sort_key
                    || previous.descending != settings.descending
                {
                    populate_named_collection_model(&model, &visible.borrow(), &settings);
                }
                content.apply_settings(&settings);
                page_shell.apply_library_list_settings(key, &settings);
                *applied_settings.borrow_mut() = settings.clone();
                read.request_with_if_running(NamedCollectionReadRequest {
                    identity: identity.clone(),
                    query: search.text().trim().to_string(),
                    settings,
                });
            })
        };
        let update = {
            let read = Rc::clone(&read);
            let identity = identity.clone();
            let shell = Rc::clone(self);
            let search = search.clone();
            Rc::new(move |update: &crate::runtime::SelectedLibraryUpdate| {
                let changed = match kind {
                    NamedCollectionKind::Genres => !update.change.genres.is_empty(),
                    NamedCollectionKind::Moods => !update.change.moods.is_empty(),
                };
                if changed {
                    read.request_with(NamedCollectionReadRequest {
                        identity: identity.clone(),
                        query: search.text().trim().to_string(),
                        settings: shell.settings.current.borrow().library_list(key),
                    });
                }
            })
        };
        page_shell
            .mounted_route(resume, content.item_navigation())
            .with_library_update(update)
    }
}

pub(crate) fn load_named_collection(
    loaded: &Arc<Library>,
    music_folder_id: Option<&MusicFolderId>,
    kind: NamedCollectionKind,
    query: &str,
    settings: &LibraryListSettings,
) -> Result<PreparedNamedCollection, String> {
    let source = match kind {
        NamedCollectionKind::Genres => loaded.genres(music_folder_id).map(|items| {
            items
                .iter()
                .cloned()
                .map(NamedCollectionItem::Genre)
                .collect::<Vec<_>>()
                .into()
        }),
        NamedCollectionKind::Moods => loaded.moods(music_folder_id).map(|items| {
            items
                .iter()
                .cloned()
                .map(NamedCollectionItem::Mood)
                .collect::<Vec<_>>()
                .into()
        }),
    }
    .map_err(|error| error.to_string())?;
    Ok(prepare_named_collection(source, query, settings))
}

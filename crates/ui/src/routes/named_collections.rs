use std::{cell::RefCell, cmp::Ordering, rc::Rc, sync::Arc};

use ::library::{ActiveLibraryQuery, Genre, Mood, play_context::PlayContextDescriptor};
use adw::prelude::*;
use artwork::ArtworkBinding;
use gtk::{gio, glib};
use playback::{ContextPlayRequest, QueuePlacement};
use tracing::warn;

use super::collection_context::install_genre_context_menu;
use crate::format_duration_units;
use crate::localization::localized_column;
use crate::shell::Shell;
use crate::shell::cover::THUMB_COVER_SIZE;
use crate::shell::cover::presentation::stable_seed;
use crate::shell::route::MountedRoute;
use crate::{LibraryField, LibraryLayout, LibraryListKey, LibraryListSettings};
use localization::msgid;
use localization::{album_count_text, track_count_text};

use super::cards;
use super::collection_routes::{CollectionRouteSpec, load_complete_cached_items};
use super::collections::{
    CollectionTableProjection, LibraryCollectionProjection, LibraryPresentationProjection,
    collection_column_width, dynamic_collection_table,
};
use super::columns::{column_fit_width, row_index_column};
use super::grid_cells::{
    CollectionGridCardCell, CollectionGridProjection, ReusableCollectionGridCell, collection_grid,
    collection_grid_cover_shell, install_dynamic_genre_context_menu,
};
use super::library_fields::{
    apply_desc, clear_list_item_child, cmp_string, column_width, item_at_from_item,
};
use super::play_context::selected_music_folder_id;
use super::route::Route;
use super::table_sizing::route_column_view_initial_width;

#[derive(Clone, Copy)]
pub(crate) enum NamedCollectionKind {
    Genres,
    Moods,
}

#[derive(Clone)]
pub(crate) enum NamedCollectionItem {
    Genre(Genre),
    Mood(Mood),
}

impl NamedCollectionKind {
    fn key(self) -> LibraryListKey {
        match self {
            Self::Genres => LibraryListKey::Genres,
            Self::Moods => LibraryListKey::Moods,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Genres => "genres",
            Self::Moods => "moods",
        }
    }

    fn fallback_warning(self) -> &'static str {
        match self {
            Self::Genres => msgid("Cached entries will appear here after sync finishes"),
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

    fn page(self, query: &ActiveLibraryQuery, limit: usize) -> Result<NamedPage, String> {
        match self {
            Self::Genres => query.genres_page(0, limit).map(NamedPage::from_genres),
            Self::Moods => query.moods_page(0, limit).map(NamedPage::from_moods),
        }
    }

    pub(crate) fn load_items(self, query: &ActiveLibraryQuery) -> Vec<NamedCollectionItem> {
        load_complete_cached_items(|limit| self.page(query, limit).map(NamedPage::into_inner))
            .unwrap_or_else(|error| {
                warn!(%error, page = self.name(), "failed to load collection page");
                Vec::new()
            })
    }
}

impl NamedCollectionItem {
    fn name(&self) -> &str {
        match self {
            Self::Genre(genre) => &genre.name,
            Self::Mood(mood) => &mood.name,
        }
    }

    fn seed(&self) -> u32 {
        match self {
            Self::Genre(genre) => stable_seed(genre.id.as_str()),
            Self::Mood(mood) => stable_seed(mood.id.as_str()),
        }
    }

    fn route(&self) -> Route {
        match self {
            Self::Genre(genre) => Route::GenreDetail(genre.id.clone()),
            Self::Mood(mood) => Route::MoodDetail(mood.id.clone()),
        }
    }

    fn artwork(&self) -> Vec<ArtworkBinding> {
        match self {
            Self::Genre(genre) => ArtworkBinding::genre_slots(genre),
            Self::Mood(mood) => ArtworkBinding::mood_slots(mood),
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

    fn play(
        &self,
        controller: &playback::QueueHandle,
        placement: QueuePlacement,
        music_folder_id: Option<::library::MusicFolderId>,
    ) {
        let descriptor = match self {
            Self::Genre(genre) => PlayContextDescriptor::Genre {
                genre_id: genre.id.clone(),
                music_folder_id,
            },
            Self::Mood(mood) => PlayContextDescriptor::Mood {
                mood_id: mood.id.clone(),
                music_folder_id,
            },
        };
        controller.play_context(ContextPlayRequest::store(descriptor, placement));
    }

    fn install_context_menu(&self, widget: &impl IsA<gtk::Widget>, shell: &Rc<Shell>) {
        if let Self::Genre(genre) = self {
            install_genre_context_menu(widget, shell, genre.clone());
        }
    }
}

struct NamedPage(library::PagedResponse<NamedCollectionItem>);

impl NamedPage {
    fn from_genres(page: library::PagedResponse<Genre>) -> Self {
        Self(library::PagedResponse::new(
            page.items
                .into_iter()
                .map(NamedCollectionItem::Genre)
                .collect(),
            page.total,
        ))
    }

    fn from_moods(page: library::PagedResponse<Mood>) -> Self {
        Self(library::PagedResponse::new(
            page.items
                .into_iter()
                .map(NamedCollectionItem::Mood)
                .collect(),
            page.total,
        ))
    }

    fn into_inner(self) -> library::PagedResponse<NamedCollectionItem> {
        self.0
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

fn replace_named_collection_items(
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
            named_collection_text_column(shell, "Title", 180, true, |item| item.name().to_string())
        }
        _ => named_collection_text_column(shell, field.title(), column_width(field), false, {
            move |item| item.field(field)
        }),
    }
}

fn named_collection_text_column<F>(
    shell: &Rc<Shell>,
    title: &str,
    width: i32,
    expand: bool,
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
    column.set_resizable(true);
    column.set_expand(expand);
    column
}

pub(super) struct NamedCollectionGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_button: gtk::Button,
    current_item: Rc<RefCell<Option<NamedCollectionItem>>>,
    current_genre: Rc<RefCell<Option<Genre>>>,
}

impl NamedCollectionGridCell {
    pub(super) fn new(
        shell: Rc<Shell>,
        kind: NamedCollectionKind,
        fields: &[LibraryField],
    ) -> Self {
        let current_item = Rc::new(RefCell::new(None::<NamedCollectionItem>));
        let current_genre = Rc::new(RefCell::new(None::<Genre>));

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
        let controller = shell.products.playback.queue.clone();
        let play_shell = Rc::clone(&shell);
        let play_item = Rc::clone(&current_item);
        controls.play.connect_clicked(move |_| {
            if let Some(item) = play_item.borrow().as_ref() {
                item.play(
                    &controller,
                    QueuePlacement::Now,
                    selected_music_folder_id(&play_shell),
                );
            }
        });

        let controller = shell.products.playback.queue.clone();
        let next_shell = Rc::clone(&shell);
        let next_item = Rc::clone(&current_item);
        controls.play_next.connect_clicked(move |_| {
            if let Some(item) = next_item.borrow().as_ref() {
                item.play(
                    &controller,
                    QueuePlacement::Next,
                    selected_music_folder_id(&next_shell),
                );
            }
        });

        let controller = shell.products.playback.queue.clone();
        let last_shell = Rc::clone(&shell);
        let last_item = Rc::clone(&current_item);
        controls.play_last.connect_clicked(move |_| {
            if let Some(item) = last_item.borrow().as_ref() {
                item.play(
                    &controller,
                    QueuePlacement::Last,
                    selected_music_folder_id(&last_shell),
                );
            }
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let cover = cards::square_cover_frame(&overlay);
        let body = CollectionGridCardCell::new(&shell, fields, cover.upcast());
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

fn named_collection_route_spec(
    kind: NamedCollectionKind,
    query: ActiveLibraryQuery,
) -> CollectionRouteSpec<NamedCollectionItem> {
    let load_query = query.clone();
    CollectionRouteSpec {
        key: kind.key(),
        empty_body: kind.fallback_warning(),
        load_items: Arc::new(move || kind.load_items(&load_query)),
        matches_query: Rc::new(|item, query| item.matches_query(query)),
        populate_model: Rc::new(populate_named_collection_model),
        build_content: Rc::new(move |shell, model| named_collection_projection(shell, model, kind)),
        affected: Rc::new(move |delta| {
            delta.reset.is_some()
                || match kind {
                    NamedCollectionKind::Genres => !delta.genres.is_empty(),
                    NamedCollectionKind::Moods => {
                        !delta.tracks.added.is_empty()
                            || !delta.tracks.deleted.is_empty()
                            || !delta.tracks.fields.is_empty()
                            || !delta.tracks.metadata.is_empty()
                            || !delta.tracks.cover_refs.is_empty()
                    }
                }
        }),
    }
}

impl Shell {
    pub(crate) fn library_named_collection_route_from_prepared(
        self: &Rc<Self>,
        kind: NamedCollectionKind,
        query: ActiveLibraryQuery,
        items: Vec<NamedCollectionItem>,
    ) -> MountedRoute {
        named_collection_route_spec(kind, query).view_from_items(self, items)
    }
}

use super::*;
use crate::i18n::msgid;

#[derive(Clone, Copy)]
pub(in crate::ui) enum NamedCollectionKind {
    Genres,
    Moods,
}

#[derive(Clone)]
pub(in crate::ui) enum NamedCollectionItem {
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

    fn route(self) -> Route {
        match self {
            Self::Genres => Route::Genres,
            Self::Moods => Route::Moods,
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

    fn page(self, shell: &Shell, offset: usize, limit: usize) -> Result<NamedPage, String> {
        match self {
            Self::Genres => shell
                .controller
                .cached_genres_page(offset, limit)
                .map(NamedPage::from_genres),
            Self::Moods => shell
                .controller
                .cached_moods_page(offset, limit)
                .map(NamedPage::from_moods),
        }
    }

    fn page_matching(
        self,
        shell: &Shell,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<NamedPage, String> {
        match self {
            Self::Genres => shell
                .controller
                .cached_genres_page_matching(query, offset, limit)
                .map(NamedPage::from_genres),
            Self::Moods => shell
                .controller
                .cached_moods_page_matching(query, offset, limit)
                .map(NamedPage::from_moods),
        }
    }

    fn initial_page(self, shell: &Rc<Shell>) -> NamedPage {
        match self {
            Self::Genres => shell
                .complete_genre_snapshot_page()
                .map(NamedPage::from_genres)
                .unwrap_or_else(|| {
                    self.page(shell, 0, GRID_ROUTE_PAGE_SIZE)
                        .unwrap_or_else(|error| {
                            warn!(%error, "failed to load cached genres page");
                            let library = shell.state.library.borrow();
                            let genres = library
                                .genres
                                .iter()
                                .take(GRID_ROUTE_PAGE_SIZE)
                                .cloned()
                                .collect::<Vec<_>>();
                            NamedPage::from_genres(library::PagedResponse::new(
                                genres,
                                library.genres.len(),
                            ))
                        })
                }),
            Self::Moods => self
                .page(shell, 0, GRID_ROUTE_PAGE_SIZE)
                .unwrap_or_else(|error| {
                    warn!(%error, "failed to load cached moods page");
                    NamedPage::empty()
                }),
        }
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

    fn artwork(&self) -> Vec<CandidateSet> {
        match self {
            Self::Genre(genre) => CandidateSet::genre_slots(genre),
            Self::Mood(mood) => CandidateSet::mood_slots(mood),
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

    fn play_now(&self, controller: &crate::controller::AppController) {
        match self {
            Self::Genre(genre) => {
                let genre_id = genre.id.clone();
                if let Ok(Some(detail)) = controller.cached_genre_detail(&genre_id) {
                    let tracks = detail.tracks;
                    controller.play_genre_tracks_window(genre_id, tracks.len(), 0, |index| {
                        tracks.get(index).cloned()
                    });
                }
            }
            Self::Mood(mood) => {
                let mood_id = mood.id.clone();
                if let Ok(Some(detail)) = controller.cached_mood_detail(&mood_id) {
                    let tracks = detail.tracks;
                    controller.play_mood_tracks_window(mood_id, tracks.len(), 0, |index| {
                        tracks.get(index).cloned()
                    });
                }
            }
        }
    }

    fn play_next(&self, controller: &crate::controller::AppController) {
        if let Some(tracks) = self.tracks(controller) {
            for track in tracks.iter().rev() {
                controller.play_next(track.clone());
            }
        }
    }

    fn play_last(&self, controller: &crate::controller::AppController) {
        if let Some(tracks) = self.tracks(controller) {
            controller.play_last(tracks);
        }
    }

    fn tracks(&self, controller: &crate::controller::AppController) -> Option<Vec<Track>> {
        match self {
            Self::Genre(genre) => controller
                .cached_genre_detail(&genre.id)
                .ok()
                .flatten()
                .map(|detail| detail.tracks),
            Self::Mood(mood) => controller
                .cached_mood_detail(&mood.id)
                .ok()
                .flatten()
                .map(|detail| detail.tracks),
        }
    }

    fn install_context_menu(&self, widget: &impl IsA<gtk::Widget>, shell: &Rc<Shell>) {
        if let Self::Genre(genre) = self {
            install_genre_context_menu(widget, shell, genre.clone());
        }
    }
}

struct NamedPage(library::PagedResponse<NamedCollectionItem>);

impl NamedPage {
    fn empty() -> Self {
        Self(library::PagedResponse::new(Vec::new(), 0))
    }

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

pub(in crate::ui) fn sort_named_collection_items(
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

fn append_named_collection_items(
    model: &gio::ListStore,
    items: impl IntoIterator<Item = NamedCollectionItem>,
) {
    append_boxed_items_to_model(model, items);
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

pub(in crate::ui) fn named_collection_widget(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    kind: NamedCollectionKind,
) -> gtk::Widget {
    match shell.library_settings(kind.key()).layout {
        LibraryLayout::Row => named_collection_table(shell, model, kind).upcast(),
        LibraryLayout::Grid | LibraryLayout::Detail => {
            named_collection_grid(shell, model, kind).upcast()
        }
    }
}

fn named_collection_grid(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    kind: NamedCollectionKind,
) -> gtk::GridView {
    let (columns, card_size) = shell.collection_card_grid_metrics();
    let fields = shell.library_settings(kind.key()).grid_fields;
    let cell_shell = Rc::clone(shell);
    let activate_shell = Rc::clone(shell);
    collection_grid(
        model,
        columns,
        move || NamedCollectionGridCell::new(Rc::clone(&cell_shell), kind, &fields, card_size),
        move |_, item: NamedCollectionItem| activate_shell.navigate(item.route()),
    )
}

fn named_collection_table(
    shell: &Rc<Shell>,
    model: gio::ListStore,
    kind: NamedCollectionKind,
) -> gtk::ColumnView {
    let fields = shell.library_settings(kind.key()).row_fields;
    let columns = collection_table_columns(
        fields,
        |field| named_collection_column(shell, field),
        |field| {
            if matches!(field, LibraryField::Title | LibraryField::TitleMerged) {
                180
            } else {
                collection_column_width(field)
            }
        },
    );
    let activate_shell = Rc::clone(shell);
    collection_table(
        shell,
        model,
        columns,
        true,
        Some(Box::new(move |_, item: NamedCollectionItem| {
            activate_shell.navigate(item.route());
        })),
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
    let column = gtk::ColumnViewColumn::new(Some(&tr(title)), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column.set_expand(expand);
    column
}

pub(super) struct NamedCollectionGridCell {
    body: CollectionGridCardCell,
    shell: Rc<Shell>,
    cover_button: gtk::Button,
    size: i32,
    current_item: Rc<RefCell<Option<NamedCollectionItem>>>,
    current_genre: Rc<RefCell<Option<Genre>>>,
}

impl NamedCollectionGridCell {
    pub(super) fn new(
        shell: Rc<Shell>,
        kind: NamedCollectionKind,
        fields: &[LibraryField],
        size: i32,
    ) -> Self {
        let current_item = Rc::new(RefCell::new(None::<NamedCollectionItem>));
        let current_genre = Rc::new(RefCell::new(None::<Genre>));

        let overlay = cards::cover_overlay(size);
        let cover_button = collection_grid_cover_shell(size);
        let open_shell = Rc::clone(&shell);
        let open_item = Rc::clone(&current_item);
        cover_button.connect_clicked(move |_| {
            let Some(item) = open_item.borrow().as_ref().cloned() else {
                return;
            };
            open_shell.navigate(item.route());
        });
        overlay.set_child(Some(&cover_button));

        let controls = cards::cover_play_hover_controls(size, kind.play_label());
        let controller = shell.controller.clone();
        let play_item = Rc::clone(&current_item);
        controls.play.connect_clicked(move |_| {
            if let Some(item) = play_item.borrow().as_ref() {
                item.play_now(&controller);
            }
        });

        let controller = shell.controller.clone();
        let next_item = Rc::clone(&current_item);
        controls.play_next.connect_clicked(move |_| {
            if let Some(item) = next_item.borrow().as_ref() {
                item.play_next(&controller);
            }
        });

        let controller = shell.controller.clone();
        let last_item = Rc::clone(&current_item);
        controls.play_last.connect_clicked(move |_| {
            if let Some(item) = last_item.borrow().as_ref() {
                item.play_last(&controller);
            }
        });
        controls.add_to_overlay(&overlay);
        controls.connect_hover(&overlay);

        let body = CollectionGridCardCell::new(&shell, fields, size, overlay.upcast());
        install_dynamic_genre_context_menu(&body.card, &shell, Rc::clone(&current_genre));

        Self {
            body,
            shell,
            cover_button,
            size,
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
            .set_child(Some(&self.shell.cover_group_tile_for_artwork(
                &item.artwork(),
                item.seed(),
                self.size,
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
}

fn named_collection_route_spec(
    kind: NamedCollectionKind,
) -> CollectionRouteSpec<NamedCollectionItem> {
    CollectionRouteSpec {
        key: kind.key(),
        route: kind.route(),
        page_name: kind.name(),
        empty_body: kind.fallback_warning(),
        initial_page: Rc::new(move |shell| kind.initial_page(shell).into_inner()),
        load_page: Some(Rc::new(move |shell, offset, limit| {
            kind.page(shell, offset, limit).map(NamedPage::into_inner)
        })),
        load_matching_page: Some(Rc::new(move |shell, query, offset, limit| {
            kind.page_matching(shell, query, offset, limit)
                .map(NamedPage::into_inner)
        })),
        matches_query: Rc::new(|item, query| item.matches_query(query)),
        sort_items: Rc::new(sort_named_collection_items),
        populate_model: Rc::new(populate_named_collection_model),
        append_model: Rc::new(append_named_collection_items),
        build_content: Rc::new(move |shell, model| named_collection_widget(shell, model, kind)),
        after_replace: None,
    }
}

impl Shell {
    pub(in crate::ui) fn library_named_collection_view(
        self: &Rc<Self>,
        kind: NamedCollectionKind,
    ) -> gtk::Widget {
        named_collection_route_spec(kind).view(self)
    }
}

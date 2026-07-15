use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::Arc,
};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::localization::bind_search_placeholder;
use crate::shell::Shell;
use crate::shell::route::{MountedRoute, MountedRouteDeltaApplier};
use crate::{LibraryListKey, LibraryListSettings};

use super::collections::LibraryCollectionProjection;
use super::route_shell::LibraryPageShellOptions;

const COMPLETE_ROUTE_LIMIT: usize = i64::MAX as usize;

pub(crate) fn load_complete_cached_items<T>(
    mut load: impl FnMut(usize) -> Result<library::PagedResponse<T>, String>,
) -> Result<Vec<T>, String> {
    Ok(load(COMPLETE_ROUTE_LIMIT)?.items)
}

pub(crate) struct CompletePreparedItems<T> {
    pub(crate) items: Arc<Vec<T>>,
    pub(crate) prepared_guard: Arc<Vec<T>>,
}

pub(crate) fn complete_prepared_items<T>(
    prepared: library::PreparedPage<T>,
    load: impl FnMut(usize) -> Result<library::PagedResponse<T>, String>,
) -> Result<CompletePreparedItems<T>, String> {
    let prepared_guard = Arc::clone(&prepared.items);
    let items = if prepared.items.len() == prepared.total {
        prepared.items
    } else {
        Arc::new(load_complete_cached_items(load)?)
    };
    Ok(CompletePreparedItems {
        items,
        prepared_guard,
    })
}

pub(crate) fn smart_playlist_detail_affected(
    delta: &library::LibraryDelta,
    smart_playlist_id: &library::SmartPlaylistId,
) -> bool {
    delta.reset.is_some()
        || !delta.tracks.is_empty()
        || delta.smart_playlists.added.contains(smart_playlist_id)
        || delta.smart_playlists.deleted.contains(smart_playlist_id)
        || delta.smart_playlists.fields.contains(smart_playlist_id)
        || delta.smart_playlists.stats.contains(smart_playlist_id)
        || delta.smart_playlists.links.contains(smart_playlist_id)
        || delta.smart_playlists.cover_refs.contains(smart_playlist_id)
}

pub(super) type CollectionLoader<T> = Arc<dyn Fn() -> Vec<T> + Send + Sync>;
pub(super) type MountedRefreshLoader<T> = Arc<dyn Fn() -> T + Send + Sync>;
pub(super) type CollectionSearchMatcher<T> = Rc<dyn Fn(&T, &str) -> bool>;
pub(super) type CollectionModelPopulator<T> =
    Rc<dyn Fn(&gio::ListStore, &[T], &LibraryListSettings)>;
pub(super) type CollectionContentBuilder =
    Rc<dyn Fn(&Rc<Shell>, gio::ListStore) -> LibraryCollectionProjection>;
pub(super) type CollectionDeltaPredicate = Rc<dyn Fn(&library::LibraryDelta) -> bool>;

pub(super) struct CollectionRouteSpec<T: Clone + Send + 'static> {
    pub(super) key: LibraryListKey,
    pub(super) empty_body: &'static str,
    pub(super) load_items: CollectionLoader<T>,
    pub(super) matches_query: CollectionSearchMatcher<T>,
    pub(super) populate_model: CollectionModelPopulator<T>,
    pub(super) build_content: CollectionContentBuilder,
    pub(super) affected: CollectionDeltaPredicate,
}

struct MountedCollectionState<T: Clone + Send + 'static> {
    shell: Weak<Shell>,
    spec: Rc<CollectionRouteSpec<T>>,
    model: gio::ListStore,
    source_items: Rc<RefCell<Vec<T>>>,
    query: Rc<RefCell<String>>,
    page_shell: super::route_shell::LibraryPageShell,
}

impl<T: Clone + Send + 'static> MountedCollectionState<T> {
    fn replace(&self, loaded: Vec<T>) -> Result<(), Vec<T>> {
        let Some(shell) = self.shell.upgrade() else {
            return Err(loaded);
        };
        let settings = shell.settings.current.borrow().library_list(self.spec.key);
        let displaced = self.source_items.replace(loaded);
        let normalized = self.query.borrow().trim().to_lowercase();
        let empty = if normalized.is_empty() {
            let source_items = self.source_items.borrow();
            (self.spec.populate_model)(&self.model, &source_items, &settings);
            source_items.is_empty()
        } else {
            let visible = self
                .source_items
                .borrow()
                .iter()
                .filter(|item| (self.spec.matches_query)(item, &normalized))
                .cloned()
                .collect::<Vec<_>>();
            (self.spec.populate_model)(&self.model, &visible, &settings);
            visible.is_empty()
        };
        self.page_shell.set_empty(empty);
        release_loaded_items(displaced);
        Ok(())
    }
}

pub(crate) struct MountedRouteRefresh<T: Send + 'static> {
    apply: Weak<dyn Fn(T)>,
    load: MountedRefreshLoader<T>,
    context: &'static str,
    generation: Cell<u64>,
    running: Cell<bool>,
}

impl<T: Send + 'static> MountedRouteRefresh<T> {
    pub(crate) fn new(
        apply: Weak<dyn Fn(T)>,
        load: MountedRefreshLoader<T>,
        context: &'static str,
    ) -> Rc<Self> {
        Rc::new(Self {
            apply,
            load,
            context,
            generation: Cell::new(0),
            running: Cell::new(false),
        })
    }

    pub(crate) fn request(self: &Rc<Self>) {
        self.generation.set(self.generation.get().wrapping_add(1));
        self.start();
    }

    fn start(self: &Rc<Self>) {
        if self.running.replace(true) {
            return;
        }
        if self.apply.upgrade().is_none() {
            self.running.set(false);
            return;
        }

        let generation = self.generation.get();
        let load = Arc::clone(&self.load);
        let refresh = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || load()).await;
            let loaded = match result {
                Ok(loaded) => loaded,
                Err(_) => {
                    tracing::warn!(
                        context = refresh
                            .upgrade()
                            .map_or("detached route", |refresh| refresh.context),
                        "mounted route refresh task panicked"
                    );
                    if let Some(refresh) = refresh.upgrade() {
                        refresh.running.set(false);
                        if refresh.generation.get() != generation {
                            refresh.start();
                        }
                    }
                    return;
                }
            };
            let Some(refresh) = refresh.upgrade() else {
                release_loaded_value(loaded);
                return;
            };
            refresh.running.set(false);

            if refresh.generation.get() != generation {
                release_loaded_value(loaded);
                refresh.start();
                return;
            }
            let Some(apply) = refresh.apply.upgrade() else {
                release_loaded_value(loaded);
                return;
            };
            apply(loaded);
        });
    }
}

fn release_loaded_value<T: Send + 'static>(value: T) {
    glib::spawn_future_local(async move {
        let _ = gio::spawn_blocking(move || drop(value)).await;
    });
}

fn release_loaded_items<T: Send + 'static>(items: Vec<T>) {
    if items.is_empty() {
        return;
    }
    release_loaded_value(items);
}

impl<T: Clone + Send + 'static> CollectionRouteSpec<T> {
    pub(super) fn view_from_items(self, shell: &Rc<Shell>, loaded: Vec<T>) -> MountedRoute {
        let this = Rc::new(self);

        let settings = shell.settings.current.borrow().library_list(this.key);
        let applied_settings = Rc::new(RefCell::new(settings.clone()));
        let applied_playlist_artwork = Rc::new(Cell::new(
            shell
                .settings
                .current
                .borrow()
                .prefer_server_playlist_covers,
        ));
        let source_items = Rc::new(RefCell::new(loaded));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        (this.populate_model)(&model, &source_items.borrow(), &settings);

        let search = gtk::SearchEntry::new();
        bind_search_placeholder(&search, "Search");
        search.set_hexpand(true);
        let query = Rc::new(RefCell::new(String::new()));

        {
            let spec = Rc::clone(&this);
            let shell = Rc::clone(shell);
            let model = model.clone();
            let source_items = Rc::clone(&source_items);
            let query = Rc::clone(&query);
            search.connect_search_changed(move |entry| {
                let text = entry.text().trim().to_string();
                *query.borrow_mut() = text.clone();
                let normalized = text.to_lowercase();
                let settings = shell.settings.current.borrow().library_list(spec.key);
                if normalized.is_empty() {
                    (spec.populate_model)(&model, &source_items.borrow(), &settings);
                } else {
                    let values = source_items
                        .borrow()
                        .iter()
                        .filter(|item| (spec.matches_query)(item, &normalized))
                        .cloned()
                        .collect::<Vec<_>>();
                    (spec.populate_model)(&model, &values, &settings);
                }
            });
        }

        let content = (this.build_content)(shell, model.clone());
        let page_shell = shell.library_page_shell(LibraryPageShellOptions {
            key: this.key,
            empty: source_items.borrow().is_empty(),
            empty_body: this.empty_body,
            search,
            content: content.scrolling_widget(),
        });
        let state = Rc::new(MountedCollectionState {
            shell: Rc::downgrade(shell),
            spec: Rc::clone(&this),
            model: model.clone(),
            source_items: Rc::clone(&source_items),
            query: Rc::clone(&query),
            page_shell: page_shell.clone(),
        });
        let apply_loaded: Rc<dyn Fn(Vec<T>)> = {
            let state = Rc::clone(&state);
            Rc::new(move |loaded| {
                if let Err(loaded) = state.replace(loaded) {
                    release_loaded_items(loaded);
                }
            })
        };
        let refresh = MountedRouteRefresh::new(
            Rc::downgrade(&apply_loaded),
            Arc::clone(&this.load_items),
            "mounted collection",
        );
        let affected_by = Rc::clone(&this.affected);
        let apply_delta = {
            let apply_loaded = Rc::clone(&apply_loaded);
            let refresh = Rc::clone(&refresh);
            Rc::new(move |_: &library::LibraryDelta| {
                let _ = &apply_loaded;
                refresh.request();
            }) as MountedRouteDeltaApplier
        };
        let resume = {
            let shell = Rc::clone(shell);
            let spec = Rc::clone(&this);
            let content = content.clone();
            let page_shell = page_shell.clone();
            let model = model.clone();
            let source_items = Rc::clone(&source_items);
            let query = Rc::clone(&query);
            let applied_settings = Rc::clone(&applied_settings);
            let applied_playlist_artwork = Rc::clone(&applied_playlist_artwork);
            Rc::new(move || {
                let settings = shell.settings.current.borrow().library_list(spec.key);
                let prefer_server_playlist_covers = shell
                    .settings
                    .current
                    .borrow()
                    .prefer_server_playlist_covers;
                let previous = applied_settings.borrow().clone();
                let playlist_artwork_changed = spec.key == LibraryListKey::Playlists
                    && applied_playlist_artwork.get() != prefer_server_playlist_covers;
                if previous.sort_key != settings.sort_key
                    || previous.descending != settings.descending
                    || playlist_artwork_changed
                {
                    let normalized = query.borrow().trim().to_lowercase();
                    if normalized.is_empty() {
                        (spec.populate_model)(&model, &source_items.borrow(), &settings);
                    } else {
                        let visible = source_items
                            .borrow()
                            .iter()
                            .filter(|item| (spec.matches_query)(item, &normalized))
                            .cloned()
                            .collect::<Vec<_>>();
                        (spec.populate_model)(&model, &visible, &settings);
                    }
                }
                content.apply_settings(&settings);
                page_shell.apply_library_list_settings(spec.key, &settings);
                *applied_settings.borrow_mut() = settings;
                applied_playlist_artwork.set(prefer_server_playlist_covers);
            })
        };
        MountedRoute::new(page_shell.widget(), affected_by, apply_delta, resume)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::complete_prepared_items;

    #[test]
    fn incomplete_warm_page_cannot_truncate_a_complete_route() {
        let warm = Arc::new(vec![1_u8, 2]);
        let completed = complete_prepared_items(
            library::PreparedPage {
                items: Arc::clone(&warm),
                total: 4,
            },
            |_| Ok(library::PagedResponse::new(vec![1_u8, 2, 3, 4], 4)),
        )
        .expect("complete route items");

        assert_eq!(completed.items.as_slice(), [1, 2, 3, 4]);
    }
}

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use artwork::{ArtworkBinding, PrefetchPriority};
use gtk::glib;
use tracing::warn;

use crate::Settings as UiSettings;

use super::Shell;

pub(crate) const GRID_COVER_SIZE: u32 = 256;
pub(crate) const DETAIL_COVER_SIZE: u32 = 512;
pub(crate) const THUMB_COVER_SIZE: u32 = 96;
const ROUTE_ARTWORK_SCROLL_SETTLE: Duration = Duration::from_millis(160);
const ROUTE_ARTWORK_PREFETCH_RESUME: Duration = Duration::from_millis(1_500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlaybackArtworkPath {
    pub(crate) path: PathBuf,
}

pub(crate) mod presentation;
mod source_warm;
mod tile;
mod tiles;

pub(super) use source_warm::SourceWarmState;

pub(crate) use tile::{ArtworkTile, ArtworkTileWeak};
pub(crate) use tiles::CoverGroupProjection;

pub(crate) fn cover_decode_size(display_size: i32, fetch_size: u32) -> i32 {
    display_size.max(fetch_size as i32).max(1)
}

pub(crate) fn cover_fetch_size_for_display(display_size: i32) -> u32 {
    if display_size <= THUMB_COVER_SIZE as i32 {
        THUMB_COVER_SIZE
    } else if display_size <= GRID_COVER_SIZE as i32 {
        GRID_COVER_SIZE
    } else {
        DETAIL_COVER_SIZE
    }
}

#[derive(Clone)]
pub(crate) struct CoverBinding {
    pub(crate) tile: ArtworkTileWeak,
    pub(crate) generation: u64,
}

#[derive(Clone)]
pub(super) struct LiveArtworkBinding {
    tile: ArtworkTileWeak,
    source_id: Option<::library::SourceId>,
    artwork: ArtworkBinding,
    seed: u32,
    render_size: i32,
    fetch_size: u32,
    defer_during_route_scroll: bool,
}

#[derive(Default)]
pub(super) struct RouteArtworkInteraction {
    active: Cell<bool>,
    deadline: Cell<Option<Instant>>,
    settle: RefCell<Option<glib::JoinHandle<()>>>,
    prefetch_resume_deadline: Cell<Option<Instant>>,
    prefetch_resume: RefCell<Option<glib::JoinHandle<()>>>,
    prefetch_paused: Cell<bool>,
    deferred: RefCell<HashSet<usize>>,
    adjustment_handler: RefCell<Option<RouteArtworkAdjustmentHandler>>,
}

struct RouteArtworkAdjustmentHandler {
    object: glib::WeakRef<glib::Object>,
    signal: glib::SignalHandlerId,
}

pub(super) struct ArtworkState {
    pub(super) startup_prime_pending: RefCell<HashSet<artwork::RequestId>>,
    pub(super) bindings: RefCell<HashMap<artwork::RequestId, Vec<CoverBinding>>>,
    pub(super) live_bindings: RefCell<HashMap<usize, LiveArtworkBinding>>,
    pub(super) route_interaction: Rc<RouteArtworkInteraction>,
    pub(super) source_warm: Rc<SourceWarmState>,
}

enum ArtworkBindingResult {
    Ready(gtk::gdk::Texture),
    Missing,
    Failed,
}

impl Shell {
    pub(crate) fn bind_artwork_tile(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        artwork: ArtworkBinding,
        seed: u32,
        render_size: i32,
        fetch_size: u32,
    ) {
        self.bind_live_artwork_tile(
            tile,
            LiveArtworkBinding {
                tile: tile.downgrade(),
                source_id: None,
                artwork,
                seed,
                render_size,
                fetch_size,
                defer_during_route_scroll: true,
            },
        );
    }

    pub(crate) fn bind_playback_artwork_tile(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        source_id: &::library::SourceId,
        artwork: ArtworkBinding,
        seed: u32,
        render_size: i32,
        fetch_size: u32,
    ) {
        self.bind_live_artwork_tile(
            tile,
            LiveArtworkBinding {
                tile: tile.downgrade(),
                source_id: Some(source_id.clone()),
                artwork,
                seed,
                render_size,
                fetch_size,
                defer_during_route_scroll: false,
            },
        );
    }

    fn bind_live_artwork_tile(self: &Rc<Self>, tile: &ArtworkTile, binding: LiveArtworkBinding) {
        if binding.artwork.is_empty() {
            self.cancel_artwork_tile_request(tile);
            tile.bind_missing(binding.seed);
            self.artwork
                .route_interaction
                .deferred
                .borrow_mut()
                .remove(&tile.identity());
            self.remember_artwork_binding(tile, binding);
            return;
        }

        let source_id = binding.source_id.clone();
        let artwork = binding.artwork.clone();
        let seed = binding.seed;
        let render_size = binding.render_size;
        let fetch_size = binding.fetch_size;
        let cache_only =
            binding.defer_during_route_scroll && self.artwork.route_interaction.active.get();

        let render_size = cover_decode_size(render_size, fetch_size).max(1);
        let settings = self.settings.current.borrow().clone();
        let external = artwork_external_policy(&settings);
        let prepared = match self.products.artwork.prepare(
            source_id.as_ref(),
            artwork,
            fetch_size,
            render_size as u32,
            external,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                warn!(%error, "failed to identify artwork request");
                self.cancel_artwork_tile_request(tile);
                tile.bind_pending(seed);
                self.remember_artwork_binding(tile, binding);
                return;
            }
        };
        if cache_only && prepared.ready.is_none() {
            self.defer_route_artwork_binding(tile, binding);
            return;
        }

        self.artwork
            .route_interaction
            .deferred
            .borrow_mut()
            .remove(&tile.identity());
        self.remember_artwork_binding(tile, binding);
        let outcome = tile.bind_selected_cover(
            seed,
            prepared.identity.visual.clone(),
            prepared.identity.request.clone(),
        );
        if !outcome.request_needed {
            return;
        }
        if let Some(image) = prepared.ready.as_ref() {
            self.cancel_artwork_tile_request(tile);
            if let Some(texture) = texture_from_decoded(image) {
                tile.set_texture_if_current(outcome.generation, texture);
            } else {
                tile.set_blank_if_current(outcome.generation);
            }
            return;
        }
        if !outcome.request_changed && tile.artwork_request_id().is_some() {
            return;
        }
        self.cancel_artwork_tile_request(tile);

        match self.products.artwork.request(prepared) {
            Ok(projection) => match projection.readiness {
                artwork::Readiness::Pending => {
                    let request_id = projection.request_id;
                    if let Some(previous) = tile.replace_artwork_request_id(request_id) {
                        self.products.artwork.cancel(previous);
                        self.artwork.bindings.borrow_mut().remove(&previous);
                    }
                    self.register_cover_binding(request_id, tile, outcome.generation);
                }
                artwork::Readiness::Ready(image) => {
                    if let Some(texture) = texture_from_decoded(&image) {
                        tile.set_texture_if_current(outcome.generation, texture);
                    } else {
                        tile.set_blank_if_current(outcome.generation);
                    }
                }
                artwork::Readiness::Missing => {
                    tile.set_missing_if_current(outcome.generation);
                }
                artwork::Readiness::Failed(_) => {
                    tile.set_blank_if_current(outcome.generation);
                }
            },
            Err(error) => {
                warn!(%error, "failed to start artwork request");
                tile.set_blank_if_current(outcome.generation);
            }
        }
    }

    fn defer_route_artwork_binding(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        binding: LiveArtworkBinding,
    ) {
        self.cancel_artwork_tile_request(tile);
        tile.bind_pending(binding.seed);
        self.remember_artwork_binding(tile, binding);
        self.artwork
            .route_interaction
            .deferred
            .borrow_mut()
            .insert(tile.identity());
    }

    pub(crate) fn clear_artwork_tile(self: &Rc<Self>, tile: &ArtworkTile) {
        self.artwork
            .live_bindings
            .borrow_mut()
            .remove(&tile.identity());
        self.artwork
            .route_interaction
            .deferred
            .borrow_mut()
            .remove(&tile.identity());
        self.cancel_artwork_tile_request(tile);
        tile.clear_image();
    }

    fn remember_artwork_binding(self: &Rc<Self>, tile: &ArtworkTile, binding: LiveArtworkBinding) {
        let shell = Rc::downgrade(self);
        tile.install_cleanup_hook_once(move |identity, request_id| {
            let Some(shell) = shell.upgrade() else {
                return;
            };
            shell.release_artwork_tile_registration(identity, request_id);
        });
        tile.mark_artwork_bound();
        self.artwork
            .live_bindings
            .borrow_mut()
            .insert(tile.identity(), binding);
    }

    pub(crate) fn refresh_artwork_bindings(self: &Rc<Self>) {
        if self.artwork.route_interaction.active.get() {
            let deferred = self
                .artwork
                .live_bindings
                .borrow()
                .iter()
                .filter_map(|(identity, binding)| {
                    binding.defer_during_route_scroll.then_some(*identity)
                })
                .collect::<Vec<_>>();
            self.artwork
                .route_interaction
                .deferred
                .borrow_mut()
                .extend(deferred);
            return;
        }
        let bindings = {
            let mut bindings = self.artwork.live_bindings.borrow_mut();
            bindings.retain(|_, binding| binding.tile.is_bound());
            bindings.values().cloned().collect::<Vec<_>>()
        };
        for binding in bindings {
            let Some(tile) = binding.tile.upgrade() else {
                continue;
            };
            self.bind_live_artwork_tile(&tile, binding);
        }
    }

    pub(crate) fn install_route_artwork_interaction(self: &Rc<Self>, adjustment: &gtk::Adjustment) {
        let shell = Rc::downgrade(self);
        replace_route_artwork_adjustment_handler(
            &self.artwork.route_interaction,
            adjustment,
            move || {
                let Some(shell) = shell.upgrade() else {
                    return;
                };
                shell.defer_route_artwork_until_scroll_settles();
            },
        );
    }

    pub(crate) fn cancel_route_artwork_interaction(&self) {
        self.resume_route_artwork_prefetch();
        cancel_route_artwork_settle(&self.artwork.route_interaction);
    }

    fn defer_route_artwork_until_scroll_settles(self: &Rc<Self>) {
        self.pause_route_artwork_prefetch();
        let shell = Rc::downgrade(self);
        defer_route_artwork_settle(
            &glib::MainContext::default(),
            &self.artwork.route_interaction,
            ROUTE_ARTWORK_SCROLL_SETTLE,
            move || {
                let Some(shell) = shell.upgrade() else {
                    return;
                };
                shell.refresh_deferred_route_artwork_bindings();
            },
        );
    }

    fn refresh_deferred_route_artwork_bindings(self: &Rc<Self>) {
        let deferred = std::mem::take(&mut *self.artwork.route_interaction.deferred.borrow_mut());
        let bindings = {
            let mut bindings = self.artwork.live_bindings.borrow_mut();
            bindings.retain(|_, binding| binding.tile.is_bound());
            deferred
                .into_iter()
                .filter_map(|identity| bindings.get(&identity).cloned())
                .collect::<Vec<_>>()
        };
        for binding in bindings {
            let Some(tile) = binding.tile.upgrade() else {
                continue;
            };
            self.bind_live_artwork_tile(&tile, binding);
        }
    }

    fn pause_route_artwork_prefetch(self: &Rc<Self>) {
        let interaction = &self.artwork.route_interaction;
        if !interaction.prefetch_paused.replace(true) {
            for priority in [
                PrefetchPriority::Viewport,
                PrefetchPriority::Background,
                PrefetchPriority::Idle,
            ] {
                self.products.artwork.set_prefetch_paused(priority, true);
            }
        }
        interaction
            .prefetch_resume_deadline
            .set(Some(Instant::now() + ROUTE_ARTWORK_PREFETCH_RESUME));
        if interaction.prefetch_resume.borrow().is_none() {
            self.schedule_route_artwork_prefetch_resume(ROUTE_ARTWORK_PREFETCH_RESUME);
        }
    }

    fn schedule_route_artwork_prefetch_resume(self: &Rc<Self>, delay: Duration) {
        let shell = Rc::downgrade(self);
        let resume = glib::MainContext::default().spawn_local(async move {
            glib::timeout_future(delay).await;
            let Some(shell) = shell.upgrade() else {
                return;
            };
            let interaction = &shell.artwork.route_interaction;
            interaction.prefetch_resume.borrow_mut().take();
            let remaining = interaction
                .prefetch_resume_deadline
                .get()
                .and_then(|deadline| {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    (!remaining.is_zero()).then_some(remaining)
                });
            if let Some(remaining) = remaining {
                shell.schedule_route_artwork_prefetch_resume(remaining);
                return;
            }
            interaction.prefetch_resume_deadline.set(None);
            shell.resume_route_artwork_prefetch();
        });
        self.artwork
            .route_interaction
            .prefetch_resume
            .replace(Some(resume));
    }

    fn resume_route_artwork_prefetch(&self) {
        let interaction = &self.artwork.route_interaction;
        if !interaction.prefetch_paused.replace(false) {
            return;
        }
        for priority in [
            PrefetchPriority::Viewport,
            PrefetchPriority::Background,
            PrefetchPriority::Idle,
        ] {
            self.products.artwork.set_prefetch_paused(priority, false);
        }
    }

    fn cancel_artwork_tile_request(self: &Rc<Self>, tile: &ArtworkTile) {
        let Some(request_id) = tile.artwork_request_id() else {
            return;
        };
        tile.clear_artwork_request_id(request_id);
        self.cancel_artwork_request_registration(request_id);
    }

    fn release_artwork_tile_registration(
        self: &Rc<Self>,
        identity: usize,
        request_id: Option<artwork::RequestId>,
    ) {
        self.artwork.live_bindings.borrow_mut().remove(&identity);
        self.artwork
            .route_interaction
            .deferred
            .borrow_mut()
            .remove(&identity);
        if let Some(request_id) = request_id {
            self.cancel_artwork_request_registration(request_id);
        }
    }

    fn cancel_artwork_request_registration(self: &Rc<Self>, request_id: artwork::RequestId) {
        self.artwork.bindings.borrow_mut().remove(&request_id);
        let startup_prime_finished = {
            let mut pending = self.artwork.startup_prime_pending.borrow_mut();
            pending.remove(&request_id) && pending.is_empty()
        };
        self.products.artwork.cancel(request_id);
        if startup_prime_finished {
            self.try_reveal_startup_route();
        }
    }

    pub(crate) fn current_playback_cached_artwork_path(
        &self,
        source_id: &::library::SourceId,
        entry: &playback::SequenceEntry,
        preferred_size: u32,
    ) -> Option<PlaybackArtworkPath> {
        let candidates = ArtworkBinding::track(&entry.track);
        let settings = self.settings.current.borrow().clone();
        let external = artwork_external_policy(&settings);
        let path = self.products.artwork.cached_path(
            source_id,
            candidates,
            preferred_size,
            preferred_size,
            external,
        )?;
        Some(PlaybackArtworkPath { path })
    }

    fn register_cover_binding(
        &self,
        request_id: artwork::RequestId,
        tile: &ArtworkTile,
        generation: u64,
    ) {
        let mut bindings = self.artwork.bindings.borrow_mut();
        bindings.entry(request_id).or_default().push(CoverBinding {
            tile: tile.downgrade(),
            generation,
        });
    }

    pub(crate) fn apply_artwork_event(self: &Rc<Self>, event: artwork::ArtworkEvent) {
        match event {
            artwork::ArtworkEvent::Changed(projection) => self.apply_artwork_projection(projection),
            artwork::ArtworkEvent::Invalidated(request_id) => {
                self.finish_artwork_request(request_id, ArtworkBindingResult::Failed)
            }
        }
    }

    fn apply_artwork_projection(self: &Rc<Self>, projection: artwork::ArtworkProjection) {
        match projection.readiness {
            artwork::Readiness::Pending => {}
            artwork::Readiness::Ready(image) => {
                if let Some(texture) = texture_from_decoded(&image) {
                    self.finish_artwork_request(
                        projection.request_id,
                        ArtworkBindingResult::Ready(texture),
                    );
                } else {
                    self.finish_artwork_request(
                        projection.request_id,
                        ArtworkBindingResult::Failed,
                    );
                }
            }
            artwork::Readiness::Missing => {
                self.finish_artwork_request(projection.request_id, ArtworkBindingResult::Missing);
            }
            artwork::Readiness::Failed(_) => {
                self.finish_artwork_request(projection.request_id, ArtworkBindingResult::Failed);
            }
        }
    }

    fn finish_artwork_request(
        self: &Rc<Self>,
        request_id: artwork::RequestId,
        result: ArtworkBindingResult,
    ) {
        self.artwork
            .startup_prime_pending
            .borrow_mut()
            .remove(&request_id);
        let bindings = self
            .artwork
            .bindings
            .borrow_mut()
            .remove(&request_id)
            .unwrap_or_default();
        for binding in bindings {
            let Some(tile) = binding.tile.upgrade() else {
                continue;
            };
            tile.clear_artwork_request_id(request_id);
            match &result {
                ArtworkBindingResult::Ready(texture) => {
                    tile.set_texture_if_current(binding.generation, texture.clone());
                }
                ArtworkBindingResult::Missing => {
                    tile.set_missing_if_current(binding.generation);
                }
                ArtworkBindingResult::Failed => {
                    tile.set_blank_if_current(binding.generation);
                }
            }
        }
        self.try_reveal_startup_route();
    }

    pub(crate) fn reset_cover_pipeline_state(&self) {
        let bindings = std::mem::take(&mut *self.artwork.bindings.borrow_mut());
        for (request_id, request_bindings) in bindings {
            for binding in request_bindings {
                if let Some(tile) = binding.tile.upgrade() {
                    tile.clear_artwork_request_id(request_id);
                }
            }
            self.products.artwork.cancel(request_id);
        }
        self.finish_startup_cover_prime_gate();
    }

    pub(crate) fn reset_route_covers(&self) {
        self.reconcile_artwork_requests();
    }

    pub(in crate::shell) fn begin_startup_cover_prime(&self) {
        *self.artwork.startup_prime_pending.borrow_mut() = self.pending_artwork_requests();
    }

    pub(in crate::shell) fn startup_cover_prime_pending_count(&self) -> usize {
        self.artwork.startup_prime_pending.borrow().len()
    }

    pub(in crate::shell) fn finish_startup_cover_prime_gate(&self) {
        self.artwork.startup_prime_pending.borrow_mut().clear();
    }

    fn pending_artwork_requests(&self) -> HashSet<artwork::RequestId> {
        self.reconcile_artwork_requests();
        self.artwork.bindings.borrow().keys().copied().collect()
    }

    fn reconcile_artwork_requests(&self) {
        let stale = {
            let mut bindings = self.artwork.bindings.borrow_mut();
            let mut stale = Vec::new();
            bindings.retain(|request_id, request_bindings| {
                request_bindings.retain(|binding| {
                    let Some(tile) = binding.tile.upgrade() else {
                        return false;
                    };
                    if tile.is_current_generation(binding.generation) {
                        true
                    } else {
                        tile.clear_artwork_request_id(*request_id);
                        false
                    }
                });
                if request_bindings.is_empty() {
                    stale.push(*request_id);
                    false
                } else {
                    true
                }
            });
            stale
        };
        for request_id in stale {
            self.products.artwork.cancel(request_id);
            self.artwork
                .startup_prime_pending
                .borrow_mut()
                .remove(&request_id);
        }
    }
}

fn defer_route_artwork_settle(
    context: &glib::MainContext,
    interaction: &Rc<RouteArtworkInteraction>,
    delay: Duration,
    settle: impl Fn() + 'static,
) {
    interaction.active.set(true);
    interaction.deadline.set(Some(Instant::now() + delay));
    if interaction.settle.borrow().is_some() {
        return;
    }
    schedule_route_artwork_settle(context.clone(), interaction, delay, Rc::new(settle));
}

fn schedule_route_artwork_settle(
    context: glib::MainContext,
    interaction: &Rc<RouteArtworkInteraction>,
    delay: Duration,
    settle: Rc<dyn Fn()>,
) {
    let interaction_weak = Rc::downgrade(interaction);
    let next_context = context.clone();
    let pending = context.spawn_local(async move {
        glib::timeout_future(delay).await;
        let Some(interaction) = interaction_weak.upgrade() else {
            return;
        };
        interaction.settle.borrow_mut().take();
        let remaining = interaction.deadline.get().and_then(|deadline| {
            let remaining = deadline.saturating_duration_since(Instant::now());
            (!remaining.is_zero()).then_some(remaining)
        });
        if let Some(remaining) = remaining {
            schedule_route_artwork_settle(next_context, &interaction, remaining, settle);
            return;
        }
        interaction.deadline.set(None);
        interaction.active.set(false);
        settle();
    });
    interaction.settle.replace(Some(pending));
}

fn cancel_route_artwork_settle(interaction: &RouteArtworkInteraction) {
    disconnect_route_artwork_adjustment_handler(interaction);
    if let Some(pending) = interaction.settle.borrow_mut().take() {
        pending.abort();
    }
    interaction.deadline.set(None);
    interaction.active.set(false);
    interaction.deferred.borrow_mut().clear();
    interaction.prefetch_resume_deadline.set(None);
    if let Some(resume) = interaction.prefetch_resume.borrow_mut().take() {
        resume.abort();
    }
}

fn replace_route_artwork_adjustment_handler(
    interaction: &RouteArtworkInteraction,
    adjustment: &gtk::Adjustment,
    changed: impl Fn() + 'static,
) {
    replace_route_artwork_signal_handler(interaction, adjustment, || {
        adjustment.connect_value_changed(move |_| changed())
    });
}

fn replace_route_artwork_signal_handler(
    interaction: &RouteArtworkInteraction,
    object: &impl IsA<glib::Object>,
    connect: impl FnOnce() -> glib::SignalHandlerId,
) {
    disconnect_route_artwork_adjustment_handler(interaction);
    let signal = connect();
    interaction
        .adjustment_handler
        .replace(Some(RouteArtworkAdjustmentHandler {
            object: object.as_ref().downgrade(),
            signal,
        }));
}

fn disconnect_route_artwork_adjustment_handler(interaction: &RouteArtworkInteraction) {
    let Some(handler) = interaction.adjustment_handler.borrow_mut().take() else {
        return;
    };
    let Some(object) = handler.object.upgrade() else {
        return;
    };
    object.disconnect(handler.signal);
}

fn artwork_external_policy(settings: &UiSettings) -> artwork::ExternalPolicy {
    artwork::ExternalPolicy::new(
        settings.metadata.external_metadata_enabled,
        settings.metadata.external_metadata_enabled && !settings.private_mode,
        settings.lastfm_api_key.clone(),
    )
}

fn texture_from_decoded(image: &artwork::DecodedImage) -> Option<gtk::gdk::Texture> {
    let width = i32::try_from(image.width()).ok()?;
    let height = i32::try_from(image.height()).ok()?;
    let row_stride = usize::try_from(image.row_stride()).ok()?;
    let bytes = glib::Bytes::from_owned(image.shared_rgba());
    Some(
        gtk::gdk::MemoryTexture::new(
            width,
            height,
            gtk::gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            row_stride,
        )
        .upcast(),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    use gtk::gio;
    use gtk::gio::prelude::ActionExt;

    use super::{
        RouteArtworkInteraction, defer_route_artwork_settle,
        disconnect_route_artwork_adjustment_handler, replace_route_artwork_signal_handler,
    };

    #[test]
    fn scroll_burst_keeps_one_settle_task_and_runs_one_refresh_after_quiescence() {
        let context = gtk::glib::MainContext::new();
        let interaction = Rc::new(RouteArtworkInteraction::default());
        let refresh_count = Rc::new(Cell::new(0));
        let mut settle_source = None;

        for _ in 0..3 {
            let refresh_count = Rc::clone(&refresh_count);
            defer_route_artwork_settle(&context, &interaction, Duration::ZERO, move || {
                refresh_count.set(refresh_count.get() + 1);
            });
            let current_source = interaction
                .settle
                .borrow()
                .as_ref()
                .and_then(gtk::glib::JoinHandle::as_raw_source_id);
            if let Some(settle_source) = settle_source {
                assert_eq!(current_source, Some(settle_source));
            } else {
                settle_source = current_source;
            }
        }

        assert!(interaction.active.get());
        for _ in 0..8 {
            context.iteration(false);
            if !interaction.active.get() {
                break;
            }
        }

        assert!(!interaction.active.get());
        assert!(interaction.settle.borrow().is_none());
        assert_eq!(refresh_count.get(), 1);
    }

    #[test]
    fn replacing_route_adjustment_disconnects_the_previous_route_callback() {
        let interaction = RouteArtworkInteraction::default();
        let previous = gio::SimpleAction::new("previous", None);
        let current = gio::SimpleAction::new("current", None);
        let previous_calls = Rc::new(Cell::new(0));
        let current_calls = Rc::new(Cell::new(0));

        let calls = Rc::clone(&previous_calls);
        replace_route_artwork_signal_handler(&interaction, &previous, || {
            previous.connect_activate(move |_, _| calls.set(calls.get() + 1))
        });
        let calls = Rc::clone(&current_calls);
        replace_route_artwork_signal_handler(&interaction, &current, || {
            current.connect_activate(move |_, _| calls.set(calls.get() + 1))
        });

        previous.activate(None);
        current.activate(None);

        assert_eq!(previous_calls.get(), 0);
        assert_eq!(current_calls.get(), 1);

        disconnect_route_artwork_adjustment_handler(&interaction);
        current.activate(None);
        assert_eq!(current_calls.get(), 1);
    }
}

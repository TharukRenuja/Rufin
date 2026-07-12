use super::*;

mod tiles;

#[derive(Clone)]
pub(in crate::ui) struct CoverBinding {
    pub(in crate::ui) tile: ArtworkTileWeak,
    pub(in crate::ui) generation: u64,
}

pub(in crate::ui::root) struct CoverWorkStats {
    pub(in crate::ui::root) prime_pending: usize,
    pub(in crate::ui::root) requests: usize,
    pub(in crate::ui::root) bindings: usize,
}

enum ArtworkBindingResult {
    Ready(Pixbuf),
    Missing,
    Failed,
}

impl Shell {
    pub(in crate::ui::root) fn cover_work_stats(&self) -> CoverWorkStats {
        let bindings = self.state.cover_bindings.borrow();
        CoverWorkStats {
            prime_pending: self.state.startup_cover_prime_pending.borrow().len(),
            requests: bindings.len(),
            bindings: bindings.values().map(Vec::len).sum(),
        }
    }

    pub(in crate::ui) fn bind_artwork_tile(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        candidates: CandidateSet,
        seed: u32,
        render_size: i32,
        fetch_size: u32,
    ) {
        self.bind_artwork_tile_for_source(tile, None, candidates, seed, render_size, fetch_size);
    }

    pub(in crate::ui) fn bind_playback_artwork_tile(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        source_id: &::library::SourceId,
        candidates: CandidateSet,
        seed: u32,
        render_size: i32,
        fetch_size: u32,
    ) {
        self.bind_artwork_tile_for_source(
            tile,
            Some(source_id),
            candidates,
            seed,
            render_size,
            fetch_size,
        );
    }

    fn bind_artwork_tile_for_source(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        source_id: Option<&::library::SourceId>,
        candidates: CandidateSet,
        seed: u32,
        render_size: i32,
        fetch_size: u32,
    ) {
        if candidates.is_empty() {
            self.cancel_artwork_tile_request(tile);
            tile.bind_image(seed, None);
            return;
        }

        let render_size = cover_decode_size(render_size, fetch_size).max(1);
        let settings = self.state.settings.borrow().clone();
        let prepared = match source_id {
            Some(source_id) => self.controller.prepare_playback_artwork(
                source_id,
                candidates,
                fetch_size,
                render_size as u32,
                &settings,
            ),
            None => match self.controller.prepare_artwork(
                candidates,
                fetch_size,
                render_size as u32,
                &settings,
            ) {
                Ok(prepared) => prepared,
                Err(error) => {
                    warn!(%error, "failed to identify artwork request");
                    self.cancel_artwork_tile_request(tile);
                    tile.bind_image(seed, None);
                    return;
                }
            },
        };
        let outcome = tile.bind_selected_cover(
            seed,
            prepared.identity.visual.clone(),
            prepared.identity.request.clone(),
        );
        if !outcome.request_needed {
            return;
        }
        if !outcome.request_changed && tile.artwork_request_id().is_some() {
            return;
        }
        self.cancel_artwork_tile_request(tile);

        match self.controller.request_artwork(prepared) {
            Ok(projection) => match projection.readiness {
                artwork::Readiness::Pending => {
                    let request_id = projection.request_id;
                    if let Some(previous) = tile.replace_artwork_request_id(request_id) {
                        self.controller.cancel_artwork(previous);
                        self.state.cover_bindings.borrow_mut().remove(&previous);
                    }
                    self.register_cover_binding(request_id, tile, outcome.generation);
                }
                artwork::Readiness::Ready(image) => {
                    if let Some(pixbuf) = pixbuf_from_decoded(&image) {
                        tile.set_pixbuf_if_current(outcome.generation, pixbuf);
                    } else {
                        tile.clear_image_if_current(outcome.generation);
                    }
                }
                artwork::Readiness::Missing => {
                    tile.set_missing_if_current(outcome.generation);
                }
                artwork::Readiness::Failed(_) => {
                    tile.clear_image_if_current(outcome.generation);
                }
            },
            Err(error) => {
                warn!(%error, "failed to start artwork request");
                tile.clear_image_if_current(outcome.generation);
            }
        }
    }

    pub(in crate::ui) fn clear_artwork_tile(&self, tile: &ArtworkTile) {
        self.cancel_artwork_tile_request(tile);
        tile.clear_image();
    }

    fn cancel_artwork_tile_request(&self, tile: &ArtworkTile) {
        let Some(request_id) = tile.artwork_request_id() else {
            return;
        };
        tile.clear_artwork_request_id(request_id);
        self.state.cover_bindings.borrow_mut().remove(&request_id);
        self.state
            .startup_cover_prime_pending
            .borrow_mut()
            .remove(&request_id);
        self.controller.cancel_artwork(request_id);
    }

    pub(in crate::ui) fn current_playback_cached_artwork_path(
        &self,
        source_id: &::library::SourceId,
        entry: &playback::SequenceEntry,
        preferred_size: u32,
    ) -> Option<PlaybackArtworkPath> {
        let candidates = CandidateSet::track(&entry.track);
        let settings = self.state.settings.borrow().clone();
        let path = self.controller.cached_artwork_path(
            source_id,
            candidates,
            preferred_size,
            preferred_size,
            &settings,
        )?;
        Some(PlaybackArtworkPath { path })
    }

    fn register_cover_binding(
        &self,
        request_id: artwork::RequestId,
        tile: &ArtworkTile,
        generation: u64,
    ) {
        let mut bindings = self.state.cover_bindings.borrow_mut();
        bindings.entry(request_id).or_default().push(CoverBinding {
            tile: tile.downgrade(),
            generation,
        });
    }

    pub(in crate::ui) fn apply_artwork_event(self: &Rc<Self>, event: artwork::ArtworkEvent) {
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
                if let Some(pixbuf) = pixbuf_from_decoded(&image) {
                    self.finish_artwork_request(
                        projection.request_id,
                        ArtworkBindingResult::Ready(pixbuf),
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

    fn finish_artwork_request(&self, request_id: artwork::RequestId, result: ArtworkBindingResult) {
        self.state
            .startup_cover_prime_pending
            .borrow_mut()
            .remove(&request_id);
        let bindings = self
            .state
            .cover_bindings
            .borrow_mut()
            .remove(&request_id)
            .unwrap_or_default();
        for binding in bindings {
            let Some(tile) = binding.tile.upgrade() else {
                continue;
            };
            tile.clear_artwork_request_id(request_id);
            match &result {
                ArtworkBindingResult::Ready(pixbuf) => {
                    tile.set_pixbuf_if_current(binding.generation, pixbuf.clone());
                }
                ArtworkBindingResult::Missing => {
                    tile.set_missing_if_current(binding.generation);
                }
                ArtworkBindingResult::Failed => {
                    tile.clear_image_if_current(binding.generation);
                }
            }
        }
    }

    pub(in crate::ui) fn reset_cover_pipeline_state(&self) {
        let bindings = std::mem::take(&mut *self.state.cover_bindings.borrow_mut());
        for (request_id, request_bindings) in bindings {
            for binding in request_bindings {
                if let Some(tile) = binding.tile.upgrade() {
                    tile.clear_artwork_request_id(request_id);
                }
            }
            self.controller.cancel_artwork(request_id);
        }
        self.finish_startup_cover_prime_gate();
    }

    pub(in crate::ui) fn reset_route_covers(&self) {
        self.reconcile_artwork_requests();
    }

    pub(in crate::ui::root) fn begin_startup_cover_prime(&self) -> u64 {
        let generation = self
            .state
            .startup_cover_prime_generation
            .get()
            .wrapping_add(1);
        self.state.startup_cover_prime_generation.set(generation);
        *self.state.startup_cover_prime_pending.borrow_mut() = self.pending_artwork_requests();
        generation
    }

    pub(in crate::ui::root) fn startup_cover_prime_pending_count(
        &self,
        generation: Option<u64>,
    ) -> usize {
        if generation == Some(self.state.startup_cover_prime_generation.get()) {
            self.state.startup_cover_prime_pending.borrow().len()
        } else {
            0
        }
    }

    pub(in crate::ui::root) fn reconcile_startup_cover_prime_pending(&self) {
        self.reconcile_artwork_requests();
    }

    pub(in crate::ui::root) fn finish_startup_cover_prime_gate(&self) {
        self.state.startup_cover_prime_generation.set(
            self.state
                .startup_cover_prime_generation
                .get()
                .wrapping_add(1),
        );
        self.state.startup_cover_prime_pending.borrow_mut().clear();
    }

    fn pending_artwork_requests(&self) -> HashSet<artwork::RequestId> {
        self.reconcile_artwork_requests();
        self.state.cover_bindings.borrow().keys().copied().collect()
    }

    fn reconcile_artwork_requests(&self) {
        let stale = {
            let mut bindings = self.state.cover_bindings.borrow_mut();
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
            self.controller.cancel_artwork(request_id);
            self.state
                .startup_cover_prime_pending
                .borrow_mut()
                .remove(&request_id);
        }
    }
}

fn pixbuf_from_decoded(image: &artwork::DecodedImage) -> Option<Pixbuf> {
    let width = i32::try_from(image.width()).ok()?;
    let height = i32::try_from(image.height()).ok()?;
    let row_stride = i32::try_from(image.row_stride()).ok()?;
    let bytes = glib::Bytes::from_owned(image.rgba().to_vec());
    Some(Pixbuf::from_bytes(
        &bytes,
        Colorspace::Rgb,
        true,
        8,
        width,
        height,
        row_stride,
    ))
}

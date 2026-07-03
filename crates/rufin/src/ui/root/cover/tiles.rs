use super::*;

struct CoverTileRequest {
    tile: ArtworkTile,
    key: String,
    image_ref: ImageRef,
    generation: u64,
    size: i32,
    fetch_size: u32,
}

impl Shell {
    pub(in crate::ui) fn cover_tile_for(
        self: &Rc<Self>,
        image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        self.cover_tile_for_dimensions(image_ref, seed, size, size, fetch_size)
    }
    pub(in crate::ui) fn cover_tile_for_dimensions(
        self: &Rc<Self>,
        image_ref: Option<&ImageRef>,
        seed: u32,
        width: i32,
        height: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let tile = ArtworkTile::new_sized(width, height, seed);
        let widget = tile.widget();
        self.bind_cover_tile_for_dimensions(&tile, image_ref, seed, width.max(height), fetch_size);
        widget
    }

    pub(in crate::ui) fn cover_collection_tile_for(
        self: &Rc<Self>,
        image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let tile = ArtworkTile::new_sized(size, size, seed);
        let widget = tile.widget();
        self.bind_cover_tile_for_dimensions(
            &tile,
            image_ref,
            seed,
            collection_cover_decode_extent(fetch_size, size),
            fetch_size,
        );
        widget
    }

    pub(in crate::ui) fn bind_cover_tile_for(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) {
        self.bind_cover_tile_for_dimensions(tile, image_ref, seed, size, fetch_size);
    }

    pub(in crate::ui) fn bind_cover_tile_for_dimensions(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        image_ref: Option<&ImageRef>,
        seed: u32,
        decode_extent: i32,
        fetch_size: u32,
    ) {
        let decode_size = cover_decode_size(decode_extent, fetch_size);

        let Some(image_ref) = image_ref else {
            tile.bind_image(seed, None);
            return;
        };
        let Some(key) = self.cover_cache_key(image_ref, fetch_size) else {
            tile.bind_image(seed, None);
            return;
        };

        if let Some((cache_key, pixbuf)) =
            self.decoded_cover_for_ref(image_ref, fetch_size, decode_size)
        {
            self.touch_visible_decoded_cover(&cache_key);
            tile.bind_selected_cover(
                seed,
                cover_artwork_id_for_key(&key, image_ref),
                cover_request_id_for_key(&key, decode_size),
                Some(pixbuf),
            );
            return;
        }

        let outcome = tile.bind_selected_cover(
            seed,
            cover_artwork_id_for_key(&key, image_ref),
            cover_request_id_for_key(&key, decode_size),
            None,
        );
        if !outcome.request_needed {
            return;
        }
        let generation = outcome.generation;
        self.register_cover_bindings_for_ref(
            &key,
            image_ref,
            fetch_size,
            decode_size,
            tile,
            generation,
        );
        let shell = Rc::clone(self);
        let image_ref = image_ref.clone();
        let key_for_request = key.clone();
        if tile.area.is_mapped() {
            shell.schedule_cover_request_for_tile(
                tile.clone(),
                key_for_request,
                image_ref,
                generation,
                decode_size,
                fetch_size,
            );
        } else {
            let started = Rc::new(Cell::new(false));
            let tile_for_map = tile.downgrade();
            tile.area.connect_map(move |_| {
                if started.replace(true) {
                    return;
                }
                let Some(tile_for_request) = tile_for_map.upgrade() else {
                    return;
                };
                if !tile_for_request.is_live_generation(generation) {
                    return;
                }
                shell.schedule_cover_request_for_tile(
                    tile_for_request,
                    key_for_request.clone(),
                    image_ref.clone(),
                    generation,
                    decode_size,
                    fetch_size,
                );
            });
        };
    }
    pub(in crate::ui) fn prime_cached_cover(
        self: &Rc<Self>,
        image_ref: Option<&ImageRef>,
        fetch_size: u32,
        size: i32,
    ) {
        let Some(image_ref) = image_ref else {
            return;
        };
        let decode_size = cover_decode_size(size, fetch_size);
        if self
            .decoded_cover_for_ref(image_ref, fetch_size, decode_size)
            .is_some()
        {
            return;
        }
        let Some(key) = self.cover_cache_key(image_ref, fetch_size) else {
            return;
        };
        if self.decoded_cover_has_min_size(&key, decode_size) {
            return;
        }
        if !self.decoded_cover_has_warm_capacity(decode_size) {
            return;
        }
        self.start_cached_cover_path_lookup(CoverPathLookupRequest {
            key,
            image_ref: image_ref.clone(),
            fetch_size,
            size: decode_size,
            intent: CoverPathLookupIntent::Warm,
        });
    }
    pub(in crate::ui) fn cover_group_tile_for_artwork(
        self: &Rc<Self>,
        artwork: &crate::cover_art_policy::SelectedArtwork,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        match artwork.selection {
            crate::cover_art_policy::ArtworkSelection::ImageRefs => {
                self.cover_group_tile_for_refs(artwork.image_refs.clone(), seed, size, fetch_size)
            }
            crate::cover_art_policy::ArtworkSelection::FinalMissing => {
                self.cover_collection_tile_for(None, seed, size, fetch_size)
            }
        }
    }

    fn cover_group_tile_for_refs(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let image_refs = crate::cover_art_policy::selected_collection_slots(&image_refs);
        match image_refs.len() {
            0 => self.cover_collection_tile_for(None, seed, size, fetch_size),
            1 => self.cover_collection_tile_for(image_refs.first(), seed, size, fetch_size),
            _ => {
                let grid = gtk::Grid::new();
                grid.add_css_class("cover-tile");
                grid.add_css_class("card");
                grid.set_size_request(size, size);
                grid.set_width_request(size);
                grid.set_height_request(size);
                grid.set_overflow(gtk::Overflow::Hidden);
                grid.set_row_homogeneous(true);
                grid.set_column_homogeneous(true);
                grid.set_hexpand(false);
                grid.set_vexpand(false);
                grid.set_halign(gtk::Align::Start);
                grid.set_valign(gtk::Align::Start);

                let cell_size = (size / 2).max(1);
                for index in 0..4 {
                    let child = self.cover_tile_for(
                        image_refs.get(index),
                        seed.wrapping_add((index as u32).wrapping_mul(0x9e37_79b9)),
                        cell_size,
                        fetch_size,
                    );
                    grid.attach(&child, (index % 2) as i32, (index / 2) as i32, 1, 1);
                }
                grid.upcast()
            }
        }
    }
    fn schedule_cover_request_for_tile(
        self: &Rc<Self>,
        tile: ArtworkTile,
        key: String,
        image_ref: ImageRef,
        generation: u64,
        size: i32,
        fetch_size: u32,
    ) {
        self.schedule_cover_request_for_tile_after(
            CoverTileRequest {
                tile,
                key,
                image_ref,
                generation,
                size,
                fetch_size,
            },
            Duration::from_millis(COVER_VISIBLE_REQUEST_DELAY_MS),
        );
    }

    fn schedule_cover_request_for_tile_after(
        self: &Rc<Self>,
        request: CoverTileRequest,
        delay: Duration,
    ) {
        let shell = Rc::clone(self);
        glib::timeout_add_local_once(delay, move || {
            let CoverTileRequest {
                tile,
                key,
                image_ref,
                generation,
                size,
                fetch_size,
            } = request;
            if !tile.is_live_generation(generation) || !tile.area.is_mapped() {
                shell.prune_cover_bindings_for_ref(&key, &image_ref, fetch_size);
                return;
            }
            if let Some(remaining) = shell.cover_visible_pause_remaining() {
                shell.schedule_cover_request_for_tile_after(
                    CoverTileRequest {
                        tile,
                        key,
                        image_ref,
                        generation,
                        size,
                        fetch_size,
                    },
                    remaining + Duration::from_millis(COVER_VISIBLE_REQUEST_DELAY_MS),
                );
                return;
            }
            shell.request_cover_for_tile(&tile, key, image_ref, size, fetch_size);
        });
    }

    pub(in crate::ui) fn request_bound_cover_for_tile(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        key: String,
        image_ref: ImageRef,
        generation: u64,
        size: i32,
        fetch_size: u32,
    ) {
        let decode_size = cover_decode_size(size, fetch_size);
        if let Some((cache_key, pixbuf)) =
            self.decoded_cover_for_ref(&image_ref, fetch_size, decode_size)
        {
            self.touch_visible_decoded_cover(&cache_key);
            tile.set_pixbuf_if_current(generation, pixbuf);
            return;
        }
        self.register_cover_bindings_for_ref(
            &key,
            &image_ref,
            fetch_size,
            decode_size,
            tile,
            generation,
        );
        self.start_cached_cover_path_lookup(CoverPathLookupRequest {
            key,
            image_ref,
            fetch_size,
            size: decode_size,
            intent: CoverPathLookupIntent::Visible,
        });
    }

    pub(in crate::ui) fn request_cover_for_tile(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        key: String,
        image_ref: ImageRef,
        size: i32,
        fetch_size: u32,
    ) {
        let decode_size = cover_decode_size(size, fetch_size);
        if let Some((cache_key, pixbuf)) =
            self.decoded_cover_for_ref(&image_ref, fetch_size, decode_size)
        {
            self.touch_visible_decoded_cover(&cache_key);
            tile.set_pixbuf_if_current(tile.generation(), pixbuf);
            return;
        }
        self.start_cached_cover_path_lookup(CoverPathLookupRequest {
            key,
            image_ref,
            fetch_size,
            size: decode_size,
            intent: CoverPathLookupIntent::Visible,
        });
    }

    fn prune_cover_bindings_for_ref(
        &self,
        primary_key: &str,
        image_ref: &ImageRef,
        fetch_size: u32,
    ) {
        let mut seen = HashSet::new();
        for key in self.cover_cache_candidate_keys(image_ref, fetch_size) {
            if seen.insert(key.clone()) {
                self.cover_binding_has_live(&key);
            }
        }
        if seen.insert(primary_key.to_string()) {
            self.cover_binding_has_live(primary_key);
        }
    }

    fn register_cover_bindings_for_ref(
        &self,
        primary_key: &str,
        image_ref: &ImageRef,
        fetch_size: u32,
        decode_size: i32,
        tile: &ArtworkTile,
        generation: u64,
    ) {
        let request = CoverPathLookupRequest {
            key: primary_key.to_string(),
            image_ref: image_ref.clone(),
            fetch_size,
            size: decode_size,
            intent: CoverPathLookupIntent::StartupPrime,
        };
        let mut seen = HashSet::new();
        for key in self.cover_cache_candidate_keys(image_ref, fetch_size) {
            let clear_on_failure = key == primary_key;
            self.register_cover_binding(
                &key,
                tile,
                generation,
                clear_on_failure,
                (key == primary_key).then_some(request.clone()),
            );
            seen.insert(key);
        }
        if !seen.contains(primary_key) {
            self.register_cover_binding(primary_key, tile, generation, true, Some(request));
        }
    }

    fn register_cover_binding(
        &self,
        key: &str,
        tile: &ArtworkTile,
        generation: u64,
        clear_on_failure: bool,
        request: Option<CoverPathLookupRequest>,
    ) {
        let mut cover_bindings = self.state.cover_bindings.borrow_mut();
        let bindings = cover_bindings.entry(key.to_string()).or_default();
        bindings.retain(|binding| binding.tile.is_current_generation(binding.generation));
        bindings.push(CoverBinding {
            tile: tile.downgrade(),
            generation,
            clear_on_failure,
            request,
        });
    }
}

pub(in crate::ui) fn cover_artwork_id_for_key(key: &str, image_ref: &ImageRef) -> String {
    if cover_size_from_cache_key(key).is_some()
        && let Some((base, _)) = key.rsplit_once('/')
    {
        return base.to_string();
    }
    format!(
        "{}:{}",
        image_ref.item_id,
        image_ref.tag.as_deref().unwrap_or(IMAGE_TAG_UNTAGGED)
    )
}

pub(in crate::ui) fn cover_request_id_for_key(key: &str, min_size: i32) -> String {
    format!("{key}:{}", min_size.max(1))
}

pub(in crate::ui) fn collection_cover_decode_extent(fetch_size: u32, size: i32) -> i32 {
    if fetch_size <= THUMB_COVER_SIZE {
        THUMB_COVER_SIZE as i32
    } else {
        size
    }
}

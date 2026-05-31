use super::*;

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
        self.bind_cover_tile_for_dimensions(&tile, image_ref, seed, width, height, fetch_size);
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
        self.bind_cover_tile_for_dimensions(tile, image_ref, seed, size, size, fetch_size);
    }

    pub(in crate::ui) fn bind_cover_tile_for_dimensions(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        image_ref: Option<&ImageRef>,
        seed: u32,
        width: i32,
        height: i32,
        fetch_size: u32,
    ) {
        let decode_size = cover_decode_size(width.max(height), fetch_size);

        let Some(image_ref) = image_ref else {
            tile.bind_image(seed, None);
            self.record_perf_coverless_tile();
            return;
        };
        let Some(key) = self.cover_cache_key(image_ref, fetch_size) else {
            tile.bind_image(seed, None);
            self.record_perf_coverless_tile();
            return;
        };

        if let Some((cache_key, pixbuf)) =
            self.decoded_cover_for_ref(image_ref, fetch_size, decode_size)
        {
            self.record_perf_cover_cache_hit(&cache_key);
            self.touch_decoded_cover(&cache_key, CoverDecodePriority::Visible);
            tile.bind_cover_image(seed, Some(pixbuf));
            return;
        }

        let generation = tile.bind_cover_image(seed, None);
        self.register_cover_bindings_for_ref(&key, image_ref, fetch_size, tile, generation);
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
    pub(in crate::ui) fn prime_cover_ref_from_cache_now(
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
    pub(in crate::ui) fn cover_group_tile_for(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        fallback_image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let image_refs = cover_group_slots(&image_refs);
        match image_refs.len() {
            0 => self.cover_tile_for(fallback_image_ref, seed, size, fetch_size),
            1 => self.cover_tile_for(image_refs.first(), seed, size, fetch_size),
            _ => {
                let grid = gtk::Grid::new();
                grid.add_css_class("cover-tile");
                grid.add_css_class("card");
                grid.set_size_request(size, size);
                grid.set_width_request(size);
                grid.set_height_request(size);
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
        let shell = Rc::clone(self);
        glib::timeout_add_local_once(
            Duration::from_millis(COVER_VISIBLE_REQUEST_DELAY_MS),
            move || {
                if !tile.is_live_generation(generation) || !tile.area.is_mapped() {
                    shell.record_perf_cover_stale_key(&key);
                    return;
                }
                shell.request_cover_for_tile(&tile, key, image_ref, size, fetch_size);
            },
        );
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
            self.record_perf_cover_cache_hit(&cache_key);
            self.touch_decoded_cover(&cache_key, CoverDecodePriority::Visible);
            tile.set_pixbuf_if_current(tile.generation(), pixbuf);
            return;
        }

        self.record_perf_cover_bind_request(&key);
        let generation = tile.generation();
        self.register_cover_bindings_for_ref(&key, &image_ref, fetch_size, tile, generation);
        self.start_cached_cover_path_lookup(CoverPathLookupRequest {
            key,
            image_ref,
            fetch_size,
            size: decode_size,
            intent: CoverPathLookupIntent::Visible,
        });
    }

    fn register_cover_bindings_for_ref(
        &self,
        primary_key: &str,
        image_ref: &ImageRef,
        fetch_size: u32,
        tile: &ArtworkTile,
        generation: u64,
    ) {
        let mut seen = HashSet::new();
        for key in self.cover_cache_candidate_keys(image_ref, fetch_size) {
            let clear_on_failure = key == primary_key;
            self.register_cover_binding(&key, tile, generation, clear_on_failure);
            seen.insert(key);
        }
        if !seen.contains(primary_key) {
            self.register_cover_binding(primary_key, tile, generation, true);
        }
    }

    fn register_cover_binding(
        &self,
        key: &str,
        tile: &ArtworkTile,
        generation: u64,
        clear_on_failure: bool,
    ) {
        let mut cover_bindings = self.state.cover_bindings.borrow_mut();
        let bindings = cover_bindings.entry(key.to_string()).or_default();
        bindings.retain(|binding| binding.tile.is_current_generation(binding.generation));
        bindings.push(CoverBinding {
            tile: tile.downgrade(),
            generation,
            clear_on_failure,
        });
    }
}

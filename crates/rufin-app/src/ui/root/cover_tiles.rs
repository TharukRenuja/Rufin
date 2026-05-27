impl Shell {
    fn cover_tile_for(
        self: &Rc<Self>,
        image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        self.cover_tile_for_dimensions(image_ref, seed, size, size, fetch_size)
    }
    fn cover_tile_for_dimensions(
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

    fn bind_cover_tile_for(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) {
        self.bind_cover_tile_for_dimensions(tile, image_ref, seed, size, size, fetch_size);
    }

    fn bind_cover_tile_for_dimensions(
        self: &Rc<Self>,
        tile: &ArtworkTile,
        image_ref: Option<&ImageRef>,
        seed: u32,
        width: i32,
        height: i32,
        fetch_size: u32,
    ) {
        let decode_size = cover_decode_size(width.max(height), fetch_size);

        if let Some(image_ref) = image_ref
            && let Some(key) = self.cover_cache_key(image_ref, fetch_size)
        {
            if let Some((cache_key, pixbuf)) =
                self.decoded_cover_for_ref(image_ref, fetch_size, decode_size)
            {
                self.record_perf_cover_cache_hit(&cache_key);
                self.touch_decoded_cover(&cache_key, CoverDecodePriority::Visible);
                tile.bind_image(seed, Some(pixbuf));
            } else {
                let generation = tile.bind_image(seed, None);
                let shell = Rc::clone(self);
                let image_ref = image_ref.clone();
                let key_for_request = key.clone();
                if tile.area.is_mapped() {
                    let tile_for_request = tile.clone();
                    glib::idle_add_local_once(move || {
                        if !tile_for_request.is_live_generation(generation) {
                            return;
                        }
                        shell.request_cover_for_tile(
                            &tile_for_request,
                            key_for_request,
                            image_ref,
                            decode_size,
                            fetch_size,
                        );
                    });
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
                        shell.request_cover_for_tile(
                            &tile_for_request,
                            key_for_request.clone(),
                            image_ref.clone(),
                            decode_size,
                            fetch_size,
                        );
                    });
                }
            }
        } else {
            tile.bind_image(seed, None);
            self.record_perf_coverless_tile();
        }
    }
    fn prime_cover_ref_from_cache_now(
        &self,
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
        let Some((key, path)) = self.cached_cover_path_for_startup_prime(image_ref, fetch_size)
        else {
            return;
        };
        if self.decoded_cover_has_min_size(&key, decode_size) {
            return;
        }
        if !self.decoded_cover_has_warm_capacity(decode_size) {
            return;
        }
        match Pixbuf::from_file_at_scale(&path, decode_size, decode_size, true) {
            Ok(pixbuf) => {
                self.remember_decoded_cover(key, pixbuf, CoverDecodePriority::Warm);
            }
            Err(error) => {
                debug!(%error, path = %path.display(), "failed to prime cached cover");
            }
        }
    }
    fn cover_group_tile_for(
        self: &Rc<Self>,
        image_refs: Vec<ImageRef>,
        fallback_image_ref: Option<&ImageRef>,
        seed: u32,
        size: i32,
        fetch_size: u32,
    ) -> gtk::Widget {
        let image_refs = unique_cover_refs(image_refs);
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
                if image_refs.len() == 3 {
                    let tall = self.cover_tile_for_dimensions(
                        image_refs.first(),
                        seed,
                        cell_size,
                        size,
                        fetch_size,
                    );
                    let top = self.cover_tile_for(
                        image_refs.get(1),
                        seed.wrapping_add(0x9e37_79b9),
                        cell_size,
                        fetch_size,
                    );
                    let bottom = self.cover_tile_for(
                        image_refs.get(2),
                        seed.wrapping_add(0x3c6e_f372),
                        cell_size,
                        fetch_size,
                    );
                    grid.attach(&tall, 0, 0, 1, 2);
                    grid.attach(&top, 1, 0, 1, 1);
                    grid.attach(&bottom, 1, 1, 1, 1);
                } else {
                    for index in 0..4 {
                        let image_ref = image_refs.get(index % image_refs.len());
                        let child = self.cover_tile_for(
                            image_ref,
                            seed.wrapping_add((index as u32).wrapping_mul(0x9e37_79b9)),
                            cell_size,
                            fetch_size,
                        );
                        grid.attach(&child, (index % 2) as i32, (index / 2) as i32, 1, 1);
                    }
                }
                grid.upcast()
            }
        }
    }
    fn request_cover_for_tile(
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
        {
            let mut cover_bindings = self.state.cover_bindings.borrow_mut();
            let bindings = cover_bindings.entry(key.clone()).or_default();
            bindings.retain(|binding| binding.tile.is_live_generation(binding.generation));
            bindings.push(CoverBinding {
                tile: tile.downgrade(),
                generation,
            });
        }
        let shell = Rc::clone(self);
        let controller = self.controller.clone();
        let candidate_keys = self.cover_cache_candidate_keys(&image_ref, fetch_size);
        glib::spawn_future_local(async move {
            let path = gtk::gio::spawn_blocking(move || {
                candidate_keys
                    .iter()
                    .find_map(|key| controller.cached_cover_path_for_key(key))
            })
            .await
            .ok()
            .flatten();
            if let Some(path) = path {
                if !shell.cover_binding_has_live(&key) {
                    shell.record_perf_cover_stale_key(&key);
                    return;
                }
                shell.record_perf_cover_path_ready(&key);
                shell.record_perf_cover_ready(&key);
                shell.start_cover_decode_from_path(
                    key,
                    path,
                    decode_size,
                    CoverDecodePriority::Visible,
                );
            } else {
                shell.state.cover_bindings.borrow_mut().remove(&key);
                shell.record_perf_cover_stale_key(&key);
                shell.record_perf_coverless_tile();
            }
        });
    }
}

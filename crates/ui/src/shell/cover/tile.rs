use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

#[derive(Clone)]
pub(crate) struct ArtworkTile {
    pub(crate) area: gtk::Overlay,
    fallback: gtk::Picture,
    image: gtk::Picture,
    size: Rc<Cell<i32>>,
    seed: Rc<Cell<u32>>,
    known_missing: Rc<Cell<bool>>,
    artwork_id: Rc<RefCell<Option<artwork::ArtworkVisualIdentity>>>,
    request_key: Rc<RefCell<Option<artwork::ArtworkRequestIdentity>>>,
    artwork_request: Rc<RefCell<Option<glib::JoinHandle<()>>>>,
    generation: Rc<Cell<u64>>,
    binding_active: Rc<Cell<bool>>,
    cleanup_hook_installed: Rc<Cell<bool>>,
}

#[derive(Clone)]
pub(crate) struct ArtworkTileWeak {
    area: glib::WeakRef<gtk::Overlay>,
    fallback: glib::WeakRef<gtk::Picture>,
    image: glib::WeakRef<gtk::Picture>,
    size: Rc<Cell<i32>>,
    seed: Rc<Cell<u32>>,
    known_missing: Rc<Cell<bool>>,
    artwork_id: Rc<RefCell<Option<artwork::ArtworkVisualIdentity>>>,
    request_key: Rc<RefCell<Option<artwork::ArtworkRequestIdentity>>>,
    artwork_request: Rc<RefCell<Option<glib::JoinHandle<()>>>>,
    generation: Rc<Cell<u64>>,
    binding_active: Rc<Cell<bool>>,
    cleanup_hook_installed: Rc<Cell<bool>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ArtworkBindOutcome {
    pub(super) generation: u64,
    pub(super) request_needed: bool,
    pub(super) request_changed: bool,
}

impl ArtworkTile {
    pub(crate) fn new(size: i32, seed: u32) -> Self {
        Self::new_sized(size, size, seed)
    }

    pub(crate) fn new_elastic_square(seed: u32) -> Self {
        let tile = Self::new_sized(1, 1, seed);
        tile.area.set_hexpand(true);
        tile.area.set_vexpand(true);
        tile.area.set_halign(gtk::Align::Fill);
        tile.area.set_valign(gtk::Align::Fill);
        tile
    }

    pub(crate) fn new_sized(width: i32, height: i32, seed: u32) -> Self {
        let area = gtk::Overlay::new();
        area.add_css_class("cover-tile");
        area.add_css_class("card");
        area.set_width_request(width);
        area.set_height_request(height);
        area.set_size_request(width, height);
        area.set_hexpand(false);
        area.set_vexpand(false);
        area.set_halign(gtk::Align::Start);
        area.set_valign(gtk::Align::Start);
        area.set_overflow(gtk::Overflow::Hidden);

        let sizing = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        sizing.set_can_target(false);
        sizing.set_accessible_role(gtk::AccessibleRole::Presentation);
        area.set_child(Some(&sizing));

        let fallback = cover_picture(gtk::ContentFit::Fill);
        fallback.set_visible(false);
        area.add_overlay(&fallback);
        area.set_measure_overlay(&fallback, false);
        area.set_clip_overlay(&fallback, true);

        let image = cover_picture(gtk::ContentFit::Cover);
        image.set_visible(false);
        area.add_overlay(&image);
        area.set_measure_overlay(&image, false);
        area.set_clip_overlay(&image, true);
        area.set_opacity(0.0);

        let seed = Rc::new(Cell::new(seed));
        let size = Rc::new(Cell::new(width.max(height)));
        let known_missing = Rc::new(Cell::new(false));
        let artwork_id = Rc::new(RefCell::new(None::<artwork::ArtworkVisualIdentity>));
        let request_key = Rc::new(RefCell::new(None::<artwork::ArtworkRequestIdentity>));
        let artwork_request = Rc::new(RefCell::new(None));
        let generation = Rc::new(Cell::new(0));
        let binding_active = Rc::new(Cell::new(false));
        let cleanup_hook_installed = Rc::new(Cell::new(false));

        Self {
            area,
            fallback,
            image,
            size,
            seed,
            known_missing,
            artwork_id,
            request_key,
            artwork_request,
            generation,
            binding_active,
            cleanup_hook_installed,
        }
    }

    pub(crate) fn widget(&self) -> gtk::Widget {
        self.area.clone().upcast()
    }

    pub(super) fn identity(&self) -> usize {
        self.area.as_ptr() as usize
    }

    pub(crate) fn downgrade(&self) -> ArtworkTileWeak {
        ArtworkTileWeak {
            area: self.area.downgrade(),
            fallback: self.fallback.downgrade(),
            image: self.image.downgrade(),
            size: Rc::clone(&self.size),
            seed: Rc::clone(&self.seed),
            known_missing: Rc::clone(&self.known_missing),
            artwork_id: Rc::clone(&self.artwork_id),
            request_key: Rc::clone(&self.request_key),
            artwork_request: Rc::clone(&self.artwork_request),
            generation: Rc::clone(&self.generation),
            binding_active: Rc::clone(&self.binding_active),
            cleanup_hook_installed: Rc::clone(&self.cleanup_hook_installed),
        }
    }

    pub(super) fn install_cleanup_hook_once<F>(&self, cleanup: F)
    where
        F: FnOnce(usize) + 'static,
    {
        if self.cleanup_hook_installed.replace(true) {
            return;
        }

        let identity = self.identity();
        let artwork_request = Rc::clone(&self.artwork_request);
        let binding_active = Rc::clone(&self.binding_active);
        let cleanup = RefCell::new(Some(cleanup));
        self.area.connect_destroy(move |_| {
            binding_active.set(false);
            if let Some(request) = artwork_request.borrow_mut().take() {
                request.abort();
            }
            if let Some(cleanup) = cleanup.borrow_mut().take() {
                cleanup(identity);
            }
        });
    }

    fn advance_generation(&self) {
        self.generation.set(self.generation.get().saturating_add(1));
    }

    pub(super) fn bind_selected_cover(
        &self,
        seed: u32,
        artwork_id: artwork::ArtworkVisualIdentity,
        request_key: artwork::ArtworkRequestIdentity,
    ) -> ArtworkBindOutcome {
        let same_artwork = self.artwork_id.borrow().as_ref() == Some(&artwork_id);
        let same_request = self.request_key.borrow().as_ref() == Some(&request_key);
        let has_texture = self.image.paintable().is_some();
        let terminal_missing =
            same_artwork && same_request && !has_texture && self.known_missing.get();

        let request_changed = !same_artwork || !same_request;
        if request_changed {
            self.advance_generation();
            *self.artwork_id.borrow_mut() = Some(artwork_id);
            *self.request_key.borrow_mut() = Some(request_key);
        }

        self.update_seed(seed);
        if !same_artwork {
            self.image.set_paintable(Option::<&gtk::gdk::Texture>::None);
        }
        self.known_missing.set(terminal_missing);

        let has_texture = self.image.paintable().is_some();
        self.sync_presentation(has_texture, terminal_missing);
        self.area.queue_draw();

        ArtworkBindOutcome {
            generation: self.generation.get(),
            request_needed: request_changed || (!has_texture && !terminal_missing),
            request_changed,
        }
    }

    pub(super) fn has_artwork_request(&self) -> bool {
        self.artwork_request.borrow().is_some()
    }

    pub(super) fn replace_artwork_request(&self, request: glib::JoinHandle<()>) {
        self.cancel_artwork_request();
        self.artwork_request.replace(Some(request));
    }

    pub(super) fn cancel_artwork_request(&self) {
        if let Some(request) = self.artwork_request.borrow_mut().take() {
            request.abort();
        }
    }

    pub(crate) fn set_seed(&self, seed: u32) {
        self.update_seed(seed);
    }

    pub(crate) fn set_square_size(&self, size: i32) {
        let size = size.max(1);
        if self.size.replace(size) == size {
            return;
        }
        self.area.set_width_request(size);
        self.area.set_height_request(size);
        self.area.set_size_request(size, size);
        self.area.queue_resize();
    }

    pub(super) fn bind_pending(&self, seed: u32) -> u64 {
        self.binding_active.set(false);
        self.bind_image_state(seed, None, false)
    }

    pub(super) fn bind_missing(&self, seed: u32) -> u64 {
        self.binding_active.set(false);
        self.bind_image_state(seed, None, true)
    }

    pub(super) fn mark_artwork_bound(&self) {
        self.binding_active.set(true);
    }

    fn bind_image_state(
        &self,
        seed: u32,
        texture: Option<gtk::gdk::Texture>,
        known_missing: bool,
    ) -> u64 {
        let generation = self.generation.get().saturating_add(1);
        self.generation.set(generation);
        self.update_seed(seed);
        let has_texture = texture.is_some();
        self.image.set_paintable(texture.as_ref());
        self.known_missing.set(known_missing);
        *self.artwork_id.borrow_mut() = None;
        *self.request_key.borrow_mut() = None;
        self.sync_presentation(has_texture, known_missing);
        generation
    }

    pub(super) fn set_texture_if_current(
        &self,
        generation: u64,
        texture: gtk::gdk::Texture,
    ) -> bool {
        if self.generation.get() != generation {
            return false;
        }
        self.image.set_paintable(Some(&texture));
        self.known_missing.set(false);
        self.sync_presentation(true, false);
        true
    }

    pub(super) fn clear_image(&self) {
        self.binding_active.set(false);
        self.advance_generation();
        self.image.set_paintable(Option::<&gtk::gdk::Texture>::None);
        self.known_missing.set(false);
        *self.artwork_id.borrow_mut() = None;
        *self.request_key.borrow_mut() = None;
        self.sync_presentation(false, false);
    }

    pub(super) fn set_blank_if_current(&self, generation: u64) -> bool {
        if self.generation.get() != generation {
            return false;
        }
        self.generation.set(self.generation.get().saturating_add(1));
        self.image.set_paintable(Option::<&gtk::gdk::Texture>::None);
        self.known_missing.set(false);
        *self.artwork_id.borrow_mut() = None;
        *self.request_key.borrow_mut() = None;
        self.sync_presentation(false, false);
        true
    }

    pub(super) fn set_missing_if_current(&self, generation: u64) -> bool {
        if self.generation.get() != generation {
            return false;
        }
        self.generation.set(self.generation.get().saturating_add(1));
        self.image.set_paintable(Option::<&gtk::gdk::Texture>::None);
        self.known_missing.set(true);
        self.sync_presentation(false, true);
        true
    }

    fn update_seed(&self, seed: u32) {
        if self.seed.replace(seed) == seed {
            return;
        }
        if self.known_missing.get() {
            self.fallback
                .set_paintable(Some(&fallback_cover_texture(seed)));
        }
    }

    fn sync_presentation(&self, has_texture: bool, known_missing: bool) {
        self.image.set_visible(has_texture);
        if has_texture {
            self.fallback.set_visible(false);
            self.fallback
                .set_paintable(Option::<&gtk::gdk::Texture>::None);
            self.area.set_opacity(1.0);
        } else if known_missing {
            if self.fallback.paintable().is_none() {
                self.fallback
                    .set_paintable(Some(&fallback_cover_texture(self.seed.get())));
            }
            self.fallback.set_visible(true);
            self.area.set_opacity(1.0);
        } else {
            self.fallback.set_visible(false);
            self.fallback
                .set_paintable(Option::<&gtk::gdk::Texture>::None);
            self.area.set_opacity(0.0);
        }
    }
}

impl ArtworkTileWeak {
    pub(crate) fn upgrade(&self) -> Option<ArtworkTile> {
        Some(ArtworkTile {
            area: self.area.upgrade()?,
            fallback: self.fallback.upgrade()?,
            image: self.image.upgrade()?,
            size: Rc::clone(&self.size),
            seed: Rc::clone(&self.seed),
            known_missing: Rc::clone(&self.known_missing),
            artwork_id: Rc::clone(&self.artwork_id),
            request_key: Rc::clone(&self.request_key),
            artwork_request: Rc::clone(&self.artwork_request),
            generation: Rc::clone(&self.generation),
            binding_active: Rc::clone(&self.binding_active),
            cleanup_hook_installed: Rc::clone(&self.cleanup_hook_installed),
        })
    }

    pub(super) fn is_bound(&self) -> bool {
        self.area.upgrade().is_some() && self.binding_active.get()
    }
}

fn cover_picture(content_fit: gtk::ContentFit) -> gtk::Picture {
    let picture = gtk::Picture::new();
    picture.set_accessible_role(gtk::AccessibleRole::Presentation);
    picture.set_can_shrink(true);
    picture.set_content_fit(content_fit);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_halign(gtk::Align::Fill);
    picture.set_valign(gtk::Align::Fill);
    picture.set_can_target(false);
    picture
}

fn fallback_cover_texture(seed: u32) -> gtk::gdk::Texture {
    const SIZE: usize = 64;

    let channel = |value: u8| (f64::from(value).mul_add(0.7, 255.0 * 0.18)).round() as u8;
    let base = [
        channel((seed & 0xff) as u8),
        channel(((seed >> 8) & 0xff) as u8),
        channel(((seed >> 16) & 0xff) as u8),
    ];
    let highlight = base.map(|value| (f64::from(value).mul_add(0.82, 255.0 * 0.18)).round() as u8);
    let mut rgba = vec![0_u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let normalized_x = (x as f64 + 0.5) / SIZE as f64;
            let normalized_y = (y as f64 + 0.5) / SIZE as f64;
            let highlighted = 0.8f64.mul_add(normalized_y, 0.2 * normalized_x) >= 0.16
                && 0.2f64.mul_add(normalized_y, -0.8 * normalized_x) >= -0.64
                && 0.2f64.mul_add(normalized_x, 0.8 * normalized_y) <= 0.84
                && 0.8f64.mul_add(normalized_x, -0.2 * normalized_y) >= -0.04;
            let color = if highlighted { highlight } else { base };
            let offset = (y * SIZE + x) * 4;
            rgba[offset..offset + 3].copy_from_slice(&color);
            rgba[offset + 3] = u8::MAX;
        }
    }

    let bytes = glib::Bytes::from_owned(rgba);
    gtk::gdk::MemoryTexture::new(
        SIZE as i32,
        SIZE as i32,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        SIZE * 4,
    )
    .upcast()
}

use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;
use std::sync::Arc;

use artwork::{DecodedImage, DecodedImageIdentity};
use gtk::gdk;
use gtk::glib;
use gtk::prelude::{Cast, ObjectExt};
use library::SourceId;

const MAX_TEXTURES: usize = 20_480;
const MAX_TEXTURE_BYTES: usize = 256 * 1024 * 1024;
const THUMBNAIL_RESERVE_BYTES: usize = 128 * 1024 * 1024;
const MAX_THUMBNAIL_TEXTURE_SIZE: u32 = 96;

pub(in crate::shell) struct TextureCache<K = DecodedImageIdentity> {
    entries: HashMap<K, TextureEntry>,
    live_textures: HashMap<K, LiveTexture>,
    thumbnail_order: BTreeSet<TextureAccess<K>>,
    large_order: BTreeSet<TextureAccess<K>>,
    bytes: usize,
    thumbnail_bytes: usize,
    next_access: u64,
    max_textures: usize,
    max_bytes: usize,
    thumbnail_reserve_bytes: usize,
}

#[derive(Clone)]
struct LiveTexture {
    source_id: SourceId,
    texture: glib::WeakRef<gdk::Texture>,
    bytes: usize,
    class: TextureClass,
}

struct TextureEntry {
    source_id: SourceId,
    texture: gdk::Texture,
    bytes: usize,
    last_used: u64,
    class: TextureClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextureClass {
    Thumbnail,
    Large,
}

fn texture_class(width: u32, height: u32) -> TextureClass {
    if width.max(height) <= MAX_THUMBNAIL_TEXTURE_SIZE {
        TextureClass::Thumbnail
    } else {
        TextureClass::Large
    }
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct TextureAccess<K> {
    last_used: u64,
    key: K,
}

struct TexturePixels(Arc<DecodedImage>);

impl AsRef<[u8]> for TexturePixels {
    fn as_ref(&self) -> &[u8] {
        self.0.rgba()
    }
}

impl<K> TextureCache<K>
where
    K: Clone + Eq + Hash + Ord,
{
    #[cfg(test)]
    fn with_limits(max_textures: usize, max_bytes: usize) -> Self {
        Self::with_limits_and_thumbnail_reserve(max_textures, max_bytes, max_bytes / 2)
    }

    fn with_limits_and_thumbnail_reserve(
        max_textures: usize,
        max_bytes: usize,
        thumbnail_reserve_bytes: usize,
    ) -> Self {
        Self {
            entries: HashMap::new(),
            live_textures: HashMap::new(),
            thumbnail_order: BTreeSet::new(),
            large_order: BTreeSet::new(),
            bytes: 0,
            thumbnail_bytes: 0,
            next_access: 0,
            max_textures,
            max_bytes,
            thumbnail_reserve_bytes: thumbnail_reserve_bytes.min(max_bytes),
        }
    }

    fn get(&mut self, key: &K) -> Option<gdk::Texture> {
        let last_used = self.next_access();
        let (previous_access, class, texture) = {
            let entry = self.entries.get_mut(key)?;
            let previous_access = TextureAccess {
                last_used: entry.last_used,
                key: key.clone(),
            };
            entry.last_used = last_used;
            (previous_access, entry.class, entry.texture.clone())
        };
        self.order_mut(class).remove(&previous_access);
        self.order_mut(class).insert(TextureAccess {
            last_used,
            key: key.clone(),
        });
        Some(texture)
    }

    fn insert_with_class(
        &mut self,
        key: K,
        source_id: SourceId,
        texture: gdk::Texture,
        bytes: usize,
        class: TextureClass,
    ) {
        self.remove(&key);
        self.live_textures.insert(
            key.clone(),
            LiveTexture {
                source_id: source_id.clone(),
                texture: texture.downgrade(),
                bytes,
                class,
            },
        );
        let last_used = self.next_access();
        self.bytes = self.bytes.saturating_add(bytes);
        if class == TextureClass::Thumbnail {
            self.thumbnail_bytes = self.thumbnail_bytes.saturating_add(bytes);
        }
        self.entries.insert(
            key.clone(),
            TextureEntry {
                source_id,
                texture,
                bytes,
                last_used,
                class,
            },
        );
        self.order_mut(class)
            .insert(TextureAccess { last_used, key });
        self.evict_to_limits();
        if self.live_textures.len() > self.entries.len().saturating_add(self.max_textures) {
            self.live_textures
                .retain(|_, entry| entry.texture.upgrade().is_some());
        }
    }

    fn insert_source_warm(
        &mut self,
        key: K,
        source_id: SourceId,
        texture: gdk::Texture,
        bytes: usize,
    ) {
        self.insert_with_class(key, source_id, texture, bytes, TextureClass::Thumbnail);
    }

    fn get_or_revive(&mut self, key: &K) -> Option<gdk::Texture> {
        if let Some(texture) = self.get(key) {
            return Some(texture);
        }
        let live = self.live_textures.get(key)?.clone();
        let Some(texture) = live.texture.upgrade() else {
            self.live_textures.remove(key);
            return None;
        };
        self.insert_with_class(
            key.clone(),
            live.source_id,
            texture.clone(),
            live.bytes,
            live.class,
        );
        Some(texture)
    }

    fn invalidate_source(&mut self, source_id: &SourceId) {
        let stale = self
            .live_textures
            .iter()
            .filter(|(_, entry)| &entry.source_id == source_id)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in stale {
            self.remove(&key);
            self.live_textures.remove(&key);
        }
    }

    fn release_source_warm(&mut self, source_id: &SourceId) {
        let stale = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                &entry.source_id == source_id && entry.class == TextureClass::Thumbnail
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in stale {
            self.remove(&key);
        }
        self.live_textures
            .retain(|_, entry| &entry.source_id != source_id || entry.texture.upgrade().is_some());
    }

    fn remove(&mut self, key: &K) -> Option<TextureEntry> {
        let entry = self.entries.remove(key)?;
        self.order_mut(entry.class).remove(&TextureAccess {
            last_used: entry.last_used,
            key: key.clone(),
        });
        self.bytes = self.bytes.saturating_sub(entry.bytes);
        if entry.class == TextureClass::Thumbnail {
            self.thumbnail_bytes = self.thumbnail_bytes.saturating_sub(entry.bytes);
        }
        Some(entry)
    }

    fn next_access(&mut self) -> u64 {
        self.next_access = self.next_access.wrapping_add(1).max(1);
        self.next_access
    }

    fn order_mut(&mut self, class: TextureClass) -> &mut BTreeSet<TextureAccess<K>> {
        match class {
            TextureClass::Thumbnail => &mut self.thumbnail_order,
            TextureClass::Large => &mut self.large_order,
        }
    }

    fn oldest_class(&self) -> Option<TextureClass> {
        match (self.thumbnail_order.first(), self.large_order.first()) {
            (Some(thumbnail), Some(large)) => Some(if thumbnail <= large {
                TextureClass::Thumbnail
            } else {
                TextureClass::Large
            }),
            (Some(_), None) => Some(TextureClass::Thumbnail),
            (None, Some(_)) => Some(TextureClass::Large),
            (None, None) => None,
        }
    }

    fn byte_pressure_class(&self) -> Option<TextureClass> {
        let large_bytes = self.bytes.saturating_sub(self.thumbnail_bytes);
        let large_reserve_bytes = self.max_bytes.saturating_sub(self.thumbnail_reserve_bytes);
        match (
            self.thumbnail_bytes > self.thumbnail_reserve_bytes,
            large_bytes > large_reserve_bytes,
        ) {
            (true, false) => Some(TextureClass::Thumbnail),
            (false, true) => Some(TextureClass::Large),
            _ => self.oldest_class(),
        }
    }

    fn oldest_key(&self, class: TextureClass) -> Option<K> {
        match class {
            TextureClass::Thumbnail => self.thumbnail_order.first(),
            TextureClass::Large => self.large_order.first(),
        }
        .map(|access| access.key.clone())
    }

    fn evict_to_limits(&mut self) {
        while self.entries.len() > self.max_textures || self.bytes > self.max_bytes {
            let class = if self.bytes > self.max_bytes {
                self.byte_pressure_class()
            } else {
                self.oldest_class()
            };
            let Some(key) = class.and_then(|class| self.oldest_key(class)) else {
                break;
            };
            self.remove(&key);
        }
    }

    fn thumbnail_warm_limit(&self, render_size: u32) -> usize {
        let Ok(render_size) = usize::try_from(render_size) else {
            return 0;
        };
        let bytes = render_size.saturating_mul(render_size).saturating_mul(4);
        if bytes == 0 {
            return 0;
        }
        let available_bytes = self
            .thumbnail_reserve_bytes
            .saturating_sub(self.thumbnail_bytes);
        let available_entries = self.max_textures.saturating_sub(self.entries.len());
        (available_bytes / bytes).min(available_entries)
    }

    #[cfg(test)]
    fn assert_consistent(&self) {
        let bytes = self
            .entries
            .values()
            .map(|entry| entry.bytes)
            .sum::<usize>();
        let thumbnail_bytes = self
            .entries
            .values()
            .filter(|entry| entry.class == TextureClass::Thumbnail)
            .map(|entry| entry.bytes)
            .sum::<usize>();
        assert_eq!(self.bytes, bytes);
        assert_eq!(self.thumbnail_bytes, thumbnail_bytes);
        assert!(self.bytes <= self.max_bytes);
        assert!(self.entries.len() <= self.max_textures);
        assert_eq!(
            self.thumbnail_order.len(),
            self.entries
                .values()
                .filter(|entry| entry.class == TextureClass::Thumbnail)
                .count()
        );
        assert_eq!(
            self.large_order.len(),
            self.entries
                .values()
                .filter(|entry| entry.class == TextureClass::Large)
                .count()
        );
        for access in &self.thumbnail_order {
            assert_eq!(
                self.entries.get(&access.key).map(|entry| entry.class),
                Some(TextureClass::Thumbnail)
            );
        }
        for access in &self.large_order {
            assert_eq!(
                self.entries.get(&access.key).map(|entry| entry.class),
                Some(TextureClass::Large)
            );
        }
        for (key, entry) in &self.entries {
            let access = TextureAccess {
                last_used: entry.last_used,
                key: key.clone(),
            };
            assert!(match entry.class {
                TextureClass::Thumbnail => self.thumbnail_order.contains(&access),
                TextureClass::Large => self.large_order.contains(&access),
            });
        }
    }
}

impl Default for TextureCache {
    fn default() -> Self {
        Self::with_limits_and_thumbnail_reserve(
            MAX_TEXTURES,
            MAX_TEXTURE_BYTES,
            THUMBNAIL_RESERVE_BYTES,
        )
    }
}

impl TextureCache {
    pub(super) fn source_thumbnail_warm_limit(&self, render_size: u32) -> usize {
        self.thumbnail_warm_limit(render_size)
    }

    pub(super) fn texture(
        &mut self,
        source_id: &SourceId,
        image: Arc<DecodedImage>,
    ) -> Option<gdk::Texture> {
        let identity = image.identity();
        if let Some(texture) = self.get_or_revive(&identity) {
            return Some(texture);
        }
        let bytes = image.rgba().len();
        let class = texture_class(image.width(), image.height());
        let texture = texture_from_decoded(image)?;
        self.insert_with_class(identity, source_id.clone(), texture.clone(), bytes, class);
        Some(texture)
    }

    pub(super) fn try_retain_source_warm_texture(
        &mut self,
        source_id: &SourceId,
        image: Arc<DecodedImage>,
    ) -> bool {
        let identity = image.identity();
        if self.get(&identity).is_some() {
            return true;
        }
        let bytes = image.rgba().len();
        if self.thumbnail_bytes.saturating_add(bytes) > self.thumbnail_reserve_bytes
            || self.entries.len() >= self.max_textures
        {
            return false;
        }
        let texture = match self.live_textures.get(&identity).cloned() {
            Some(live) => match live.texture.upgrade() {
                Some(texture) => texture,
                None => {
                    self.live_textures.remove(&identity);
                    let Some(texture) = texture_from_decoded(image) else {
                        return false;
                    };
                    texture
                }
            },
            None => {
                let Some(texture) = texture_from_decoded(image) else {
                    return false;
                };
                texture
            }
        };
        self.insert_source_warm(identity, source_id.clone(), texture, bytes);
        true
    }

    pub(super) fn release_source_warm_textures(&mut self, source_id: &SourceId) {
        self.release_source_warm(source_id);
    }

    pub(super) fn release_source(&mut self, source_id: &SourceId) {
        self.invalidate_source(source_id);
    }
}

fn texture_from_decoded(image: Arc<DecodedImage>) -> Option<gdk::Texture> {
    let width = i32::try_from(image.width()).ok()?;
    let height = i32::try_from(image.height()).ok()?;
    let row_stride = usize::try_from(image.row_stride()).ok()?;
    let bytes = glib::Bytes::from_owned(TexturePixels(image));
    Some(
        gdk::MemoryTexture::new(
            width,
            height,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            row_stride,
        )
        .upcast(),
    )
}

#[cfg(test)]
mod tests {
    use gtk::prelude::ObjectType;
    use proptest::prelude::*;

    use super::*;

    fn texture(value: u8) -> gdk::Texture {
        let bytes = glib::Bytes::from_owned([value, value, value, 255]);
        gdk::MemoryTexture::new(1, 1, gdk::MemoryFormat::R8g8b8a8, &bytes, 4).upcast()
    }

    #[test]
    fn texture_class_uses_the_longest_decoded_edge() {
        assert_eq!(texture_class(96, 96), TextureClass::Thumbnail);
        assert_eq!(texture_class(192, 48), TextureClass::Large);
    }

    #[test]
    fn shared_texture_cache_reuses_one_texture_and_evicts_by_owned_bytes() {
        let source = SourceId::new("texture-cache-source");
        let mut cache = TextureCache::<u8>::with_limits(3, 8);
        let first = texture(1);
        let second = texture(2);
        cache.insert_with_class(1, source.clone(), first.clone(), 4, TextureClass::Large);
        cache.insert_with_class(2, source.clone(), second.clone(), 4, TextureClass::Large);
        let reused = cache.get(&1).expect("the first texture remains cached");
        assert_eq!(reused.as_ptr(), first.as_ptr());

        cache.insert_with_class(3, source, texture(3), 4, TextureClass::Large);

        assert_eq!(cache.bytes, 8);
        assert!(cache.get(&1).is_some(), "the recent texture stays cached");
        assert!(cache.get(&2).is_none(), "the older strong entry is evicted");
        let revived = cache
            .get_or_revive(&2)
            .expect("a mounted texture remains interned after LRU eviction");
        assert_eq!(revived.as_ptr(), second.as_ptr());
        assert_eq!(cache.bytes, 8);
    }

    #[test]
    fn thumbnail_and_large_textures_borrow_one_shared_budget() {
        let source = SourceId::new("shared-texture-budget-source");
        let mut cache = TextureCache::<u8>::with_limits(8, 16);
        cache.insert_source_warm(1, source.clone(), texture(1), 4);
        for key in 2..=4 {
            cache.insert_with_class(key, source.clone(), texture(key), 4, TextureClass::Large);
        }
        assert_eq!(cache.thumbnail_bytes, 4);
        assert_eq!(cache.bytes, 16);

        cache.insert_source_warm(5, source, texture(5), 4);

        assert_eq!(cache.bytes, 16);
        assert_eq!(cache.thumbnail_bytes, 8);
        assert!(cache.get(&1).is_some());
        assert!(
            cache.get(&2).is_none(),
            "borrowed large-cover space is returned"
        );
        assert!(cache.get(&3).is_some());
        assert!(cache.get(&4).is_some());
        assert!(cache.get(&5).is_some());
    }

    #[test]
    fn each_texture_class_keeps_its_soft_share_under_pressure() {
        let source = SourceId::new("texture-priority-source");
        let mut cache = TextureCache::<u8>::with_limits(8, 16);
        for key in 1..=2 {
            cache.insert_with_class(
                key,
                source.clone(),
                texture(key),
                4,
                TextureClass::Thumbnail,
            );
        }
        for key in 3..=4 {
            cache.insert_with_class(key, source.clone(), texture(key), 4, TextureClass::Large);
        }

        cache.insert_with_class(5, source.clone(), texture(5), 4, TextureClass::Large);
        assert!(cache.get(&1).is_some());
        assert!(cache.get(&2).is_some());
        assert!(cache.get(&3).is_none());

        cache.insert_with_class(6, source, texture(6), 4, TextureClass::Thumbnail);
        assert!(cache.get(&1).is_none());
        assert!(cache.get(&2).is_some());
        assert!(cache.get(&4).is_some());
        assert!(cache.get(&5).is_some());
        assert!(cache.get(&6).is_some());
        assert_eq!(cache.bytes, 16);
    }

    #[test]
    fn thumbnail_warm_limit_follows_the_available_soft_share() {
        let source = SourceId::new("thumbnail-admission-source");
        let mut cache = TextureCache::<u8>::with_limits(8, 16);
        assert_eq!(cache.thumbnail_warm_limit(1), 2);
        assert_eq!(cache.thumbnail_warm_limit(2), 0);

        cache.insert_source_warm(1, source, texture(1), 4);

        assert_eq!(cache.thumbnail_warm_limit(1), 1);
    }

    #[test]
    fn releasing_source_warm_keeps_a_mounted_texture_revivable() {
        let source = SourceId::new("released-warm-texture-source");
        let mut cache = TextureCache::<u8>::with_limits(1, 4);
        let mounted = texture(1);
        cache.insert_source_warm(1, source.clone(), mounted.clone(), 4);

        cache.release_source_warm(&source);

        assert!(cache.entries.is_empty());
        let revived = cache
            .get_or_revive(&1)
            .expect("GTK still owns the mounted texture");
        assert_eq!(revived.as_ptr(), mounted.as_ptr());
        assert_eq!(cache.bytes, 4);
    }

    #[test]
    fn source_release_drops_only_that_sources_textures() {
        let first_source = SourceId::new("first-texture-source");
        let second_source = SourceId::new("second-texture-source");
        let mut cache = TextureCache::<u8>::with_limits(3, 12);
        cache.insert_with_class(1, first_source.clone(), texture(1), 4, TextureClass::Large);
        cache.insert_with_class(2, second_source, texture(2), 4, TextureClass::Large);

        cache.invalidate_source(&first_source);

        assert!(cache.get_or_revive(&1).is_none());
        assert!(cache.get_or_revive(&2).is_some());
        assert_eq!(cache.bytes, 4);
    }

    proptest! {
        #[test]
        fn arbitrary_cache_operations_preserve_the_shared_bounds(
            operations in prop::collection::vec(
                (0u8..6, 0u8..24, 1usize..=24, any::<bool>(), any::<bool>()),
                1..=96,
            ),
        ) {
            let first_source = SourceId::new("property-source-one");
            let second_source = SourceId::new("property-source-two");
            let mut cache = TextureCache::<u8>::with_limits(16, 64);

            for (operation, key, bytes, thumbnail, second) in operations {
                let source = if second {
                    second_source.clone()
                } else {
                    first_source.clone()
                };
                match operation {
                    0 | 1 => cache.insert_with_class(
                        key,
                        source,
                        texture(key),
                        bytes,
                        if thumbnail {
                            TextureClass::Thumbnail
                        } else {
                            TextureClass::Large
                        },
                    ),
                    2 => {
                        cache.get(&key);
                    }
                    3 => cache.release_source_warm(&source),
                    4 => cache.invalidate_source(&source),
                    _ => {
                        cache.get_or_revive(&key);
                    }
                }
                cache.assert_consistent();
            }
        }
    }
}

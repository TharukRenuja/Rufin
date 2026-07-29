use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;
use std::sync::Arc;

use artwork::{DecodedImage, DecodedImageIdentity};
use gtk::gdk;
use gtk::glib;
use gtk::prelude::{Cast, ObjectExt};
use library::SourceId;

const MAX_RECENT_TEXTURES: usize = 4_096;
const MAX_RECENT_TEXTURE_BYTES: usize = 128 * 1024 * 1024;

pub(in crate::shell) struct TextureCache<K = DecodedImageIdentity> {
    entries: HashMap<K, TextureEntry>,
    source_warm: HashMap<K, TextureEntry>,
    live_textures: HashMap<K, LiveTexture>,
    eviction_order: BTreeSet<TextureAccess<K>>,
    bytes: usize,
    next_access: u64,
    max_textures: usize,
    max_bytes: usize,
}

#[derive(Clone)]
struct LiveTexture {
    source_id: SourceId,
    texture: glib::WeakRef<gdk::Texture>,
    bytes: usize,
}

struct TextureEntry {
    source_id: SourceId,
    texture: gdk::Texture,
    bytes: usize,
    last_used: u64,
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
    fn with_limits(max_textures: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            source_warm: HashMap::new(),
            live_textures: HashMap::new(),
            eviction_order: BTreeSet::new(),
            bytes: 0,
            next_access: 0,
            max_textures,
            max_bytes,
        }
    }

    fn get(&mut self, key: &K) -> Option<gdk::Texture> {
        if let Some(entry) = self.source_warm.get(key) {
            return Some(entry.texture.clone());
        }
        let last_used = self.next_access();
        let (previous_access, texture) = {
            let entry = self.entries.get_mut(key)?;
            let previous_access = TextureAccess {
                last_used: entry.last_used,
                key: key.clone(),
            };
            entry.last_used = last_used;
            (previous_access, entry.texture.clone())
        };
        self.eviction_order.remove(&previous_access);
        self.eviction_order.insert(TextureAccess {
            last_used,
            key: key.clone(),
        });
        Some(texture)
    }

    fn insert(&mut self, key: K, source_id: SourceId, texture: gdk::Texture, bytes: usize) {
        self.remove(&key);
        self.source_warm.remove(&key);
        self.live_textures.insert(
            key.clone(),
            LiveTexture {
                source_id: source_id.clone(),
                texture: texture.downgrade(),
                bytes,
            },
        );
        let last_used = self.next_access();
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries.insert(
            key.clone(),
            TextureEntry {
                source_id,
                texture,
                bytes,
                last_used,
            },
        );
        self.eviction_order.insert(TextureAccess { last_used, key });
        self.evict_to_limits();
        if self.live_textures.len()
            > self
                .entries
                .len()
                .saturating_add(self.source_warm.len())
                .saturating_add(self.max_textures)
        {
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
        self.remove(&key);
        self.source_warm.remove(&key);
        self.live_textures.insert(
            key.clone(),
            LiveTexture {
                source_id: source_id.clone(),
                texture: texture.downgrade(),
                bytes,
            },
        );
        self.source_warm.insert(
            key,
            TextureEntry {
                source_id,
                texture,
                bytes,
                last_used: 0,
            },
        );
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
        self.insert(key.clone(), live.source_id, texture.clone(), live.bytes);
        Some(texture)
    }

    fn get_or_retain_source_warm(&mut self, key: &K) -> Option<gdk::Texture> {
        if let Some(entry) = self.source_warm.get(key) {
            return Some(entry.texture.clone());
        }
        if let Some(entry) = self.remove(key) {
            let texture = entry.texture.clone();
            self.source_warm.insert(key.clone(), entry);
            return Some(texture);
        }
        let live = self.live_textures.get(key)?.clone();
        let Some(texture) = live.texture.upgrade() else {
            self.live_textures.remove(key);
            return None;
        };
        self.insert_source_warm(key.clone(), live.source_id, texture.clone(), live.bytes);
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
            self.source_warm.remove(&key);
            self.live_textures.remove(&key);
        }
    }

    fn release_source_warm(&mut self, source_id: &SourceId) {
        let stale = self
            .source_warm
            .iter()
            .filter(|(_, entry)| &entry.source_id == source_id)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in stale {
            self.source_warm.remove(&key);
        }
        self.live_textures
            .retain(|_, entry| &entry.source_id != source_id || entry.texture.upgrade().is_some());
    }

    fn remove(&mut self, key: &K) -> Option<TextureEntry> {
        let entry = self.entries.remove(key)?;
        self.eviction_order.remove(&TextureAccess {
            last_used: entry.last_used,
            key: key.clone(),
        });
        self.bytes = self.bytes.saturating_sub(entry.bytes);
        Some(entry)
    }

    fn next_access(&mut self) -> u64 {
        self.next_access = self.next_access.wrapping_add(1).max(1);
        self.next_access
    }

    fn evict_to_limits(&mut self) {
        while self.entries.len() > self.max_textures || self.bytes > self.max_bytes {
            let Some(access) = self.eviction_order.first().cloned() else {
                break;
            };
            self.remove(&access.key);
        }
    }
}

impl Default for TextureCache {
    fn default() -> Self {
        Self::with_limits(MAX_RECENT_TEXTURES, MAX_RECENT_TEXTURE_BYTES)
    }
}

impl TextureCache {
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
        let texture = texture_from_decoded(image)?;
        self.insert(identity, source_id.clone(), texture.clone(), bytes);
        Some(texture)
    }

    pub(super) fn retain_source_warm_texture(
        &mut self,
        source_id: &SourceId,
        image: Arc<DecodedImage>,
    ) -> Option<gdk::Texture> {
        let identity = image.identity();
        if let Some(texture) = self.get_or_retain_source_warm(&identity) {
            return Some(texture);
        }
        let bytes = image.rgba().len();
        let texture = texture_from_decoded(image)?;
        self.insert_source_warm(identity, source_id.clone(), texture.clone(), bytes);
        Some(texture)
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

    use super::*;

    fn texture(value: u8) -> gdk::Texture {
        let bytes = glib::Bytes::from_owned([value, value, value, 255]);
        gdk::MemoryTexture::new(1, 1, gdk::MemoryFormat::R8g8b8a8, &bytes, 4).upcast()
    }

    #[test]
    fn final_texture_cache_reuses_one_texture_and_evicts_by_owned_bytes() {
        let source = SourceId::new("texture-cache-source");
        let mut cache = TextureCache::<u8>::with_limits(3, 8);
        let first = texture(1);
        let second = texture(2);
        cache.insert(1, source.clone(), first.clone(), 4);
        cache.insert(2, source.clone(), second.clone(), 4);
        let reused = cache.get(&1).expect("the first texture remains cached");
        assert_eq!(reused.as_ptr(), first.as_ptr());

        cache.insert(3, source, texture(3), 4);

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
    fn source_warm_textures_do_not_compete_with_recent_textures() {
        let source = SourceId::new("warm-texture-source");
        let mut cache = TextureCache::<u8>::with_limits(1, 4);
        let warm = texture(1);
        cache.insert(1, source.clone(), warm.clone(), 4);
        let retained = cache
            .get_or_retain_source_warm(&1)
            .expect("the recent texture becomes source-wide warmth");
        assert_eq!(retained.as_ptr(), warm.as_ptr());
        cache.insert(2, source.clone(), texture(2), 4);
        cache.insert(3, source, texture(3), 4);

        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, 4);
        assert_eq!(
            cache
                .get(&1)
                .expect("the source-wide small texture remains retained")
                .as_ptr(),
            warm.as_ptr()
        );
        assert!(cache.get(&2).is_none());
        assert!(cache.get(&3).is_some());
    }

    #[test]
    fn releasing_source_warm_keeps_a_mounted_texture_revivable() {
        let source = SourceId::new("released-warm-texture-source");
        let mut cache = TextureCache::<u8>::with_limits(1, 4);
        let mounted = texture(1);
        cache.insert_source_warm(1, source.clone(), mounted.clone(), 4);

        cache.release_source_warm(&source);

        assert!(cache.source_warm.is_empty());
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
        cache.insert(1, first_source.clone(), texture(1), 4);
        cache.insert(2, second_source, texture(2), 4);

        cache.invalidate_source(&first_source);

        assert!(cache.get_or_revive(&1).is_none());
        assert!(cache.get_or_revive(&2).is_some());
        assert_eq!(cache.bytes, 4);
    }
}

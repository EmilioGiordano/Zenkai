use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::RenderImage;

use super::layout::PageIdx;

pub const BITMAP_CAP_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BitmapKey {
    pub page: PageIdx,
    pub pixel_width: u32,
}

struct Slot {
    image: Arc<RenderImage>,
    bytes: usize,
    last_used: u64,
}

// Rendered pages under a memory cap. Whatever leaves the cache is handed back so the caller
// can also drop its texture from the GPU atlas.
pub struct BitmapCache {
    slots: HashMap<BitmapKey, Slot>,
    used: usize,
    cap: usize,
    clock: u64,
}

impl BitmapCache {
    pub fn new(cap: usize) -> BitmapCache {
        BitmapCache {
            slots: HashMap::new(),
            used: 0,
            cap,
            clock: 0,
        }
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }

    pub fn contains(&self, key: BitmapKey) -> bool {
        self.slots.contains_key(&key)
    }

    pub fn get(&mut self, key: BitmapKey) -> Option<Arc<RenderImage>> {
        self.clock += 1;
        let slot = self.slots.get_mut(&key)?;
        slot.last_used = self.clock;
        Some(slot.image.clone())
    }

    // The best stand-in while the exact bitmap renders: the same page at another width.
    pub fn any_width(&mut self, page: PageIdx) -> Option<Arc<RenderImage>> {
        let key = self
            .slots
            .keys()
            .copied()
            .filter(|key| key.page == page)
            .max_by_key(|key| key.pixel_width)?;
        self.get(key)
    }

    // Evicts the least recently used bitmaps outside `keep` until the cap holds. A bitmap
    // inside `keep` stays even when it alone is over the cap, because it is on screen.
    pub fn insert(
        &mut self,
        key: BitmapKey,
        image: Arc<RenderImage>,
        bytes: usize,
        keep: impl Fn(PageIdx) -> bool,
    ) -> Vec<Arc<RenderImage>> {
        self.clock += 1;
        let mut dropped = Vec::new();
        if let Some(old) = self.slots.insert(
            key,
            Slot {
                image,
                bytes,
                last_used: self.clock,
            },
        ) {
            self.used -= old.bytes;
            dropped.push(old.image);
        }
        self.used += bytes;
        while self.used > self.cap {
            let victim = self
                .slots
                .iter()
                .filter(|(slot_key, _)| slot_key.page != key.page && !keep(slot_key.page))
                .min_by_key(|(_, slot)| slot.last_used)
                .map(|(slot_key, _)| *slot_key);
            let Some(victim) = victim else {
                break;
            };
            if let Some(slot) = self.slots.remove(&victim) {
                self.used -= slot.bytes;
                dropped.push(slot.image);
            }
        }
        dropped
    }

    pub fn clear(&mut self) -> Vec<Arc<RenderImage>> {
        self.used = 0;
        self.slots.drain().map(|(_, slot)| slot.image).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(Vec::<image::Frame>::new()))
    }

    fn key(page: usize, width: u32) -> BitmapKey {
        BitmapKey {
            page: PageIdx(page),
            pixel_width: width,
        }
    }

    #[test]
    fn inserting_past_the_cap_evicts_the_least_recently_used_page() {
        let mut cache = BitmapCache::new(250);
        cache.insert(key(0, 100), image(), 100, |_| false);
        cache.insert(key(1, 100), image(), 100, |_| false);
        assert!(cache.get(key(0, 100)).is_some());
        let dropped = cache.insert(key(2, 100), image(), 100, |_| false);
        assert_eq!(dropped.len(), 1);
        assert!(cache.contains(key(0, 100)));
        assert!(!cache.contains(key(1, 100)));
        assert!(cache.contains(key(2, 100)));
        assert_eq!(cache.used_bytes(), 200);
    }

    #[test]
    fn visible_pages_are_never_evicted() {
        let mut cache = BitmapCache::new(150);
        cache.insert(key(0, 100), image(), 100, |_| true);
        let dropped = cache.insert(key(1, 100), image(), 100, |page| page.0 <= 1);
        assert!(dropped.is_empty());
        assert_eq!(cache.used_bytes(), 200);
    }

    #[test]
    fn scrolling_on_drops_bitmaps_left_behind_so_the_cap_holds() {
        let mut cache = BitmapCache::new(300);
        for page in 0..20usize {
            let visible = page.saturating_sub(1)..page + 1;
            cache.insert(key(page, 100), image(), 100, |kept| {
                visible.contains(&kept.0)
            });
            assert!(cache.used_bytes() <= 300);
        }
        assert!(!cache.contains(key(0, 100)));
        assert!(cache.contains(key(19, 100)));
    }

    #[test]
    fn replacing_a_bitmap_keeps_the_count_right() {
        let mut cache = BitmapCache::new(1000);
        cache.insert(key(0, 100), image(), 100, |_| false);
        let dropped = cache.insert(key(0, 100), image(), 60, |_| false);
        assert_eq!(dropped.len(), 1);
        assert_eq!(cache.used_bytes(), 60);
    }

    #[test]
    fn a_stand_in_comes_from_the_widest_cached_width_of_the_page() {
        let mut cache = BitmapCache::new(1000);
        cache.insert(key(0, 100), image(), 10, |_| false);
        cache.insert(key(0, 300), image(), 10, |_| false);
        assert!(cache.any_width(PageIdx(0)).is_some());
        assert!(cache.any_width(PageIdx(1)).is_none());
    }

    #[test]
    fn clearing_hands_back_everything() {
        let mut cache = BitmapCache::new(1000);
        cache.insert(key(0, 100), image(), 10, |_| false);
        cache.insert(key(1, 100), image(), 10, |_| false);
        assert_eq!(cache.clear().len(), 2);
        assert_eq!(cache.used_bytes(), 0);
    }
}

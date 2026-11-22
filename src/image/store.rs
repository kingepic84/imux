//! Per-pane image store. A placed image covers `cols` x `rows` cells; each covered cell carries a
//! tile id (`Cell::tile`, 0 = none) naming the image and which cell of its footprint it is.
//! Because cells hold the references, images scroll, erase and fall out of scrollback exactly
//! like text. Tiles are interned and collected lazily (`retain`) once the cells that used them
//! are gone.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use super::Bitmap;

pub type ImageId = u32;

/// Default memory budget for decoded pixels in one pane.
pub const DEFAULT_BUDGET: usize = 256 * 1024 * 1024;

/// Memory budget for decoded pixels in all panes together. Every pane is under its own budget
/// as well, so this is what stops many panes from adding up to more than the machine has: a
/// pane whose store finds the total over the limit drops its own oldest pictures.
pub const GLOBAL_BUDGET: usize = 1024 * 1024 * 1024;

thread_local! {
    // The server owns every store and uses them from its one loop thread, so the count is
    // per thread, which also keeps the tests that make stores from interfering.
    static TOTAL: Cell<usize> = const { Cell::new(0) };
    static LIMIT: Cell<usize> = const { Cell::new(GLOBAL_BUDGET) };
}

fn add_total(bytes: usize) {
    TOTAL.with(|t| t.set(t.get().saturating_add(bytes)));
}

fn sub_total(bytes: usize) {
    TOTAL.with(|t| t.set(t.get().saturating_sub(bytes)));
}

/// Bytes of decoded pixels held by all stores of this thread.
pub fn total_bytes() -> usize {
    TOTAL.with(Cell::get)
}

#[cfg(test)]
fn set_global_limit(limit: usize) {
    LIMIT.with(|l| l.set(limit));
}

/// Which cell of an image's footprint a grid cell shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tile {
    pub image: ImageId,
    pub tx: u16,
    pub ty: u16,
}

/// A decoded image and the cells it was given when placed.
pub struct Placed {
    pub bitmap: Arc<Bitmap>,
    pub cols: usize,
    pub rows: usize,
}

pub struct Store {
    images: HashMap<ImageId, Placed>,
    /// Oldest first, for eviction.
    order: VecDeque<ImageId>,
    bytes: usize,
    budget: usize,
    next: ImageId,
    tiles: Vec<Tile>,
    interned: HashMap<Tile, u32>,
    /// Images shown by placeholder characters: kept by a collection even while no cell uses them,
    /// because the program may print the placeholders again at any time.
    pinned: HashSet<ImageId>,
    /// How many tiles the last collection left alive.
    after_collection: usize,
}

impl Store {
    pub fn new(budget: usize) -> Store {
        Store {
            images: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            budget,
            next: 1,
            tiles: Vec::new(),
            interned: HashMap::new(),
            pinned: HashSet::new(),
            after_collection: 0,
        }
    }

    /// Show other pixels of the same size for an image (the next frame of an animation).
    pub fn set_bitmap(&mut self, id: ImageId, bitmap: Arc<Bitmap>) {
        if let Some(p) = self.images.get_mut(&id)
            && (bitmap.w, bitmap.h) == (p.bitmap.w, p.bitmap.h)
        {
            p.bitmap = bitmap;
        }
    }

    pub fn pin(&mut self, image: ImageId) {
        self.pinned.insert(image);
    }

    pub fn unpin(&mut self, image: ImageId) {
        self.pinned.remove(&image);
    }

    pub fn add(&mut self, bitmap: Bitmap, cols: usize, rows: usize) -> ImageId {
        let id = self.next;
        self.next += 1;
        self.bytes += bitmap.bytes();
        add_total(bitmap.bytes());
        self.images.insert(id, Placed { bitmap: Arc::new(bitmap), cols, rows });
        self.order.push_back(id);
        id
    }

    pub fn image(&self, id: ImageId) -> Option<&Placed> {
        self.images.get(&id)
    }

    /// The tile id for one cell of an image, creating it on first use. Never 0.
    pub fn tile(&mut self, image: ImageId, tx: u16, ty: u16) -> u32 {
        let tile = Tile { image, tx, ty };
        if let Some(&id) = self.interned.get(&tile) {
            return id;
        }
        self.tiles.push(tile);
        let id = self.tiles.len() as u32;
        self.interned.insert(tile, id);
        id
    }

    pub fn lookup(&self, tile: u32) -> Option<Tile> {
        self.tiles.get((tile as usize).checked_sub(1)?).copied()
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// More than `max` tiles, and twice as many as the last collection left: a collection walks
    /// every cell of the history, so one after every small picture (while the history holds a
    /// million live tiles that no collection can free) would stall the server (security review P-02).
    pub fn tiles_over(&self, max: usize) -> bool {
        self.tiles.len() > max.max(self.after_collection.saturating_mul(2))
    }

    /// Over this pane's budget, or the pictures of all panes are over the global one (then each
    /// pane that is asked gives up its oldest pictures, the one that just added most of all).
    pub fn over_budget(&self) -> bool {
        self.bytes > self.budget || (self.bytes > 0 && total_bytes() > LIMIT.with(Cell::get))
    }

    /// Drop the oldest image's pixels. Its tiles stay until collected and then draw as a
    /// placeholder. `false` when there is nothing left to drop.
    pub fn evict_oldest(&mut self) -> bool {
        let Some(id) = self.order.pop_front() else { return false };
        if let Some(p) = self.images.remove(&id) {
            self.bytes -= p.bitmap.bytes();
            sub_total(p.bitmap.bytes());
        }
        true
    }

    /// Keep only the tiles in `live`, renumbering them, and drop images no live tile refers to.
    /// Returns the new id for each old id (index = old id, 0 = gone).
    pub fn retain(&mut self, live: &HashSet<u32>) -> Vec<u32> {
        let mut map = vec![0u32; self.tiles.len() + 1];
        let mut kept = Vec::new();
        for (i, tile) in self.tiles.iter().enumerate() {
            if live.contains(&(i as u32 + 1)) {
                kept.push(*tile);
                map[i + 1] = kept.len() as u32;
            }
        }
        let mut used: HashSet<ImageId> = kept.iter().map(|t| t.image).collect();
        used.extend(self.pinned.iter().copied());
        let dead: Vec<ImageId> = self.images.keys().copied().filter(|id| !used.contains(id)).collect();
        for id in dead {
            if let Some(p) = self.images.remove(&id) {
                self.bytes -= p.bitmap.bytes();
                sub_total(p.bitmap.bytes());
            }
        }
        self.order.retain(|id| self.images.contains_key(id));
        self.interned = kept.iter().enumerate().map(|(i, t)| (*t, i as u32 + 1)).collect();
        self.tiles = kept;
        self.after_collection = self.tiles.len();
        map
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        sub_total(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmp(w: usize, h: usize) -> Bitmap {
        Bitmap::new(w, h)
    }

    #[test]
    fn tiles_are_interned_and_never_zero() {
        let mut s = Store::new(1 << 20);
        let id = s.add(bmp(4, 4), 2, 2);
        let a = s.tile(id, 0, 0);
        let b = s.tile(id, 1, 0);
        assert_ne!(a, 0);
        assert_ne!(a, b);
        assert_eq!(s.tile(id, 0, 0), a);
        assert_eq!(s.lookup(b), Some(Tile { image: id, tx: 1, ty: 0 }));
        assert_eq!(s.lookup(0), None);
        assert_eq!(s.lookup(99), None);
    }

    #[test]
    fn eviction_drops_the_oldest_pixels_but_keeps_tiles() {
        let mut s = Store::new(100);
        let first = s.add(bmp(4, 4), 1, 1);
        let t = s.tile(first, 0, 0);
        let second = s.add(bmp(4, 4), 1, 1);
        assert!(s.over_budget());
        assert!(s.evict_oldest());
        assert!(s.image(first).is_none());
        assert!(s.image(second).is_some());
        assert_eq!(s.lookup(t).map(|t| t.image), Some(first));
        assert_eq!(s.bytes(), 64);
    }

    #[test]
    fn retain_renumbers_live_tiles_and_frees_unused_images() {
        let mut s = Store::new(1 << 20);
        let a = s.add(bmp(2, 2), 1, 1);
        let b = s.add(bmp(2, 2), 1, 1);
        let ta = s.tile(a, 0, 0);
        let tb = s.tile(b, 0, 0);
        let map = s.retain(&HashSet::from([tb]));
        assert_eq!(map[ta as usize], 0);
        assert_eq!(map[tb as usize], 1);
        assert!(s.image(a).is_none());
        assert!(s.image(b).is_some());
        assert_eq!(s.lookup(1).map(|t| t.image), Some(b));
        assert_eq!(s.tile_count(), 1);
        assert_eq!(s.bytes(), 16);
        assert_eq!(s.tile(b, 0, 0), 1);
    }

    #[test]
    fn evicting_with_nothing_stored_reports_it() {
        assert!(!Store::new(0).evict_oldest());
    }

    #[test]
    fn the_total_follows_adds_evictions_retains_and_drops() {
        let base = total_bytes();
        let mut a = Store::new(1 << 20);
        let mut b = Store::new(1 << 20);
        let first = a.add(bmp(4, 4), 1, 1);
        b.add(bmp(2, 2), 1, 1);
        assert_eq!(total_bytes(), base + 64 + 16);
        let t = a.tile(first, 0, 0);
        a.add(bmp(4, 4), 1, 1);
        assert_eq!(total_bytes(), base + 64 + 64 + 16);
        assert!(a.evict_oldest());
        assert_eq!(total_bytes(), base + 64 + 16);
        a.retain(&HashSet::from([t]));
        assert_eq!(total_bytes(), base + 16);
        drop(b);
        assert_eq!(total_bytes(), base);
    }

    #[test]
    fn panes_together_cannot_pass_the_global_limit() {
        set_global_limit(total_bytes() + 100);
        let mut a = Store::new(1 << 20);
        let mut b = Store::new(1 << 20);
        a.add(bmp(4, 4), 1, 1);
        assert!(!a.over_budget());
        b.add(bmp(4, 4), 1, 1);
        assert!(a.over_budget() && b.over_budget(), "64 + 64 is over 100");
        assert!(b.evict_oldest());
        assert!(!a.over_budget() && !b.over_budget());
        set_global_limit(GLOBAL_BUDGET);
    }

    #[test]
    fn a_pane_without_pictures_is_never_over_the_global_limit() {
        set_global_limit(0);
        let empty = Store::new(1 << 20);
        assert!(!empty.over_budget());
        let mut full = Store::new(1 << 20);
        full.add(bmp(1, 1), 1, 1);
        assert!(full.over_budget());
        assert!(full.evict_oldest());
        assert!(!full.over_budget());
        set_global_limit(GLOBAL_BUDGET);
    }
}

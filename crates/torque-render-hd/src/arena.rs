//! The static geometry's cells ([`torque_render_scene::hd::cells`]) on the GPU in a few big buffers instead of a buffer pair a cell:
//!
//! - **Slabs** ([`Arena`]): pages of one vertex buffer and one index buffer each ([`PAGE_VERTICES`] and [`PAGE_INDICES`] at
//!   most). Each cell owns a slab of each in one page, from a deterministic best-fit allocator ([`RangeAlloc`]: the smallest free
//!   run that fits, the lowest place among equals, freed runs joined with their neighbours). A cell's indices stay relative to
//!   its first vertex: its draws carry the slab's place as their base vertex. A changed cell gets new slabs and its old ones are
//!   freed at once: the queue runs the frame that drew from them before the writes that reuse them (`write_buffer` lands before
//!   the next submit's commands, after the last one's). When a slab fits no page, the last page is made again larger with its
//!   live slabs packed at its start while it is under the page's most, else a page whose free room is in pieces is packed, else
//!   a new page of an eighth of the most is made, to grow in its turn (a big scene's one arena was made again at 1.4 GB in a
//!   frame of 30 ms).
//! - **The cluster table** ([`Table`]): one record a cluster (its box, its cell, its draw's indices and base vertex), kept in
//!   segments, each holding the clusters of one draw group (a texture slot and translucency). A group's segments double in size,
//!   64 to 16,384 slots, and never move; a freed slot is reused first (the lowest), and an empty segment is freed. The order of a
//!   group's draws is its segments' order, then the slots' order: the same in the lists the CPU writes and those the GPU writes.
//! - **Lists** ([`Lists`]): one indexed indirect draw a passing cluster, at its segment's place in the list, and a count a segment;
//!   drawn by one multi-draw a segment (with the device's indirect count; without it over the count the CPU knows, and without
//!   indirect drawing at all one draw a cluster).

use std::collections::{BTreeMap, BTreeSet};

use torque_render_core::wgpu;
use torque_render_scene::hd::HD_VERTEX_BYTES;

/// A best-fit allocator of runs in `0..capacity` (units of the caller's choosing), deterministic: the same calls give the same
/// places.
#[derive(Debug, Clone, Default)]
pub struct RangeAlloc {
    capacity: u64,
    /// The free runs by place, and by (length, place).
    free_at: BTreeMap<u64, u64>,
    free_by: BTreeSet<(u64, u64)>,
    used: u64,
}

impl RangeAlloc {
    pub fn new(capacity: u64) -> RangeAlloc {
        let mut a = RangeAlloc {
            capacity,
            ..RangeAlloc::default()
        };
        a.insert_free(0, capacity);
        a
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    pub fn used(&self) -> u64 {
        self.used
    }

    /// The longest free run.
    pub fn largest_free(&self) -> u64 {
        self.free_by.iter().next_back().map_or(0, |r| r.0)
    }

    fn insert_free(&mut self, at: u64, len: u64) {
        if len == 0 {
            return;
        }
        let mut at = at;
        let mut len = len;
        if let Some((&p, &pl)) = self.free_at.range(..at).next_back()
            && p + pl == at
        {
            self.free_at.remove(&p);
            self.free_by.remove(&(pl, p));
            at = p;
            len += pl;
        }
        if let Some(&nl) = self.free_at.get(&(at + len)) {
            self.free_at.remove(&(at + len));
            self.free_by.remove(&(nl, at + len));
            len += nl;
        }
        self.free_at.insert(at, len);
        self.free_by.insert((len, at));
    }

    /// A run of `len` (more than 0): its place, or None when no free run is that long.
    pub fn alloc(&mut self, len: u64) -> Option<u64> {
        if len == 0 {
            return None;
        }
        let &(l, at) = self.free_by.range((len, 0)..).next()?;
        self.free_by.remove(&(l, at));
        self.free_at.remove(&at);
        if l > len {
            self.free_at.insert(at + len, l - len);
            self.free_by.insert((l - len, at + len));
        }
        self.used += len;
        Some(at)
    }

    /// Gives back the run at `at` of `len` (one [`RangeAlloc::alloc`] returned).
    pub fn free(&mut self, at: u64, len: u64) {
        if len == 0 {
            return;
        }
        self.used = self.used.saturating_sub(len);
        self.insert_free(at, len);
    }

    /// The capacity raised to `capacity` (the new room free at the end).
    pub fn grow(&mut self, capacity: u64) {
        if capacity > self.capacity {
            let old = self.capacity;
            self.capacity = capacity;
            self.insert_free(old, capacity - old);
        }
    }

    /// Every run free again but `used` at the start, of a `capacity` (after the caller packed its runs there).
    pub fn packed(capacity: u64, used: u64) -> RangeAlloc {
        let mut a = RangeAlloc::new(capacity);
        if used > 0 {
            a.alloc(used);
        }
        a
    }
}

/// A cell's two slabs: their page, its vertices' place and count, its indices' place and count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slab {
    pub page: u32,
    pub vertex: u32,
    pub vertices: u32,
    pub index: u32,
    pub indices: u32,
}

/// The vertices and indices a new arena has room for.
const FIRST_VERTICES: u64 = 1 << 12;
const FIRST_INDICES: u64 = 3 << 11;

/// The most vertices and indices a page holds (252 MiB and 48 MiB: a quad's six indices to its four vertices).
pub const PAGE_VERTICES: u64 = 7 << 20;
pub const PAGE_INDICES: u64 = 12 << 20;

/// A page of slabs: its vertex and index buffers and their allocators.
pub struct Page {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    valloc: RangeAlloc,
    ialloc: RangeAlloc,
}

impl Page {
    fn fits(&self, nv: u64, ni: u64) -> bool {
        self.valloc.largest_free() >= nv && self.ialloc.largest_free() >= ni
    }

    fn bytes(&self) -> u64 {
        self.vertices.size() + self.indices.size()
    }
}

/// The cells' slabs.
pub struct Arena {
    pub pages: Vec<Page>,
    /// The most vertices and indices a page holds ([`PAGE_VERTICES`], [`PAGE_INDICES`]; the tests set fewer).
    pub page_vertices: u64,
    pub page_indices: u64,
    /// The live slabs by id (a cell keeps its id while its slabs move).
    slabs: Vec<Option<Slab>>,
    free_ids: BTreeSet<u32>,
    /// The times the buffers were made again (the slabs packed) since the arena was made.
    pub repacks: u64,
    /// The most bytes the vertex and index buffers may take together after a repack (the old pair lives beside the new one
    /// while it copies): past it the upload is refused, never the device's memory exhausted.
    pub budget: u64,
    /// The last page's next, larger buffers made on a thread of their own once it is three quarters full ([`AHEAD_BYTES`] or
    /// more; [`Arena::make_ahead`]): making a 243 MiB pair took 11 ms of a loading frame. Off: every buffer made when needed.
    pub ahead_on: bool,
    /// The least bytes of a pair made ahead ([`AHEAD_BYTES`]; the tests set less).
    pub ahead_bytes: u64,
    ahead: Option<Ahead>,
    /// The repacks that took buffers made ahead.
    pub repacks_ahead: u64,
}

/// The least bytes of a buffer pair made ahead (smaller ones are made in well under a millisecond).
pub const AHEAD_BYTES: u64 = 8 << 20;

/// A page's buffers made ahead: their vertices and indices, and the pair once the thread made it (None: the device refused).
struct Ahead {
    page: usize,
    vertices: u64,
    indices: u64,
    rx: std::sync::mpsc::Receiver<Option<(wgpu::Buffer, wgpu::Buffer)>>,
    made: Option<(wgpu::Buffer, wgpu::Buffer)>,
}

impl Ahead {
    /// Whether the thread has made the pair by now.
    fn ready(&mut self) -> bool {
        if self.made.is_none()
            && let Ok(Some(pair)) = self.rx.try_recv()
        {
            self.made = Some(pair);
        }
        self.made.is_some()
    }
}

fn vertex_buffer(device: &wgpu::Device, vertices: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hd static vertices"),
        size: vertices * HD_VERTEX_BYTES as u64,
        usage: wgpu::BufferUsages::VERTEX
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// The bytes of a cell's index: 16 bits, each relative to its cluster's first vertex (a cluster spans at most 4,096 vertices; the
/// record's base vertex is its first's). A slab's indices are an even count, so every place and copy is a whole 4 bytes.
pub const INDEX_BYTES: u64 = 2;

fn index_buffer(device: &wgpu::Device, indices: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hd static indices"),
        size: (indices * INDEX_BYTES).next_multiple_of(4),
        usage: wgpu::BufferUsages::INDEX
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

impl Arena {
    pub fn new(device: &wgpu::Device) -> Arena {
        Arena {
            pages: vec![Page {
                vertices: vertex_buffer(device, FIRST_VERTICES),
                indices: index_buffer(device, FIRST_INDICES),
                valloc: RangeAlloc::new(FIRST_VERTICES),
                ialloc: RangeAlloc::new(FIRST_INDICES),
            }],
            page_vertices: PAGE_VERTICES,
            page_indices: PAGE_INDICES,
            slabs: Vec::new(),
            free_ids: BTreeSet::new(),
            repacks: 0,
            budget: ARENA_BUDGET,
            ahead_on: true,
            ahead_bytes: AHEAD_BYTES,
            ahead: None,
            repacks_ahead: 0,
        }
    }

    /// The last page's next buffers started on a thread when it is three quarters full and under a page's most: the size its
    /// growth by half makes (`Arena::repack`'s), once that is at least [`AHEAD_BYTES`] and within the budget. Called after a
    /// frame's submit: the device's allocator is locked while the thread makes them, and a write that needs a staging buffer
    /// waits for it (made in the upload, a 204 MiB pair held its cell puts 8 ms).
    pub fn make_ahead(&mut self, device: &wgpu::Device) {
        if !self.ahead_on {
            return;
        }
        let last = self.pages.len() - 1;
        if self.ahead.as_ref().is_some_and(|a| a.page == last) {
            return;
        }
        self.ahead = None;
        let p = &self.pages[last];
        let (vcap, icap) = (p.valloc.capacity(), p.ialloc.capacity());
        let full = p.valloc.used() * 4 >= vcap * 3 || p.ialloc.used() * 4 >= icap * 3;
        if !full || (vcap >= self.page_vertices && icap >= self.page_indices) {
            return;
        }
        let max = device.limits().max_buffer_size;
        let vertices = (vcap * 3 / 2).min(self.page_vertices.max(vcap));
        let indices = (icap * 3 / 2)
            .min(self.page_indices.max(icap))
            .next_multiple_of(2);
        let bytes = vertices * HD_VERTEX_BYTES as u64 + indices * INDEX_BYTES;
        if bytes < self.ahead_bytes
            || vertices * HD_VERTEX_BYTES as u64 > max
            || indices * INDEX_BYTES > max
            || self.bytes() + bytes > self.budget
        {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let device = device.clone();
        let spawned = std::thread::Builder::new()
            .name("hd arena ahead".into())
            .spawn(move || {
                // An allocation the device cannot make is no pair: the repack then makes its own (refused as before).
                let scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
                let pair = (
                    vertex_buffer(&device, vertices),
                    index_buffer(&device, indices),
                );
                let ok = !out_of_memory(scope);
                let _ = tx.send(ok.then_some(pair));
            });
        if spawned.is_ok() {
            self.ahead = Some(Ahead {
                page: last,
                vertices,
                indices,
                rx,
                made: None,
            });
        }
    }

    /// The GPU bytes of the slabs' buffers.
    pub fn bytes(&self) -> u64 {
        self.pages.iter().map(Page::bytes).sum()
    }

    /// The bytes the live slabs hold.
    pub fn used_bytes(&self) -> u64 {
        self.pages
            .iter()
            .map(|p| p.valloc.used() * HD_VERTEX_BYTES as u64 + p.ialloc.used() * INDEX_BYTES)
            .sum()
    }

    /// Page `page`'s vertex and index buffers.
    pub fn page_buffers(&self, page: u32) -> Option<(&wgpu::Buffer, &wgpu::Buffer)> {
        self.pages
            .get(page as usize)
            .map(|p| (&p.vertices, &p.indices))
    }

    /// Page `page`'s buffers bound for drawing (its vertices at the given vertex buffer slots, its indices).
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>, page: u32, slots: &[u32]) -> bool {
        let Some((v, i)) = self.page_buffers(page) else {
            return false;
        };
        for &s in slots {
            pass.set_vertex_buffer(s, v.slice(..));
        }
        pass.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint16);
        true
    }

    pub fn slab(&self, id: u32) -> Option<Slab> {
        self.slabs.get(id as usize).copied().flatten()
    }

    /// Slab `id`'s vertices from its `first` on (whole vertices, within the slab).
    pub fn write_vertices(&self, queue: &wgpu::Queue, id: u32, first: u64, bytes: &[u8]) {
        let Some(s) = self.slab(id) else { return };
        let Some(page) = self.pages.get(s.page as usize) else {
            return;
        };
        let n = (bytes.len() / HD_VERTEX_BYTES) as u64;
        if bytes.is_empty() || first + n > u64::from(s.vertices) {
            return;
        }
        let at = (u64::from(s.vertex) + first) * HD_VERTEX_BYTES as u64;
        queue.write_buffer(&page.vertices, at, &bytes[..n as usize * HD_VERTEX_BYTES]);
    }

    /// Slab `id`'s indices from its `first` on (within the slab; `first` and the count even, a slab's last odd index padded).
    pub fn write_indices(&self, queue: &wgpu::Queue, id: u32, first: u64, indices: &[u16]) {
        let Some(s) = self.slab(id) else { return };
        let Some(page) = self.pages.get(s.page as usize) else {
            return;
        };
        if indices.is_empty()
            || !first.is_multiple_of(2)
            || first + indices.len() as u64 > u64::from(s.indices)
        {
            return;
        }
        let mut bytes: Vec<u8> = indices.iter().flat_map(|x| x.to_le_bytes()).collect();
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        queue.write_buffer(
            &page.indices,
            (u64::from(s.index) + first) * INDEX_BYTES,
            &bytes,
        );
    }

    /// The live slabs' ids.
    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.slabs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_some())
            .map(|(i, _)| i as u32)
    }

    /// A slab pair for `vertices` (their bytes, [`HD_VERTEX_BYTES`] each) and `indices`, written now: its id, and whether the
    /// buffers were made again to fit it (every slab moved: the caller reads their places again). None when the device cannot
    /// hold a buffer that large.
    #[cfg(test)]
    pub fn put(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[u8],
        indices: &[u16],
    ) -> Option<(u32, bool)> {
        let nv = (vertices.len() / HD_VERTEX_BYTES) as u64;
        let ni = indices.len() as u64;
        let (id, moved) = self.alloc(device, queue, nv, ni)?;
        self.write_vertices(queue, id, 0, vertices);
        self.write_indices(queue, id, 0, indices);
        Some((id, moved))
    }

    /// A slab of `nv` vertices and `ni` indices, nothing written in it yet (and whether others moved to make room): in the
    /// first page with the room; else the last page made larger, a page packed, or a new page (None: more than a page holds,
    /// or past the budget or the device).
    pub fn alloc(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        nv: u64,
        ni: u64,
    ) -> Option<(u32, bool)> {
        // An even count of indices (each place and copy a whole 4 bytes).
        let ni = ni.next_multiple_of(2);
        if nv == 0 || ni == 0 || nv > self.page_vertices || ni > self.page_indices {
            return None;
        }
        let mut moved = false;
        let mut page = self.pages.iter().position(|p| p.fits(nv, ni));
        if page.is_none() {
            page = self.make_room(device, queue, nv, ni);
            moved = page.is_some();
        }
        let page = page?;
        let p = &mut self.pages[page];
        let (v, i) = (p.valloc.alloc(nv)?, p.ialloc.alloc(ni));
        let Some(i) = i else {
            p.valloc.free(v, nv);
            return None;
        };
        let slab = Slab {
            page: page as u32,
            vertex: v as u32,
            vertices: nv as u32,
            index: i as u32,
            indices: ni as u32,
        };
        let id = match self.free_ids.pop_first() {
            Some(id) => {
                self.slabs[id as usize] = Some(slab);
                id
            }
            None => {
                self.slabs.push(Some(slab));
                (self.slabs.len() - 1) as u32
            }
        };
        Some((id, moved))
    }

    /// A page with room for a slab of `nv` vertices and `ni` indices: the last page made larger while it is under a page's most,
    /// else a page whose free room is enough but in pieces packed, else a new page of the most.
    fn make_room(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        nv: u64,
        ni: u64,
    ) -> Option<usize> {
        let last = self.pages.len() - 1;
        let (pv, pi) = (self.page_vertices, self.page_indices);
        let under = {
            let p = &self.pages[last];
            p.valloc.capacity() < pv || p.ialloc.capacity() < pi
        };
        if under && self.repack(device, queue, last, nv, ni, true) && self.pages[last].fits(nv, ni)
        {
            return Some(last);
        }
        let pieces = self.pages.iter().position(|p| {
            p.valloc.capacity() - p.valloc.used() >= nv
                && p.ialloc.capacity() - p.ialloc.used() >= ni
        });
        if let Some(k) = pieces
            && self.repack(device, queue, k, nv, ni, false)
            && self.pages[k].fits(nv, ni)
        {
            return Some(k);
        }
        // A new page, an eighth of the most (or the slab) at first, grown as the last page as it fills (a big scene's last
        // page held a fraction of its 252 MiB); an allocation the device cannot make is a refused upload, not a lost device.
        let (fv, fi) = (
            (pv / 8).max(nv).min(pv),
            (pi / 8).max(ni).min(pi).next_multiple_of(2),
        );
        if self.bytes() + fv * HD_VERTEX_BYTES as u64 + fi * INDEX_BYTES > self.budget {
            return None;
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let page = Page {
            vertices: vertex_buffer(device, fv),
            indices: index_buffer(device, fi),
            valloc: RangeAlloc::new(fv),
            ialloc: RangeAlloc::new(fi),
        };
        if out_of_memory(scope) {
            return None;
        }
        self.pages.push(page);
        Some(self.pages.len() - 1)
    }

    /// Room made for `nv` more vertices and `ni` more indices (an upload's cells): the last page made larger once, up to a page's
    /// most, when no page has the room; past it [`Arena::alloc`] makes new pages a cell at a time. False when the device cannot
    /// hold it.
    pub fn reserve(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        nv: u64,
        ni: u64,
    ) -> bool {
        if self.pages.iter().any(|p| p.fits(nv, ni)) {
            return true;
        }
        let last = self.pages.len() - 1;
        let p = &self.pages[last];
        if p.valloc.capacity() >= self.page_vertices && p.ialloc.capacity() >= self.page_indices {
            return true;
        }
        let grow = p.valloc.used() > 0;
        let (nv, ni) = (
            nv.min(self.page_vertices.saturating_sub(p.valloc.used())),
            ni.min(self.page_indices.saturating_sub(p.ialloc.used())),
        );
        self.repack(device, queue, last, nv, ni, grow)
    }

    /// Frees a slab pair.
    pub fn free(&mut self, id: u32) {
        if let Some(s) = self.slabs.get_mut(id as usize).and_then(Option::take) {
            if let Some(p) = self.pages.get_mut(s.page as usize) {
                p.valloc.free(u64::from(s.vertex), u64::from(s.vertices));
                p.ialloc.free(u64::from(s.index), u64::from(s.indices));
            }
            self.free_ids.insert(id);
        }
    }

    /// Page `page`'s buffers made again with its live slabs packed at the start (in the order of their places) and room for `nv`
    /// more vertices and `ni` more indices: the same size when that fits and a quarter or more was free, else the live slabs and
    /// the new ones and a sixteenth more (`grow`: at least half again the old size, for a cell at a time), at most a page's most.
    fn repack(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        page: usize,
        nv: u64,
        ni: u64,
        grow: bool,
    ) -> bool {
        let max = device.limits().max_buffer_size;
        let size = |alloc: &RangeAlloc, need: u64, unit: u64, most: u64| -> Option<u64> {
            let (cap, used) = (alloc.capacity(), alloc.used());
            let fits = used + need <= cap && (cap - used) * 4 >= cap;
            let want = if fits {
                cap
            } else {
                let tight = if used == 0 {
                    need
                } else {
                    (used + need) * 17 / 16
                };
                // Grown (not packed to take less where only the free room was in pieces): half again at least.
                if grow && used + need > cap {
                    tight.max(cap * 3 / 2)
                } else {
                    tight
                }
            };
            let want = want.min(most.max(cap));
            (want * unit <= max).then_some(want)
        };
        let (pv, pi) = (self.page_vertices, self.page_indices);
        let Some(old) = self.pages.get(page) else {
            return false;
        };
        let (Some(vcap), Some(icap)) = (
            size(&old.valloc, nv, HD_VERTEX_BYTES as u64, pv),
            size(&old.ialloc, ni.next_multiple_of(2), INDEX_BYTES, pi),
        ) else {
            return false;
        };
        let others = self.bytes() - old.bytes();
        let icap = icap.next_multiple_of(2);
        if others + vcap * HD_VERTEX_BYTES as u64 + icap * INDEX_BYTES > self.budget {
            return false;
        }
        // The pair made ahead when it is large enough and within the budget; else one made now.
        let ahead = self.ahead.as_mut().is_some_and(|a| {
            a.page == page
                && a.vertices >= vcap
                && a.indices >= icap
                && others + a.vertices * HD_VERTEX_BYTES as u64 + a.indices * INDEX_BYTES
                    <= self.budget
                && a.ready()
        });
        let made = if ahead {
            self.ahead
                .take()
                .and_then(|a| Some((a.made?, a.vertices, a.indices)))
        } else {
            None
        };
        let (vertices, indices, vcap, icap) = match made {
            Some(((v, i), av, ai)) => {
                self.repacks_ahead += 1;
                (v, i, av, ai)
            }
            None => {
                // An allocation the device cannot make is a refused upload, not a lost device.
                let scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
                let vertices = vertex_buffer(device, vcap);
                let indices = index_buffer(device, icap);
                if out_of_memory(scope) {
                    return false;
                }
                (vertices, indices, vcap, icap)
            }
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("hd static arena repack"),
        });
        // Each buffer's live runs in place order, copied to their packed places (neighbours joined into one copy).
        let mut order: Vec<u32> = self
            .ids()
            .filter(|&id| self.slabs[id as usize].is_some_and(|s| s.page as usize == page))
            .collect();
        let mut packed = |order: &mut Vec<u32>,
                          slabs: &mut Vec<Option<Slab>>,
                          from: &wgpu::Buffer,
                          to: &wgpu::Buffer,
                          unit: u64,
                          get: fn(&Slab) -> (u32, u32),
                          set: fn(&mut Slab, u32)|
         -> u64 {
            order.sort_by_key(|&id| slabs[id as usize].map_or(0, |s| get(&s).0));
            let mut at = 0u64;
            let mut run: Option<(u64, u64, u64)> = None;
            for &id in order.iter() {
                let Some(s) = slabs[id as usize].as_mut() else {
                    continue;
                };
                let (place, len) = get(s);
                let (place, len) = (u64::from(place), u64::from(len));
                match &mut run {
                    Some((src, dst, n)) if *src + *n == place => *n += len,
                    _ => {
                        if let Some((src, dst, n)) = run.take() {
                            encoder.copy_buffer_to_buffer(
                                from,
                                src * unit,
                                to,
                                dst * unit,
                                n * unit,
                            );
                        }
                        run = Some((place, at, len));
                    }
                }
                set(s, at as u32);
                at += len;
            }
            if let Some((src, dst, n)) = run {
                encoder.copy_buffer_to_buffer(from, src * unit, to, dst * unit, n * unit);
            }
            at
        };
        let old = &self.pages[page];
        let vused = packed(
            &mut order,
            &mut self.slabs,
            &old.vertices,
            &vertices,
            HD_VERTEX_BYTES as u64,
            |s| (s.vertex, s.vertices),
            |s, at| s.vertex = at,
        );
        let iused = packed(
            &mut order,
            &mut self.slabs,
            &old.indices,
            &indices,
            INDEX_BYTES,
            |s| (s.index, s.indices),
            |s, at| s.index = at,
        );
        queue.submit([encoder.finish()]);
        self.pages[page] = Page {
            vertices,
            indices,
            valloc: RangeAlloc::packed(vcap, vused),
            ialloc: RangeAlloc::packed(icap, iused),
        };
        self.repacks += 1;
        // A pair made ahead for this page at its old size is of no use now.
        if self.ahead.as_ref().is_some_and(|a| a.page == page) {
            self.ahead = None;
        }
        true
    }
}

/// The arena's default byte budget ([`Arena::budget`]): 4 GiB for its vertex and index buffers together.
pub const ARENA_BUDGET: u64 = 4 << 30;

/// The most clusters the table holds (the compaction's two levels of 1,024).
pub const MAX_CLUSTERS: u32 = 1 << 20;
/// The most segments of the table.
pub const MAX_SEGMENTS: u32 = 4096;
/// A group's first segment's slots, and the most of any segment.
const FIRST_SEGMENT: u32 = 64;
const LAST_SEGMENT: u32 = 16384;
/// The slots a new table has room for.
const FIRST_SLOTS: u32 = 4096;

/// A draw group: translucency, then the texture slot (the order the groups are drawn in).
pub type GroupKey = (bool, u32, u32);

/// A record's flags.
pub const CLUSTER_LIVE: u32 = 1;
pub const CLUSTER_TRANSLUCENT: u32 = 2;
/// A cluster drawn only within a view's near distance (by a view that draws them: [`crate::cull::VIEW_NEAR`]).
pub const CLUSTER_NEAR: u32 = 4;

/// One cluster as the GPU reads it (`cull.wgsl`'s `Cluster`, 48 bytes).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Record {
    pub lo: [f32; 3],
    pub cell: u32,
    pub hi: [f32; 3],
    pub segment: u32,
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    pub flags: u32,
}

pub const RECORD_BYTES: u64 = 48;

impl Record {
    fn write(&self, out: &mut Vec<u8>) {
        for v in self.lo {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.cell.to_le_bytes());
        for v in self.hi {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for w in [
            self.segment,
            self.first_index,
            self.index_count,
            self.base_vertex as u32,
            self.flags,
        ] {
            out.extend_from_slice(&w.to_le_bytes());
        }
    }
}

/// A segment of the table: its slots `first .. first + len`, its group, its live records, the slots handed out so far and the
/// freed ones below that.
#[derive(Debug, Clone)]
pub struct Segment {
    pub first: u32,
    pub len: u32,
    pub group: GroupKey,
    live: u32,
    next: u32,
    holes: BTreeSet<u32>,
}

/// The cluster table.
pub struct Table {
    alloc: RangeAlloc,
    pub buffer: wgpu::Buffer,
    /// The records as written (the CPU's lists read them; `capacity` of them).
    pub mirror: Vec<Record>,
    segments: Vec<Option<Segment>>,
    groups: BTreeMap<GroupKey, Vec<u32>>,
    /// The slots changed since the last flush, and whether the buffer must be made again (the table grew).
    dirty: BTreeSet<u32>,
    regrown: bool,
    /// Clusters a full table could not take.
    pub refused: u64,
}

fn record_buffer(device: &wgpu::Device, slots: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hd static clusters"),
        size: u64::from(slots) * RECORD_BYTES,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Table {
    pub fn new(device: &wgpu::Device) -> Table {
        Table {
            alloc: RangeAlloc::new(u64::from(FIRST_SLOTS)),
            buffer: record_buffer(device, FIRST_SLOTS),
            mirror: vec![Record::default(); FIRST_SLOTS as usize],
            segments: Vec::new(),
            groups: BTreeMap::new(),
            dirty: BTreeSet::new(),
            regrown: false,
            refused: 0,
        }
    }

    /// The table's slots (the lists' capacity).
    pub fn capacity(&self) -> u32 {
        self.mirror.len() as u32
    }

    pub fn live(&self) -> u64 {
        self.segments
            .iter()
            .flatten()
            .map(|s| u64::from(s.live))
            .sum()
    }

    pub fn segment(&self, id: u32) -> Option<&Segment> {
        self.segments.get(id as usize).and_then(Option::as_ref)
    }

    /// The highest segment id in use, plus one.
    pub fn segment_ids(&self) -> u32 {
        self.segments.len() as u32
    }

    /// The groups in draw order, each with its segments' ids in order.
    pub fn groups(&self) -> impl Iterator<Item = (&GroupKey, &Vec<u32>)> {
        self.groups.iter()
    }

    /// The segments of one group in order.
    pub fn group(&self, key: GroupKey) -> &[u32] {
        self.groups.get(&key).map_or(&[], Vec::as_slice)
    }

    /// Puts a cluster's record in its group (the record's segment is filled in): its slot, or None when the table is full.
    pub fn insert(&mut self, group: GroupKey, mut record: Record) -> Option<u32> {
        let ids = self.groups.get(&group).cloned().unwrap_or_default();
        for id in &ids {
            let Some(seg) = self.segments[*id as usize].as_mut() else {
                continue;
            };
            let rel = if let Some(h) = seg.holes.pop_first() {
                Some(h)
            } else if seg.next < seg.len {
                seg.next += 1;
                Some(seg.next - 1)
            } else {
                None
            };
            if let Some(rel) = rel {
                seg.live += 1;
                let slot = seg.first + rel;
                record.segment = *id;
                self.put(slot, record);
                return Some(slot);
            }
        }
        // A new segment for the group, twice its last one.
        let len = (FIRST_SEGMENT << ids.len().min(10)).min(LAST_SEGMENT);
        let id = match self.segments.iter().position(Option::is_none) {
            Some(i) => i as u32,
            None if (self.segments.len() as u32) < MAX_SEGMENTS => {
                self.segments.push(None);
                (self.segments.len() - 1) as u32
            }
            None => {
                self.refused += 1;
                return None;
            }
        };
        let first = match self.alloc.alloc(u64::from(len)) {
            Some(f) => f,
            None => {
                let cap = self.alloc.capacity();
                let want = (cap * 2)
                    .max(cap + u64::from(len))
                    .min(u64::from(MAX_CLUSTERS));
                self.alloc.grow(want);
                match self.alloc.alloc(u64::from(len)) {
                    Some(f) => {
                        self.mirror.resize(want as usize, Record::default());
                        self.regrown = true;
                        f
                    }
                    None => {
                        self.refused += 1;
                        if self.segments.last().is_some_and(Option::is_none) {
                            self.segments.pop();
                        }
                        return None;
                    }
                }
            }
        } as u32;
        self.segments[id as usize] = Some(Segment {
            first,
            len,
            group,
            live: 1,
            next: 1,
            holes: BTreeSet::new(),
        });
        self.groups.entry(group).or_default().push(id);
        record.segment = id;
        self.put(first, record);
        Some(first)
    }

    fn put(&mut self, slot: u32, record: Record) {
        self.mirror[slot as usize] = record;
        self.dirty.insert(slot);
    }

    /// Changes a live record in place (its slab moved).
    pub fn update(&mut self, slot: u32, f: impl FnOnce(&mut Record)) {
        if let Some(r) = self.mirror.get_mut(slot as usize) {
            f(r);
            self.dirty.insert(slot);
        }
    }

    /// Frees a cluster's slot (its segment too when that was its last).
    pub fn remove(&mut self, slot: u32) {
        let Some(r) = self.mirror.get(slot as usize).copied() else {
            return;
        };
        if r.flags & CLUSTER_LIVE == 0 {
            return;
        }
        self.put(slot, Record::default());
        let id = r.segment;
        let Some(seg) = self.segments.get_mut(id as usize).and_then(Option::as_mut) else {
            return;
        };
        seg.live -= 1;
        seg.holes.insert(slot - seg.first);
        if seg.live == 0 {
            let seg = self.segments[id as usize].take().expect("a segment");
            self.alloc.free(u64::from(seg.first), u64::from(seg.len));
            if let Some(list) = self.groups.get_mut(&seg.group) {
                list.retain(|&s| s != id);
                if list.is_empty() {
                    self.groups.remove(&seg.group);
                }
            }
            while self.segments.last().is_some_and(Option::is_none) {
                self.segments.pop();
            }
        }
    }

    /// The changed records to the GPU (the buffer made again when the table grew); whether it was made again.
    pub fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let regrown = std::mem::take(&mut self.regrown);
        if regrown {
            self.buffer = record_buffer(device, self.capacity());
            let mut bytes = Vec::with_capacity(self.mirror.len() * RECORD_BYTES as usize);
            for r in &self.mirror {
                r.write(&mut bytes);
            }
            queue.write_buffer(&self.buffer, 0, &bytes);
            self.dirty.clear();
            return true;
        }
        let dirty = std::mem::take(&mut self.dirty);
        let mut run: Option<(u32, u32)> = None;
        let write = |(first, end): (u32, u32)| {
            let mut bytes = Vec::with_capacity((end - first) as usize * RECORD_BYTES as usize);
            for r in &self.mirror[first as usize..end as usize] {
                r.write(&mut bytes);
            }
            queue.write_buffer(&self.buffer, u64::from(first) * RECORD_BYTES, &bytes);
        };
        for s in dirty {
            run = match run {
                Some((a, b)) if b == s => Some((a, s + 1)),
                Some(r) => {
                    write(r);
                    Some((s, s + 1))
                }
                None => Some((s, s + 1)),
            };
        }
        if let Some(r) = run {
            write(r);
        }
        false
    }
}

/// How a list is drawn: one multi-draw a segment with the count the GPU reads, one over the count the CPU knows, or one draw a
/// cluster (no indirect drawing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawWay {
    Count,
    Multi,
    Direct,
}

impl DrawWay {
    /// The device's way: indirect drawing on DX12, Vulkan and Metal (the backends that always execute it), with the count where
    /// the device has `MULTI_DRAW_INDIRECT_COUNT`.
    pub fn of(device: &wgpu::Device) -> DrawWay {
        let backend = device.adapter_info().backend;
        if !matches!(
            backend,
            wgpu::Backend::Dx12 | wgpu::Backend::Vulkan | wgpu::Backend::Metal
        ) {
            DrawWay::Direct
        } else if device
            .features()
            .contains(wgpu::Features::MULTI_DRAW_INDIRECT_COUNT)
        {
            DrawWay::Count
        } else {
            DrawWay::Multi
        }
    }
}

/// A view's draw lists: the indirect draws at their segments' places, a count a segment, and the CPU's copy of what it wrote (or
/// `gpu`: the GPU's cull wrote them, and only the GPU knows the counts).
pub struct Lists {
    pub args: wgpu::Buffer,
    pub counts: wgpu::Buffer,
    capacity: u32,
    pub gpu: bool,
    /// The draws the CPU wrote, by segment id (each five words).
    pub cpu: Vec<Vec<[u32; 5]>>,
}

fn args_buffer(device: &wgpu::Device, slots: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hd static lists"),
        size: u64::from(slots.max(1)) * 20,
        usage: wgpu::BufferUsages::INDIRECT
            | wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Lists {
    pub fn new(device: &wgpu::Device) -> Lists {
        Lists {
            args: args_buffer(device, FIRST_SLOTS),
            counts: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hd static list counts"),
                // A count a segment (a whole list: a count a page, then each segment's count and place in its page's list).
                size: u64::from(MAX_SEGMENTS) * 12,
                usage: wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: FIRST_SLOTS,
            gpu: false,
            cpu: Vec::new(),
        }
    }

    /// Room for a table of `slots` (the GPU's cull writes the lists: the CPU's copy is dropped).
    pub fn for_gpu(&mut self, device: &wgpu::Device, slots: u32) {
        if self.capacity < slots {
            self.capacity = slots;
            self.args = args_buffer(device, self.capacity);
        }
        self.gpu = true;
        self.cpu.clear();
    }

    pub fn bytes(&self) -> u64 {
        self.args.size() + self.counts.size()
    }

    /// The lists written by the CPU: each live record `pass` lets through, at its segment's place in slot order
    /// (`first_instance`: the slot is each draw's first instance, else 0).
    pub fn write_cpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        table: &Table,
        first_instance: bool,
        mut pass: impl FnMut(&Record) -> bool,
    ) {
        self.gpu = false;
        if self.capacity < table.capacity() {
            self.capacity = table.capacity();
            self.args = args_buffer(device, self.capacity);
        }
        let n = table.segment_ids() as usize;
        self.cpu.resize(n, Vec::new());
        self.cpu.truncate(n);
        let mut counts = vec![0u32; n.max(1)];
        for (id, (list, count)) in self.cpu.iter_mut().zip(counts.iter_mut()).enumerate() {
            list.clear();
            let Some(seg) = table.segment(id as u32) else {
                continue;
            };
            for slot in seg.first..seg.first + seg.next {
                let r = &table.mirror[slot as usize];
                if r.flags & CLUSTER_LIVE != 0 && pass(r) {
                    list.push([
                        r.index_count,
                        1,
                        r.first_index,
                        r.base_vertex as u32,
                        if first_instance { slot } else { 0 },
                    ]);
                }
            }
            *count = list.len() as u32;
            if !list.is_empty() {
                let bytes: Vec<u8> = list
                    .iter()
                    .flatten()
                    .flat_map(|w| w.to_le_bytes())
                    .collect();
                queue.write_buffer(&self.args, u64::from(seg.first) * 20, &bytes);
            }
        }
        let bytes: Vec<u8> = counts.iter().flat_map(|w| w.to_le_bytes()).collect();
        queue.write_buffer(&self.counts, 0, &bytes);
    }

    /// Draws one group's segments (the arena's buffers bound): the multi-draws or draws issued.
    pub fn draw_group(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        table: &Table,
        group: GroupKey,
        way: DrawWay,
    ) -> usize {
        let mut calls = 0;
        for &id in table.group(group) {
            if self.gpu {
                let Some(seg) = table.segment(id) else {
                    continue;
                };
                pass.multi_draw_indexed_indirect_count(
                    &self.args,
                    u64::from(seg.first) * 20,
                    &self.counts,
                    u64::from(id) * 4,
                    seg.len,
                );
                calls += 1;
                continue;
            }
            let (Some(seg), Some(cpu)) = (table.segment(id), self.cpu.get(id as usize)) else {
                continue;
            };
            if cpu.is_empty() {
                continue;
            }
            match way {
                DrawWay::Count => pass.multi_draw_indexed_indirect_count(
                    &self.args,
                    u64::from(seg.first) * 20,
                    &self.counts,
                    u64::from(id) * 4,
                    seg.len,
                ),
                DrawWay::Multi => pass.multi_draw_indexed_indirect(
                    &self.args,
                    u64::from(seg.first) * 20,
                    cpu.len() as u32,
                ),
                DrawWay::Direct => {
                    for d in cpu {
                        pass.draw_indexed(d[2]..d[2] + d[0], d[3] as i32, d[4]..d[4] + 1);
                    }
                    calls += cpu.len() - 1;
                }
            }
            calls += 1;
        }
        crate::count_draws(calls);
        calls
    }

    /// Draws one group's clusters `pass_test` lets through without lists, one draw a cluster in the lists' order (the arena's
    /// buffers bound; a view the CPU culls: the reference, and a view the frame had no cull slot for): the draws issued.
    pub fn draw_group_direct(
        pass: &mut wgpu::RenderPass<'_>,
        table: &Table,
        group: GroupKey,
        first_instance: bool,
        mut pass_test: impl FnMut(&Record) -> bool,
    ) -> usize {
        let mut calls = 0;
        for &id in table.group(group) {
            let Some(seg) = table.segment(id) else {
                continue;
            };
            for slot in seg.first..seg.first + seg.next {
                let r = &table.mirror[slot as usize];
                if r.flags & CLUSTER_LIVE != 0 && pass_test(r) {
                    let i = if first_instance { slot } else { 0 };
                    pass.draw_indexed(
                        r.first_index..r.first_index + r.index_count,
                        r.base_vertex,
                        i..i + 1,
                    );
                    calls += 1;
                }
            }
        }
        crate::count_draws(calls);
        calls
    }

    /// Whether any of a group's segments has a draw (with the GPU's lists: whether it has a cluster).
    pub fn group_drawn(&self, table: &Table, group: GroupKey) -> bool {
        if self.gpu {
            return !table.group(group).is_empty();
        }
        table
            .group(group)
            .iter()
            .any(|&id| self.cpu.get(id as usize).is_some_and(|l| !l.is_empty()))
    }
}

impl Lists {
    /// The CPU's copies' bytes (the draws it wrote).
    pub fn cpu_bytes(&self) -> u64 {
        (self.cpu.capacity() * std::mem::size_of::<Vec<[u32; 5]>>()
            + self.cpu.iter().map(|l| l.capacity() * 20).sum::<usize>()) as u64
    }
}

impl Table {
    /// The CPU's copies' bytes (the records' mirror, the segments, the groups, the dirty slots).
    pub fn cpu_bytes(&self) -> u64 {
        (self.mirror.capacity() * std::mem::size_of::<Record>()
            + self.segments.capacity() * std::mem::size_of::<Option<Segment>>()
            + self
                .groups
                .values()
                .map(|g| g.capacity() * 4 + 64)
                .sum::<usize>()
            + self.dirty.len() * 24) as u64
    }
}

/// Whether the allocations made under `scope` (an out-of-memory error scope) failed. A browser answers only on its own event
/// loop, which cannot be waited for: there the scope is closed unread (a failure is then the device's uncaptured error).
fn out_of_memory(scope: wgpu::ErrorScopeGuard) -> bool {
    if cfg!(target_arch = "wasm32") {
        drop(scope);
        return false;
    }
    pollster::block_on(scope.pop()).is_some()
}

#[cfg(test)]
#[path = "arena_tests.rs"]
mod tests;

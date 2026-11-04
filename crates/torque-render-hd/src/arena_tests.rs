use super::*;

#[test]
fn the_allocator_fits_best_and_joins_freed_runs() {
    let mut a = RangeAlloc::new(100);
    let x = a.alloc(10).unwrap();
    let y = a.alloc(20).unwrap();
    let z = a.alloc(30).unwrap();
    assert_eq!((x, y, z), (0, 10, 30));
    a.free(y, 20);
    // Best fit: the 20-run hole, not the 40 at the end.
    assert_eq!(a.alloc(15), Some(10));
    assert_eq!(a.alloc(5), Some(25));
    a.free(x, 10);
    a.free(10, 15);
    a.free(25, 5);
    // 0..30 joined again.
    assert_eq!(a.alloc(30), Some(0));
    assert_eq!(a.used(), 60);
    assert_eq!(a.largest_free(), 40);
    assert_eq!(a.alloc(41), None);
    a.grow(200);
    assert_eq!(a.alloc(141), None);
    assert_eq!(a.alloc(140), Some(60));
    assert_eq!(a.alloc(0), None);
}

#[test]
fn the_allocator_is_deterministic_and_never_overlaps() {
    let run = || {
        let mut a = RangeAlloc::new(1 << 16);
        let mut live: Vec<(u64, u64)> = Vec::new();
        let mut s = 12345u64;
        let mut out = Vec::new();
        for _ in 0..20_000 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            if live.is_empty() || !s.is_multiple_of(3) {
                let len = 1 + s % 300;
                if let Some(at) = a.alloc(len) {
                    live.push((at, len));
                    out.push(at);
                }
            } else {
                let (at, len) = live.swap_remove((s as usize / 3) % live.len());
                a.free(at, len);
            }
        }
        let mut sorted = live.clone();
        sorted.sort_unstable();
        assert!(
            sorted.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0),
            "overlap"
        );
        assert_eq!(a.used(), live.iter().map(|l| l.1).sum::<u64>());
        out
    };
    assert_eq!(run(), run());
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    match torque_render_core::Gpu::new() {
        Ok(r) => Some((r.device().clone(), r.queue().clone())),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn record(n: u32) -> Record {
    Record {
        first_index: n,
        index_count: 6,
        flags: CLUSTER_LIVE,
        ..Record::default()
    }
}

#[test]
fn the_table_keeps_groups_in_segments_and_reuses_the_lowest_slot() {
    let Some((device, _)) = device() else { return };
    let mut t = Table::new(&device);
    let a: Vec<u32> = (0..100)
        .map(|n| t.insert((false, 0, 0), record(n)).unwrap())
        .collect();
    let b: Vec<u32> = (0..10)
        .map(|n| t.insert((true, 3, 0), record(n)).unwrap())
        .collect();
    // Group (false, 0): a segment of 64, then one of 128.
    let segs = t.group((false, 0, 0)).to_vec();
    assert_eq!(segs.len(), 2);
    assert_eq!(t.segment(segs[0]).unwrap().len, 64);
    assert_eq!(t.segment(segs[1]).unwrap().len, 128);
    assert!(a[..64].windows(2).all(|w| w[1] == w[0] + 1));
    assert!(
        b.iter()
            .all(|s| t.mirror[*s as usize].segment == t.group((true, 3, 0))[0])
    );
    // A freed slot is the next one used; a segment emptied is freed.
    t.remove(a[5]);
    t.remove(a[9]);
    assert_eq!(t.insert((false, 0, 0), record(1000)), Some(a[5]));
    assert_eq!(t.insert((false, 0, 0), record(1001)), Some(a[9]));
    for s in &b {
        t.remove(*s);
    }
    assert!(t.group((true, 3, 0)).is_empty());
    assert_eq!(t.live(), 100);
    // The groups come opaque first, by slot.
    t.insert((true, 1, 0), record(0)).unwrap();
    t.insert((false, 7, 0), record(0)).unwrap();
    let order: Vec<GroupKey> = t.groups().map(|(g, _)| *g).collect();
    assert_eq!(order, vec![(false, 0, 0), (false, 7, 0), (true, 1, 0)]);
}

#[test]
fn a_full_table_refuses_and_stays_whole() {
    let Some((device, queue)) = device() else {
        return;
    };
    let mut t = Table::new(&device);
    let mut taken = 0u64;
    for n in 0..MAX_CLUSTERS + 1000 {
        if t.insert((false, n % 3, 0), record(n)).is_some() {
            taken += 1;
        }
    }
    assert!(taken <= u64::from(MAX_CLUSTERS));
    assert!(taken > u64::from(MAX_CLUSTERS) * 9 / 10, "{taken}");
    assert_eq!(t.refused, u64::from(MAX_CLUSTERS) + 1000 - taken);
    assert_eq!(t.capacity(), MAX_CLUSTERS);
    t.flush(&device, &queue);
    assert_eq!(t.buffer.size(), u64::from(MAX_CLUSTERS) * RECORD_BYTES);
}

fn read(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u8> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: buffer.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut e = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    e.copy_buffer_to_buffer(buffer, 0, &staging, 0, buffer.size());
    queue.submit([e.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let out = slice.get_mapped_range().unwrap().to_vec();
    staging.unmap();
    out
}

/// Every live slab's bytes where its slab says, in its page, and no two slabs of a page overlap.
fn check(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    arena: &Arena,
    live: &BTreeMap<u32, (Vec<u8>, Vec<u16>)>,
) {
    let pages: Vec<(Vec<u8>, Vec<u8>)> = arena
        .pages
        .iter()
        .map(|p| {
            (
                read(device, queue, &p.vertices),
                read(device, queue, &p.indices),
            )
        })
        .collect();
    let mut places: Vec<(u32, u32, u32)> = Vec::new();
    for (id, (v, i)) in live {
        let s = arena.slab(*id).unwrap();
        let (vb, ib) = &pages[s.page as usize];
        let at = s.vertex as usize * HD_VERTEX_BYTES;
        assert_eq!(&vb[at..at + v.len()], &v[..], "slab {id}'s vertices");
        let at = s.index as usize * INDEX_BYTES as usize;
        let want: Vec<u8> = i.iter().flat_map(|x| x.to_le_bytes()).collect();
        assert_eq!(&ib[at..at + want.len()], &want[..], "slab {id}'s indices");
        places.push((s.page, s.vertex, s.vertices));
    }
    places.sort_unstable();
    assert!(
        places
            .windows(2)
            .all(|w| w[0].0 != w[1].0 || w[0].1 + w[0].2 <= w[1].1),
        "slabs overlap"
    );
}

#[test]
fn slabs_fill_pages_of_a_set_most_and_keep_their_bytes() {
    let Some((device, queue)) = device() else {
        return;
    };
    let mut arena = Arena::new(&device);
    // Small pages: the slabs go to several.
    arena.page_vertices = 4_096;
    arena.page_indices = 6_144;
    let mut live: BTreeMap<u32, (Vec<u8>, Vec<u16>)> = BTreeMap::new();
    let mut s = 7u64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for round in 0..400u32 {
        let r = next();
        if live.len() > 4 && r % 4 == 0 {
            let k = *live.keys().nth((r / 4) as usize % live.len()).unwrap();
            live.remove(&k);
            arena.free(k);
            continue;
        }
        let nv = 4 + (r % 900) as usize;
        let vertices: Vec<u8> = (0..nv * HD_VERTEX_BYTES)
            .map(|i| (i as u32 ^ round).to_le_bytes()[0] ^ (round as u8))
            .collect();
        let indices: Vec<u16> = (0..(nv as u32 * 3 / 2).next_multiple_of(2))
            .map(|i| (i ^ round) as u16)
            .collect();
        let (id, _) = arena.put(&device, &queue, &vertices, &indices).unwrap();
        live.insert(id, (vertices, indices));
    }
    assert!(arena.pages.len() > 2, "{} pages", arena.pages.len());
    assert!(
        arena
            .pages
            .iter()
            .all(|p| p.valloc.capacity() <= 4_096 && p.ialloc.capacity() <= 6_144)
    );
    // More than a page holds is refused.
    let big = vec![0u8; 5_000 * HD_VERTEX_BYTES];
    assert!(arena.put(&device, &queue, &big, &[0; 6]).is_none());
    check(&device, &queue, &arena, &live);
}

/// The last page grown into buffers made ahead on their thread (each made after a put, as a frame's end makes them) keeps
/// every slab's bytes; the repacks take the pairs made ahead.
#[test]
fn slabs_keep_their_bytes_through_growth_into_buffers_made_ahead() {
    let Some((device, queue)) = device() else {
        return;
    };
    let mut arena = Arena::new(&device);
    arena.ahead_bytes = 0;
    let mut live: BTreeMap<u32, (Vec<u8>, Vec<u16>)> = BTreeMap::new();
    for round in 0..300u32 {
        let nv = 200 + (round * 37 % 700) as usize;
        let vertices: Vec<u8> = (0..nv * HD_VERTEX_BYTES)
            .map(|i| (i as u32 ^ round).to_le_bytes()[0] ^ (round as u8))
            .collect();
        let indices: Vec<u16> = (0..(nv as u32 * 3 / 2).next_multiple_of(2))
            .map(|i| (i ^ round) as u16)
            .collect();
        let (id, _) = arena.put(&device, &queue, &vertices, &indices).unwrap();
        live.insert(id, (vertices, indices));
        if round % 5 == 0 {
            let k = *live.keys().next().unwrap();
            live.remove(&k);
            arena.free(k);
        }
        arena.make_ahead(&device);
        // (Waits for the thread: the test's growth comes faster than a load's.)
        for _ in 0..1000 {
            if arena.ahead.as_mut().is_none_or(Ahead::ready) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    assert!(
        arena.repacks_ahead > 3,
        "{} of {} repacks took buffers made ahead",
        arena.repacks_ahead,
        arena.repacks
    );
    check(&device, &queue, &arena, &live);
}

#[test]
fn slabs_keep_their_bytes_through_churn_and_repacks() {
    let Some((device, queue)) = device() else {
        return;
    };
    let mut arena = Arena::new(&device);
    let mut live: BTreeMap<u32, (Vec<u8>, Vec<u16>)> = BTreeMap::new();
    let mut s = 99u64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for round in 0..600u32 {
        let r = next();
        if live.len() > 4 && r % 3 == 0 {
            let k = *live.keys().nth((r / 3) as usize % live.len()).unwrap();
            live.remove(&k);
            arena.free(k);
            continue;
        }
        let nv = 4 + (r % 500) as usize;
        let vertices: Vec<u8> = (0..nv * HD_VERTEX_BYTES)
            .map(|i| (i as u32 ^ round).to_le_bytes()[0] ^ (round as u8))
            .collect();
        let indices: Vec<u16> = (0..(nv as u32 * 3 / 2).next_multiple_of(2))
            .map(|i| (i ^ round) as u16)
            .collect();
        let (id, _) = arena.put(&device, &queue, &vertices, &indices).unwrap();
        assert!(!live.contains_key(&id));
        live.insert(id, (vertices, indices));
    }
    assert!(arena.repacks > 0, "the arena grew");
    check(&device, &queue, &arena, &live);
    assert_eq!(
        arena.used_bytes(),
        live.values()
            .map(|(v, i)| (v.len() + i.len() * INDEX_BYTES as usize) as u64)
            .sum::<u64>()
    );
}

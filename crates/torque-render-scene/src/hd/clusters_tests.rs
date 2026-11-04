use super::*;

/// The counting sort orders a batch as the stable sort by sub-box did (equal sub-boxes in the order they came).
#[test]
fn the_quads_order_by_sub_box_as_a_stable_sort_does() {
    let v = |p: [f32; 3], id: u32| crate::hd::HdVertex {
        position: p,
        normal: [0, 0],
        uv: [0.0; 2],
        color: [0; 4],
        material: 0,
        extra: id,
    };
    let origin = [32.0, -64.0, 0.0];
    let mut quads: Vec<[crate::hd::HdVertex; 4]> = (0..5_000u32)
        .map(|i| {
            let p = [
                32.0 + (i * 7919 % 3200) as f32 * 0.01,
                -64.0 + (i * 104_729 % 3200) as f32 * 0.01,
                (i % 13) as f32 * 2.7 - 3.0,
            ];
            [v(p, i), v(p, i), v(p, i), v(p, i)]
        })
        .collect();
    quads.push([v([f32::NAN; 3], 9); 4]);
    let mut want = quads.clone();
    want.sort_by_cached_key(|q| sub_box(q, origin));
    order(&mut quads, origin);
    let ids = |q: &[[crate::hd::HdVertex; 4]]| q.iter().map(|q| q[0].extra).collect::<Vec<_>>();
    assert_eq!(ids(&quads), ids(&want));
    order(&mut quads, origin);
    assert_eq!(ids(&quads), ids(&want), "an ordered batch stays");
}

fn quad(at: [f32; 3], id: u32) -> [HdVertex; 4] {
    let v = |dx: f32, dy: f32| HdVertex {
        position: [at[0] + dx, at[1] + dy, at[2]],
        normal: [0, 0],
        uv: [0.0; 2],
        color: [255; 4],
        material: 0,
        extra: id,
    };
    [v(0.0, 0.0), v(0.5, 0.0), v(0.5, 0.5), v(0.0, 0.5)]
}

fn lay_out(batches: Vec<(u32, Vec<[HdVertex; 4]>)>, key: [i32; 3]) -> (Vec<HdVertex>, Vec<Batch>) {
    let mut vertices = Vec::new();
    let mut out = Vec::new();
    for (slot, mut quads) in batches {
        order(&mut quads, cell_origin(key));
        let first = (vertices.len() / 4 * 6) as u32;
        for q in &quads {
            vertices.extend_from_slice(q);
        }
        out.push(Batch {
            slot,
            translucent: false,
            first,
            count: (vertices.len() / 4 * 6) as u32 - first,
        });
    }
    (vertices, out)
}

#[test]
fn clusters_keep_to_their_sub_box_and_batch_and_cover_every_quad() {
    let key = [1, -1, 0];
    let o = cell_origin(key);
    // Quads in creation order across the cell's sub-boxes, in two batches.
    let mut a = Vec::new();
    let mut b = Vec::new();
    for i in 0..40u32 {
        let p = [
            o[0] + (i * 7 % 32) as f32,
            o[1] + (i * 13 % 32) as f32,
            o[2] + (i % 3) as f32 * 9.0,
        ];
        a.push(quad(p, i));
        if i % 4 == 0 {
            b.push(quad(p, 100 + i));
        }
    }
    let (vertices, batches) = lay_out(vec![(0, a), (5, b)], key);
    let clusters = of_cell(&vertices, &batches, key);
    let mut covered = 0;
    for c in &clusters {
        let batch = batches.iter().find(|bt| bt.slot == c.slot).unwrap();
        assert!(c.first >= batch.first && c.first + c.count <= batch.first + batch.count);
        let quads: Vec<&[HdVertex]> = (c.first / 6..(c.first + c.count) / 6)
            .map(|q| &vertices[q as usize * 4..q as usize * 4 + 4])
            .collect();
        let subs: std::collections::BTreeSet<u32> = quads
            .iter()
            .map(|q| sub_box((*q).try_into().unwrap(), o))
            .collect();
        assert_eq!(subs.len(), 1, "a cluster in one sub-box");
        for q in quads {
            for v in q {
                assert!((0..3).all(
                    |k| v.position[k] >= c.bounds[0][k] && v.position[k] <= c.bounds[1][k]
                ));
            }
        }
        covered += c.count;
    }
    assert_eq!(
        covered as usize,
        vertices.len() / 4 * 6,
        "every quad in one cluster"
    );
    // Within a sub-box the quads keep their order.
    for c in &clusters {
        let ids: Vec<u32> = (c.first / 6..(c.first + c.count) / 6)
            .map(|q| vertices[q as usize * 4].extra)
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
    }
}

#[test]
fn a_crowded_cell_keeps_to_the_caps() {
    let key = [0, 0, 0];
    // 3,000 quads in one sub-box: three clusters of at most 1,024.
    let one: Vec<[HdVertex; 4]> = (0..3000).map(|i| quad([1.0, 1.0, 1.0], i)).collect();
    let (vertices, batches) = lay_out(vec![(0, one)], key);
    let clusters = of_cell(&vertices, &batches, key);
    assert_eq!(clusters.len(), 3);
    assert!(clusters.iter().all(|c| c.count / 6 <= MAX_CLUSTER_QUADS));
    // A quad in each of the 64 sub-boxes in each of 20 batches would make 1,280 clusters: cut by count instead.
    let mut many = Vec::new();
    for slot in 0..20 {
        let mut qs = Vec::new();
        for s in 0..64u32 {
            let p = [
                (s % 4) as f32 * 8.0 + 1.0,
                (s / 4 % 4) as f32 * 8.0 + 1.0,
                (s / 16) as f32 * 8.0 + 1.0,
            ];
            qs.push(quad(p, s));
        }
        many.push((slot, qs));
    }
    let (vertices, batches) = lay_out(many, key);
    let clusters = of_cell(&vertices, &batches, key);
    assert!(clusters.len() <= MAX_CELL_CLUSTERS, "{}", clusters.len());
    assert_eq!(
        clusters.iter().map(|c| c.count).sum::<u32>() as usize,
        vertices.len() / 4 * 6
    );
    // Non-finite places land in a sub-box (never a panic).
    let mut bad = quad([f32::NAN, f32::INFINITY, -1e30], 0);
    bad[0].position = [f32::NAN; 3];
    assert!(sub_box(&bad, [0.0; 3]) < 64);
}

use super::*;

/// Three objects found in the order 1, 2, 3, each an ortho box (a Player with a shape).
#[derive(Debug)]
struct Three {
    reversed: bool,
}

impl World for Three {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        let x = id.0 as f32 * 2.0;
        let b = Box3::new([-0.5; 3], [0.5; 3]);
        let mut m = IDENTITY;
        m[3] = x;
        let mut inv = IDENTITY;
        inv[3] = -x;
        Some(ObjectView {
            obj_to_world: m,
            world_to_obj: inv,
            scale: [1.0; 3],
            obj_box: b,
            world_box: Box3::new([x - 0.5, -0.5, -0.5], [x + 0.5, 0.5, 0.5]),
            world_radius: 1.0,
            type_mask: crate::types::PLAYER,
            collision_disabled: false,
        })
    }

    fn find_objects(
        &mut self,
        _query: &Box3,
        _mask: u32,
        _exclude: Option<ObjectId>,
        _limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        out.extend([ObjectId(1), ObjectId(2), ObjectId(3)]);
        Ok(())
    }

    fn order_candidates(&mut self, ids: &mut [ObjectId]) {
        if self.reversed {
            ids.reverse();
        }
    }

    fn convex_source(&self, _id: ObjectId) -> ConvexSource {
        ConvexSource::Player { has_shape: true }
    }

    fn hull(&self, _id: ObjectId, _slot: u32) -> Option<Rc<Hull>> {
        None
    }

    fn charge(&mut self, _units: u64) -> bool {
        true
    }
}

fn built(reversed: bool) -> Vec<u32> {
    let mut w = Three { reversed };
    let mut ws = WorkingSet::new();
    ws.update(
        &mut w,
        ObjectId(99),
        &Box3::new([-10.0; 3], [10.0; 3]),
        u32::MAX,
    )
    .unwrap();
    ws.iter().map(|c| c.object.0).collect()
}

#[test]
fn the_working_set_builds_the_candidates_in_the_containers_order_unless_the_host_reorders_them() {
    // Torque's updateWorkingList: no sort, each candidate built in the container's order and added at the head.
    assert_eq!(built(false), vec![3, 2, 1]);
    // A host's own order.
    assert_eq!(built(true), vec![1, 2, 3]);
}

//! Rope ends hung from a child of the scene hierarchy follow the child where the client resolves it (its parents' poses times its
//! local pose), not the pose its own record last carried: a real server and an engine client over the memory transport.

#[path = "net/rig.rs"]
mod rig;

use rig::{PeerRopes, Rig, RigRopes, create, engine, files, rope};
use torque_app::GameFiles;
use torque_collide::math::{Matrix, mul_p, translation};
use torque_plugin_rope::{Anchor, RopeEnd};

/// The rope test game with the engine's own protocol, which carries the hierarchy.
fn hierarchy_files() -> GameFiles {
    let mut files = files();
    files.insert(
        "game.toml",
        b"[game]\nname = \"Rope hierarchy test\"\nserver = \"server.cs\"\nclient = \"main.cs\"\n\n[net]\nprotocol = \"engine\"\n\n[plugins]\ndisable = [\"torque.audio\", \"torque.water\", \"torque.terrain\", \"torque.server-query\"]\n",
    );
    files
}

/// The id of the static shape `code` makes on the server (its `setTransform` reaches clients; a camera's does not).
fn shape(rig: &mut Rig, position: &str) -> u32 {
    rig.eval(&format!(
        "return new StaticShape() {{ dataBlock = MoverData; position = \"{position}\"; }};"
    ))
    .parse()
    .expect("a static shape")
}

/// A root, its child and its grandchild, hung on each other at local poses that turn and lift.
fn tree(rig: &mut Rig) -> [u32; 3] {
    let root = shape(rig, "0 0 2");
    let child = shape(rig, "3 0 2");
    let grandchild = shape(rig, "5 1 3");
    let linked = rig.eval(&format!(
        "return {child}.setParent({root}) && {child}.setLocalTransform(\"3 0 0 0 0 1 0.6\") \
         && {grandchild}.setParent({child}) && {grandchild}.setLocalTransform(\"2 1 0.5 0 0 1 0.4\");"
    ));
    assert_eq!(linked, "1", "{}", rig.eval("return getParentError();"));
    [root, child, grandchild]
}

/// The root's pose at tick `t`: it slides, rises and turns by whole steps, so every tick's pose differs.
fn root_pose(t: usize) -> String {
    let t = t as f32;
    format!(
        "{} {} {} 0 0 1 {}",
        t * 0.04,
        t * 0.01,
        2.0 + (t * 0.05).sin(),
        t * 0.02
    )
}

/// The largest difference of two points along an axis.
fn apart(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max)
}

/// The ghost the rope's end hangs from on `peer`, once it has the rope.
fn end_ghost(peer: &rig::Peer) -> Option<u32> {
    if peer.rope_ids().len() != 1 {
        return None;
    }
    match rope(peer).1.anchor(RopeEnd::End) {
        Anchor::Ghost { ghost, .. } => Some(ghost),
        _ => None,
    }
}

/// Where the client resolves ghost `ghost` (its transform), and where its own record puts it.
fn client_places(rig: &Rig, ghost: u32) -> (Matrix, [f32; 3]) {
    rig.peers[0]
        .client
        .with_world(|w| {
            let (m, _) = w.records.world_pose(ghost, &|_, _| None).expect("a pose");
            let own = w.records.ghosts.get(&ghost).and_then(|g| g.position);
            (m, own.unwrap_or_default())
        })
        .expect("connected")
}

/// Hangs a rope's end from the grandchild at `offset` in its own frame, moves the root for 600 frames, and counts the frames the
/// end is within a millimeter of `offset` turned by the grandchild's resolved transform (the point the drawn path puts it at,
/// `FrameAnchors::mount_at`) and the frames the grandchild's own record is more than a centimeter from it.
fn follow(offset: [f32; 3]) -> (usize, usize) {
    let files = hierarchy_files();
    let mut rig = Rig::new(&files, &[]);
    rig.join(&files, engine(&files, true), 50_001, &[]);
    assert!(
        rig.run_until(400, |r| r.peers[0].ghosts() >= 2),
        "{:?}",
        rig.peers[0].client.client_log
    );
    let [root, _, grandchild] = tree(&mut rig);
    let id = create(&mut rig, "Cable");
    let [x, y, z] = offset;
    rig.eval(&format!(
        "ropeNetAttachAt({id}, 1, {grandchild}, \"\", \"{x} {y} {z}\");"
    ));
    let mut held = None;
    rig.run_until(400, |r| {
        held = end_ghost(&r.peers[0]);
        held.is_some()
    });
    let ghost = held.unwrap_or_else(|| panic!("{:?}", rig.peers[0].client.client_log));
    // The end walks to its anchor at the rope's own pin speed; the frames counted start where it has arrived.
    let arrived = rig.run_until(60, |r| {
        let end = rope(&r.peers[0]).1.points().last().expect("points");
        apart(end, mul_p(&client_places(r, ghost).0, offset)) <= 0.001
    });
    assert!(arrived, "the end never reached the resolved anchor");
    let (mut good, mut stale_record) = (0, 0);
    for t in 0..600 {
        rig.move_static(root, &root_pose(t));
        rig.step();
        let (resolved, record) = client_places(&rig, ghost);
        let end = rope(&rig.peers[0]).1.points().last().expect("points");
        good += usize::from(apart(end, mul_p(&resolved, offset)) <= 0.001);
        // The record's own pose is not where the child is: a rope that read it would miss by far more than a centimeter.
        stale_record += usize::from(apart(record, translation(&resolved)) > 0.01);
    }
    (good, stale_record)
}

#[test]
fn a_rope_hung_from_a_grandchild_ghost_follows_its_resolved_pose_on_600_of_600_frames() {
    let (good, stale_record) = follow([0.0; 3]);
    assert_eq!(
        good, 600,
        "frames with the end within 1 mm of the resolved anchor"
    );
    assert!(
        stale_record > 500,
        "the record pose was near the resolved one on {} frames",
        600 - stale_record
    );
}

#[test]
fn a_rope_hung_at_an_offset_from_a_turning_grandchild_ends_where_the_drawn_path_puts_it() {
    let (good, _) = follow([0.5, 1.2, 0.3]);
    assert_eq!(
        good, 600,
        "frames with the end within 1 mm of the offset turned by the resolved transform"
    );
}

//! The arena's figure in its poses on the server (`game/fixtures/floor.mis`, no clients): the datablock's crouched and prone
//! poses are the ones the clients predict with; a crouched or prone figure is hit where its body is (a bolt over a crouched
//! head misses, one through its lowered torso hits; one through a standing chest misses a prone figure, one along the ground
//! hits it); and a hit plays a reaction away from the shot, the head's for a head shot. The test's Player class holds the
//! pose triggers (the moves a client would send).

use std::cell::Cell;
use std::net::Ipv4Addr;
use std::rc::Rc;

use torque::app::server::GameServer;
use torque::game::sim::player::poses::Pose;
use torque::game::sim_classes::{PlayerKind, SimClasses, set_sim_classes};
use torque::script::vm::sim::ObjId;
use torque::script::vm::vm::Vm;
use torque::sim::player::{PlayerClass, PlayerHost, PlayerState};
use torque_example_fps::files::GameFiles;
use torque_example_fps::server::{ArenaServer, ServerOptions};

/// The triggers every Player holds.
#[derive(Debug, Default)]
struct Triggers(Cell<[bool; 6]>);

#[derive(Debug)]
struct Posing(Rc<Triggers>);

impl PlayerClass for Posing {
    fn ai_move(
        &self,
        _s: &mut PlayerState,
        _host: &mut dyn PlayerHost,
    ) -> Option<torque::sim::Move> {
        Some(torque::sim::Move {
            triggers: self.0.0.get(),
            ..torque::sim::Move::default()
        })
    }
}

#[derive(Debug)]
struct PosingKind(Rc<Triggers>);

impl PlayerKind for PosingKind {
    fn class(&self) -> Rc<dyn PlayerClass> {
        Rc::new(Posing(Rc::clone(&self.0)))
    }
}

struct PosingClasses(Rc<Triggers>);

impl SimClasses for PosingClasses {
    fn player_kind(&self, _vm: &Vm, _db: ObjId) -> Rc<dyn PlayerKind> {
        Rc::new(PosingKind(Rc::clone(&self.0)))
    }
}

fn server(triggers: &Rc<Triggers>) -> GameServer {
    let mut s = ArenaServer::start(
        &GameFiles::bundled().unwrap(),
        &ServerOptions {
            address: Ipv4Addr::LOCALHOST,
            port: 0,
            name: "Poses".to_string(),
            mission: "fixtures/floor.mis".to_string(),
            public: false,
        },
    )
    .unwrap();
    set_sim_classes(s.vm_mut(), Rc::new(PosingClasses(Rc::clone(triggers))));
    s.tick();
    s
}

fn eval(s: &mut GameServer, text: &str) -> String {
    String::from_utf8(s.vm_mut().eval(text.as_bytes()).unwrap()).unwrap()
}

fn id(s: &mut GameServer, name: &str) -> ObjId {
    eval(s, &format!("return {name}.getId();")).parse().unwrap()
}

/// The shooter well away, and the target at the origin facing +y, settled, holding `held`.
fn targets(s: &mut GameServer, triggers: &Triggers, held: &[usize]) {
    triggers.0.set([false; 6]);
    for (name, x, y) in [("Shooter", 6.0, -8.0), ("Target", 0.0, 0.0)] {
        eval(
            s,
            &format!(
                "MissionCleanup.add(new Player({name}) {{ dataBlock = ArenaFigure; position = \"{x} {y} 0\"; }});"
            ),
        );
    }
    for _ in 0..10 {
        s.tick();
    }
    let mut t = [false; 6];
    for &h in held {
        t[h] = true;
    }
    triggers.0.set(t);
    // Through the kneel between standing and prone, and the blend into the pose.
    for _ in 0..40 {
        s.tick();
    }
}

/// A bolt along +y from `x -3 z`, then a few ticks; the part it hit, if any (the server's line), and the damage now.
fn bolt(s: &mut GameServer, x: f32, z: f32) -> (Option<String>, f32) {
    s.drain_log();
    eval(
        s,
        &format!(
            "MissionCleanup.add(new Projectile() {{ dataBlock = ArenaBolt; initialPosition = \"{x} -3 {z}\"; \
             initialVelocity = \"0 150 0\"; sourceObject = Shooter.getId(); }});"
        ),
    );
    for _ in 0..3 {
        s.tick();
    }
    let part = s.drain_log().iter().find_map(|l| {
        l.split_once("hit in the ")
            .map(|(_, p)| p.trim_end_matches('.').to_string())
    });
    let damage = eval(s, "return Target.getDamageLevel();").parse().unwrap();
    (part, damage)
}

#[test]
fn the_servers_poses_are_the_ones_the_clients_predict_with() {
    let triggers = Rc::new(Triggers::default());
    let mut s = server(&triggers);
    targets(&mut s, &triggers, &[]);
    let target = id(&mut s, "Target");
    let info = torque::game::sim::player::info(s.vm_mut(), target).unwrap();
    assert_eq!(info.data.poses, torque_example_common::figure_poses());
}

#[test]
fn a_crouched_figure_is_hit_where_it_crouches() {
    let triggers = Rc::new(Triggers::default());
    let mut s = server(&triggers);
    let crouch = torque_example_common::figure_poses().triggers.crouch;
    targets(&mut s, &triggers, &[crouch]);
    let target = id(&mut s, "Target");
    assert_eq!(
        torque::game::sim::player::state(s.vm_mut(), target)
            .unwrap()
            .pose,
        Pose::Crouch
    );
    // Where a standing head is, nothing now.
    let (part, damage) = bolt(&mut s, 0.0, 1.85);
    assert_eq!((part, damage), (None, 0.0), "over the crouched head");
    // Its lowered torso and its head where they are now.
    let (part, _) = bolt(&mut s, 0.0, 0.95);
    assert_eq!(part.as_deref(), Some("torso"));
    let (part, _) = bolt(&mut s, 0.0, 1.38);
    assert_eq!(part.as_deref(), Some("head"));
}

#[test]
fn a_prone_figure_is_hit_along_the_ground_and_missed_above_it() {
    let triggers = Rc::new(Triggers::default());
    let mut s = server(&triggers);
    let prone = torque_example_common::figure_poses().triggers.prone;
    targets(&mut s, &triggers, &[prone]);
    let target = id(&mut s, "Target");
    assert_eq!(
        torque::game::sim::player::state(s.vm_mut(), target)
            .unwrap()
            .pose,
        Pose::Prone
    );
    let (part, damage) = bolt(&mut s, 0.0, 1.3);
    assert_eq!(
        (part, damage),
        (None, 0.0),
        "through where a standing chest is"
    );
    let (part, _) = bolt(&mut s, 0.0, 0.25);
    assert!(part.is_some(), "along the ground it hits the body");
}

#[test]
fn a_hit_plays_a_reaction_away_from_the_shot_and_the_heads_for_a_head_shot() {
    let triggers = Rc::new(Triggers::default());
    let mut s = server(&triggers);
    targets(&mut s, &triggers, &[]);
    let target = id(&mut s, "Target");
    let reaction = |s: &mut GameServer| -> Vec<String> {
        let vm = s.vm_mut();
        let st = torque::game::scene::shapebase::state(vm, target).unwrap();
        let shape = st.body.as_ref().unwrap().shape();
        st.scripts
            .iter()
            .filter(|t| t.state == torque::game::scene::shapebase::THREAD_PLAY)
            .filter_map(|t| usize::try_from(t.sequence).ok())
            .map(|q| {
                String::from_utf8_lossy(&shape.names[shape.sequences[q].name as usize]).into_owned()
            })
            .collect()
    };
    // The bolt flies along +y into the figure's back (it faces +y): a body shot from behind.
    let (part, _) = bolt(&mut s, 0.0, 1.0);
    assert_eq!(part.as_deref(), Some("torso"));
    assert!(
        reaction(&mut s).contains(&"flinch_back".to_string()),
        "{:?}",
        reaction(&mut s)
    );
    // The reaction is over in a third of a second, its thread stopped.
    for _ in 0..15 {
        s.tick();
    }
    assert!(reaction(&mut s).is_empty(), "{:?}", reaction(&mut s));
    // Turned to face the shooter's way: a head shot from the front snaps the head.
    eval(&mut s, "Target.setTransform(\"0 0 0 0 0 1 3.14159\");");
    for _ in 0..5 {
        s.tick();
    }
    let (part, _) = bolt(&mut s, 0.0, 1.85);
    assert_eq!(part.as_deref(), Some("head"));
    assert!(
        reaction(&mut s).contains(&"flinch_head_front".to_string()),
        "{:?}",
        reaction(&mut s)
    );
}

//! A Projectile on a server with TGE's game classes ([`torque_server::game::install`]): it starts where its persist fields put
//! it, flies along its velocity a tick at a time, hits a Player in its way (the datablock's `onCollision` and `onExplode`), and
//! is deleted half a second after it exploded; one that hits nothing is deleted when its lifetime runs out.

use std::cell::RefCell;
use std::rc::Rc;

use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

fn game_server() -> Server {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    let files = MemFiles::new();
    let b = [-0.5, -0.5, 0.0, 0.5, 0.5, 2.0];
    let body = torque_shapes::synth::hull_shape(
        b,
        &[("detail2", b)],
        &["eye", "cam"],
        &["root", "look", "hold"],
    );
    let mut body = body;
    // The synthetic shape names its root node `root` too; a shape's names are unique.
    body.shape.names[1] = b"base".to_vec();
    files.insert("body.dts", &torque_shapes::write::write_dts(&body));
    vm.set_file_host(files);
    let state = Rc::new(RefCell::new(ServerState::default()));
    torque_server::game::install(&mut vm, &state);
    let mut s = Server::with_state(vm, state);
    // The first run covers the start-up (the clock jumps ahead at once).
    s.run(TICK_MS, |_| false);
    s.vm.eval(
        b"datablock ProjectileData(Bolt) { muzzleVelocity = 50; lifetime = 320; };
datablock PlayerData(Body) { shapeFile = \"body.dts\"; boundingBox = \"1 1 2\"; maxDamage = 100; };
function Bolt::onCollision(%this, %obj, %col, %fade, %pos, %normal)
{
   $hit = %col; $hitPos = %pos; $hitData = %this.getName(); $bodyData = %col.getDataBlock().getName();
}
function Bolt::onExplode(%this, %obj, %pos, %fade) { $exploded = %obj; }",
    )
    .unwrap();
    s
}

fn global(s: &Server, name: &str) -> String {
    String::from_utf8_lossy(&s.vm.global(name).unwrap_or_default()).into_owned()
}

fn position(s: &mut Server, obj: &str) -> [f32; 3] {
    let text =
        s.vm.eval(format!("return {obj}.getPosition();").as_bytes())
            .unwrap();
    let text = String::from_utf8(text).unwrap();
    let v: Vec<f32> = text.split(' ').map(|w| w.parse().unwrap()).collect();
    [v[0], v[1], v[2]]
}

#[test]
fn a_projectile_flies_hits_a_player_and_goes() {
    let mut s = game_server();
    s.vm.eval(
        b"new Player(Target) { dataBlock = Body; position = \"0 6 0\"; };
new Projectile(Shot) { dataBlock = Bolt; initialPosition = \"0 0 1\"; initialVelocity = \"0 50 0\"; sourceSlot = 0; };",
    )
    .unwrap();
    assert_eq!(
        position(&mut s, "Shot"),
        [0.0, 0.0, 1.0],
        "it starts at initialPosition"
    );
    s.run(TICK_MS, |_| false);
    let after = position(&mut s, "Shot");
    assert!(
        after[1] > 0.0 && after[1] <= 1.6 + 1e-3,
        "one tick along its velocity: {after:?}"
    );
    for _ in 0..8 {
        s.run(TICK_MS, |_| false);
    }
    let target = s.vm.objects().find(b"Target").unwrap();
    assert_eq!(
        global(&s, "$hit"),
        target.to_string(),
        "onCollision names the Player"
    );
    assert_eq!(global(&s, "$hitData"), "Bolt");
    assert_eq!(
        global(&s, "$bodyData"),
        "Body",
        "getDataBlock answers the datablock"
    );
    let y: f32 = global(&s, "$hitPos")
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (5.0..=5.6).contains(&y),
        "the hit is on the box's near face: {y}"
    );
    let shot = s.vm.objects().find(b"Shot").unwrap();
    assert_eq!(
        global(&s, "$exploded"),
        shot.to_string(),
        "onExplode follows"
    );
    let state = torque_game::sim::projectile::state(&s.vm, shot).unwrap();
    assert!(state.exploded && state.velocity == [0.0; 3]);
    for _ in 0..20 {
        s.run(TICK_MS, |_| false);
    }
    assert!(
        s.vm.objects().find(b"Shot").is_none(),
        "deleted after it exploded"
    );
}

#[test]
fn a_projectile_that_hits_nothing_is_deleted_at_its_lifetime() {
    let mut s = game_server();
    s.vm.eval(b"new Projectile(Stray) { dataBlock = Bolt; initialPosition = \"0 0 1\"; initialVelocity = \"0 0 50\"; };")
        .unwrap();
    // 320 ms is ten ticks.
    for _ in 0..9 {
        s.run(TICK_MS, |_| false);
    }
    assert!(s.vm.objects().find(b"Stray").is_some());
    for _ in 0..2 {
        s.run(TICK_MS, |_| false);
    }
    assert!(
        s.vm.objects().find(b"Stray").is_none(),
        "its lifetime ran out"
    );
    assert_eq!(global(&s, "$hit"), "");
}

#[test]
fn the_ghost_view_reads_a_players_arm_action_and_a_projectiles_flight() {
    use torque_ghosts::{ObjectView, Value};
    use torque_server::net::torque::{FieldView, GameView};

    let mut s = game_server();
    s.vm.eval(
        b"new Player(Holder) { dataBlock = Body; position = \"8 0 0\"; };
new Projectile(Shot) { dataBlock = Bolt; initialPosition = \"0 0 1\"; initialVelocity = \"0 50 0\"; sourceSlot = 2; };",
    )
    .unwrap();
    let holder = s.vm.objects().find(b"Holder").unwrap();
    fn view(vm: &Vm, obj: u32) -> GameView<'_> {
        GameView {
            fields: FieldView { vm, obj },
            controlled: false,
        }
    }
    // The table's eight actions, then the shape's sequences that are not among them: `look` (the arm thread's first action),
    // then `hold`.
    assert_eq!(view(&s.vm, holder).get("look_action", 0), Value::Int(8));
    assert_eq!(
        view(&s.vm, holder).get("arm_action", 0),
        Value::Int(8),
        "the arm thread starts on look"
    );
    s.vm.eval(b"$armed = Holder.setArmThread(hold);").unwrap();
    assert_eq!(global(&s, "$armed"), "1");
    assert_eq!(view(&s.vm, holder).get("arm_action", 0), Value::Int(9));
    s.run(TICK_MS, |_| false);
    let shot = s.vm.objects().find(b"Shot").unwrap();
    let v = view(&s.vm, shot);
    assert_eq!(v.get("velocity", 1), Value::F32(50.0));
    assert_eq!(v.get("current_tick", 0), Value::Int(1));
    assert_eq!(v.get("source_slot", 0), Value::Int(2));
    assert_eq!(v.get("hidden", 0), Value::Bool(false));
    assert!(matches!(v.get("position", 1), Value::F32(y) if y > 1.5));
}

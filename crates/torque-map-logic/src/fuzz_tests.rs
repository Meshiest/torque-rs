//! The hostile-input fuzz: seeded mutations of synthetic FGD text and of connection values (none from a game), each read
//! under `catch_unwind`; 0 panics. Then random small maps of the supported classes wired to each other at random, run for a
//! while: 0 panics and every cap held. `TORQUE_FUZZ_MAP_LOGIC` sets the count (20,000 by default; the maps get a tenth).

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::bus::{BrushVolumes, Limits, MapLogic, Toucher, ToucherKind};
use crate::connection::{entity_from_pairs, parse_add_output, parse_connection};
use crate::defaults::Defaults;
use crate::fgd::{EntityDefs, Fgd};
use crate::volume::Volume;

fn cases() -> usize {
    std::env::var("TORQUE_FUZZ_MAP_LOGIC")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

const PIECES: &[&[u8]] = &[
    b"@",
    b"(",
    b")",
    b"[",
    b"]",
    b"=",
    b":",
    b",",
    b"+",
    b"\"",
    b"//",
    b"\n",
    b"@include",
    b"@BaseClass",
    b"@PointClass",
    b"@SolidClass",
    b"@ExtendClass",
    b"base(",
    b"input",
    b"output",
    b"spawnflags(flags)",
    b"choices",
    b"readonly",
    b"\xff",
    b"\xe9",
    b"-1",
    b"0.5",
    b"\"a.fgd\"",
    b"[[[[",
    b"]]]]",
];

fn mutate(seed: &[u8], r: &mut Rng) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..1 + r.below(6) {
        let at = r.below(b.len() + 1);
        match r.below(6) {
            0 if !b.is_empty() => {
                let i = at.min(b.len() - 1);
                b[i] ^= 1 << r.below(8);
            }
            1 => b.truncate(at),
            2 => {
                let p = PIECES[r.below(PIECES.len())];
                b.splice(at..at, p.iter().copied());
            }
            3 if !b.is_empty() => {
                let end = (at + r.below(64) + 1).min(b.len());
                let chunk = b[at.min(end)..end].to_vec();
                for _ in 0..r.below(4) + 1 {
                    b.splice(at..at, chunk.iter().copied());
                }
            }
            4 if !b.is_empty() => {
                let end = (at + r.below(32) + 1).min(b.len());
                b.drain(at.min(end)..end);
            }
            _ => b.insert(at, r.next() as u8),
        }
    }
    b
}

const FGD_SEED: &str = r#"
@include "base.fgd"
@mapsize(-1, 1)
@BaseClass = Named [ targetname(target_source) : "Name" : : "help" input Kill(void) : "x" output OnUser1(void) : "y" ]
@BaseClass base(Named) = Switch [ StartDisabled(choices) : "S" : 0 = [ 0 : "No" 1 : "Yes" ] ]
@SolidClass base(Named, Switch) = thing : "desc" + "more"
[
    spawnflags(flags) = [ 1 : "a" : 1 2 : "b" : 0 ]
    wait(integer) readonly report : "Wait" : 1 : "help"
    color(color255) : "C" : "1 2 3"
    input Toggle(void) : "t"
    output OnTrigger(string) : "o"
]
@ExtendClass = thing [ extra(boolean) : "e" : 1 ]
@AutoVisGroup = "x" [ "y" [ "thing" ] ]
"#;

#[test]
fn the_fgd_reader_never_panics() {
    let mut r = Rng(0x0F6D_5EED);
    let mut panics = 0;
    for _ in 0..cases() {
        let main = mutate(FGD_SEED.as_bytes(), &mut r);
        let base = mutate(FGD_SEED.as_bytes(), &mut r);
        let src =
            crate::fgd::MemorySource(vec![("main.fgd".into(), main), ("base.fgd".into(), base)]);
        let ok = catch_unwind(AssertUnwindSafe(|| {
            let fgd = Fgd::load(&src, "main.fgd");
            let defs = EntityDefs::new(fgd);
            if let Some(c) = defs.class("thing") {
                let _ = c.default_spawnflags();
                let _ = defs.typed("thing", "color", "1 2 3");
            }
        }));
        panics += usize::from(ok.is_err());
    }
    assert_eq!(panics, 0);
}

const CONNECTION_SEEDS: &[&str] = &[
    "door\u{1b}Open\u{1b}\u{1b}0.5\u{1b}-1",
    "door,Open,,0.5,-1",
    "t,AddOutput,OnUser1 x:y:z:1:2,0,1",
    "!activator,SetHealth,100,1e3,999999999999",
];

#[test]
fn the_connection_parser_never_panics() {
    let mut r = Rng(0xC0_77EC7);
    let mut panics = 0;
    let mut parsed = 0;
    for i in 0..cases() {
        let seed = CONNECTION_SEEDS[i % CONNECTION_SEEDS.len()].as_bytes();
        let b = mutate(seed, &mut r);
        let text = String::from_utf8_lossy(&b).into_owned();
        let ok = catch_unwind(AssertUnwindSafe(|| {
            let c = parse_connection(&text);
            if let Some(c) = &c {
                assert!(c.delay.is_finite() && c.delay >= 0.0);
                assert!(c.times != Some(0));
            }
            let _ = parse_add_output(&format!("OnTrigger {text}"));
            let _ = entity_from_pairs(&[
                ("classname".into(), "x".into()),
                ("OnTrigger".into(), text.clone()),
            ]);
            c.is_some()
        }));
        match ok {
            Ok(true) => parsed += 1,
            Ok(false) => {}
            Err(_) => panics += 1,
        }
    }
    assert_eq!(panics, 0);
    assert!(parsed > 0);
}

const CLASSES: &[&str] = &[
    "logic_relay",
    "logic_auto",
    "logic_timer",
    "logic_case",
    "logic_compare",
    "logic_branch",
    "math_counter",
    "trigger_once",
    "trigger_multiple",
    "filter_activator_name",
    "filter_multi",
    "func_door",
    "func_door_rotating",
    "func_movelinear",
    "func_button",
    "func_brush",
    "ambient_generic",
    "probe",
];
const INPUTS: &[&str] = &[
    "Trigger",
    "Toggle",
    "Enable",
    "Disable",
    "Kill",
    "CancelPending",
    "AddOutput",
    "FireUser1",
    "InValue",
    "PickRandomShuffle",
    "Add",
    "Divide",
    "SetValueTest",
    "Compare",
    "Open",
    "Close",
    "Press",
    "SetPosition",
    "PlaySound",
    "TestActivator",
    "RefireTime",
    "SetSpeed",
    "TouchTest",
    "StartTouch",
];
const VALUES: &[&str] = &[
    "",
    "0",
    "1",
    "-1",
    "1e30",
    "NaN",
    "inf",
    "0.01",
    "OnUser1 e*:Trigger::0:-1",
    "targetname e1",
];

#[test]
fn random_maps_run_within_their_caps() {
    let mut r = Rng(0x3A9_10C1C);
    let mut panics = 0;
    let limits = Limits {
        max_queue: 2_000,
        max_events_per_tick: 500,
        ..Limits::default()
    };
    let mut volumes = BrushVolumes::new();
    volumes.insert(1, vec![Volume::of_box([-32.0; 3], [32.0; 3])]);
    for _ in 0..(cases() / 10).max(1) {
        let n = 1 + r.below(12);
        let mut map = Vec::new();
        for i in 0..n {
            let mut kv: Vec<(String, String)> = vec![
                ("classname".into(), CLASSES[r.below(CLASSES.len())].into()),
                ("targetname".into(), format!("e{}", r.below(n))),
                ("model".into(), "*1".into()),
                ("origin".into(), format!("{} 0 0", r.below(64))),
                ("spawnflags".into(), format!("{}", r.next() % 4096)),
                ("RefireTime".into(), VALUES[r.below(VALUES.len())].into()),
                ("speed".into(), VALUES[r.below(VALUES.len())].into()),
                ("message".into(), format!("s{i}.wav")),
            ];
            for _ in 0..r.below(5) {
                let out = [
                    "OnTrigger",
                    "OnTimer",
                    "OnMapSpawn",
                    "OutValue",
                    "OnFullyOpen",
                    "OnStartTouch",
                    "OnPass",
                    "OnUser1",
                ][r.below(8)];
                let target = [
                    "e0",
                    "e1",
                    "e*",
                    "!self",
                    "!activator",
                    "!caller",
                    "probe",
                    "",
                ][r.below(8)];
                let delay = ["0", "0", "0.05", "1", "-3"][r.below(5)];
                kv.push((
                    out.into(),
                    format!(
                        "{target}\u{1b}{}\u{1b}{}\u{1b}{delay}\u{1b}-1",
                        INPUTS[r.below(INPUTS.len())],
                        VALUES[r.below(VALUES.len())]
                    ),
                ));
            }
            map.push(entity_from_pairs(&kv));
        }
        let mut pokes = Vec::new();
        for _ in 0..r.below(20) {
            pokes.push((
                r.below(60),
                format!("e{}", r.below(n)),
                INPUTS[r.below(INPUTS.len())],
                VALUES[r.below(VALUES.len())],
            ));
        }
        let ok = catch_unwind(AssertUnwindSafe(|| {
            let mut ml = MapLogic::with_limits(&map, &volumes, &mut Defaults::coded(), limits);
            let p = ml.add_player("pl");
            for t in 0..60 {
                for (at, target, input, value) in &pokes {
                    if *at == t {
                        ml.fire_input(target, input, value, 0.0, Some(p));
                    }
                }
                let x = (t as f32 * 3.0) - 60.0;
                ml.tick(&[Toucher {
                    id: p,
                    kind: ToucherKind::Player,
                    min: [x - 16.0, -16.0, -16.0],
                    max: [x + 16.0, 16.0, 56.0],
                }]);
                assert!(ml.queued() <= limits.max_queue);
                let _ = ml.mover_ghosts();
                let _ = ml.mover_hit([-1.0; 3], [1.0; 3]);
                let _ = ml.take_sounds();
            }
            assert!(ml.stats.events <= 60 * limits.max_events_per_tick as u64 + 1000);
        }));
        panics += usize::from(ok.is_err());
    }
    assert_eq!(panics, 0);
}

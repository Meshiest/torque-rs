//! Random op sequences against the applier, and random input against the layers.

use torque_client_script_format::ui::{self, KINDS, KindDef, PropDef, PropType};

use super::*;

/// How many random sequences the applier fuzz runs.
const SEQUENCES: usize = 100_000;
/// How many random input events the layers fuzz runs.
const INPUT_EVENTS: usize = 10_000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn percent(&mut self, p: u64) -> bool {
        self.below(100) < p
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const PIECES: [&str; 9] = [
    "x",
    " ",
    "<b>",
    "</b>",
    "<<",
    "<color:ff0000>",
    "</color>",
    "<size:12>",
    "<br>",
];

fn random_text(rng: &mut Rng, max: usize, rich: bool) -> String {
    if rng.percent(4) {
        return "y".repeat(max + 1);
    }
    if rng.percent(4) {
        return "bad\u{7}text".to_string();
    }
    if rng.percent(3) {
        return ["a\u{202e}b", "a\u{200f}", "a\u{2028}", "\u{2066}x"][rng.below(4) as usize]
            .to_string();
    }
    let mut out = String::new();
    for _ in 0..rng.below(6) {
        let piece = if rich { *rng.pick(&PIECES) } else { "ab " };
        out.push_str(piece);
    }
    out
}

fn random_value(rng: &mut Rng, def: &PropDef) -> PropValue {
    let valid = rng.percent(88);
    match def.ty {
        PropType::Bool => PropValue::Bool(rng.percent(50)),
        PropType::Number { min, max } => PropValue::Number(if valid {
            min + rng.unit() * (max - min)
        } else {
            max.abs() * 2.0 + 1.0
        }),
        PropType::Int { min, max } => PropValue::Int(if valid {
            min + rng.below(u64::from(max - min) + 1) as u32
        } else {
            max.saturating_add(1)
        }),
        PropType::Vec2 { min, max } => {
            let one = |rng: &mut Rng| {
                if valid {
                    min + rng.unit() * (max - min)
                } else {
                    max.abs() * 2.0 + 1.0
                }
            };
            PropValue::Vec2(one(rng), one(rng))
        }
        PropType::Color => PropValue::Color([rng.next() as u8, 1, 2, 255]),
        PropType::Text { max } => PropValue::Text(random_text(rng, max, false)),
        PropType::Rich { max } => PropValue::Text(random_text(rng, max, true)),
        PropType::Enum(names) => PropValue::Enum(if valid {
            rng.below(names.len() as u64) as u8
        } else {
            names.len() as u8 + 3
        }),
        PropType::Image => PropValue::Text(
            if valid && rng.percent(70) {
                "mods/m/a.png"
            } else if valid {
                ""
            } else {
                "../x.png"
            }
            .to_string(),
        ),
        PropType::Rows => {
            let rows = (0..rng.below(5))
                .map(|_| random_text(rng, 8, false))
                .collect();
            PropValue::Rows(rows)
        }
    }
}

fn random_props(rng: &mut Rng, kind: &KindDef) -> Vec<(Prop, PropValue)> {
    let props: Vec<Prop> = kind.props().collect();
    let mut out = Vec::new();
    for _ in 0..rng.below(5) {
        let prop = *rng.pick(&props);
        if out.iter().any(|(p, _)| *p == prop) {
            continue;
        }
        out.push((prop, random_value(rng, ui::prop_def(prop))));
    }
    // A prop the kind lacks, now and then.
    if rng.percent(3) {
        let stray = rng.pick(ui::PROPS);
        if out.iter().all(|(p, _)| *p != stray.prop) {
            out.push((stray.prop, random_value(rng, stray)));
        }
    }
    out
}

/// A handle: usually a live one, sometimes any number.
fn some_handle(rng: &mut Rng, live: &[u32], next: u32) -> u32 {
    if !live.is_empty() && rng.percent(80) {
        *rng.pick(live)
    } else {
        rng.below(u64::from(next) + 3) as u32
    }
}

fn random_op(rng: &mut Rng, live: &[u32], next: u32) -> Ops {
    let mut ops = Ops::default();
    let handle = if rng.percent(94) { next } else { next + 1 };
    match rng.below(100) {
        0..=34 => {
            let kind = rng.pick(KINDS);
            let parent = some_handle(rng, live, next);
            ops.create(handle, parent, kind.kind, &random_props(rng, kind));
        }
        35..=54 => {
            let target = some_handle(rng, live, next);
            let kind = rng.pick(KINDS);
            ops.set(target, &random_props(rng, kind));
        }
        55..=66 => {
            ops.destroy(some_handle(rng, live, next));
        }
        67..=76 => {
            ops.parent(some_handle(rng, live, next), some_handle(rng, live, next));
        }
        77..=81 => {
            ops.order(some_handle(rng, live, next), rng.percent(50));
        }
        82..=84 => {
            ops.interactive(rng.percent(50));
        }
        85..=89 => {
            ops.focus(some_handle(rng, live, next));
        }
        90..=91 => {
            let name = *rng.pick(&["a", "b", "mod", "..", ""]);
            ops.root(handle, name);
        }
        92..=96 => {
            let len = rng.below(24) as usize;
            let body: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let kind = rng.below(10) as u8;
            ops.record(kind, |w| body.iter().for_each(|b| w.u8(*b)));
        }
        _ => {
            let mark = ops.0.begin(Tier::Net.id(), 0);
            ops.0.end(mark).unwrap();
        }
    }
    ops
}

#[test]
fn client_script_applier_fuzz_100000_sequences() {
    let mut rig = Rig::new(&[Tier::Ui, Tier::Net]);
    rig.realm.set_content_check(store("mods/m/"));
    let evals = rig.vm.eval_count();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut live = Vec::new();
    let (mut applied, mut refused) = (0u64, 0u64);
    let mut clock = 0u64;
    // A realm that has a root, so creates have somewhere to go.
    let mut start = Ops::default();
    start.root(1, "mod");
    rig.apply_ok(&start);
    for sequence in 0..SEQUENCES {
        for _ in 0..=rng.below(6) {
            let next = rig.realm.next_handle();
            let ops = random_op(&mut rng, &live, next);
            clock += 1000;
            match rig
                .realm
                .apply_flush(&mut rig.vm, &rig.gui, ops.bytes(), clock)
            {
                Ok(_) => applied += 1,
                Err(_) => refused += 1,
            }
        }
        assert!(rig.realm.control_count() <= MAX_CONTROLS);
        if sequence % 50 == 0 {
            live = rig.realm.handles();
        }
        if sequence % 250 == 0 {
            rig.realm
                .check(&rig.vm, &rig.gui)
                .unwrap_or_else(|e| panic!("sequence {sequence}: {e}"));
            rig.frame();
            let report = rig.draw();
            assert!(
                report.realm_controls == 0 || report.chip,
                "sequence {sequence}: the realm is on screen with no chip"
            );
        }
    }
    rig.realm
        .check(&rig.vm, &rig.gui)
        .expect("the invariants hold at the end");
    assert_eq!(rig.vm.eval_count(), evals, "no eval ran");
    assert!(
        applied > 10_000 && refused > 10_000,
        "applied {applied}, refused {refused}"
    );
    assert!(rig.realm.control_count() > 0);
}

#[test]
fn client_script_input_fuzz_never_gives_the_realm_an_escape_or_a_dialogs_input() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = button_ops();
    ops.create(
        3,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 50.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    )
    .create(
        4,
        1,
        Kind::Slider,
        &[
            (Prop::Pos, vec2(10.0, 80.0)),
            (Prop::Size, vec2(100.0, 20.0)),
        ],
    )
    .create(
        5,
        1,
        Kind::List,
        &[
            (Prop::Pos, vec2(10.0, 110.0)),
            (Prop::Size, vec2(100.0, 50.0)),
            (Prop::Rows, PropValue::Rows(vec!["a".into(); 9])),
        ],
    )
    .interactive(true);
    rig.apply_ok(&ops);
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let games = [
        "Canvas.pushDialog(DialogOne);",
        "Canvas.popLayer(0);",
        "Canvas.setContent(MenuGui);",
        "Canvas.setContent(PlayGui);",
    ];
    for i in 0..INPUT_EVENTS {
        match rng.below(10) {
            0 => rig.game(rng.pick(&games)),
            1 | 2 => {
                let shift = if rng.percent(10) { SI_SHIFT } else { 0 };
                let action = *rng.pick(&[SI_MAKE, SI_BREAK, crate::keys::SI_REPEAT]);
                escape(&mut rig, action, shift);
            }
            3 | 4 => {
                rig.key(
                    0x30 + rng.below(40) as u16,
                    32 + rng.below(90) as u16,
                    SI_MAKE,
                    0,
                );
            }
            5..=7 => rig.move_to(rng.below(640) as i32, rng.below(200) as i32),
            _ => {
                rig.button(if rng.percent(50) { SI_MAKE } else { SI_BREAK });
            }
        }
        let up = rig.gui.0.borrow();
        let dialogs = !rig.gui.stack(&rig.vm).1.is_empty();
        let before = up.client_script.stats.input_events;
        drop(up);
        if dialogs {
            // With a dialog up no further input reaches the realm.
            rig.move_to(rng.below(640) as i32, rng.below(200) as i32);
            rig.button(SI_MAKE);
            rig.button(SI_BREAK);
            assert_eq!(
                rig.gui.client_script_stats().input_events,
                before,
                "step {i}"
            );
        }
        assert!(
            !rig.gui.client_script_holds().cursor || !dialogs,
            "step {i}: the cursor is held under a dialog"
        );
        assert_eq!(rig.gui.client_script_stats().escape_keys, 0, "step {i}");
    }
}

/// How many random texts the rich-text fuzz reads: more when `TORQUE_CLIENT_SCRIPT_FUZZ` is set (the justfile's
/// `client-script-check`).
fn rich_cases() -> usize {
    if std::env::var_os("TORQUE_CLIENT_SCRIPT_FUZZ").is_some() {
        300_000
    } else {
        60_000
    }
}

const RICH_PIECES: [&str; 36] = [
    "x",
    "word",
    " ",
    "  ",
    "\n",
    "é",
    "✓",
    ">",
    "<",
    "<<",
    "<b>",
    "</b>",
    "<i>",
    "</i>",
    "<color:ff8800>",
    "<color:ff880080>",
    "<color:zz>",
    "</color>",
    "<size:12>",
    "<size:8>",
    "<size:72>",
    "<size:9999>",
    "<size:-3>",
    "</size>",
    "<align:left>",
    "<align:center>",
    "<align:right>",
    "<align:up>",
    "<br>",
    "<a:gotoWebPage>",
    "</a>",
    "<bitmap:base/x>",
    "<font:Arial:12>",
    "</>",
    "<:>",
    "<color:",
];

/// A text made of markup pieces, now and then with a stray character.
fn random_rich(rng: &mut Rng) -> String {
    let mut text = String::new();
    for _ in 0..rng.below(30) {
        if rng.percent(3) {
            text.push(char::from_u32(rng.below(0x3000) as u32).unwrap_or('?'));
        } else {
            text.push_str(rng.pick(&RICH_PIECES));
        }
    }
    text
}

/// What a text that passed `check_rich` shows, by a second reading of it: `<<` is `<`, `<br>` and newlines break the line and
/// every other tag vanishes.
fn visible(text: &str) -> String {
    let text = text.replace("<<", "\u{1}");
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after.find('>').expect("a text that passed has no stray <");
        if &after[..end] == "br" {
            out.push('\n');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out.replace('\u{1}', "<")
}

#[test]
fn client_script_rich_text_fuzz_never_panics_and_lays_out_within_its_rules() {
    use crate::client_script::rich::layout;
    use torque_client_script_format::ui::check_rich;

    let cases = rich_cases();
    let mut rng = Rng(0x8F1B_BCDC_A953_9B7D);
    let (mut passed, mut refused) = (0usize, 0usize);
    for case in 0..cases {
        let text = random_rich(&mut rng);
        let verdict = check_rich(&text, 4096);
        let wrap = *rng.pick(&[None, Some(1), Some(10), Some(60), Some(i32::MAX)]);
        let mut measured = 0usize;
        let lines = layout(&text, wrap, 14, |word, size| {
            measured += 1;
            i32::try_from(word.len())
                .unwrap_or(i32::MAX)
                .saturating_mul(size)
        });
        assert!(!lines.is_empty(), "case {case}: {text:?}");
        let runs: usize = lines.iter().map(|l| l.runs.len()).sum();
        assert_eq!(measured, runs, "case {case}: each word measured once");
        let mut shown = 0;
        for line in &lines {
            for (run, _) in &line.runs {
                shown += run.text.len();
                assert!(
                    run.size.is_none_or(|s| (8..=72).contains(&s)),
                    "case {case}: size {:?}",
                    run.size
                );
            }
            if let Some(limit) = wrap
                && line.runs.len() > 1
            {
                assert!(
                    line.width() <= limit,
                    "case {case}: {text:?} wrapped at {limit}"
                );
            }
        }
        assert!(shown <= text.len(), "case {case}: more shown than given");
        if verdict.is_ok() {
            passed += 1;
            if wrap.is_none() {
                let shown: Vec<String> = lines
                    .iter()
                    .map(|l| l.runs.iter().map(|(r, _)| r.text.as_str()).collect())
                    .collect();
                assert_eq!(shown.join("\n"), visible(&text), "case {case}: {text:?}");
            }
        } else {
            refused += 1;
        }
    }
    println!(
        "rich-text fuzz: {cases} cases, {passed} passed the check, {refused} refused, 0 panics"
    );
    assert!(
        passed > cases / 20 && refused > cases / 20,
        "{passed} passed, {refused} refused: both paths are reached"
    );
}

/// How many frames the spoofing fuzz draws.
fn spoof_frames() -> usize {
    if std::env::var_os("TORQUE_CLIENT_SCRIPT_FUZZ").is_some() {
        100_000
    } else {
        20_000
    }
}

/// The place of a layer in the draw order the canvas keeps: content, the server layer, the game's dialogs, the engine layer,
/// the chip.
fn layer_rank(mark: crate::client_script::LayerMark) -> u8 {
    use crate::client_script::LayerMark;
    match mark {
        LayerMark::Content => 0,
        LayerMark::Server => 1,
        LayerMark::Dialog => 2,
        LayerMark::Engine => 3,
        LayerMark::Chip => 4,
    }
}

#[test]
fn client_script_spoof_fuzz_draws_the_realm_only_in_its_place_and_never_without_the_chip() {
    use crate::client_script::LayerMark;

    let mut rig = Rig::new(&[Tier::Ui, Tier::Net]);
    rig.realm.set_content_check(store("mods/m/"));
    let mut rng = Rng(0xA24B_AED4_963E_E407);
    let mut start = Ops::default();
    start.root(1, "mod").interactive(true);
    rig.apply_ok(&start);
    let games = [
        "Canvas.pushDialog(DialogOne);",
        "Canvas.pushDialog(DialogTwo);",
        "Canvas.popDialog(DialogOne);",
        "Canvas.popLayer(0);",
        "Canvas.setContent(MenuGui);",
        "Canvas.setContent(PlayGui);",
    ];
    // PlayGui is the screen the game marks for server controls.
    let mut marked_screen = true;
    let mut live = Vec::new();
    let (mut shown, mut with_dialog, mut off_screen) = (0usize, 0usize, 0usize);
    for step in 0..spoof_frames() {
        match rng.below(8) {
            0 => {
                let game = *rng.pick(&games);
                rig.game(game);
                if game.contains("setContent(MenuGui)") {
                    marked_screen = false;
                } else if game.contains("setContent(PlayGui)") {
                    marked_screen = true;
                }
            }
            _ => {
                let next = rig.realm.next_handle();
                let ops = random_op(&mut rng, &live, next);
                let clock = (step as u64 + 1) * 1000;
                let _ = rig
                    .realm
                    .apply_flush(&mut rig.vm, &rig.gui, ops.bytes(), clock);
                if step % 40 == 0 {
                    live = rig.realm.handles();
                }
            }
        }
        rig.frame();
        let report = rig.draw();
        let ranks: Vec<u8> = report.layers.iter().map(|m| layer_rank(*m)).collect();
        assert!(
            ranks.windows(2).all(|w| w[0] <= w[1]),
            "step {step}: the layers were drawn out of order: {:?}",
            report.layers
        );
        let server_drawn = report.layers.contains(&LayerMark::Server);
        // A game that popped its content away leaves no screen for the server layer to sit on.
        assert!(
            !server_drawn || report.layers.first() == Some(&LayerMark::Content),
            "step {step}: a server layer with no content under it: {:?}",
            report.layers
        );
        if report.realm_controls > 0 {
            shown += 1;
            assert!(
                server_drawn,
                "step {step}: realm controls drew outside the server layer"
            );
            assert!(
                report.chip,
                "step {step}: the realm is on screen with no chip"
            );
            assert_eq!(
                report.layers.last(),
                Some(&LayerMark::Chip),
                "step {step}: something was drawn after the chip"
            );
            let dialogs = report
                .layers
                .iter()
                .filter(|m| **m == LayerMark::Dialog)
                .count();
            with_dialog += usize::from(dialogs > 0);
            assert!(
                marked_screen,
                "step {step}: drawn over a screen the game did not mark"
            );
        }
        if !marked_screen {
            off_screen += 1;
            assert!(
                !server_drawn,
                "step {step}: a server layer over an unmarked screen"
            );
        }
    }
    println!(
        "spoof fuzz: {} frames, {shown} drew the realm ({with_dialog} under a dialog), {off_screen} on an unmarked screen, 0 out of place",
        spoof_frames()
    );
    assert!(shown > 100 && with_dialog > 10 && off_screen > 100);
}

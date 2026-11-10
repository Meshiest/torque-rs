//! Consent: nothing of a realm before the player's answer, what is remembered, which abilities a policy and an answer come to,
//! and that no script can reach the remembered choices.

mod common;

use common::*;
use torque_client_script_format::{Grants, Tier, Value};
use torque_client_script_host::{GrantSet, HOST_TIERS, REVIEWED_TIERS};
use torque_plugin_client_script::abilities::{self, ABILITIES};
use torque_plugin_client_script::messages::Status;
use torque_plugin_client_script::policy::{
    AskRow, Decision, Inputs, Policies, Policy, Remembered, Setting, Why, answer, asked, decide,
};
use torque_plugin_client_script::{Choices, ConsentAnswer, RealmState};
use torque_ts_vm::host::DirHost;

fn ui_net() -> Grants {
    Grants::of(&[Tier::Ui, Tier::Net])
}

fn objects_of_the_realm(w: &World) -> usize {
    let objects = w.vm.objects();
    objects
        .iter()
        .filter(|(_, o)| objects.classes.name(o.class()).starts_with(b"ClientScript"))
        .count()
}

fn allow(w: &World, ticked: &[(Tier, bool)], remember: bool) {
    *w.asked.answer.borrow_mut() = Some(ConsentAnswer::Allow {
        ticked: ticked.to_vec(),
        remember,
    });
}

#[test]
fn no_realm_instance_and_no_file_read_before_allow() {
    let mut w = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    w.offer(&["ui", "net"]);
    // The files are not here yet: nothing is asked and nothing is read.
    w.frames(5);
    assert_eq!(w.realms.state(), RealmState::Waiting);
    assert_eq!(w.asked.requests.borrow().len(), 0);
    // They arrive: the question is shown, once.
    w.files.here.set(true);
    w.frames(5);
    assert_eq!(w.realms.state(), RealmState::Asking);
    assert_eq!(w.asked.requests.borrow().len(), 1);
    // The player has not answered. Many frames later: no instance, no read of a file, no control, no realm on the GUI.
    w.frames(30);
    assert_eq!(w.script.instances.get(), 0, "no guest instance");
    assert_eq!(w.files.reads.get(), 0, "no file of the server read");
    assert_eq!(objects_of_the_realm(&w), 0, "no control of the realm");
    assert!(!w.gui.client_script_active());
    assert_eq!(w.realms.stats().opened, 0);
    // The question names the abilities by stable key, with their English lines.
    let request = w.asked.requests.borrow()[0].clone();
    let keys: Vec<&str> = request.lines.iter().map(|l| l.key.as_str()).collect();
    assert_eq!(
        keys,
        ["client_script.ability.ui", "client_script.ability.net"]
    );
    assert_eq!(request.lines[0].english, "Show its own menus and HUD");
    assert!(request.lines.iter().all(|l| l.on && l.locked.is_none()));
    // Allow: now, and only now, the realm exists.
    allow(&w, &[(Tier::Ui, true), (Tier::Net, true)], true);
    w.frames(3);
    assert_eq!(w.realms.state(), RealmState::Running);
    assert_eq!(w.script.instances.get(), 1);
    assert_eq!(w.files.reads.get(), 1);
    assert!(w.gui.client_script_active());
}

#[test]
fn not_now_and_never_start_nothing() {
    for (answer, remembered) in [(ConsentAnswer::NotNow, false), (ConsentAnswer::Never, true)] {
        let mut w = World::new(Setting::Ask, ui_net(), Choices::in_memory());
        w.files.here.set(true);
        w.offer(&["ui", "net"]);
        w.frames(3);
        assert_eq!(w.realms.state(), RealmState::Asking);
        *w.asked.answer.borrow_mut() = Some(answer);
        w.frames(5);
        assert_eq!(w.realms.state(), RealmState::Off);
        assert_eq!(w.script.instances.get(), 0);
        assert_eq!(w.files.reads.get(), 0);
        assert!(!w.gui.client_script_active());
        assert_eq!(
            w.realms.choices().get(server_key()).is_some(),
            remembered,
            "only never is remembered"
        );
        w.up();
        assert_eq!(w.status(), Some(Status::Declined));
    }
}

#[test]
fn never_is_remembered_across_a_reconnect_with_no_prompt_and_no_instance() {
    let dir = scratch("never-remembered");
    let file = dir.join("choices.txt");
    let mut w = World::new(Setting::Ask, ui_net(), Choices::open(&file));
    w.files.here.set(true);
    w.offer(&["ui", "net"]);
    w.frames(3);
    *w.asked.answer.borrow_mut() = Some(ConsentAnswer::Never);
    w.frames(3);
    assert_eq!(w.realms.state(), RealmState::Off);
    assert!(file.exists(), "remembered in the engine's file");
    // Reconnect: the same game, a new connection.
    w.realms.disconnect(&mut w.vm, &w.gui);
    assert_eq!(w.realms.state(), RealmState::Idle);
    let prompts = w.realms.stats().prompts;
    w.offer(&["ui", "net"]);
    w.frames(10);
    assert_eq!(w.realms.stats().prompts, prompts, "0 prompts");
    assert_eq!(w.realms.state(), RealmState::Off);
    assert_eq!(w.script.instances.get(), 0, "0 instances");
    assert_eq!(w.files.reads.get(), 0);
    // Restart: a new game reads the file and still asks nothing.
    let mut again = World::new(Setting::Ask, ui_net(), Choices::open(&file));
    again.files.here.set(true);
    again.offer(&["ui", "net"]);
    again.frames(10);
    assert_eq!(
        again.asked.requests.borrow().len(),
        0,
        "0 prompts after a restart"
    );
    assert_eq!(again.script.instances.get(), 0);
    // Forgetting the server asks again.
    assert!(
        again
            .realms
            .choices_mut()
            .forget(server_key())
            .expect("saves")
    );
    again.realms.disconnect(&mut again.vm, &again.gui);
    again.offer(&["ui", "net"]);
    again.frames(5);
    assert_eq!(again.asked.requests.borrow().len(), 1);
}

#[test]
fn an_allow_that_is_remembered_asks_nothing_the_next_time_and_a_new_ability_asks_again() {
    let dir = scratch("allow-remembered");
    let mut w = World::new(
        Setting::Ask,
        ui_net(),
        Choices::open(dir.join("choices.txt")),
    );
    w.files.here.set(true);
    w.offer(&["ui"]);
    w.frames(3);
    allow(&w, &[(Tier::Ui, true)], true);
    w.frames(3);
    assert_eq!(w.realms.state(), RealmState::Running);
    w.realms.disconnect(&mut w.vm, &w.gui);
    // The same request: no question, a realm at once.
    w.offer(&["ui"]);
    w.frames(4);
    assert_eq!(w.realms.state(), RealmState::Running);
    assert_eq!(w.asked.requests.borrow().len(), 1, "asked once in all");
    w.realms.disconnect(&mut w.vm, &w.gui);
    // A new ability the player has not decided: asked again, with the earlier one shown as decided.
    w.offer(&["ui", "net"]);
    w.frames(4);
    assert_eq!(w.realms.state(), RealmState::Asking);
    let request = w.asked.requests.borrow()[1].clone();
    assert_eq!(request.lines.len(), 2);
    assert_eq!(request.lines[0].locked, Some(true), "ui was decided");
    assert_eq!(request.lines[1].locked, None, "net was not");
    // Not remembered: the next connection asks again.
    let mut once = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    once.files.here.set(true);
    once.offer(&["ui"]);
    once.frames(3);
    allow(&once, &[(Tier::Ui, true)], false);
    once.frames(3);
    once.realms.disconnect(&mut once.vm, &once.gui);
    once.offer(&["ui"]);
    once.frames(3);
    assert_eq!(once.asked.requests.borrow().len(), 2);
}

#[test]
fn the_server_learns_declined_unavailable_or_the_granted_abilities_only() {
    // Allowed: the abilities, nothing more.
    let mut w = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    w.files.here.set(true);
    w.offer(&["ui", "net"]);
    w.frames(3);
    allow(&w, &[(Tier::Ui, true), (Tier::Net, false)], false);
    w.frames(4);
    w.up();
    assert_eq!(w.status(), Some(Status::Allowed(vec![Tier::Ui])));
    // Unavailable: the request names only what this build does not offer.
    let mut none = World::new(Setting::Ask, Grants(1), Choices::in_memory());
    none.files.here.set(true);
    none.offer(&["ui", "net"]);
    none.frames(3);
    none.up();
    assert_eq!(none.status(), Some(Status::Unavailable));
    // Declined: the global setting is never, which the server cannot tell from a player's no.
    let mut never = World::new(Setting::Never, ui_net(), Choices::in_memory());
    never.offer(&["ui", "net"]);
    never.frames(3);
    never.up();
    assert_eq!(never.status(), Some(Status::Declined));
    assert_eq!(never.asked.requests.borrow().len(), 0);
    // A game that gave no runner cannot run them, and says so.
    let mut w = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    let mut no_runner = torque_plugin_client_script::Realms::new(Choices::in_memory(), None);
    no_runner.setting = Setting::Ask;
    no_runner.offerable = ui_net();
    w.realms = no_runner;
    w.offer(&["ui"]);
    w.frames(3);
    w.up();
    assert_eq!(w.status(), Some(Status::Unavailable));
}

#[test]
fn an_unreviewed_tier_is_never_granted_to_a_request() {
    // The release request: reviewed tiers only. This build lists none beyond the base.
    let all = Grants(Grants::VALID);
    let granted = GrantSet::from_request(["ui", "net", "camera"], all);
    assert_eq!(granted.grants().0 & !REVIEWED_TIERS.0, 0);
    // So a game built as released offers nothing, whatever a server asks.
    let released = Grants(REVIEWED_TIERS.0 & HOST_TIERS.0);
    assert_eq!(released.0 & !1, 0, "no tier is reviewed yet");
    assert_eq!(
        decide(&Inputs {
            setting: Setting::Ask,
            policies: &Policies::default(),
            remembered: None,
            requested: &[Tier::Ui, Tier::Net],
            offerable: released,
            reduce_motion: false,
        }),
        Decision::Off(Why::NothingAsked)
    );
}

#[test]
fn a_server_asking_for_unknown_abilities_gets_the_ones_this_game_knows() {
    let mut w = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    w.files.here.set(true);
    w.offer(&["ui", "teleport", "net"]);
    w.frames(3);
    let request = w.asked.requests.borrow()[0].clone();
    assert_eq!(request.lines.len(), 2, "teleport is not an ability");
}

// ---- the table: abilities against policies, remembered choices and answers

fn tier_set(mask: u32) -> Vec<Tier> {
    ABILITIES
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, t)| *t)
        .collect()
}

/// The grants an honest reading of the rules gives, written out again without `decide`.
fn expected(
    setting: Setting,
    policies: &Policies,
    remembered: Option<&Remembered>,
    requested: &[Tier],
    offerable: Grants,
    ticks: &dyn Fn(Tier) -> bool,
) -> Option<Grants> {
    if setting == Setting::Never || matches!(remembered, Some(Remembered::Never)) {
        return None;
    }
    let mut granted = Grants(1);
    for tier in ABILITIES {
        if !requested.contains(&tier) || !offerable.has(tier) {
            continue;
        }
        let on = match policies.get(tier) {
            Policy::Always => true,
            Policy::Never => false,
            Policy::Ask => match remembered.and_then(|r| r.get(tier)) {
                Some(on) => on,
                None => ticks(tier),
            },
        };
        if on {
            granted = granted.with(tier);
        }
    }
    (granted != Grants(1)).then_some(granted)
}

/// What the player ends with when `decision` is followed through, ticking the lines that `ticks` says.
fn followed(decision: &Decision, ticks: &dyn Fn(Tier) -> bool) -> Option<Grants> {
    match decision {
        Decision::Off(_) => None,
        Decision::Run(grants) => Some(*grants),
        Decision::Ask { rows, always } => {
            let ticked: Vec<(Tier, bool)> = rows.iter().map(|r| (r.tier, ticks(r.tier))).collect();
            let (granted, _) = answer(*always, rows, &ticked);
            (granted != Grants(1)).then_some(granted)
        }
    }
}

#[test]
fn grants_for_every_policy_and_remembered_choice() {
    let policies_of = |p: Policy| {
        let mut policies = Policies::default();
        for tier in ABILITIES {
            policies.set(tier, p);
        }
        policies
    };
    let mut cases = 0;
    // Every ability alone against every policy, setting and remembered answer, ticked or not.
    for setting in [Setting::Never, Setting::Ask] {
        for policy in [Policy::Ask, Policy::Always, Policy::Never] {
            for remembered in [
                None,
                Some(Remembered::Never),
                Some(Remembered::Tiers(Vec::new())),
            ]
            .into_iter()
            .chain(
                ABILITIES
                    .iter()
                    .flat_map(|t| [true, false].map(|on| Some(Remembered::Tiers(vec![(*t, on)])))),
            ) {
                for tier in ABILITIES {
                    for ticked in [true, false] {
                        let policies = policies_of(policy);
                        let decision = decide(&Inputs {
                            setting,
                            policies: &policies,
                            remembered: remembered.as_ref(),
                            requested: &[tier],
                            offerable: Grants(Grants::VALID),
                            reduce_motion: false,
                        });
                        let ticks = |_: Tier| ticked;
                        assert_eq!(
                            followed(&decision, &ticks),
                            expected(
                                setting,
                                &policies,
                                remembered.as_ref(),
                                &[tier],
                                Grants(Grants::VALID),
                                &ticks
                            ),
                            "{setting:?} {policy:?} {remembered:?} {tier:?} ticked {ticked}"
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    // Every subset of the abilities asked, with mixed policies, a build that offers some of them, and a remembered yes or no
    // for some.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..20_000 {
        let requested = tier_set((next() % 512) as u32);
        let offerable = ABILITIES
            .iter()
            .enumerate()
            .filter(|(i, _)| next() >> 8 & 1 == 1 || *i < 2)
            .fold(Grants(1), |g, (_, t)| g.with(*t));
        let mut policies = Policies::default();
        for tier in ABILITIES {
            policies.set(
                tier,
                [Policy::Ask, Policy::Always, Policy::Never][(next() % 3) as usize],
            );
        }
        policies.basics_without_asking = next() & 1 == 1;
        let remembered = match next() % 4 {
            0 => None,
            1 => Some(Remembered::Never),
            _ => {
                let mut list = Vec::new();
                for tier in ABILITIES {
                    if next() & 1 == 1 {
                        list.push((tier, next() & 1 == 1));
                    }
                }
                Some(Remembered::Tiers(list))
            }
        };
        let setting = if next() % 8 == 0 {
            Setting::Never
        } else {
            Setting::Ask
        };
        let mask = next();
        let ticks = |t: Tier| mask >> t.id() & 1 == 1;
        let decision = decide(&Inputs {
            setting,
            policies: &policies,
            remembered: remembered.as_ref(),
            requested: &requested,
            offerable,
            reduce_motion: next() & 1 == 1,
        });
        let want = expected(
            setting,
            &policies,
            remembered.as_ref(),
            &requested,
            offerable,
            &ticks,
        );
        assert_eq!(
            followed(&decision, &ticks),
            want,
            "{requested:?} {policies:?} {remembered:?}"
        );
        // A tier that policy refuses is never ticked-on by an answer, and one nobody asked for is never granted.
        if let Some(granted) = followed(&decision, &|_| true) {
            for tier in ABILITIES {
                if granted.has(tier) {
                    assert!(requested.contains(&tier) && offerable.has(tier), "{tier:?}");
                    assert_ne!(policies.get(tier), Policy::Never, "{tier:?}");
                }
            }
        }
        cases += 1;
    }
    println!("{cases} cases");
    assert!(cases > 20_000);
}

#[test]
fn the_basics_option_grants_menus_and_hud_without_asking_and_nothing_else() {
    let mut policies = Policies::default();
    policies.basics_without_asking = true;
    let requested: Vec<Tier> = ABILITIES.to_vec();
    let decision = decide(&Inputs {
        setting: Setting::Ask,
        policies: &policies,
        remembered: None,
        requested: &requested,
        offerable: Grants(Grants::VALID),
        reduce_motion: false,
    });
    let Decision::Ask { rows, always } = decision else {
        panic!("the rest still asks");
    };
    for tier in ABILITIES {
        assert_eq!(always.has(tier), abilities::is_basic(tier), "{tier:?}");
    }
    assert!(rows.iter().all(|r| !abilities::is_basic(r.tier)));
    assert!(rows.iter().all(|r| r.locked.is_none()));
}

#[test]
fn the_camera_starts_unticked_when_the_player_wants_reduced_motion() {
    for (reduce, on) in [(false, true), (true, false)] {
        let decision = decide(&Inputs {
            setting: Setting::Ask,
            policies: &Policies::default(),
            remembered: None,
            requested: &[Tier::Camera, Tier::Ui],
            offerable: Grants(Grants::VALID),
            reduce_motion: reduce,
        });
        let Decision::Ask { rows, .. } = decision else {
            panic!("asks");
        };
        let camera: &AskRow = rows
            .iter()
            .find(|r| r.tier == Tier::Camera)
            .expect("camera");
        assert_eq!(camera.on, on);
        assert!(rows.iter().find(|r| r.tier == Tier::Ui).expect("ui").on);
    }
    assert_eq!(
        asked(&[Tier::Ui, Tier::Ui, Tier::Realm], Grants(Grants::VALID)),
        [Tier::Ui]
    );
}

// ---- the choices file is the engine's

#[test]
fn the_choices_file_is_beyond_every_script_file_function() {
    // The game's files are a folder; the choices file is a sibling folder, as a per-user folder is.
    let base = scratch("choices-unreachable");
    let game = base.join("game");
    std::fs::create_dir_all(game.join("art")).expect("the game folder");
    std::fs::write(game.join("art/pic.txt"), "x").expect("a game file");
    let user = base.join("user");
    let file = user.join("choices.txt");
    let mut choices = Choices::open(&file);
    choices
        .set(server_key(), "203.0.113.9:28000", Remembered::Never)
        .expect("saved");
    assert!(file.exists());
    let mut vm = torque_ts_vm::vm::Vm::new();
    vm.set_file_host(DirHost::new(&game));
    // The control: a file in the game's folder is there.
    assert_eq!(eval(&mut vm, "return isFile(\"art/pic.txt\");"), "1");
    let absolute = file.to_string_lossy().replace('\\', "/");
    let forms = [
        absolute.clone(),
        "../user/choices.txt".to_string(),
        "art/../../user/choices.txt".to_string(),
        "..\\user\\choices.txt".to_string(),
        absolute.replace('/', "\\"),
        "choices.txt".to_string(),
        format!("{}/choices.txt", user.display()).replace('\\', "/"),
    ];
    for path in &forms {
        for call in [
            format!("return isFile(\"{path}\");"),
            format!("return getFileCRC(\"{path}\");"),
            format!("return exec(\"{path}\");"),
            format!("return findFirstFile(\"{path}\");"),
        ] {
            let result = eval(&mut vm, &call.replace('\\', "\\\\"));
            assert!(
                result.is_empty() || result == "0" || result == "-1",
                "{call} gave {result}"
            );
        }
    }
    // Nothing in the game's folder gives the server's key either.
    assert!(
        !eval(&mut vm, "return findFirstFile(\"*\");").contains("choices"),
        "no listing shows it"
    );
}

fn eval(vm: &mut torque_ts_vm::vm::Vm, code: &str) -> String {
    match vm.eval(code.as_bytes()) {
        Ok(out) => String::from_utf8_lossy(&out).into_owned(),
        Err(_) => String::new(),
    }
}

#[test]
fn no_script_reads_the_remembered_choices() {
    let dir = scratch("no-script-reads");
    let mut w = World::new(
        Setting::Ask,
        ui_net(),
        Choices::open(dir.join("choices.txt")),
    );
    w.files.here.set(true);
    w.offer(&["ui"]);
    w.frames(3);
    allow(&w, &[(Tier::Ui, true)], true);
    w.frames(3);
    assert!(w.realms.choices().get(server_key()).is_some());
    let key = server_key().to_hex();
    // The choices are in no extension of either script engine, and no pref or global of the client's holds the server's key.
    for vm in [&w.vm, &w.server_vm] {
        assert!(vm.extension::<Choices>().is_none());
        assert!(
            vm.extension::<torque_plugin_client_script::Realms>()
                .is_none()
        );
    }
    for name in [
        "$Pref::ClientScript::Choices",
        "$Pref::ClientScript::Setting",
        "$ClientScript::Choices",
        "$Client::ServerScripts",
    ] {
        assert!(w.vm.global(name).is_none(), "{name}");
    }
    let dump = eval(&mut w.vm, "return getVariableDump();");
    assert!(!dump.contains(&key));
    // The only script functions the plugin adds are the server's, and none answers with the choices.
    for call in [
        "return clientScriptOffer(1);",
        "return clientScriptSend(1, \"x\");",
    ] {
        let result = eval(&mut w.server_vm, call);
        assert!(
            !result.contains(&key) && !result.contains("allow"),
            "{call}: {result}"
        );
    }
    // The realm's own surface, the effects the host applies, has no call that returns anything from the choices: the realm
    // learns only its own ops' results. A Lua realm has no file, network or host call at all (the guest's import check and
    // escape suite in `torque-client-script-host` and `torque-plugin-lua`).
    let _ = Value::Nil;
}

#[test]
fn a_new_connection_always_starts_at_idle_even_if_no_frame_saw_the_old_one_end() {
    let mut w = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    w.files.here.set(true);
    w.offer(&["ui", "net"]);
    w.frames(3);
    allow(&w, &[(Tier::Ui, true), (Tier::Net, true)], false);
    w.frames(3);
    assert_eq!(w.realms.state(), RealmState::Running);
    assert_eq!(w.script.instances.get(), 1);
    // The old server's message is still waiting when the player switches servers.
    w.send_down(&text("from the old server"));
    w.new_connection();
    w.frame();
    assert_eq!(
        w.realms.state(),
        RealmState::Idle,
        "the old realm is not carried over"
    );
    assert!(!w.gui.client_script_active());
    assert_eq!(objects_of_the_realm(&w), 0);
    let inbox =
        w.vm.extension::<torque_plugin_client_script::ScriptInbox>()
            .expect("inbox");
    assert!(
        inbox.take_values(w.rpc.epoch()).is_empty(),
        "nothing of the old connection reaches the new one"
    );
    // The same, when only the server dialed changes (the link never went away).
    let mut other = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    other.files.here.set(true);
    other.offer(&["ui", "net"]);
    other.frames(3);
    allow(&other, &[(Tier::Ui, true), (Tier::Net, true)], false);
    other.frames(3);
    assert_eq!(other.realms.state(), RealmState::Running);
    other
        .vm
        .set_extension(std::rc::Rc::new(torque_engine::net::ConnectTarget(
            std::net::SocketAddr::from(([203, 0, 113, 77], 28000)),
        )));
    other.frame();
    assert_eq!(other.realms.state(), RealmState::Idle);
    assert!(!other.gui.client_script_active());
    // And a question that was up is taken down.
    let mut asking = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    asking.files.here.set(true);
    asking.offer(&["ui"]);
    asking.frames(3);
    assert_eq!(asking.realms.state(), RealmState::Asking);
    asking.new_connection();
    asking.frame();
    assert_eq!(asking.realms.state(), RealmState::Idle);
    assert_eq!(asking.asked.cancelled.get(), 1);
}

/// A flush of a mod root and one button on it.
fn button_flush() -> Vec<u8> {
    use torque_client_script_format::ui::{Kind, Prop, PropValue, write_props};
    use torque_client_script_format::{Writer, op};
    let mut out = Writer::new();
    let mark = out.begin(Tier::Ui.id(), op::ui::ROOT);
    out.u32(1);
    out.text("hud");
    out.end(mark).expect("fits");
    let mark = out.begin(Tier::Ui.id(), op::ui::CREATE);
    out.u32(2);
    out.u32(1);
    out.u8(Kind::Button as u8);
    let props = [
        (Prop::Pos, PropValue::Vec2(10.0, 10.0)),
        (Prop::Size, PropValue::Vec2(100.0, 30.0)),
        (Prop::Text, PropValue::Text("Go".into())),
    ];
    write_props(&mut out, props.iter().map(|(p, v)| (*p, v)));
    out.end(mark).expect("fits");
    out.into_bytes()
}

fn key(w: &mut World, action: u8, modifier: u8) -> bool {
    use torque_gui::input::InputEvent;
    use torque_gui::keys::{KEY_ESCAPE, KEYBOARD_DEVICE, SI_KEY};
    let ev = InputEvent {
        device: KEYBOARD_DEVICE,
        obj_type: SI_KEY,
        obj_inst: KEY_ESCAPE,
        action,
        modifier,
        ..InputEvent::default()
    };
    let (consumed, error) = w.gui.input_event(&mut w.vm, &ev);
    assert!(error.is_none());
    consumed
}

fn click(w: &mut World, x: i32, y: i32) {
    use torque_gui::input::InputEvent;
    use torque_gui::keys::{KEY_BUTTON0, MOUSE_DEVICE, SI_BREAK, SI_BUTTON, SI_MAKE};
    assert!(w.gui.mouse_moved(&mut w.vm, (x, y), 0).is_none());
    for action in [SI_MAKE, SI_BREAK] {
        let ev = InputEvent {
            device: MOUSE_DEVICE,
            obj_type: SI_BUTTON,
            obj_inst: KEY_BUTTON0,
            action,
            ..InputEvent::default()
        };
        assert!(w.gui.input_event(&mut w.vm, &ev).1.is_none());
    }
}

/// The chip's rectangle (x, y, width, height).
fn chip_rect(w: &World) -> [i32; 4] {
    let objects = w.vm.objects();
    let chip = objects
        .iter()
        .find(|(_, o)| objects.classes.name(o.class()) == b"ClientScriptChip")
        .map(|(id, _)| id)
        .expect("a chip");
    let (pos, ext) = torque_gui::bounds(&w.vm, chip);
    [pos.0, pos.1, ext.0, ext.1]
}

/// A world whose realm runs with `ui` and shows one button.
fn showing_a_button() -> World {
    let mut w = World::new(Setting::Ask, ui_net(), Choices::in_memory());
    w.files.here.set(true);
    w.script.flushes.borrow_mut().push_back(button_flush());
    w.offer(&["ui"]);
    w.frames(3);
    allow(&w, &[(Tier::Ui, true)], false);
    w.frames(3);
    assert_eq!(w.realms.state(), RealmState::Running);
    w.gui.frame(&mut w.vm);
    w
}

#[test]
fn shift_escape_ends_the_realm_for_the_connection_only_and_the_chip_remembers_never() {
    use torque_gui::keys::{SI_MAKE, SI_SHIFT};
    let mut w = showing_a_button();
    assert!(key(&mut w, SI_MAKE, SI_SHIFT), "the host takes it");
    w.frame();
    assert_eq!(w.realms.state(), RealmState::Off);
    assert!(!w.gui.client_script_active());
    assert_eq!(objects_of_the_realm(&w), 0);
    assert_eq!(
        w.realms.choices().get(server_key()),
        None,
        "Shift+Escape remembers nothing"
    );
    // A reconnect asks again.
    w.realms.disconnect(&mut w.vm, &w.gui);
    let prompts = w.realms.stats().prompts;
    w.offer(&["ui"]);
    w.frames(3);
    assert_eq!(w.realms.state(), RealmState::Asking);
    assert_eq!(w.realms.stats().prompts, prompts + 1);

    // The chip's "Turn off for this server" remembers Never. Rows: the header, ui, hide, turn off.
    let mut w = showing_a_button();
    let rect = chip_rect(&w);
    click(&mut w, rect[0] + 5, rect[1] + 5);
    w.gui.frame(&mut w.vm);
    let open = chip_rect(&w);
    click(&mut w, open[0] + 5, open[1] + 22 + 40 + 5);
    w.frame();
    assert_eq!(w.realms.state(), RealmState::Off);
    assert_eq!(
        w.realms.choices().get(server_key()),
        Some(&Remembered::Never)
    );
}

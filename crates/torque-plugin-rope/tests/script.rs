//! The rope plugin in a client: the datablock, the console functions, ropes hung from script objects, and the plugin being out of
//! the default plugins.

use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameClient, GameServer};
use torque_engine::{Engine, EngineBuilder, PluginType};
use torque_plugin_rope::{Anchor, RopeEnd, RopePlugin, RopeShared, RopeStyle, encode_png};
use torque_ts_vm::host::MemFiles;

/// A client over the default plugins and the rope plugin, having run `setup` (top level script), and its engine.
fn client(setup: &str) -> (GameClient<GameServer>, Engine) {
    client_with_files(&[], setup)
}

/// The same with `files` in the client's file host.
fn client_with_files(files: &[(&str, Vec<u8>)], setup: &str) -> (GameClient<GameServer>, Engine) {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, RopePlugin::default()))
        .build()
        .expect("the engine builds");
    let mut builder =
        ClientBuilder::from_engine(&engine, None, ClientNet::None).expect("the client");
    if !files.is_empty() {
        let host = MemFiles::new();
        for (path, bytes) in files {
            host.insert(path, bytes);
        }
        builder.vm.set_file_host(host);
    }
    builder.vm.eval(setup.as_bytes()).expect("the setup runs");
    (builder.build::<GameServer>(), engine)
}

/// A small PNG made in code: red and blue texels, half of them see-through.
fn png() -> Vec<u8> {
    let texels: Vec<u8> = [
        [255, 0, 0, 255],
        [0, 0, 255, 255],
        [0, 0, 255, 0],
        [255, 0, 0, 0],
    ]
    .into_iter()
    .flatten()
    .collect();
    encode_png(2, 2, &texels).unwrap()
}

fn ropes(engine: &Engine) -> RopeShared {
    engine
        .resource::<RopeShared>()
        .expect("the plugin's ropes")
        .clone()
}

fn eval(client: &GameClient<GameServer>, name: &str) -> String {
    String::from_utf8_lossy(&client.vm().global(name).unwrap_or_default()).into_owned()
}

const CABLE: &str = r#"
datablock RopeData(Cable)
{
   segments = 10;
   length = 9;
   thickness = 0.1;
   color = "10 20 30 255";
   castShadow = false;
};
"#;

#[test]
fn a_script_makes_a_rope_from_a_datablock_and_the_client_ticks_run_it() {
    let (mut client, engine) = client(&format!(
        r#"{CABLE} $r = ropeCreate("-4 0 3", "4 0 3", Cable); $n = ropeCount();"#
    ));
    assert_eq!(eval(&client, "$r"), "1");
    assert_eq!(eval(&client, "$n"), "1");
    let before = ropes(&engine).lock().substeps();
    client.step(2000);
    let shared = ropes(&engine);
    let world = shared.lock();
    assert!(world.substeps() >= before + 200, "the ticks ran the ropes");
    let rope = world.rope(1).unwrap();
    let params = rope.params();
    assert_eq!(
        (params.segments, params.length, params.thickness),
        (10, 9.0, 0.1)
    );
    assert_eq!(params.color, [10, 20, 30, 255]);
    assert!(!params.cast_shadow);
    let lowest = rope.points().map(|p| p[2]).fold(f32::MAX, f32::min);
    assert!(lowest < 1.5, "the rope sags: {lowest}");
}

#[test]
fn the_style_picks_how_a_rope_is_drawn() {
    let (client, engine) = client(
        r#"
datablock RopeData(Vine) { style = "Ribbon"; thickness = 0.1; };
datablock RopeData(Laser) { style = "line"; emissive = true; color = "255 0 0 128"; castShadow = true; };
datablock RopeData(Plain) { };
datablock RopeData(Dark) { receiveShadow = 0; };
$a = ropeCreate("0 0 2", "4 0 2", Vine);
$b = ropeCreate("0 1 2", "4 1 2", Laser);
$c = ropeCreate("0 2 2", "4 2 2", Plain);
$d = ropeCreate("0 3 2", "4 3 2", Dark);"#,
    );
    let shared = ropes(&engine);
    let world = shared.lock();
    let style = |id| world.rope(id).unwrap().params().style;
    assert_eq!(style(1), RopeStyle::Ribbon);
    assert_eq!(style(2), RopeStyle::Line);
    assert_eq!(style(3), RopeStyle::Tube, "a tube is the default");
    let laser = world.rope(2).unwrap().params();
    // castShadow = true is written, so the line casts; an emissive line takes no shadow unless asked.
    assert!(
        laser.emissive && laser.cast_shadow && !laser.receive_shadow,
        "{laser:?}"
    );
    let vine = world.rope(1).unwrap().params();
    assert!(!vine.cast_shadow && vine.receive_shadow, "{vine:?}");
    let plain = world.rope(3).unwrap().params();
    assert!(plain.cast_shadow && plain.receive_shadow, "{plain:?}");
    assert!(!world.rope(4).unwrap().params().receive_shadow);
    assert_eq!(laser.color, [255, 0, 0, 128]);
    assert!(!world.rope(1).unwrap().params().emissive);
    assert_eq!(world.rope(2).unwrap().segments(), 1);
    assert!(
        !client
            .vm()
            .log_texts()
            .join(
                "
"
            )
            .contains("is not tube")
    );
}

#[test]
fn an_unknown_style_warns_and_makes_a_tube() {
    let (client, engine) = client(
        r#"datablock RopeData(Odd) { style = "chain"; };
        $a = ropeCreate("0 0 2", "4 0 2", Odd);"#,
    );
    assert_eq!(
        ropes(&engine).lock().rope(1).unwrap().params().style,
        RopeStyle::Tube
    );
    let log = client.vm().log_texts().join(
        "
",
    );
    assert!(
        log.contains("the style \"chain\" is not tube, ribbon or line"),
        "{log}"
    );
}

#[test]
fn a_rope_hangs_from_an_object_and_follows_it() {
    let (mut client, engine) = client(&format!(
        r#"{CABLE}
        new ScriptObject(PostB) {{ position = "4 0 3"; }};
        $r = ropeCreate("-4 0 3", "4 0 3", Cable);
        ropeAttach($r, 1, PostB);"#
    ));
    client.step(500);
    let world = ropes(&engine);
    let id = 1;
    assert!(matches!(
        world.lock().rope(id).unwrap().anchor(RopeEnd::End),
        Anchor::Object { .. }
    ));
    // The post moves; the end follows in a moment.
    client
        .vm_mut()
        .eval(b"PostB.position = \"4 0 9\";")
        .expect("the post moves");
    client.step(1000);
    let end = world.lock().rope(id).unwrap().points().last().unwrap();
    assert!((end[2] - 9.0).abs() < 0.2, "{end:?}");
    // The post goes; the end lets go and falls.
    client
        .vm_mut()
        .eval(b"PostB.delete();")
        .expect("the post goes");
    client.step(2000);
    let world = world.lock();
    let rope = world.rope(id).unwrap();
    assert_eq!(rope.anchor(RopeEnd::End), Anchor::Free);
    let end = rope.points().last().unwrap();
    assert!(end[2] < 6.0, "{end:?}");
}

#[test]
fn the_other_functions_pin_release_push_and_delete() {
    let (client, engine) = client(&format!(
        r#"{CABLE}
        $r = ropeCreate("0 0 5", "6 0 5", Cable);
        $a = ropePin($r, "end", "6 0 8");
        $b = ropeRelease($r, start);
        $c = ropeImpulse($r, "1 2 3");
        $gone = ropePin(99, 0, "0 0 0");
        ropeFloor(2);
        $d = ropeDelete($r);
        $e = ropeDelete($r);
        $f = ropeCount();
        $r2 = ropeCreate("0 0 0", "1 0 0", Cable);
        ropeClear();
        $g = ropeCount();"#
    ));
    for (name, want) in [
        ("$a", "1"),
        ("$b", "1"),
        ("$c", "1"),
        ("$gone", "0"),
        ("$d", "1"),
        ("$e", "0"),
        ("$f", "0"),
        ("$g", "0"),
    ] {
        assert_eq!(eval(&client, name), want, "{name}");
    }
    assert_eq!(ropes(&engine).lock().floor(), Some(2.0));
}

#[test]
fn a_wrong_call_prints_an_error_with_the_usage_and_makes_nothing() {
    let (client, engine) = client(&format!(
        r#"{CABLE}
        $a = ropeCreate("0 0", "1 0 0", Cable);
        $b = ropeCreate("0 0 0", "1 0 0", Missing);
        $c = ropeCreate("0 0 0", "1 0 0", MissionGroup);
        $d = ropePin(1, "middle", "0 0 0");"#
    ));
    assert_eq!(ropes(&engine).lock().len(), 0);
    let log = client.vm().log_texts().join("\n");
    assert!(log.contains("the start must be \"x y z\""), "{log}");
    assert!(log.contains("there is no object \"Missing\""), "{log}");
    assert!(
        log.contains("usage: ropeCreate(start, end, datablock)"),
        "{log}"
    );
    assert!(
        log.contains("the end must be 0 or start, 1 or end"),
        "{log}"
    );
}

#[test]
fn a_datablock_of_hostile_values_is_clamped_and_the_rope_runs() {
    let (mut client, engine) = client(
        r#"datablock RopeData(Odd)
        {
           segments = -40;
           length = 1e300;
           thickness = -3;
           stiffness = 99;
           damping = -1;
           gravity = nan;
        };
        $r = ropeCreate("nan 0 0", "1e39 0 0", Odd);"#,
    );
    client.step(1000);
    let shared = ropes(&engine);
    let world = shared.lock();
    let rope = world.rope(1).expect("the rope");
    let p = rope.params();
    assert_eq!(p.segments, 2);
    assert!(p.length <= torque_plugin_rope::MAX_LENGTH);
    assert!((0.005..=2.0).contains(&p.thickness));
    assert!(p.stiffness <= 1.0 && p.damping >= 0.0 && p.gravity.is_finite());
    assert!(rope.points().all(|p| p.iter().all(|c| c.is_finite())));
}

#[test]
fn the_natives_have_usage_text_and_the_plugin_is_not_a_default() {
    let (client, _) = client("");
    let listing = client.vm().native_listing();
    assert!(
        listing.contains("ropeCreate 4..4 ropeCreate(start, end, datablock) - makes a rope of the datablock between two points"),
        "{listing}"
    );
    assert!(listing.contains("ropeAttach 4..5 ropeAttach(rope, end, object [, node])"));
    let plain = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .build()
        .expect("the engine builds");
    assert!(!plain.is_plugin_added::<RopePlugin>());
    let mut builder = ClientBuilder::from_engine(&plain, None, ClientNet::None).expect("client");
    assert!(
        builder
            .vm
            .eval(b"return ropeCount();")
            .is_ok_and(|v| v.is_empty()),
        "no ropeCount without the plugin"
    );
    assert_eq!(RopePlugin::ID.as_str(), "torque.rope");
}

const BRAID: &str = r#"
datablock RopeData(Braid)
{
   style = "ribbon";
   color = "200 100 50 255";
   texture = "art/braid.png";
   textureLength = 0.5;
   textureScroll = 3;
   emissive = false;
};
"#;

#[test]
fn a_datablock_names_a_texture_the_asset_loader_reads_and_ropes_share_it() {
    let (client, engine) = client_with_files(
        &[("art/braid.png", png())],
        &format!(
            r#"{BRAID} $a = ropeCreate("0 0 2", "4 0 2", Braid); $b = ropeCreate("0 1 2", "4 1 2", Braid);
            $file = ropeGetTexture($a);"#
        ),
    );
    let shared = ropes(&engine);
    let world = shared.lock();
    assert_eq!(world.textures().entries.len(), 1, "one file, one texture");
    let entry = &world.textures().entries[0];
    assert_eq!(
        (entry.path.as_str(), entry.nearest),
        ("art/braid.png", false)
    );
    assert_eq!((entry.image.width, entry.image.height), (2, 2));
    // The world holds the allocation the asset cache holds, not a copy of it.
    assert!(std::sync::Arc::strong_count(&entry.image) >= 2);
    for id in [1, 2] {
        let params = world.rope(id).unwrap().params();
        assert_eq!(
            (params.texture, params.texture_length, params.texture_scroll),
            (1, 0.5, 3.0)
        );
    }
    assert_eq!(eval(&client, "$file"), "art/braid.png");
    assert!(!client.vm().log_texts().join("\n").contains("rope texture"));
}

#[test]
fn a_texture_that_is_missing_or_not_an_image_warns_once_and_the_rope_is_plain() {
    let (client, engine) = client_with_files(
        &[("art/bad.png", b"not an image".to_vec())],
        r#"datablock RopeData(Gone) { texture = "art/none.png"; };
        datablock RopeData(Bad) { texture = "art/bad.png"; };
        $a = ropeCreate("0 0 2", "4 0 2", Gone);
        $b = ropeCreate("0 1 2", "4 1 2", Gone);
        $c = ropeCreate("0 2 2", "4 2 2", Bad);"#,
    );
    let shared = ropes(&engine);
    let world = shared.lock();
    assert_eq!(world.len(), 3, "the ropes are made all the same");
    assert!(world.textures().entries.is_empty());
    assert!((1..=3).all(|id| world.rope(id).unwrap().params().texture == 0));
    let log = client.vm().log_texts().join("\n");
    assert_eq!(log.matches("rope texture art/none.png").count(), 1, "{log}");
    assert_eq!(log.matches("rope texture art/bad.png").count(), 1, "{log}");
    assert!(log.contains("drawn plain"), "{log}");
}

#[test]
fn a_script_sets_what_one_rope_is_drawn_with_and_reads_it_back() {
    let (client, engine) = client_with_files(
        &[("art/braid.png", png()), ("art/other.png", png())],
        &format!(
            r#"{BRAID}
            $a = ropeCreate("0 0 2", "4 0 2", Braid);
            $b = ropeCreate("0 1 2", "4 1 2", Braid);
            $c0 = ropeGetColor($a);
            $ok1 = ropeSetColor($a, "255 0 0");
            $c1 = ropeGetColor($a);
            ropeSetColor($a, "10 20 30 40");
            $c2 = ropeGetColor($a);
            $other = ropeGetColor($b);
            ropeSetColor($a, "");
            $c3 = ropeGetColor($a);
            ropeSetTexture($a, "art/other.png");
            $t1 = ropeGetTexture($a);
            $tb = ropeGetTexture($b);
            ropeSetTexture($a, "");
            $t2 = ropeGetTexture($a);
            $s0 = ropeGetTextureScroll($a);
            ropeSetTextureScroll($a, -2.5);
            $s1 = ropeGetTextureScroll($a);
            $e0 = ropeGetEmissive($a);
            ropeSetEmissive($a, true);
            $e1 = ropeGetEmissive($a);
            $gone = ropeSetColor(99, "1 2 3") @ "|" @ ropeGetColor(99) @ "|" @ ropeGetTexture(99);
            ropeSetColor($a, "9 9 9 9");
            ropeSetTextureScroll($a, 8);
            ropeClearLook($a);
            $c4 = ropeGetColor($a); $s2 = ropeGetTextureScroll($a); $e2 = ropeGetEmissive($a);"#
        ),
    );
    let get = |name: &str| eval(&client, name);
    assert_eq!(get("$c0"), "200 100 50 255", "the datablock's color");
    assert_eq!(get("$ok1"), "1");
    assert_eq!(get("$c1"), "255 0 0 255", "a color without alpha is opaque");
    assert_eq!(get("$c2"), "10 20 30 40");
    assert_eq!(
        get("$other"),
        "200 100 50 255",
        "the other rope kept the datablock's"
    );
    assert_eq!(
        get("$c3"),
        "200 100 50 255",
        "clearing restores the datablock's color"
    );
    assert_eq!(
        (get("$t1").as_str(), get("$tb").as_str()),
        ("art/other.png", "art/braid.png")
    );
    assert_eq!(
        get("$t2"),
        "art/braid.png",
        "clearing restores the datablock's texture"
    );
    assert_eq!((get("$s0").as_str(), get("$s1").as_str()), ("3", "-2.5"));
    assert_eq!((get("$e0").as_str(), get("$e1").as_str()), ("0", "1"));
    assert_eq!(get("$gone"), "0||", "no such rope");
    assert_eq!(
        (
            get("$c4").as_str(),
            get("$s2").as_str(),
            get("$e2").as_str()
        ),
        ("200 100 50 255", "3", "0")
    );
    let shared = ropes(&engine);
    let world = shared.lock();
    assert_eq!(world.textures().entries.len(), 2);
    assert_eq!(
        world.rope(1).unwrap().look(),
        &torque_plugin_rope::RopeLook::default()
    );
}

#[test]
fn a_bad_color_is_a_script_error_and_changes_nothing() {
    let (client, engine) = client(&format!(
        r#"{BRAID}
        $a = ropeCreate("0 0 2", "4 0 2", Braid);
        ropeSetColor($a, "red");
        ropeSetColor($a, "1 2");
        ropeSetColor($a, "1 2 3 4 5");
        ropeSetColor($a, "1 two 3");
        ropeSetColor($a, "nan 0 0");
        $c = ropeGetColor($a);
        ropeSetColor($a, "300 -5 0");
        $clamped = ropeGetColor($a);
        ropeSetTextureScroll($a, 1e30);
        $huge = ropeGetTextureScroll($a);"#
    ));
    let log = client.vm().log_texts().join("\n");
    assert_eq!(log.matches("the color must be").count(), 5, "{log}");
    assert_eq!(eval(&client, "$c"), "200 100 50 255");
    assert_eq!(
        eval(&client, "$clamped"),
        "255 0 0 255",
        "a number past the range is held to it"
    );
    assert_eq!(eval(&client, "$huge"), "1000");
    assert_eq!(ropes(&engine).lock().len(), 1);
}

#[test]
fn a_script_gives_a_rope_a_curve_and_takes_it_away() {
    let (client, engine) = client(
        r#"datablock RopeData(Beam) { style = "ribbon"; sim = "none"; segments = 8; endThickness = 0.02; colorEnd = "0 0 255 0";
             emissive = true; emissiveIntensity = 3; additive = true; };
        datablock RopeData(Rope) { };
        $a = ropeCreate("0 0 2", "6 0 2", Beam);
        $b = ropeCreate("0 1 2", "6 1 2", Rope);
        $ok = ropeSetCurve($a, "0 1 0", "", 0.5);
        $bend = ropeGetCurveBend($a);
        ropeSetCurve($b, "mount", "mount", 9);
        $held = ropeGetCurveBend($b);
        ropeSetCurve($b, "", "1 0 0", -2);
        $neg = ropeGetCurveBend($b);
        ropeClearCurve($b);
        $cleared = ropeGetCurveBend($b);
        $none = ropeSetCurve(99, "", "", 1) @ "|" @ ropeGetCurveBend(99);"#,
    );
    assert_eq!(eval(&client, "$ok"), "1");
    assert_eq!(eval(&client, "$bend"), "0.5");
    assert_eq!(eval(&client, "$held"), "4", "a bend is held to 4");
    assert_eq!(eval(&client, "$neg"), "-2");
    assert_eq!(eval(&client, "$cleared"), "0");
    assert_eq!(eval(&client, "$none"), "0|0");
    let shared = ropes(&engine);
    let world = shared.lock();
    let beam = world.rope(1).unwrap();
    let p = beam.params();
    assert!(p.driven && p.additive && p.emissive);
    assert_eq!((p.emissive_intensity, p.thickness_end), (3.0, 0.02));
    assert_eq!(p.color_end, Some([0, 0, 255, 0]));
    assert!(beam.driven() && beam.curve().is_some());
    // The other rope was simulated, was given a curve (driven), and has it taken away again (simulated).
    assert!(!world.rope(2).unwrap().driven());
}

#[test]
fn a_bad_direction_or_bend_is_a_script_error_and_changes_nothing() {
    let (client, engine) = client(
        r#"datablock RopeData(Beam) { sim = "none"; };
        $a = ropeCreate("0 0 2", "6 0 2", Beam);
        ropeSetCurve($a, "up", "", 0.5);
        ropeSetCurve($a, "0 0 0", "", 0.5);
        ropeSetCurve($a, "1 2", "", 0.5);
        ropeSetCurve($a, "", "nan 0 1", 0.5);
        ropeSetCurve($a, "", "", nan);
        $bend = ropeGetCurveBend($a);
        datablock RopeData(Odd) { sim = "springs"; };
        $b = ropeCreate("0 0 2", "6 0 2", Odd);"#,
    );
    let log = client.vm().log_texts().join(
        "
",
    );
    assert_eq!(log.matches("a direction must be").count(), 4, "{log}");
    assert_eq!(log.matches("the bend must be a number").count(), 1, "{log}");
    assert!(
        log.contains("the sim \"springs\" is not verlet or none"),
        "{log}"
    );
    assert_eq!(eval(&client, "$bend"), "0");
    assert!(ropes(&engine).lock().rope(1).unwrap().curve().is_none());
}

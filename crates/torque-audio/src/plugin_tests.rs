use torque_engine::{ClientStage, ClientTick, Engine};
use ts_vm::host::MemFiles;
use ts_vm::testing::eval;

use super::*;

/// A PCM WAV of one second of silence, mono, 22.05 kHz, 16 bit.
fn one_second_wav() -> Vec<u8> {
    let n: u32 = 22_050 * 2;
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + 24 + 8 + n).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&22_050u32.to_le_bytes());
    out.extend_from_slice(&(22_050u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&n.to_le_bytes());
    out.extend(std::iter::repeat_n(0u8, n as usize));
    out
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins(AudioPlugin)
        .build()
        .expect("the engine builds")
}

/// A client script engine as the host makes it: the audio recipes replayed, the files set.
fn client_vm(engine: &Engine) -> Vm {
    let files = MemFiles::new();
    files.insert("base/snd/a.wav", &one_second_wav());
    let mut vm = Vm::new();
    vm.set_file_host(files);
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut vm).expect("the recipes run");
    }
    vm
}

fn tick(engine: &Engine, vm: &mut Vm) {
    for hook in engine.list::<ClientTick>() {
        hook.run(vm);
    }
}

#[test]
fn the_plugin_registers_the_alx_natives_and_the_audio_classes() {
    let engine = engine();
    let mut vm = client_vm(&engine);
    assert!(AudioPlugin::host(&vm).is_some());
    let listing = vm.native_listing();
    for name in [
        "OpenALInitDriver",
        "alxCreateSource",
        "alxPlay",
        "alGetString",
    ] {
        assert!(listing.contains(name), "{name}");
    }
    assert_eq!(
        eval(
            &mut vm,
            "new AudioDescription(D) { type = 0; }; return D.getClassName();"
        ),
        "AudioDescription"
    );
}

#[test]
fn the_natives_work_with_no_sound_card_and_the_tick_moves_the_voices() {
    let engine = engine();
    let mut vm = client_vm(&engine);
    assert_eq!(eval(&mut vm, "return OpenALInitDriver();"), "1");
    eval(
        &mut vm,
        "new AudioDescription(D) { volume = 1; type = 0; };",
    );
    eval(
        &mut vm,
        "$h = alxCreateSource(D, \"base/snd/a.wav\"); alxPlay($h);",
    );
    assert_eq!(eval(&mut vm, "return alxIsPlaying($h);"), "1");
    assert_eq!(
        eval(&mut vm, "return alxGetWaveLen(\"base/snd/a.wav\");"),
        "1000"
    );

    let host = AudioPlugin::host(&vm).expect("the audio host");
    assert!(host.borrow().device.is_none(), "no sound card is opened");

    // The first tick only sets the clock; the simulation time then passes the sound's length.
    tick(&engine, &mut vm);
    assert!(vm.advance_time(1_500).is_empty());
    tick(&engine, &mut vm);
    assert_eq!(eval(&mut vm, "return alxIsPlaying($h);"), "0");
}

#[test]
fn two_clients_share_no_audio() {
    let engine = engine();
    let mut a = client_vm(&engine);
    let b = client_vm(&engine);
    eval(
        &mut a,
        "OpenALInitDriver(); new AudioDescription(D) { type = 0; }; $h = alxCreateSource(D, \"base/snd/a.wav\");",
    );
    let (host_a, host_b) = (
        AudioPlugin::host(&a).expect("a"),
        AudioPlugin::host(&b).expect("b"),
    );
    assert!(!Rc::ptr_eq(&host_a, &host_b));
    assert_eq!(host_b.borrow().lock().sources_in_use(), 0);
}

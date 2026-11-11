use ts_vm::host::MemFiles;

use super::*;

/// The state the tests keep the natives' host in: the host itself.
fn own(host: &mut AudioHost) -> &mut AudioHost {
    host
}

/// A PCM WAV of `frames` frames.
fn wav(channels: u16, rate: u32, bits: u16, frames: u32) -> Vec<u8> {
    let block = u32::from(channels) * u32::from(bits / 8);
    let n = frames * block;
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + 24 + 8 + n).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block).to_le_bytes());
    out.extend_from_slice(&(block as u16).to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&n.to_le_bytes());
    out.extend(std::iter::repeat_n(0u8, n as usize));
    out
}

/// A script engine with the audio classes, `files` and the natives over a fresh host.
fn rig_with(files: MemFiles) -> (Vm, Rc<RefCell<AudioHost>>) {
    let mut vm = Vm::new();
    vm.set_file_host(files);
    install_classes(&mut vm);
    let host = Rc::new(RefCell::new(AudioHost::default()));
    register(&mut vm, &host, own, AlxProfile::tge());
    (vm, host)
}

fn rig() -> (Vm, Rc<RefCell<AudioHost>>) {
    let files = MemFiles::new();
    files.insert("base/snd/a.wav", &wav(1, 22_050, 16, 11_025));
    files.insert("base/snd/st.wav", &wav(2, 22_050, 16, 2_205));
    files.insert("base/snd/bad.wav", b"RIFF....WAVEjunk");
    files.insert("base/snd/x.png", b"png");
    let (mut vm, host) = rig_with(files);
    vm.set_step_limit(Some(50_000_000));
    (vm, host)
}

fn eval(vm: &mut Vm, code: &str) -> String {
    String::from_utf8_lossy(&vm.eval(code.as_bytes()).unwrap()).into_owned()
}

fn log(vm: &mut Vm) -> Vec<String> {
    vm.drain_log()
        .iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect()
}

#[test]
fn sources_play_on_the_host_clock() {
    let (mut vm, client) = rig();
    assert_eq!(eval(&mut vm, "return OpenALInitDriver();"), "1");
    eval(
        &mut vm,
        "new AudioDescription(D0) { volume = 1; type = 0; };",
    );
    assert_eq!(
        eval(
            &mut vm,
            "$h = alxCreateSource(D0, \"base/snd/a.wav\"); return $h;"
        ),
        "1"
    );
    assert_eq!(eval(&mut vm, "return alxIsPlaying($h);"), "0");
    assert_eq!(eval(&mut vm, "return alxPlay($h);"), "1");
    assert_eq!(eval(&mut vm, "return alxIsPlaying($h);"), "1");
    client.borrow().lock().advance(400.0);
    assert_eq!(eval(&mut vm, "return alxIsPlaying($h);"), "1");
    client.borrow().lock().advance(200.0);
    assert_eq!(eval(&mut vm, "return alxIsPlaying($h);"), "0");
    assert_eq!(
        eval(
            &mut vm,
            "return alxGetWaveLen(\"base/snd/a.wav\") SPC alxGetWaveLen(\"base/snd/none.wav\");"
        ),
        "500 0"
    );
    log(&mut vm);
    assert_eq!(
        eval(
            &mut vm,
            "return alxGetWaveLen(\"base/snd/bad.wav\") SPC alxGetWaveLen(\"base/snd/x.png\");"
        ),
        "0 0"
    );
    assert_eq!(
        log(&mut vm),
        [
            "alxGetWaveLen: invalid buffer",
            "alxGetWaveLen: invalid buffer"
        ]
    );
}

#[test]
fn profiles_play_and_say_what_is_missing() {
    let (mut vm, host) = rig();
    eval(
        &mut vm,
        "OpenALInitDriver(); new AudioDescription(D0) { type = 0; }; new AudioProfile(P0) { fileName = \"base/snd/a.wav\"; description = D0; preload = 1; }; new AudioProfile(P1) { fileName = \"base/snd/st.wav\"; description = D0; preload = 1; }; new AudioProfile(P2) { fileName = \"base/snd/st.wav\"; description = D0; };",
    );
    // Two profiles preload; the third has no `preload`.
    assert_eq!(host.borrow().preloaded.len(), 2);
    assert_eq!(eval(&mut vm, "return alxPlay(P0);"), "1");
    log(&mut vm);
    assert_eq!(eval(&mut vm, "return alxPlay(NoSuch);"), "0");
    assert_eq!(log(&mut vm), ["Unable to locate audio profile 'NoSuch'"]);
    assert_eq!(eval(&mut vm, "return alxCreateSource(NoSuch);"), "0");
    assert_eq!(
        log(&mut vm),
        ["Unable to locate audio profile/description 'NoSuch'"]
    );
    // A profile's id is a handle to alxPlay (atoi first): no such handle.
    assert_eq!(eval(&mut vm, "return alxPlay(P0.getId());"), "0");
}

#[test]
fn channels_listener_and_enums_answer_as_tge() {
    let (mut vm, _client) = rig();
    eval(&mut vm, "OpenALInitDriver();");
    assert_eq!(
        eval(
            &mut vm,
            "return alxGetChannelVolume(0) SPC alxGetChannelVolume(31);"
        ),
        "1 1"
    );
    assert_eq!(
        eval(
            &mut vm,
            "return alxSetChannelVolume(3, 0.25) SPC alxGetChannelVolume(3);"
        ),
        "1 0.25"
    );
    log(&mut vm);
    assert_eq!(
        eval(
            &mut vm,
            "return alxSetChannelVolume(32, 1) @ \"|\" @ alxGetChannelVolume(-1);"
        ),
        "0|0"
    );
    assert_eq!(
        log(&mut vm),
        [
            "alxSetChannelVolume: channel '32' out of range [0, 32]",
            "alxGetChannelVolume: invalid channel '-1'"
        ]
    );
    eval(&mut vm, "alxListenerf(AL_GAIN_LINEAR, 0);");
    assert_eq!(
        eval(&mut vm, "return alxGetListenerf(AL_GAIN_LINEAR);"),
        "0"
    );
    eval(&mut vm, "alxListenerf(AL_PITCH, 1);");
    assert_eq!(log(&mut vm), ["alxListenerf: invalid enum name 'AL_PITCH'"]);
    assert_eq!(eval(&mut vm, "return alGetString(AL_VENDOR);"), STRINGS[0]);
    assert_eq!(eval(&mut vm, "return alGetString(AL_GAIN);"), "");
    assert_eq!(log(&mut vm), ["alGetString: invalid enum name 'AL_GAIN'"]);
}

#[test]
fn the_gui_hook_plays_a_profile_once_per_call() {
    let (mut vm, client) = rig();
    eval(
        &mut vm,
        "OpenALInitDriver(); new AudioDescription(D0) { type = 0; }; new AudioProfile(P0) { fileName = \"base/snd/a.wav\"; description = D0; };",
    );
    let p = vm.objects().find(b"P0").unwrap();
    assert_eq!(play_profile(&mut vm, &client, own, p), 1);
    let d = vm.objects().find(b"D0").unwrap();
    assert_eq!(play_profile(&mut vm, &client, own, d), 0, "not a profile");
    assert!(client.borrow().lock().is_playing(1));
}

#[test]
fn a_loop_over_many_files_and_a_huge_file_end_at_the_step_limit() {
    let files = MemFiles::new();
    for i in 0..64 {
        files.insert(&format!("base/snd/m{i}.wav"), &wav(1, 22_050, 16, 200_000));
    }
    files.insert("base/snd/huge.wav", &wav(1, 22_050, 16, 4_000_000));
    let (mut vm, client) = rig_with(files);
    // 2,000,000 units: about 31,000 steps.
    vm.set_step_limit(Some(2_000_000 / ts_vm::vm::WORK_PER_STEP));
    let r = vm.eval(b"for ($i = 0; $i < 64; $i++) alxGetWaveLen(\"base/snd/m\" @ $i @ \".wav\");");
    assert!(matches!(r, Err(VmError::StepLimit)), "{r:?}");
    let loaded = client.borrow().cache.len();
    assert!(loaded < 10, "{loaded} files read before the limit");
    let r = vm.eval(b"return alxGetWaveLen(\"base/snd/huge.wav\");");
    assert!(matches!(r, Err(VmError::StepLimit)), "{r:?}");
}

#[test]
fn many_preloads_stay_inside_the_caps() {
    let (mut vm, client) = rig();
    vm.set_step_limit(Some(500_000_000));
    eval(
        &mut vm,
        "OpenALInitDriver(); new AudioDescription(D0) { type = 0; }; for ($i = 0; $i < 300; $i++) { new AudioProfile(\"P\" @ $i) { fileName = \"base/snd/a.wav\"; description = D0; preload = 1; }; }",
    );
    let c = client.borrow();
    assert_eq!(c.preloaded.len(), 300);
    assert_eq!(c.cache.len(), 1, "one file, one buffer");
    assert!(c.cache.pcm_bytes <= crate::MAX_TOTAL_PCM_BYTES);
}

#[test]
fn nothing_plays_before_the_driver() {
    let (mut vm, _client) = rig();
    eval(&mut vm, "new AudioDescription(D0) { type = 0; };");
    assert_eq!(
        eval(
            &mut vm,
            "return alxCreateSource(D0, \"base/snd/a.wav\") SPC alGetString(AL_VENDOR);"
        ),
        "0 "
    );
}

#[test]
fn al_get_string_answers_the_hosts_strings_then_its_report() {
    let (mut vm, host) = rig();
    eval(&mut vm, "OpenALInitDriver();");
    host.borrow_mut().strings[0] = "A mixer".to_string();
    assert_eq!(eval(&mut vm, "return alGetString(AL_VENDOR);"), "A mixer");
    {
        let mut h = host.borrow_mut();
        h.mixer = "game".to_string();
        h.report = Some(AudioReport {
            backend: "WASAPI".to_string(),
            device: Some(("Speakers".to_string(), 48_000, 2)),
        });
    }
    assert_eq!(
        eval(
            &mut vm,
            "return alGetString(AL_VENDOR) @ \"|\" @ alGetString(AL_RENDERER) @ \"|\" @ alGetString(AL_EXTENSIONS);"
        ),
        "game's own mixer|Speakers, 48000 Hz, 2 channels|None"
    );
}

#[test]
fn stream_times_are_minus_one_without_a_streamed_source() {
    let (mut vm, _host) = rig();
    eval(&mut vm, "OpenALInitDriver();");
    assert_eq!(eval(&mut vm, "return alxGetStreamPosition(0);"), "-1");
    assert_eq!(eval(&mut vm, "return alxGetStreamDuration(0);"), "-1");
    assert_eq!(eval(&mut vm, "return alxGetStreamPosition(77);"), "-1");
    assert_eq!(eval(&mut vm, "return alxGetStreamDuration(77);"), "-1");
}

#[test]
fn a_streamed_source_reports_its_times() {
    let (mut vm, _host) = rig();
    eval(
        &mut vm,
        "OpenALInitDriver(); new AudioDescription(DS) { type = 0; isStreaming = 1; }; new AudioProfile(PS) { fileName = \"base/snd/a.wav\"; description = DS; };",
    );
    let h = eval(&mut vm, "return alxPlay(alxCreateSource(PS));");
    assert_ne!(h, "0");
    assert_eq!(
        eval(&mut vm, &format!("return alxGetStreamDuration({h});")),
        "0.5"
    );
    assert_eq!(
        eval(&mut vm, &format!("return alxGetStreamPosition({h});")),
        "0"
    );
}

#[test]
fn without_a_sound_card_the_host_says_so_and_the_voices_run_on_the_clock() {
    let (mut vm, host) = rig();
    eval(&mut vm, "OpenALInitDriver();");
    // The window path found no output device (or could not open it): the host falls back to the null sink.
    host.borrow_mut().no_device();
    assert!(host.borrow().device.is_none());
    assert_eq!(host.borrow().report, Some(AudioReport::no_device()));
    assert_eq!(
        eval(&mut vm, "return alGetString(AL_RENDERER);"),
        "No output device (silent)"
    );
    // The natives work as with a sound card, on the simulation clock: a half-second sound is over after 600 ms of ticks.
    eval(
        &mut vm,
        "new AudioDescription(D0) { type = 0; }; $h = alxPlay(alxCreateSource(D0, \"base/snd/a.wav\"));",
    );
    assert_eq!(eval(&mut vm, "return alxIsPlaying($h);"), "1");
    tick(&vm, &host, own);
    assert!(vm.advance_time(600).is_empty());
    tick(&vm, &host, own);
    assert_eq!(eval(&mut vm, "return alxIsPlaying($h);"), "0");
}

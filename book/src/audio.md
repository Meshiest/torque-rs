# Sound

torque-rs plays sound the way TGE does: the server decides what is heard, each client mixes it. There is no OpenAL; the
`torque-audio` crate is the client's audio layer (the `alx` console functions, `AudioDescription` and `AudioProfile`,
WAV and Ogg Vorbis files, channels, a listener, looping sounds) over a mixer of its own, and `AudioPlugin` puts it on
every client the engine makes. Sound files are downloaded content, so every read is capped and gives a value or an
error, never a panic.

## From the server to the speakers

A server has three ways to make a client play something, each TGE's:

| Server | Wire | Client |
|---|---|---|
| `%shape.playAudio(slot, Profile)` / `%shape.stopAudio(slot)` on any ShapeBase | the object's update (`SoundMask`: one bit a slot, then whether it plays and the profile's id) | the slot's voice stops; with a profile a new one plays at the object and follows it |
| a 3D or 2D audio event (a jump sound, say) | the event | a one-shot at the event's point, or where the listener is |
| a mounted image's state with a sound | the image's state in the update | a one-shot at the object |

The slots are the common case. `playAudio` sets one of the object's four sound slots and marks its mask bit; the next
update of the object carries the slot in both network protocols, with TGE's bits in TGE's protocol. A slot whose profile
does not loop times out on the server after 500 ms (`sAudioTimeout`): the slot is free again and nothing is sent for it,
so the same profile can play in it anew. A looping one plays until `stopAudio`, another profile, or the object's end. A
client that meets the object while a slot plays is sent the slot in its first update.

```
// server: the kick, in the kicking player's first slot
datablock AudioDescription(KickDescription) { is3D = true; referenceDistance = 4; maxDistance = 40; };
datablock AudioProfile(KickSound) { fileName = "sounds/kick.wav"; description = KickDescription; };
%player.playAudio(0, KickSound);
```

On the client, `torque::app::sounds` takes the world's effects once a tick (after the connection's work, before the
plugins' ticks) and plays them on the audio plugin's host: a slot sound is kept by ghost and slot, moved to the ghost's
place every tick (`updateAudioPos`), stopped by the next update of the slot or when the ghost goes (also when the ghost
is simply no longer in the world). The effects run in their order, so a slot played and stopped in one batch ends
stopped. The listener is the
eye: its place, and its facing from the eye's yaw and pitch. The profile is the server's datablock id: it plays nothing
unless the object the client itself holds under that id is an `AudioProfile` (the client registers its copy of each
datablock under the server's id), and its description is read from the world's datablock record. `WorldSounds::of(vm)` answers what plays (the voice of a ghost's slot, how many voices
started), which is what the tests read.

## The sound card

The mixer plays through the null sink until something opens a device: every voice moves on the simulation clock, so
`alxIsPlaying` and the rest answer as they would with sound, and nothing is heard. Every test and headless run stays
there; no test opens a device.

A window client opens the default output device when the build has the `audio-device` feature (`torque/audio-device`,
which turns on `torque-app/audio-device` and `torque-audio/device`: cpal, ALSA on Linux). `window::run` does it before
the first frame and prints one line, the device's name or why none plays; a machine without one, or a device that refuses,
leaves the host on the null sink (`AudioHost::no_device`: `alGetString(AL_RENDERER)` then says "No output device
(silent)") and the game plays on. While a device runs, its callback mixes the voices and the host's tick no longer moves
them.

In a browser nothing opens a device yet: the page's mixer runs on the clock, silent. WebAudio needs a user gesture before
it may start, so the page has to open it on the first click or key; that is a follow-up.

## Maintaining

**Module map.** `torque-game/src/scene/shapebase.rs`: the slots (`SoundSlot`, `play_audio`, `stop_audio`,
`sound_timeout_step`, the server's `updateServerAudio`), called from the Camera's, the Player's and the vehicles' ticks;
`torque-game/src/sim/player/methods.rs`: `playAudio` and `stopAudio`. `torque-server/src/net/torque.rs`: the view
fields `sound_play` and `sound_profile` the ShapeBase part packs. `torque-ghosts/src/shapebase.rs`: the part's layout.
`torque-ghost-world/src/world.rs`: `apply_sounds`, the slot as an `Effect::ShapeSound`. `torque-app/src/sounds.rs`: the
effects on the mixer, the listener, the device. `torque-audio/src/natives.rs`: `AudioHost`, `open_device`, `no_device`.

**Rules a change must keep, each guarded by a test.**
- A server `playAudio` reaches the client's mixer as a voice playing at the object, in both protocols, and the voice
  follows the object; `stopAudio`, the ghost's end and a new world stop it (`crates/torque-app/tests/shape_sounds.rs`).
- A one-shot slot frees itself on the server after 500 ms and a looping one does not (`shape_sounds.rs`;
  `a_one_shot_sound_slot_times_out_on_the_server_and_a_looping_one_stays` in torque-game).
- The ShapeBase part's sound block is TGE's: a flag a slot, a flag for playing, the profile's id over the datablock range
  (`every_group_of_the_shapebase_part_has_its_fields` in torque-ghosts); the wire goldens
  (`crates/torque-trace-hash/tests/wire_golden.rs`) stay as they are.
- Without a sound card the host says so and the voices run on the clock
  (`without_a_sound_card_the_host_says_so_and_the_voices_run_on_the_clock` in torque-audio); no test opens a device.
- The soccer demo's kick is a slot sound that every client plays (`two_clients_download_the_mod_and_play_a_match` in
  `examples/mods-demo`).

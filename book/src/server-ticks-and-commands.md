# Server tick hooks and commands

Two seams let a plugin act on a server without a script function to call: work that runs on every server tick, and
commands a client may send, each held to limits before it runs.

## Tick hooks

The server's main loop runs one 32 ms tick at a time: the datagrams waiting, the process list's pass over the objects,
the send pass, then the Sim events. A plugin adds work around the process list's pass by pushing `ServerTick`s:

```rust,ignore
use torque_engine::{ServerTick, TickInfo, VmMut};

fn count(mut vm: VmMut, tick: TickInfo) {
    vm.set_global("$Plugin::Ticks", &tick.number.to_string());
}

fn build(engine: &mut EngineBuilder) {
    engine.push(ServerTick::before(|_: TickInfo| { /* before the objects tick */ }));
    engine.push(ServerTick::after(count));
}
```

- **Where.** `ServerTick::before` hooks run before the process list ticks the objects, `ServerTick::after` hooks right
  after it, before the send pass. Both come before the tick's Sim events (`schedule`), so a script's event of tick N sees what
  the hooks of tick N did.
- **Order.** Each group runs in the order the hooks were pushed, which is plugin order. A tick runs all the `before` hooks,
  the process list, then all the `after` hooks.
- **Parameters.** A hook is written like a recipe: a function of injected parameters (`VmMut`, `VmRef`, `Res<T>`,
  `ResMut<T>`, `Slot<dyn T>`, `&Engine`, `Local<T>`) and `TickInfo`, the tick's `number` (from 1) and `time_ms`
  (`number * 32`, whatever the host's pacing). The closure form `Fn(&mut Vm, &Engine, TickInfo) -> Result<(), String>` works
  too. `Local<T>` state lasts as long as the engine, so a hook that keeps state per server holds it in a resource or the
  script engine's extension.
- **Several ticks at once.** The first pass of a server covers its start-up and runs 32 ticks together, and a pass after a stall
  covers the ticks that were missed. The hooks run for each of those ticks, in tick order: every `before`, the process list over
  all of them, every `after`.
- **Errors.** A hook that returns an error prints it on the server's console (once, until its message changes) and the server
  goes on with the other hooks.
- **Nothing registered, nothing changed.** A server whose engine has no `ServerTick` ticks exactly as before: the hashes of the
  determinism scenarios do not move.

A plugin whose logic is driven by the map (switches, triggers, timers that objects of the map own) uses an `after` hook to step
its logic once a tick: it needs no script function that the game would have to call from a `schedule`. Hooks run on the
dedicated server and on the browser host's server alike, since both are a `GameServer`.

## Server commands

`commandToServer('Name', ..)` calls `serverCmdName(%client, ..)` on the server. TGE's behavior is unchanged: a command no plugin
declared runs as it always did. A plugin declares a command with its arguments and limits:

```rust,ignore
use torque_engine::command::{ArgType, CommandSpec};

fn wave(client: i32, times: i32, mut waves: ResMut<Waves>) { /* .. */ }

fn build(engine: &mut EngineBuilder) {
    let spec = CommandSpec::new()
        .arg("times", ArgType::Int)
        .rate(4, 8)            // 4 a second after a burst of 8, per client
        .max_arg_bytes(32);
    engine.server_command("Wave", spec, wave);
}
```

`server_command` registers `serverCmdWave(%client, times)` as a native (the handler's script arguments are the client and the
spec's arguments, checked when the engine is built). For a command whose function a script defines, in TorqueScript or Lua,
`server_command_limits("Wave", spec)` declares only the limits.

Before the function runs, the server holds each call a client sends to its spec:

1. **Rate.** Each client has a token bucket for each command: `burst` tokens, refilled at `rate` a second. The bucket counts
   server ticks, not the wall clock, so a run repeats exactly. A call with no token left is refused.
2. **Arguments.** The count (required and optional), the bytes of each argument (`max_arg_bytes`, 255 by default) and its type
   (`Text`, `Int`, `Float`, `Bool`) are checked. A call that fails never reaches the function. A malformed call spends a token
   too, so a flood of junk is limited like any other.

A refused call is dropped quietly and counted. `commandDrops(%client, "Wave")` returns the number of calls of that command the
client had refused, and while calls keep being refused the server prints one console line a second naming the client, the
command, how many were refused and why. Neither a refusal nor a flood drops the client. A call made from a script
(`serverCmdWave(%client, 3)`) is the server's own and is never limited.

Two plugins that declare one command name (without regard to case) fail the engine's build, and the error names both plugins.

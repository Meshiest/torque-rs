# Game template

The smallest game on the engine. Copy this folder to start a game; `src/main.rs` wires it and `game/` holds its scripts. Once copied out, point the `torque` dependency in `Cargo.toml` at a git or path source, since the engine is not on crates.io.

```text
just run       # a window with the game's name, joined to a server it runs itself
just server    # a dedicated server on 127.0.0.1:28000
just client    # a client with no window that joins it and prints what the client's script said
just test      # a client joins a server and leaves, with no window
```

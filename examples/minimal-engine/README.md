# The minimal example on the engine

The same game as [`examples/minimal`](../minimal), built on an `Engine` of `DefaultPlugins` and one small game plugin.

```text
just examples minimal-engine server    # a dedicated server on 127.0.0.1:28000
just examples minimal-engine client    # connects and prints the ghosts it is sent
just examples minimal-engine window    # the client in a window; add --local to run the server too
```

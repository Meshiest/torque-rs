//! One server and its clients in one process over the memory transport, stepped by hand: a tick of the server, then a step of
//! each client, the datagrams carried across around each. No clock is read and no socket is opened.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;

use torque::app::client::{ClientNet, GameClient, carry_between};
use torque::app::server::GameServer;
use torque::engine::Engine;

use crate::{Lua, Role, SERVER, client, client_files, engine, server, server_files};

/// The server's tick, and each client's step (ms).
pub const TICK_MS: u32 = 32;

/// A server and its clients.
pub struct Session {
    pub server: GameServer,
    pub clients: Vec<GameClient>,
    /// The engine of each client, for what its plugins keep (the rope plugin's ropes, the mods).
    pub engines: Vec<Engine>,
    /// Where the next client keeps its downloads.
    cache: PathBuf,
    lua: Option<Lua>,
    /// Whether the clients have the Lua extension too.
    lua_clients: bool,
}

impl Session {
    /// A server of the game (its mods found, its Lua extension loaded when `lua` is given) and no client. Clients' downloads go
    /// into folders under `cache`; they have the Lua extension as well when `lua_clients` is true, to show that a server's Lua
    /// never runs on them.
    pub fn new(lua: Option<Lua>, lua_clients: bool, cache: PathBuf) -> Result<Session, String> {
        let files = server_files().map_err(|e| e.to_string())?;
        let engine = engine(&files, Role::Server, lua.as_ref())?;
        Ok(Session {
            server: server(&engine)?,
            clients: Vec::new(),
            engines: vec![engine],
            cache,
            lua,
            lua_clients,
        })
    }

    /// The folder the client `n` keeps its downloads in.
    pub fn cache_of(&self, n: usize) -> PathBuf {
        self.cache.join(format!("client-{n}"))
    }

    /// Adds a client that has none of the game's mods. Its downloads go into [`Session::cache_of`] its number, which keeps what
    /// an earlier session left there.
    pub fn add_client(&mut self) -> Result<usize, String> {
        let n = self.clients.len();
        let files = client_files().map_err(|e| e.to_string())?;
        let lua = self.lua.as_ref().filter(|_| self.lua_clients);
        let engine = engine(
            &files,
            Role::Client {
                cache: self.cache_of(n),
            },
            lua,
        )?;
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_100 + n as u16);
        let client = client(
            &engine,
            ClientNet::Memory {
                client: addr,
                server: SERVER,
            },
        )?;
        self.clients.push(client);
        self.engines.push(engine);
        Ok(n)
    }

    /// Has client `n` connect to the server.
    pub fn connect(&mut self, n: usize) -> Result<(), String> {
        self.clients[n]
            .call("connectTo", &[&SERVER.to_string()])
            .map(drop)
    }

    /// Has client `n` leave.
    pub fn disconnect(&mut self, n: usize) -> Result<(), String> {
        self.clients[n].call("disconnect", &[]).map(drop)
    }

    fn carry(&self) {
        let refs: Vec<&GameClient> = self.clients.iter().collect();
        carry_between(&self.server, &refs);
    }

    /// One tick of the server, then one step of every client.
    pub fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        for client in &mut self.clients {
            client.step(TICK_MS);
        }
        self.carry();
    }

    /// `ticks` ticks.
    pub fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Ticks until `done` holds, at most `limit`; whether it did.
    pub fn run_until(&mut self, limit: u32, mut done: impl FnMut(&mut Session) -> bool) -> bool {
        for _ in 0..limit {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    /// A global of the server's script engine as text (empty when unset).
    pub fn server_global(&self, name: &str) -> String {
        String::from_utf8_lossy(&self.server.vm().global(name).unwrap_or_default()).into_owned()
    }

    /// A global of client `n`'s script engine as text.
    pub fn client_global(&self, n: usize, name: &str) -> String {
        self.clients[n].global(name)
    }

    /// Runs script `code` on the server and returns its result.
    pub fn server_eval(&mut self, code: &str) -> Result<String, String> {
        self.server
            .vm_mut()
            .eval(code.as_bytes())
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .map_err(|e| format!("{code}: {e}"))
    }

    /// The server's console since the last call.
    pub fn server_console(&mut self) -> Vec<String> {
        self.server
            .vm_mut()
            .drain_log()
            .into_iter()
            .map(|l| String::from_utf8_lossy(&l.text).into_owned())
            .collect()
    }
}

//! The example's client in a window (feature `window`): the engine's window drives an [`ExampleGame`], whose client's GUI
//! (`game/client.cs`) shows a status line and a box for each ghost, seen from above. With no server address the game runs the
//! server too, in the same process.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};

use torque::app::client::{ClientNet, GameClient};
use torque::app::orbit::Orbit;
use torque::app::server::Listen;
use torque::app::{App, Host, WorldView};
use torque::engine::Engine;
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::{client, ghosts, server};

/// The addresses of a game that runs its own server: the server's, and the client's on the in-memory link.
const SERVER: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50_000);

/// The game in a window: a client of the example server.
pub struct ExampleGame {
    /// The client; it hosts the server when the game has none to join.
    pub client: GameClient,
}

impl ExampleGame {
    /// A client of the server at `remote`, or with none, of a server it runs itself.
    pub fn new(engine: &Engine, remote: Option<SocketAddr>) -> Result<ExampleGame, String> {
        let net = match remote {
            Some(_) => ClientNet::Loopback,
            None => ClientNet::Memory {
                client: CLIENT,
                server: SERVER,
            },
        };
        let mut client = client(engine, net)?;
        if remote.is_none() {
            client.start_hosting(server(engine, Listen::Memory(SERVER))?);
        }
        client.call("connectTo", &[&remote.unwrap_or(SERVER).to_string()])?;
        Ok(ExampleGame { client })
    }

    /// The status line and the boxes, as the client's ghosts are now.
    fn show_ghosts(&mut self) {
        let status = match self.client.ghost_count() {
            0 => "connecting".to_string(),
            n => format!("{n} ghosts"),
        };
        let at: Vec<String> = ghosts(&self.client)
            .iter()
            .map(|(_, [x, y, _])| format!("{x} {y}"))
            .collect();
        if let Err(e) = self.client.call("showGhosts", &[&status, &at.join(" ")]) {
            eprintln!("example-window: {e}");
        }
    }
}

impl App for ExampleGame {
    fn host(&mut self) -> &mut Host {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.client.host
    }

    fn title(&self) -> String {
        "torque-example-engine".to_string()
    }

    fn log_name(&self) -> &str {
        "example-window"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        WorldView::new(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.client.step(ms);
        self.show_ghosts();
    }

    fn frame(&mut self, _: &mut WorldView) -> (GuiDraws, Eye) {
        (
            self.client.host.draw_frame(),
            Orbit::default().eye([0.0; 3]),
        )
    }
}

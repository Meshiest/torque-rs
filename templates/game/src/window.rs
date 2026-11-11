//! The game in a window: the engine's window drives a [`Window`], which holds the client and, with it, the server it runs
//! itself. The client's script (`game/client.cs`) makes the GUI that is drawn.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};

use torque::app::orbit::Orbit;
use torque::app::{Eye, Gpu, Host, WorldView};
use torque::gui::GuiDraws;
use torque::prelude::*;

use crate::{client, server};

/// Where the game's own server listens, and where its client is, on the in-memory link between them.
const SERVER: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50_000);

/// A client joined to a server in the same process.
pub struct Window {
    client: GameClient,
}

impl Window {
    pub fn new(engine: &Engine, files: &GameFiles) -> Result<Window, String> {
        let net = ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        };
        let mut client = client(engine, files, net)?;
        client.start_hosting(server(engine, files, Listen::Memory(SERVER))?);
        client.call("connectTo", &[&SERVER.to_string()])?;
        Ok(Window { client })
    }
}

impl App for Window {
    fn host(&mut self) -> &mut Host {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.client.host
    }

    fn title(&self) -> String {
        self.client.global("$Game::Name")
    }

    fn log_name(&self) -> &str {
        "game"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        WorldView::new(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.client.step(ms);
    }

    fn frame(&mut self, _: &mut WorldView) -> (GuiDraws, Eye) {
        (
            self.client.host.draw_frame(),
            Orbit::default().eye([0.0; 3]),
        )
    }
}

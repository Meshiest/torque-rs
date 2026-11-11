//! Doubles for the tests of crates that run a server. Behind the `testing` feature, which only dev-dependencies enable.

use std::rc::Rc;

use torque_net::huffman::Table;
use torque_server::net::NetGame;
use torque_server::net::torque::{NoGameEvents, TgeGhostGame, TorqueNet};

/// A net game of its own made of Torque's parts: Torque's string table and TGE's ghosts, with a connection part of its own.
#[derive(Debug, Default)]
pub struct TestNet(pub TorqueNet);

/// [`TestNet`]'s part of a connection.
#[derive(Debug, Default, Clone)]
pub struct TestConn {
    /// The length of the extra bytes the connect request carried.
    pub extra: usize,
}

impl NetGame for TestNet {
    type Events = NoGameEvents;
    type Ghosts = TgeGhostGame;
    type Conn = TestConn;

    fn string_table(&self) -> Rc<Table> {
        self.0.string_table()
    }

    fn conn_part(&self, extra: &[u8]) -> TestConn {
        TestConn { extra: extra.len() }
    }
}

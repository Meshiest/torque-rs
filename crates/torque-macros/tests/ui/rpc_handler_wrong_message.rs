use torque_engine::rpc::{RpcCx, RpcError};
use torque_engine::{EngineBuilder, NetMessage};

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Ask {
    #[net(max_len = 8)]
    what: String,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Answer {
    #[net(max_len = 8)]
    what: String,
}

fn main() {
    let mut engine = EngineBuilder::new();
    // The handler must return the reply type the RPC declared.
    engine
        .net()
        .rpc::<Ask, Answer>("ask")
        .to_server()
        .server(|_cx: &mut RpcCx, ask: Ask| -> Result<Ask, RpcError> { Ok(ask) });
}

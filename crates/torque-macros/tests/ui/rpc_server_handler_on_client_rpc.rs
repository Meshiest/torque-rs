use torque_engine::rpc::{RpcCx, RpcError};
use torque_engine::{EngineBuilder, NetMessage};

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Note {
    #[net(max_len = 8)]
    text: String,
}

fn main() {
    let mut engine = EngineBuilder::new();
    // The handler of a server-to-client RPC is `client`, not `server`.
    engine
        .net()
        .rpc::<Note, ()>("note")
        .to_client()
        .server(|_cx: &mut RpcCx, _note: Note| -> Result<(), RpcError> { Ok(()) });
}

use torque_engine::rpc::{RpcCx, RpcError};
use torque_engine::{EngineBuilder, NetMessage};

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Note {
    #[net(max_len = 8)]
    text: String,
}

fn main() {
    let mut engine = EngineBuilder::new();
    // `to_server` or `to_client` comes before the handler.
    engine
        .net()
        .rpc::<Note, ()>("note")
        .server(|_cx: &mut RpcCx, _note: Note| -> Result<(), RpcError> { Ok(()) });
}

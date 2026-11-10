use torque_engine::net_classes::{GhostRef, Xf};
use torque_engine::rpc::{Allow, Delivery, RpcCx, RpcError};
use torque_engine::{EngineBuilder, NetMessage};

// A type whose name only contains `ObjId` is not the raw object id.
type ObjIdentity = u32;

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct SpawnProp {
    #[net(max_len = 64)]
    data: String,
    #[net(finite)]
    at: Xf,
    #[net(bits = 8)]
    predict_id: u8,
    near: Option<GhostRef>,
    #[net(max_len = 4, finite)]
    offsets: Vec<f32>,
    identity: ObjIdentity,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Spawned {
    prop: GhostRef,
}

fn main() {
    let mut engine = EngineBuilder::new();
    engine
        .net()
        .rpc::<SpawnProp, Spawned>("props.spawn")
        .to_server()
        .delivery(Delivery::Reliable)
        .rate(4, 8)
        .max_bytes(256)
        .allow(Allow::Anyone)
        .server(|_cx: &mut RpcCx, _req: SpawnProp| -> Result<Spawned, RpcError> {
            Ok(Spawned::default())
        });
    engine
        .net()
        .rpc::<Spawned, ()>("props.spawned")
        .to_client()
        .client(|_cx: &mut RpcCx, _req: Spawned| -> Result<(), RpcError> { Ok(()) });
}

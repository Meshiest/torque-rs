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
    // A server never waits for a client's reply: only a notification may go to a client.
    engine.net().rpc::<Ask, Answer>("ask").to_client();
}

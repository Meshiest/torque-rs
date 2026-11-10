use torque_engine::NetMessage;

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Skipped {
    #[net(skip)]
    n: u32,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Masked {
    #[net(mask = 1)]
    n: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Unknown {
    #[net(colour = 1)]
    n: u32,
}

#[derive(Debug, Clone, PartialEq, Default, NetMessage)]
struct Tuple(u32);

fn main() {}

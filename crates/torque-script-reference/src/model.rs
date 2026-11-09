//! What the reference lists: plain data, read from script engines by [`crate::collect`] and written as pages by
//! [`crate::render`].

use std::collections::BTreeSet;

/// A script engine of a side: where an entry is registered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Side {
    Server,
    Client,
}

/// The sides an entry is in.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Sides(pub BTreeSet<Side>);

impl Sides {
    pub fn of(side: Side) -> Sides {
        Sides(BTreeSet::from([side]))
    }

    pub fn both(&self) -> bool {
        self.0.len() == 2
    }

    /// `server`, `client` or `server and client`.
    pub fn text(&self) -> &'static str {
        match (
            self.0.contains(&Side::Server),
            self.0.contains(&Side::Client),
        ) {
            (true, true) => "server and client",
            (true, false) => "server",
            (false, true) => "client",
            (false, false) => "",
        }
    }
}

/// How many script arguments a native takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Arity {
    pub min: u32,
    /// `None` for no limit.
    pub max: Option<u32>,
}

impl Arity {
    /// `none`, `2`, `1 to 3` or `at least 1`.
    pub fn text(&self) -> String {
        match (self.min, self.max) {
            (0, Some(0)) => "none".to_string(),
            (min, Some(max)) if min == max => min.to_string(),
            (min, Some(max)) => format!("{min} to {max}"),
            (min, None) => format!("at least {min}"),
        }
    }
}

/// The tag of an entry that comes with the client's prediction slot (any plugin with a predictor or a prediction part), in
/// place of one plugin's id.
pub const SLOT_TAG: &str = "prediction slot";

/// The tag of an entry that exists only in an engine that registers an RPC, in place of one plugin's id.
pub const RPC_TAG: &str = "rpc";

/// One console function or method.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Native {
    /// The namespace of a function (empty for the global one), or the class of a method.
    pub namespace: String,
    pub name: String,
    pub method: bool,
    pub arity: Arity,
    pub usage: String,
    /// What follows the first line of the doc comment of a native the attributes declared.
    pub docs: String,
    pub dev: bool,
    pub sides: Sides,
    /// The opt-in plugins (ids) that add it; empty for one of the default plugins.
    pub plugins: Vec<String>,
}

/// One console field of a class.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Field {
    pub name: String,
    pub ty: String,
    /// Elements of an array field; 1 for a plain one.
    pub count: u16,
    /// What a fresh object reads.
    pub default: String,
    /// `2 to 64`, when a write is clamped.
    pub range: Option<String>,
    pub doc: String,
    pub sides: Sides,
}

/// One console class and the fields it adds to its parent's.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Class {
    pub name: String,
    pub parent: String,
    pub fields: Vec<Field>,
    /// Declared with `#[derive(EngineClass)]`, so its field defaults are the declaration's. The defaults of the other
    /// classes are what the class table happens to hold, which is not what the original engine's constructors set.
    pub declared: bool,
    pub sides: Sides,
    pub plugins: Vec<String>,
}

/// One pref: a `$Pref::` global a plugin gives a default.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pref {
    pub name: String,
    pub ty: String,
    pub default: String,
    pub range: Option<String>,
    pub doc: String,
    pub sides: Sides,
    pub plugins: Vec<String>,
}

/// Who calls an RPC.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RpcDirection {
    /// A client calls it and the server handles it.
    ToServer,
    /// The server sends it and each client handles it.
    ToClient,
}

/// One field of an RPC's request or reply: what its message's layout says of it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MessageField {
    pub name: String,
    /// The field's Rust type as the message declares it.
    pub ty: String,
    /// An integer's width in bits; 0 for the type's own.
    pub bits: u8,
    /// A string's most bytes or a list's most elements; 0 for a type with no cap.
    pub max_len: usize,
    /// Whether a decoder refuses a NaN or an infinity.
    pub finite: bool,
}

/// One RPC of the sample engine: how it is called and the shape of its messages.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rpc {
    pub name: String,
    pub direction: RpcDirection,
    pub reliable: bool,
    /// Calls a second one connection may make.
    pub rate: u32,
    /// Calls one connection may make at once.
    pub burst: u32,
    /// The most bytes a message encodes to.
    pub max_bytes: usize,
    /// Who may call it, as a sentence fragment.
    pub allow: String,
    /// The script function that handles it, when the game's script does.
    pub script_handler: Option<String>,
    pub request: Vec<MessageField>,
    /// Empty for a notification.
    pub reply: Vec<MessageField>,
    /// Whether the call has a reply.
    pub has_reply: bool,
}

/// Everything the reference lists.
#[derive(Clone, Default, Debug)]
pub struct Reference {
    pub natives: Vec<Native>,
    pub classes: Vec<Class>,
    pub prefs: Vec<Pref>,
    /// The opt-in plugins the reference covers, as `(id, what it adds)`.
    pub optional: Vec<(String, String)>,
    /// The RPCs of the sample engine, in name order.
    pub rpcs: Vec<Rpc>,
}

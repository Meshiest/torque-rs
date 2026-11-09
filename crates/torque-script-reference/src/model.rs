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

/// Everything the reference lists.
#[derive(Clone, Default, Debug)]
pub struct Reference {
    pub natives: Vec<Native>,
    pub classes: Vec<Class>,
    pub prefs: Vec<Pref>,
    /// The opt-in plugins the reference covers, as `(id, what it adds)`.
    pub optional: Vec<(String, String)>,
}

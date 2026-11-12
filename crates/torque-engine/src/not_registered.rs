//! The refusal for an id nothing registered.

use std::fmt;

/// Why a choice by id was refused: nothing of that kind has the id. Carries the ids that are registered, so the message tells the
/// asker what to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotRegistered {
    /// The kind of thing asked for ("renderer", "predictor").
    pub what: &'static str,
    /// The id asked for.
    pub id: String,
    /// The ids registered, in the order they were registered.
    pub registered: Vec<String>,
}

impl NotRegistered {
    /// The refusal of `id` among the `registered` ids of `what`.
    pub fn new<'a>(
        what: &'static str,
        id: &str,
        registered: impl IntoIterator<Item = &'a str>,
    ) -> NotRegistered {
        NotRegistered {
            what,
            id: id.to_owned(),
            registered: registered.into_iter().map(str::to_owned).collect(),
        }
    }
}

impl fmt::Display for NotRegistered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no {} {:?} (registered: ", self.what, self.id)?;
        if self.registered.is_empty() {
            f.write_str("none")?;
        } else {
            f.write_str(&self.registered.join(", "))?;
        }
        f.write_str(")")
    }
}

impl std::error::Error for NotRegistered {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_names_the_kind_the_id_and_what_is_registered() {
        let some = NotRegistered::new("renderer", "x", ["a", "b"]);
        assert_eq!(some.to_string(), "no renderer \"x\" (registered: a, b)");
        let none = NotRegistered::new("predictor", "x", []);
        assert_eq!(none.to_string(), "no predictor \"x\" (registered: none)");
        assert_eq!(none.registered, Vec::<String>::new());
    }
}

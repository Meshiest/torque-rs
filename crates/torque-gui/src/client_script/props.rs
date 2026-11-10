//! A control's prop values: what the realm set, with the format's default for the rest.

use std::sync::LazyLock;

use torque_client_script_format::ui::{PROPS, Prop, PropDef, PropDefault, PropValue};

/// The format's definition of each prop, by id (a drawn control reads a handful of props every frame, most of them at their
/// default).
static DEFS: LazyLock<[Option<&'static PropDef>; 256]> = LazyLock::new(|| {
    let mut table = [None; 256];
    for def in PROPS {
        table[def.prop as usize] = Some(def);
    }
    table
});

fn prop_def(prop: Prop) -> &'static PropDef {
    DEFS[prop as usize].expect("PROPS has every prop")
}

/// Where prop values are read from: a control's [`Props`], the props an op gives, or one over the other ([`Over`]). Reading
/// through it never copies a value, so a check of an op costs what the op carries, not what the control holds.
pub(crate) trait Lookup {
    /// The value set for `prop`, if any.
    fn lookup(&self, prop: Prop) -> Option<&PropValue>;
}

/// The props an op gives, in its order: a prop given twice reads as the last one.
pub(crate) struct Given<'a>(pub(crate) &'a [(Prop, PropValue)]);

impl Lookup for Given<'_> {
    fn lookup(&self, prop: Prop) -> Option<&PropValue> {
        self.0
            .iter()
            .rev()
            .find(|(p, _)| *p == prop)
            .map(|(_, v)| v)
    }
}

/// What `top` sets, else what `base` has.
pub(crate) struct Over<'a> {
    pub(crate) top: &'a dyn Lookup,
    pub(crate) base: &'a dyn Lookup,
}

impl Lookup for Over<'_> {
    fn lookup(&self, prop: Prop) -> Option<&PropValue> {
        self.top.lookup(prop).or_else(|| self.base.lookup(prop))
    }
}

/// A Bool prop of `props`, else its default.
pub(crate) fn flag_of(props: &dyn Lookup, prop: Prop) -> bool {
    match props.lookup(prop) {
        Some(PropValue::Bool(b)) => *b,
        _ => matches!(prop_def(prop).default, PropDefault::Bool(true)),
    }
}

/// A Number prop of `props`, else its default.
pub(crate) fn num_of(props: &dyn Lookup, prop: Prop) -> f64 {
    match props.lookup(prop) {
        Some(PropValue::Number(n)) => *n,
        _ => match prop_def(prop).default {
            PropDefault::Number(n) => n,
            _ => 0.0,
        },
    }
}

/// An Int prop of `props`, else its default.
pub(crate) fn int_of(props: &dyn Lookup, prop: Prop) -> u32 {
    match props.lookup(prop) {
        Some(PropValue::Int(n)) => *n,
        _ => match prop_def(prop).default {
            PropDefault::Int(n) => n,
            _ => 0,
        },
    }
}

/// A Vec2 prop of `props`, else its default.
pub(crate) fn vec2_of(props: &dyn Lookup, prop: Prop) -> (f64, f64) {
    match props.lookup(prop) {
        Some(PropValue::Vec2(x, y)) => (*x, *y),
        _ => match prop_def(prop).default {
            PropDefault::Vec2(x, y) => (x, y),
            _ => (0.0, 0.0),
        },
    }
}

/// A text, rich text or image prop of `props`, else empty.
pub(crate) fn text_of(props: &dyn Lookup, prop: Prop) -> &str {
    match props.lookup(prop) {
        Some(PropValue::Text(t)) => t,
        _ => "",
    }
}

/// A list's rows in `props`, else none.
pub(crate) fn rows_of(props: &dyn Lookup) -> &[String] {
    match props.lookup(Prop::Rows) {
        Some(PropValue::Rows(rows)) => rows,
        _ => &[],
    }
}

/// The props a control has been given: few, so a list that is searched. Reading one it was never given reads the format's
/// default.
#[derive(Debug, Clone, Default)]
pub(crate) struct Props(Vec<(Prop, PropValue)>);

impl Lookup for Props {
    fn lookup(&self, prop: Prop) -> Option<&PropValue> {
        self.0.iter().find(|(p, _)| *p == prop).map(|(_, v)| v)
    }
}

impl Props {
    pub(crate) fn set(&mut self, prop: Prop, value: PropValue) {
        match self.0.iter_mut().find(|(p, _)| *p == prop) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((prop, value)),
        }
    }

    pub(crate) fn flag(&self, prop: Prop) -> bool {
        flag_of(self, prop)
    }

    pub(crate) fn num(&self, prop: Prop) -> f64 {
        num_of(self, prop)
    }

    pub(crate) fn int(&self, prop: Prop) -> u32 {
        int_of(self, prop)
    }

    pub(crate) fn vec2(&self, prop: Prop) -> (f64, f64) {
        vec2_of(self, prop)
    }

    pub(crate) fn color(&self, prop: Prop) -> [u8; 4] {
        match self.lookup(prop) {
            Some(PropValue::Color(c)) => *c,
            _ => match prop_def(prop).default {
                PropDefault::Color(c) => c,
                _ => [0; 4],
            },
        }
    }

    /// An enum prop's index.
    pub(crate) fn index(&self, prop: Prop) -> u8 {
        match self.lookup(prop) {
            Some(PropValue::Enum(i)) => *i,
            _ => match prop_def(prop).default {
                PropDefault::Enum(i) => i,
                _ => 0,
            },
        }
    }

    /// A text, rich text or image prop.
    pub(crate) fn text(&self, prop: Prop) -> &str {
        text_of(self, prop)
    }

    pub(crate) fn rows(&self) -> &[String] {
        rows_of(self)
    }
}

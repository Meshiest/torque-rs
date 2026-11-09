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

/// The props a control has been given: few, so a list that is searched. Reading one it was never given reads the format's
/// default.
#[derive(Debug, Clone, Default)]
pub(crate) struct Props(Vec<(Prop, PropValue)>);

impl Props {
    pub(crate) fn set(&mut self, prop: Prop, value: PropValue) {
        match self.0.iter_mut().find(|(p, _)| *p == prop) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((prop, value)),
        }
    }

    fn get(&self, prop: Prop) -> Option<&PropValue> {
        self.0.iter().find(|(p, _)| *p == prop).map(|(_, v)| v)
    }

    pub(crate) fn flag(&self, prop: Prop) -> bool {
        match self.get(prop) {
            Some(PropValue::Bool(b)) => *b,
            _ => matches!(prop_def(prop).default, PropDefault::Bool(true)),
        }
    }

    pub(crate) fn num(&self, prop: Prop) -> f64 {
        match self.get(prop) {
            Some(PropValue::Number(n)) => *n,
            _ => match prop_def(prop).default {
                PropDefault::Number(n) => n,
                _ => 0.0,
            },
        }
    }

    pub(crate) fn int(&self, prop: Prop) -> u32 {
        match self.get(prop) {
            Some(PropValue::Int(n)) => *n,
            _ => match prop_def(prop).default {
                PropDefault::Int(n) => n,
                _ => 0,
            },
        }
    }

    pub(crate) fn vec2(&self, prop: Prop) -> (f64, f64) {
        match self.get(prop) {
            Some(PropValue::Vec2(x, y)) => (*x, *y),
            _ => match prop_def(prop).default {
                PropDefault::Vec2(x, y) => (x, y),
                _ => (0.0, 0.0),
            },
        }
    }

    pub(crate) fn color(&self, prop: Prop) -> [u8; 4] {
        match self.get(prop) {
            Some(PropValue::Color(c)) => *c,
            _ => match prop_def(prop).default {
                PropDefault::Color(c) => c,
                _ => [0; 4],
            },
        }
    }

    /// An enum prop's index.
    pub(crate) fn index(&self, prop: Prop) -> u8 {
        match self.get(prop) {
            Some(PropValue::Enum(i)) => *i,
            _ => match prop_def(prop).default {
                PropDefault::Enum(i) => i,
                _ => 0,
            },
        }
    }

    /// A text, rich text or image prop.
    pub(crate) fn text(&self, prop: Prop) -> &str {
        match self.get(prop) {
            Some(PropValue::Text(t)) => t,
            _ => "",
        }
    }

    pub(crate) fn rows(&self) -> &[String] {
        match self.get(Prop::Rows) {
            Some(PropValue::Rows(rows)) => rows,
            _ => &[],
        }
    }
}

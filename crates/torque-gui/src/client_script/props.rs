//! A control's prop values: what the realm set, with the format's default for the rest.

use std::collections::HashMap;

use torque_client_script_format::ui::{Prop, PropDefault, PropValue, prop_def};

/// The props a control has been given. Reading one it was never given reads the format's default.
#[derive(Debug, Clone, Default)]
pub(crate) struct Props(HashMap<Prop, PropValue>);

impl Props {
    pub(crate) fn set(&mut self, prop: Prop, value: PropValue) {
        self.0.insert(prop, value);
    }

    pub(crate) fn flag(&self, prop: Prop) -> bool {
        match self.0.get(&prop) {
            Some(PropValue::Bool(b)) => *b,
            _ => matches!(prop_def(prop).default, PropDefault::Bool(true)),
        }
    }

    pub(crate) fn num(&self, prop: Prop) -> f64 {
        match self.0.get(&prop) {
            Some(PropValue::Number(n)) => *n,
            _ => match prop_def(prop).default {
                PropDefault::Number(n) => n,
                _ => 0.0,
            },
        }
    }

    pub(crate) fn int(&self, prop: Prop) -> u32 {
        match self.0.get(&prop) {
            Some(PropValue::Int(n)) => *n,
            _ => match prop_def(prop).default {
                PropDefault::Int(n) => n,
                _ => 0,
            },
        }
    }

    pub(crate) fn vec2(&self, prop: Prop) -> (f64, f64) {
        match self.0.get(&prop) {
            Some(PropValue::Vec2(x, y)) => (*x, *y),
            _ => match prop_def(prop).default {
                PropDefault::Vec2(x, y) => (x, y),
                _ => (0.0, 0.0),
            },
        }
    }

    pub(crate) fn color(&self, prop: Prop) -> [u8; 4] {
        match self.0.get(&prop) {
            Some(PropValue::Color(c)) => *c,
            _ => match prop_def(prop).default {
                PropDefault::Color(c) => c,
                _ => [0; 4],
            },
        }
    }

    /// An enum prop's index.
    pub(crate) fn index(&self, prop: Prop) -> u8 {
        match self.0.get(&prop) {
            Some(PropValue::Enum(i)) => *i,
            _ => match prop_def(prop).default {
                PropDefault::Enum(i) => i,
                _ => 0,
            },
        }
    }

    /// A text, rich text or image prop.
    pub(crate) fn text(&self, prop: Prop) -> &str {
        match self.0.get(&prop) {
            Some(PropValue::Text(t)) => t,
            _ => "",
        }
    }

    pub(crate) fn rows(&self) -> &[String] {
        match self.0.get(&Prop::Rows) {
            Some(PropValue::Rows(rows)) => rows,
            _ => &[],
        }
    }
}

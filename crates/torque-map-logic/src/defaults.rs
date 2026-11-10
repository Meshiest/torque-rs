//! The coded defaults of the supported classes: the value a keyvalue has when the map leaves it out, and the spawnflags an
//! entity has when the map gives none. Written here from the entities' public documentation (facts: names, numbers); no
//! definition file is copied. A [`Defaults`] puts them under an FGD's, when one was read: the FGD's default wins where both
//! exist, and each difference is reported once.
//!
//! Where games disagree (a button's speed and wait), the numbers are the older games'; an FGD read from the user's install
//! replaces them with that game's own.

use std::collections::BTreeSet;

use crate::fgd::EntityDefs;

/// One class's coded defaults.
#[derive(Debug, Clone, Copy)]
pub struct ClassDefaults {
    pub class: &'static str,
    pub keys: &'static [(&'static str, &'static str)],
    pub spawnflags: u32,
}

/// The coded defaults, by class.
pub const CODED: &[ClassDefaults] = &[
    ClassDefaults {
        class: "logic_relay",
        keys: &[("StartDisabled", "0")],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "logic_auto",
        keys: &[],
        spawnflags: 1,
    },
    ClassDefaults {
        class: "logic_timer",
        keys: &[
            ("UseRandomTime", "0"),
            ("StartDisabled", "0"),
            ("RefireTime", "0"),
            ("LowerRandomBound", "0"),
            ("UpperRandomBound", "0"),
        ],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "logic_case",
        keys: &[],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "logic_compare",
        keys: &[("InitialValue", "0"), ("CompareValue", "0")],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "logic_branch",
        keys: &[("InitialValue", "0")],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "math_counter",
        keys: &[
            ("startvalue", "0"),
            ("min", "0"),
            ("max", "0"),
            ("StartDisabled", "0"),
        ],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "trigger_multiple",
        keys: &[("wait", "1"), ("StartDisabled", "0")],
        spawnflags: 1,
    },
    ClassDefaults {
        class: "trigger_once",
        keys: &[("StartDisabled", "0")],
        spawnflags: 1,
    },
    ClassDefaults {
        class: "filter_activator_name",
        keys: &[("Negated", "0")],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "filter_activator_class",
        keys: &[("Negated", "0")],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "filter_multi",
        keys: &[("filtertype", "0"), ("Negated", "0")],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "func_door",
        keys: &[
            ("speed", "100"),
            ("movedir", "0 0 0"),
            ("lip", "0"),
            ("wait", "4"),
            ("spawnpos", "0"),
            ("dmg", "0"),
            ("forceclosed", "0"),
        ],
        spawnflags: 1024,
    },
    ClassDefaults {
        class: "func_door_rotating",
        keys: &[
            ("speed", "100"),
            ("lip", "0"),
            ("wait", "4"),
            ("distance", "90"),
            ("spawnpos", "0"),
            ("dmg", "0"),
            ("forceclosed", "0"),
        ],
        spawnflags: 1024,
    },
    ClassDefaults {
        class: "func_button",
        keys: &[
            ("movedir", "0 0 0"),
            ("speed", "5"),
            ("lip", "0"),
            ("wait", "3"),
        ],
        spawnflags: 1024,
    },
    ClassDefaults {
        class: "func_movelinear",
        keys: &[
            ("movedir", "0 0 0"),
            ("movedistance", "100"),
            ("startposition", "0"),
            ("speed", "100"),
            ("blockdamage", "0"),
        ],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "func_brush",
        keys: &[("Solidity", "0"), ("StartDisabled", "0")],
        spawnflags: 0,
    },
    ClassDefaults {
        class: "ambient_generic",
        keys: &[
            ("message", ""),
            ("health", "10"),
            ("radius", "1250"),
            ("pitch", "100"),
            ("pitchstart", "100"),
            ("fadeinsecs", "0"),
            ("fadeoutsecs", "0"),
        ],
        spawnflags: 16 | 32,
    },
];

/// The coded defaults of `class` (any case).
pub fn coded(class: &str) -> Option<&'static ClassDefaults> {
    CODED.iter().find(|c| c.class.eq_ignore_ascii_case(class))
}

/// Whether two default texts mean the same (equal as text, or as numbers).
fn same(a: &str, b: &str) -> bool {
    if a.trim() == b.trim() {
        return true;
    }
    let num = |s: &str| {
        let t = s.trim();
        (!t.is_empty() && t.split_whitespace().count() <= 3).then(|| {
            t.split_whitespace()
                .map(crate::value::atof)
                .collect::<Vec<_>>()
        })
    };
    matches!((num(a), num(b)), (Some(x), Some(y)) if x == y)
}

/// The defaults a spawn reads: an FGD's when one was read, the coded ones under them.
#[derive(Debug, Default)]
pub struct Defaults {
    pub defs: Option<EntityDefs>,
    reported: BTreeSet<(String, String)>,
    /// One line for each class and key where the FGD and the coded default disagree (the FGD's is used).
    pub differences: Vec<String>,
}

impl Defaults {
    /// Coded defaults only.
    pub fn coded() -> Defaults {
        Defaults::default()
    }

    /// An FGD's defaults over the coded ones.
    pub fn with_fgd(defs: EntityDefs) -> Defaults {
        Defaults {
            defs: Some(defs),
            ..Defaults::default()
        }
    }

    /// The default of `key` for `class`: the FGD's, else the coded one.
    pub fn key(&mut self, class: &str, key: &str) -> Option<String> {
        let coded = coded(class).and_then(|c| {
            c.keys
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| *v)
        });
        let fgd = self
            .defs
            .as_ref()
            .and_then(|d| d.class(class))
            .and_then(|c| c.key(key).and_then(|k| k.default.clone()));
        match (fgd, coded) {
            (Some(f), Some(c)) => {
                if !same(&f, c) {
                    self.report(class, key, || {
                        format!("{class}.{key}: the FGD's default {f:?} replaces the coded {c:?}")
                    });
                }
                Some(f)
            }
            (Some(f), None) => Some(f),
            (None, c) => c.map(str::to_string),
        }
    }

    /// The spawnflags of `class` when the map gives none: the FGD's default bits, else the coded ones.
    pub fn spawnflags(&mut self, class: &str) -> u32 {
        let coded = coded(class).map(|c| c.spawnflags);
        let fgd = self
            .defs
            .as_ref()
            .and_then(|d| d.class(class))
            .and_then(|c| c.default_spawnflags());
        match (fgd, coded) {
            (Some(f), Some(c)) => {
                if f != c {
                    self.report(class, "spawnflags", || {
                        format!("{class}.spawnflags: the FGD's default {f} replaces the coded {c}")
                    });
                }
                f
            }
            (Some(f), None) => f,
            (None, c) => c.unwrap_or(0),
        }
    }

    /// Whether the FGD (when read) knows `output` for `class`.
    pub fn knows_output(&self, class: &str, output: &str) -> bool {
        self.defs
            .as_ref()
            .and_then(|d| d.class(class))
            .is_some_and(|c| c.has_output(output))
    }

    fn report(&mut self, class: &str, key: &str, line: impl FnOnce() -> String) {
        if self.differences.len() < 1024
            && self
                .reported
                .insert((class.to_ascii_lowercase(), key.to_ascii_lowercase()))
        {
            self.differences.push(line());
        }
    }
}

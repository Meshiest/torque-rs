//! What the player's settings and a server's remembered choice come to: no scripts, a realm with these abilities, or a question.
//!
//! The global setting says whether servers may run scripts at all. Each ability has a policy: ask, always or never. A server the
//! player decided about before has a remembered choice: never, or a yes or no for each ability it was asked. [`decide`] turns
//! the server's request and all of that into a [`Decision`]; it never reads a file and never starts anything.

use torque_client_script_format::{Grants, Tier};

use crate::abilities::{ABILITIES, default_on, is_basic};

/// The global "Server scripts" setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Setting {
    /// No server runs scripts in this game. The default until the feature is approved.
    #[default]
    Never,
    /// A server that asks is asked about.
    Ask,
}

/// What the player decided for one ability, for every server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    /// Ask on each server that has not been decided.
    #[default]
    Ask,
    /// Grant it without asking.
    Always,
    /// Never grant it; its line is shown grayed and unticked.
    Never,
}

/// The per-ability policies, and the option to grant the basic abilities (menus, HUD, markers, sounds) without asking.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policies {
    per: [Policy; Tier::ALL.len()],
    /// Treat [`Policy::Ask`] as [`Policy::Always`] for the basic abilities. Off by default.
    pub basics_without_asking: bool,
}

impl Policies {
    /// The policy that applies to `tier` now: the one set, with the basics option applied.
    pub fn get(&self, tier: Tier) -> Policy {
        match self.per[usize::from(tier.id())] {
            Policy::Ask if self.basics_without_asking && is_basic(tier) => Policy::Always,
            other => other,
        }
    }

    /// Sets the policy of `tier` (the base cannot be set).
    pub fn set(&mut self, tier: Tier, policy: Policy) {
        if tier != Tier::Realm {
            self.per[usize::from(tier.id())] = policy;
        }
    }
}

/// What the player decided for one server before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Remembered {
    /// No scripts from this server.
    Never,
    /// A yes or no for each ability it was asked.
    Tiers(Vec<(Tier, bool)>),
}

impl Remembered {
    /// The decision for `tier`, if there is one.
    pub fn get(&self, tier: Tier) -> Option<bool> {
        match self {
            Remembered::Never => Some(false),
            Remembered::Tiers(list) => list.iter().find(|(t, _)| *t == tier).map(|(_, on)| *on),
        }
    }
}

/// Everything [`decide`] looks at.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    /// The global setting.
    pub setting: Setting,
    /// The per-ability policies.
    pub policies: &'a Policies,
    /// What was remembered for this server.
    pub remembered: Option<&'a Remembered>,
    /// The abilities the server asked for.
    pub requested: &'a [Tier],
    /// The abilities this build can offer: reviewed, with their host modules compiled in.
    pub offerable: Grants,
    /// The player's reduced-motion setting.
    pub reduce_motion: bool,
}

/// Why no scripts run, and no question is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The global setting is never.
    Setting,
    /// The player chose never for this server.
    RememberedNever,
    /// The server asked for no ability this build offers.
    NothingAsked,
    /// Every ability it asked for is refused (by policy or by a remembered no).
    Refused,
}

/// A line of the question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AskRow {
    /// The ability.
    pub tier: Tier,
    /// Whether its box starts ticked.
    pub on: bool,
    /// `Some(state)` when the line cannot be changed here: a policy or an earlier answer decided it.
    pub locked: Option<bool>,
}

/// What to do about a server's request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Run no scripts and ask nothing.
    Off(Why),
    /// Run a realm with these abilities and ask nothing.
    Run(Grants),
    /// Ask, with these lines. The abilities a policy always grants are not listed; [`answer`] adds them.
    Ask {
        /// The lines, in the order a prompt lists abilities.
        rows: Vec<AskRow>,
        /// The abilities granted without a line.
        always: Grants,
    },
}

/// The abilities a request comes to: asked for, offered by this build, each once, in [`ABILITIES`] order.
pub fn asked(requested: &[Tier], offerable: Grants) -> Vec<Tier> {
    ABILITIES
        .into_iter()
        .filter(|t| requested.contains(t) && offerable.has(*t))
        .collect()
}

/// What `inputs` come to.
pub fn decide(inputs: &Inputs<'_>) -> Decision {
    if inputs.setting == Setting::Never {
        return Decision::Off(Why::Setting);
    }
    if matches!(inputs.remembered, Some(Remembered::Never)) {
        return Decision::Off(Why::RememberedNever);
    }
    let asked = asked(inputs.requested, inputs.offerable);
    if asked.is_empty() {
        return Decision::Off(Why::NothingAsked);
    }
    let mut granted = Grants(1);
    let mut always = Grants(1);
    let mut rows = Vec::new();
    let mut undecided = false;
    for tier in asked {
        let before = inputs.remembered.and_then(|r| r.get(tier));
        match inputs.policies.get(tier) {
            Policy::Always => {
                granted = granted.with(tier);
                always = always.with(tier);
            }
            Policy::Never => rows.push(AskRow {
                tier,
                on: false,
                locked: Some(false),
            }),
            Policy::Ask => match before {
                Some(on) => {
                    if on {
                        granted = granted.with(tier);
                    }
                    rows.push(AskRow {
                        tier,
                        on,
                        locked: Some(on),
                    });
                }
                None => {
                    undecided = true;
                    rows.push(AskRow {
                        tier,
                        on: default_on(tier, inputs.reduce_motion),
                        locked: None,
                    });
                }
            },
        }
    }
    if undecided {
        return Decision::Ask { rows, always };
    }
    if granted == Grants(1) {
        Decision::Off(Why::Refused)
    } else {
        Decision::Run(granted)
    }
}

/// What the player's tick marks in a question come to: the abilities granted (the policies' always, the earlier yeses and the
/// ticked free lines; never a locked refusal) and the decisions to remember (every free line, ticked or not).
pub fn answer(
    always: Grants,
    rows: &[AskRow],
    ticked: &[(Tier, bool)],
) -> (Grants, Vec<(Tier, bool)>) {
    let mut granted = always;
    let mut decided = Vec::new();
    for row in rows {
        match row.locked {
            Some(true) => granted = granted.with(row.tier),
            Some(false) => {}
            None => {
                let on = ticked
                    .iter()
                    .find(|(t, _)| *t == row.tier)
                    .is_some_and(|(_, on)| *on);
                if on {
                    granted = granted.with(row.tier);
                }
                decided.push((row.tier, on));
            }
        }
    }
    (granted, decided)
}

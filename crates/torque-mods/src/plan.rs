//! From the mods found to the mods loaded, in order: duplicate ids, the side, prerequisites (judged by [`ModPrereqs`]) and the
//! load order (made by [`ModOrder`]). Pure: it reads no files and runs no script.

use crate::info::ModInfo;
use crate::layout::{Action, ModOrder, ModPrereqs, Problem};

/// Which script engines load a mod.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModSide {
    /// A server's.
    Server,
    /// A client's.
    Client,
}

/// A mod left out, and why.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Skipped {
    pub(crate) reason: String,
    /// Not a fault: a mod for the other side. No warning is printed.
    pub(crate) quiet: bool,
}

pub(crate) struct Plan {
    /// Positions in the list given, in load order.
    pub(crate) order: Vec<usize>,
    pub(crate) skipped: Vec<(usize, Skipped)>,
    /// What the order was repaired for (a resolver that left mods out).
    pub(crate) notes: Vec<String>,
}

pub(crate) struct Inputs<'a> {
    pub(crate) side: ModSide,
    pub(crate) has_plugin: &'a dyn Fn(&str) -> bool,
    pub(crate) order: &'a dyn ModOrder,
    pub(crate) prereqs: &'a dyn ModPrereqs,
}

/// The plan for `infos`, or the message of a problem the policy made an error.
pub(crate) fn plan(infos: &[ModInfo], inputs: &Inputs<'_>) -> Result<Plan, String> {
    let mut skipped: Vec<(usize, Skipped)> = Vec::new();
    let mut alive: Vec<usize> = Vec::new();
    for (at, info) in infos.iter().enumerate() {
        if alive.iter().any(|&a| {
            infos
                .get(a)
                .is_some_and(|i| i.id.eq_ignore_ascii_case(&info.id))
        }) {
            skipped.push((
                at,
                Skipped {
                    reason: format!("another mod already has the id `{}`", info.id),
                    quiet: false,
                },
            ));
        } else if info.client_side && inputs.side == ModSide::Server {
            skipped.push((
                at,
                Skipped {
                    reason: "a client-side mod; a server does not load it".into(),
                    quiet: true,
                },
            ));
        } else {
            alive.push(at);
        }
    }
    let mut notes = Vec::new();
    loop {
        // Prerequisites, until no mod is dropped for one.
        loop {
            let mut dropped: Vec<(usize, String)> = Vec::new();
            for &at in &alive {
                let Some(info) = infos.get(at) else { continue };
                for problem in problems(info, infos, &alive, inputs) {
                    match inputs.prereqs.decide(&info.id, &problem) {
                        Action::Skip => {
                            dropped.push((at, problem.to_string()));
                            break;
                        }
                        Action::Error => return Err(format!("the mod `{}`: {problem}", info.id)),
                        Action::Ignore => {}
                    }
                }
            }
            if dropped.is_empty() {
                break;
            }
            for (at, reason) in dropped {
                alive.retain(|&a| a != at);
                skipped.push((
                    at,
                    Skipped {
                        reason,
                        quiet: false,
                    },
                ));
            }
        }
        let list: Vec<&ModInfo> = alive.iter().filter_map(|&a| infos.get(a)).collect();
        match inputs.order.order(&list) {
            Ok(order) => {
                let order = repair(order, list.len(), &mut notes);
                let order = order
                    .into_iter()
                    .filter_map(|i| alive.get(i).copied())
                    .collect();
                return Ok(Plan {
                    order,
                    skipped,
                    notes,
                });
            }
            Err(error) => {
                let problem = Problem::Cycle {
                    path: error.path.clone(),
                };
                let first = error
                    .stuck
                    .first()
                    .and_then(|&i| list.get(i))
                    .map_or("", |m| m.id.as_str());
                match inputs.prereqs.decide(first, &problem) {
                    Action::Error => {
                        return Err(format!("the mod `{first}`: {problem}"));
                    }
                    Action::Ignore => {
                        let mut order: Vec<usize> = error.placed.clone();
                        order.extend(error.stuck.iter().copied());
                        let order = repair(order, list.len(), &mut notes);
                        let order = order
                            .into_iter()
                            .filter_map(|i| alive.get(i).copied())
                            .collect();
                        return Ok(Plan {
                            order,
                            skipped,
                            notes,
                        });
                    }
                    Action::Skip => {
                        let stuck: Vec<usize> = error
                            .stuck
                            .iter()
                            .filter_map(|&i| alive.get(i).copied())
                            .collect();
                        if stuck.is_empty() {
                            // A resolver that reports no mod stuck cannot be helped: no order.
                            return Err(
                                "the load order has a loop the resolver did not name".into()
                            );
                        }
                        for at in stuck {
                            alive.retain(|&a| a != at);
                            skipped.push((
                                at,
                                Skipped {
                                    reason: problem.to_string(),
                                    quiet: false,
                                },
                            ));
                        }
                    }
                }
            }
        }
    }
}

/// The problems of `info`'s prerequisites, given the mods still alive.
fn problems(
    info: &ModInfo,
    infos: &[ModInfo],
    alive: &[usize],
    inputs: &Inputs<'_>,
) -> Vec<Problem> {
    let mut found = Vec::new();
    for id in &info.requires_plugins {
        if !(inputs.has_plugin)(id) {
            found.push(Problem::MissingPlugin { id: id.clone() });
        }
    }
    for need in &info.requires {
        let provider = alive
            .iter()
            .filter_map(|&a| infos.get(a))
            .find(|m| m.id.eq_ignore_ascii_case(&need.id));
        match provider {
            None => found.push(Problem::MissingMod {
                id: need.id.clone(),
            }),
            Some(other) if !need.accepts(&other.version) => found.push(Problem::WrongVersion {
                needs: need.clone(),
                have: other.version.to_string(),
            }),
            Some(_) => {}
        }
    }
    found
}

/// `order` as a permutation of `0..len`: repeats and out-of-range entries dropped, the missing ones appended in list order.
fn repair(order: Vec<usize>, len: usize, notes: &mut Vec<String>) -> Vec<usize> {
    let mut seen = vec![false; len];
    let mut fixed = Vec::with_capacity(len);
    let mut bad = false;
    for at in order {
        match seen.get_mut(at) {
            Some(flag) if !*flag => {
                *flag = true;
                fixed.push(at);
            }
            _ => bad = true,
        }
    }
    for (at, flag) in seen.iter().enumerate() {
        if !flag {
            fixed.push(at);
            bad = true;
        }
    }
    if bad {
        notes.push(
            "the load order left mods out or repeated them; they were put last, in the order found"
                .into(),
        );
    }
    fixed
}

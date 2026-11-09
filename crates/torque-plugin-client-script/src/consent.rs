//! The consent API: what a game's prompt is asked, and what it answers.
//!
//! The engine decides whether to ask ([`crate::policy::decide`]) and what to do with the answer; the prompt is a slot a game
//! fills ([`ConsentPrompt`]). The request names each ability by a stable key (`client_script.ability.ui`) with its English line,
//! so a game can look the key up in its string table; the answer is the abilities ticked and whether to remember them.
//! [`EnginePrompt`] is the plain prompt that ships, drawn in the GUI's engine layer. Whoever draws the prompt, the realm's
//! controls cannot cover it: it sits in the engine layer, over every dialog, and until the answer nothing of the realm is
//! loaded.

use torque_client_script_format::Tier;
use torque_gui::Gui;
use torque_gui::client_script::{PromptAnswer, PromptRow, PromptSpec, RowLock};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::abilities::{self, english};
use crate::policy::AskRow;

/// One ability line of a question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbilityLine {
    /// The ability.
    pub tier: Tier,
    /// Its stable key, `client_script.ability.<name>`.
    pub key: String,
    /// The English line.
    pub english: &'static str,
    /// Whether its box starts ticked.
    pub on: bool,
    /// `Some(state)` when a policy or an earlier answer decided it: shown, not changeable.
    pub locked: Option<bool>,
}

/// The question: a server's request, as the player is asked about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentRequest {
    /// The server's name, plain text.
    pub server_name: String,
    /// The server's address, plain text.
    pub address: String,
    /// The abilities it asks for that the player is asked about, in the order prompts list them.
    pub lines: Vec<AbilityLine>,
}

impl ConsentRequest {
    pub(crate) fn new(server_name: &str, address: &str, rows: &[AskRow]) -> ConsentRequest {
        ConsentRequest {
            server_name: server_name.to_string(),
            address: address.to_string(),
            lines: rows
                .iter()
                .map(|row| AbilityLine {
                    tier: row.tier,
                    key: abilities::key(row.tier),
                    english: english(row.tier),
                    on: row.on,
                    locked: row.locked,
                })
                .collect(),
        }
    }
}

/// What the player answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsentAnswer {
    /// Allow, with each free line's box as it was left, and whether to remember the choice.
    Allow {
        /// `(ability, ticked)` for the lines that could be changed.
        ticked: Vec<(Tier, bool)>,
        /// Whether to remember.
        remember: bool,
    },
    /// Not now: run nothing this time, remember nothing.
    NotNow,
    /// Never for this server, remembered.
    Never,
}

/// Where the question is shown. A game fills this slot to draw its own prompt (and translate it); the engine's
/// [`EnginePrompt`] is the default.
pub trait ConsentPrompt {
    /// Shows `request`. Called once for each question; no answer comes until [`poll`](Self::poll) gives it.
    fn ask(&mut self, vm: &mut Vm, gui: &Gui, request: &ConsentRequest);

    /// The answer, once the player has given it (and the prompt is down), else `None`.
    fn poll(&mut self, vm: &mut Vm, gui: &Gui) -> Option<ConsentAnswer>;

    /// Takes the question down without an answer (the connection ended).
    fn cancel(&mut self, vm: &mut Vm, gui: &Gui);
}

/// The prompt the engine ships (`torque_gui`'s default prompt in the engine layer).
#[derive(Default)]
pub struct EnginePrompt {
    up: Option<ObjId>,
    keys: Vec<(String, Tier)>,
}

impl ConsentPrompt for EnginePrompt {
    fn ask(&mut self, vm: &mut Vm, gui: &Gui, request: &ConsentRequest) {
        self.cancel(vm, gui);
        self.keys = request
            .lines
            .iter()
            .map(|l| (l.key.clone(), l.tier))
            .collect();
        let spec = PromptSpec {
            heading: format!("{} ({})", request.server_name, request.address),
            detail: "wants to run its own scripts in your game. It asks to:".to_string(),
            rows: request
                .lines
                .iter()
                .map(|l| PromptRow {
                    key: l.key.clone(),
                    label: l.english.to_string(),
                    on: l.locked.unwrap_or(l.on),
                    lock: if l.locked.is_some() {
                        RowLock::Locked
                    } else {
                        RowLock::Free
                    },
                })
                .collect(),
            allow: "Allow".to_string(),
            not_now: "Not now".to_string(),
            never: "Never for this server".to_string(),
            remember: "Remember".to_string(),
            remember_on: true,
        };
        self.up = gui.show_client_script_prompt(vm, spec);
    }

    fn poll(&mut self, vm: &mut Vm, gui: &Gui) -> Option<ConsentAnswer> {
        let id = self.up?;
        if !vm.objects().contains(id) {
            // The prompt could not stay up (the layer was full): the question is answered no.
            self.up = None;
            return Some(ConsentAnswer::NotNow);
        }
        let answer = gui.take_client_script_prompt(vm, id)?;
        self.up = None;
        Some(match answer {
            PromptAnswer::Allow { rows, remember } => ConsentAnswer::Allow {
                ticked: rows
                    .into_iter()
                    .filter_map(|(key, on)| {
                        let tier = self.keys.iter().find(|(k, _)| *k == key)?.1;
                        Some((tier, on))
                    })
                    .collect(),
                remember,
            },
            PromptAnswer::NotNow => ConsentAnswer::NotNow,
            PromptAnswer::Never => ConsentAnswer::Never,
        })
    }

    fn cancel(&mut self, vm: &mut Vm, gui: &Gui) {
        if let Some(id) = self.up.take() {
            gui.close_client_script_prompt(vm, id);
        }
    }
}

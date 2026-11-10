//! The client's realms: from a server's offer to a running realm and back, one realm for a connection.
//!
//! [`Realms`] is what a game drives, once a frame, with its script engine and GUI ([`Realms::frame`]) and when a connection ends
//! ([`Realms::disconnect`]). It follows one connection through these states:
//!
//! 1. **Idle.** Nothing of the realm exists: no instance, no control, no file read.
//! 2. **Waiting.** The server's offer came and the settings and the remembered choice ask for scripts. It waits for the offered mods'
//!    files to be mounted for this server (a look that reads nothing, [`OfferSource::ready`]).
//! 3. **Asking.** The player is asked ([`ConsentPrompt`]). Still nothing is read or started.
//! 4. **Live.** The player allowed some abilities (or settings and a remembered choice did): the server's files are read, the guest
//!    instanced ([`RunnerFactory`]), the controls made. Each frame the server's messages and the GUI's events go in, the realm
//!    runs, and what it asked for comes out in the format's order: its `ui` ops applied, then its messages and calls sent.
//! 5. **Off.** Declined, refused, stopped or turned off: nothing more runs on this connection. A reconnect starts at Idle.
//!
//! Whatever the player answered is told to the server once as a [`Status`], and nothing else about why.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use torque_client_script_format::{Grants, Tier, Value};
use torque_client_script_host::{
    ClientMod, GrantSet, HOST_TIERS, InstanceLimits, REVIEWED_TIERS, Runner, ServerContent,
    Session, Settings, Trap,
};
use torque_engine::net::ConnectTarget;
use torque_engine::rpc::{NoGhosts, RealmInbox};
use torque_gui::Gui;
use torque_gui::client_script::{LayerAction, Realm, RealmView, TierToggle};
use torque_mods::{CacheOwner, MAX_CLIENT_IMAGE_BYTES, Mods};
use torque_server::net::rpc::ClientRpc;
use torque_ts_vm::vm::Vm;

use crate::abilities::ABILITIES;
use crate::call::{error_text, reply_value};
use crate::choices::Choices;
use crate::consent::{ConsentAnswer, ConsentPrompt, ConsentRequest, EnginePrompt};
use crate::effects::{Applying, Outgoing, Record, Recording};
use crate::inbox::ScriptInbox;
use crate::messages::{OfferedMod, STATUS, ScriptStatus, ScriptUp, Status, UP};
use crate::policy::{
    AskRow, Decision, Inputs, Policies, Remembered, Setting, Why, answer as policy_answer, decide,
};

/// Makes the guest's instance for a realm. A game supplies one: `NativeRealm` under Wasmtime natively, `WebRunner` on the web.
/// Without one, a client cannot run server scripts and says so to the server.
pub trait RunnerFactory {
    /// A fresh instance within `limits`.
    ///
    /// # Errors
    ///
    /// The trap that stopped it from starting.
    fn open(&self, limits: InstanceLimits) -> Result<Box<dyn Runner>, Trap>;
}

impl<F: Fn(InstanceLimits) -> Result<Box<dyn Runner>, Trap>> RunnerFactory for F {
    fn open(&self, limits: InstanceLimits) -> Result<Box<dyn Runner>, Trap> {
        self(limits)
    }
}

/// Reads one image file of a mod by its path within the mod, when the realm first shows it.
pub type ImageReader = Box<dyn Fn(&str) -> Option<Arc<[u8]>>>;

/// Where a server's verified files are: the look that reads nothing, and the reads that happen only after the player allowed.
pub trait OfferSource {
    /// Whether the files of `offered` are mounted for `server` on this connection and hold its entry. Reads nothing.
    fn ready(&self, vm: &Vm, server: CacheOwner, offered: &OfferedMod) -> bool;

    /// The mod's script files as `(mods/<id>/<path>, bytes)`, by the rules of [`Mods::client_scripts`].
    fn content(
        &self,
        vm: &Vm,
        server: CacheOwner,
        offered: &OfferedMod,
    ) -> Option<Vec<(String, Arc<[u8]>)>>;

    /// The reader of the mod's images ([`Mods::client_image`]'s rules), called when a control first shows one. By default the
    /// mod has none.
    fn images(&self, _vm: &Vm, _server: CacheOwner, _offered: &OfferedMod) -> ImageReader {
        Box::new(|_| None)
    }
}

/// The files `torque-mods` mounted from the server's download or its own cache entry.
pub struct ModsSource;

impl OfferSource for ModsSource {
    fn ready(&self, vm: &Vm, server: CacheOwner, offered: &OfferedMod) -> bool {
        Mods::of(vm).is_some_and(|mods| {
            mods.client_ready(server, &offered.id, &offered.hash, &offered.entry)
        })
    }

    fn content(
        &self,
        vm: &Vm,
        server: CacheOwner,
        offered: &OfferedMod,
    ) -> Option<Vec<(String, Arc<[u8]>)>> {
        Mods::of(vm)?.client_scripts(server, &offered.id, &offered.hash)
    }

    fn images(&self, vm: &Vm, server: CacheOwner, offered: &OfferedMod) -> ImageReader {
        let Some(mods) = Mods::of(vm) else {
            return Box::new(|_| None);
        };
        let (id, hash) = (offered.id.clone(), offered.hash);
        Box::new(move |path| mods.client_image(server, &id, &hash, path))
    }
}

/// A realm's images, read from the server's mods when a control first shows one and kept for the realm's life, at most
/// [`MAX_CLIENT_IMAGE_BYTES`] of them in all: an image past it is not this server's content to the realm.
struct Images {
    readers: Vec<(String, ImageReader)>,
    read: RefCell<HashMap<String, Arc<[u8]>>>,
    bytes: Cell<u64>,
}

impl Images {
    /// The bytes of the content path `mods/<id>/<path>`.
    fn get(&self, content_path: &str) -> Option<Arc<[u8]>> {
        if let Some(bytes) = self.read.borrow().get(content_path) {
            return Some(Arc::clone(bytes));
        }
        let (id, path) = content_path.strip_prefix("mods/")?.split_once('/')?;
        let (_, reader) = self.readers.iter().find(|(mine, _)| mine == id)?;
        let bytes = reader(path)?;
        let total = self.bytes.get().saturating_add(bytes.len() as u64);
        if total > MAX_CLIENT_IMAGE_BYTES {
            return None;
        }
        self.bytes.set(total);
        self.read
            .borrow_mut()
            .insert(content_path.to_string(), Arc::clone(&bytes));
        Some(bytes)
    }
}

/// How a request and what the player allowed become the realm's grants.
type GrantRule = Box<dyn Fn(&[&str], Grants) -> GrantSet>;

/// A server whose offer is being dealt with.
struct Pending {
    server: CacheOwner,
    address: String,
    name: String,
    mods: Vec<OfferedMod>,
    requested: Vec<Tier>,
}

/// What one frame of a running realm is given.
#[derive(Clone, Copy)]
struct Frame {
    epoch: u64,
    now_us: u64,
    target: Duration,
}

/// A running realm and the ends of its queues.
struct Live {
    session: Session<Recording>,
    /// The other end of the session's recorder, for the frames' [`Applying`].
    recording: Recording,
    realm: Realm,
    record: Rc<RefCell<Record>>,
    replies: Rc<RefCell<VecDeque<(u32, bool, Value)>>>,
    server: CacheOwner,
    address: String,
    warned_send: bool,
}

enum State {
    Idle,
    Waiting(Pending, Decision),
    Asking(Pending, Vec<AskRow>, Grants),
    Live(Box<Live>),
    Off,
}

/// Where a connection's realm is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RealmState {
    /// Nothing of a realm exists: no offer has come, or the connection is new.
    Idle,
    /// The offer came and the files are not all mounted yet.
    Waiting,
    /// The player is being asked.
    Asking,
    /// A realm runs.
    Running,
    /// Declined, refused, stopped or turned off: nothing more runs on this connection.
    Off,
}

/// What the realms did, for tests and the game's own checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Questions shown.
    pub prompts: u32,
    /// Realms started.
    pub opened: u32,
    /// Messages sent to servers.
    pub sent: u64,
    /// Sends and calls the realm made that the host refused.
    pub refused: u64,
}

/// The client's realms: one for the current connection.
pub struct Realms {
    /// The global "Server scripts" setting.
    pub setting: Setting,
    /// The per-ability policies.
    pub policies: Policies,
    /// The player's reduced-motion setting.
    pub reduce_motion: bool,
    /// The abilities this build can offer; by default those reviewed with their host modules compiled in.
    pub offerable: Grants,
    /// The realm's limits.
    pub settings: Settings,
    grant_set: GrantRule,
    choices: Choices,
    prompt: Box<dyn ConsentPrompt>,
    runner: Option<Box<dyn RunnerFactory>>,
    source: Box<dyn OfferSource>,
    state: State,
    status: Option<Status>,
    /// The connection the state is of: the RPC layer's epoch and the server dialed.
    connection: Option<(u64, Option<CacheOwner>)>,
    stats: Stats,
}

impl Realms {
    /// Realms that remember choices in `choices` and instance their guest with `runner` (`None`: this client cannot run server
    /// scripts). The setting is [`Setting::Ask`] until the game or the player sets it; the prompt is the engine's.
    pub fn new(choices: Choices, runner: Option<Box<dyn RunnerFactory>>) -> Realms {
        Realms {
            setting: Setting::default(),
            policies: Policies::default(),
            reduce_motion: false,
            offerable: Grants(REVIEWED_TIERS.0 & HOST_TIERS.0),
            settings: Settings::native(),
            grant_set: Box::new(|requested, allowed| {
                GrantSet::from_request(requested.iter().copied(), allowed)
            }),
            choices,
            prompt: Box::new(EnginePrompt::default()),
            runner,
            source: Box::new(ModsSource),
            state: State::Idle,
            status: None,
            connection: None,
            stats: Stats::default(),
        }
    }

    /// Draws the question with the game's own prompt instead of the engine's.
    pub fn set_prompt(&mut self, prompt: Box<dyn ConsentPrompt>) {
        self.prompt = prompt;
    }

    /// Reads a server's files from `source` instead of `torque-mods` (tests).
    pub fn set_source(&mut self, source: Box<dyn OfferSource>) {
        self.source = source;
    }

    /// Chooses how a request becomes the realm's grants, in place of [`GrantSet::from_request`]: for tests and for working on a
    /// tier before its review. Never for a release.
    pub fn set_grant_set(&mut self, f: impl Fn(&[&str], Grants) -> GrantSet + 'static) {
        self.grant_set = Box::new(f);
    }

    /// The remembered choices, to list and forget.
    pub fn choices(&self) -> &Choices {
        &self.choices
    }

    /// The remembered choices, to change.
    pub fn choices_mut(&mut self) -> &mut Choices {
        &mut self.choices
    }

    /// What the realms did.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Where this connection's realm is.
    pub fn state(&self) -> RealmState {
        match self.state {
            State::Idle => RealmState::Idle,
            State::Waiting(..) => RealmState::Waiting,
            State::Asking(..) => RealmState::Asking,
            State::Live(_) => RealmState::Running,
            State::Off => RealmState::Off,
        }
    }

    /// The abilities the running realm holds now, less those revoked.
    pub fn grants(&self) -> Option<Grants> {
        match &self.state {
            State::Live(live) => Some(live.session.grants()),
            _ => None,
        }
    }

    /// The connection ended (disconnect, kick, server change) or the player turned scripts off: the question is taken down and the
    /// realm closed with everything it made, and the next connection starts over.
    pub fn disconnect(&mut self, vm: &mut Vm, gui: &Gui) {
        self.drop_connection(vm, gui);
        self.connection = None;
        if let Some(inbox) = vm.extension::<ScriptInbox>() {
            inbox.clear();
        }
    }

    /// Ends what the state holds for the connection it was of: the question, the realm, the status owed. The inbox of the
    /// server's values keeps what a new connection already sent it.
    fn drop_connection(&mut self, vm: &mut Vm, gui: &Gui) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Live(live) => end(vm, gui, *live),
            State::Asking(..) => self.prompt.cancel(vm, gui),
            State::Idle | State::Waiting(..) | State::Off => {}
        }
        self.status = None;
        if let Some(realm_inbox) = vm.extension::<RealmInbox>() {
            realm_inbox.take();
        }
    }

    /// One frame at `now_us` (microseconds on any steady clock), whose target length is `target`.
    pub fn frame(&mut self, vm: &mut Vm, gui: &Gui, now_us: u64, target: Duration) {
        let rpc = vm.extension::<ClientRpc>();
        let ready = rpc.as_ref().is_some_and(|rpc| rpc.is_ready());
        // The state is of one connection: a different epoch or server, or a link that went, ends it even if no frame saw the
        // connection down.
        let here = rpc.as_ref().filter(|_| ready).map(|rpc| {
            let server = vm
                .extension::<ConnectTarget>()
                .map(|target| CacheOwner::of_server(target.0));
            (rpc.epoch(), server)
        });
        if self.connection.is_some() && self.connection != here {
            self.drop_connection(vm, gui);
        }
        self.connection = here;
        let (Some(rpc), Some((epoch, _))) = (rpc.filter(|_| ready), here) else {
            return;
        };
        let inbox = vm.extension::<ScriptInbox>();
        if let Some(mods) = inbox.as_ref().and_then(|inbox| inbox.take_offer(epoch))
            && matches!(self.state, State::Idle)
        {
            self.begin(vm, mods);
        }
        self.advance(vm, gui);
        if matches!(self.state, State::Live(_)) {
            self.run(
                vm,
                gui,
                &rpc,
                inbox.as_deref(),
                Frame {
                    epoch,
                    now_us,
                    target,
                },
            );
        } else if let Some(inbox) = &inbox {
            // Nothing of a realm exists: whatever the server sent for one is dropped at once, not kept.
            inbox.take_values(epoch);
            if let Some(realm_inbox) = vm.extension::<RealmInbox>() {
                realm_inbox.take();
            }
        }
        self.send_status(&rpc);
    }

    // ---- the offer

    fn begin(&mut self, vm: &mut Vm, offered: Vec<OfferedMod>) {
        let Some(target) = vm.extension::<ConnectTarget>() else {
            return;
        };
        let server = CacheOwner::of_server(target.0);
        let address = target.0.to_string();
        let mut requested: Vec<Tier> = Vec::new();
        let mut unknown = 0usize;
        for name in offered.iter().flat_map(|m| &m.tiers) {
            match Tier::from_name(name) {
                Some(tier) if tier != Tier::Realm => requested.push(tier),
                _ => unknown += 1,
            }
        }
        if unknown > 0 {
            vm.warn(format!(
                "server scripts: {address} asked for {unknown} thing(s) this game does not know; ignored"
            ));
        }
        let pending = Pending {
            server,
            name: address.clone(),
            address,
            mods: offered,
            requested,
        };
        if pending.mods.is_empty() {
            self.state = State::Off;
            return;
        }
        let decision = decide(&Inputs {
            setting: self.setting,
            policies: &self.policies,
            remembered: self.choices.get(server),
            requested: &pending.requested,
            offerable: self.offerable,
            reduce_motion: self.reduce_motion,
        });
        match decision {
            Decision::Off(why) => {
                self.status = Some(match why {
                    Why::NothingAsked => Status::Unavailable,
                    Why::Setting | Why::RememberedNever | Why::Refused => Status::Declined,
                });
                self.state = State::Off;
            }
            _ if self.runner.is_none() => {
                self.status = Some(Status::Unavailable);
                self.state = State::Off;
            }
            decision => self.state = State::Waiting(pending, decision),
        }
    }

    fn advance(&mut self, vm: &mut Vm, gui: &Gui) {
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Waiting(pending, decision) => self.waiting(vm, gui, pending, decision),
            State::Asking(pending, rows, always) => self.asking(vm, gui, pending, rows, always),
            other => self.state = other,
        }
    }

    fn waiting(&mut self, vm: &mut Vm, gui: &Gui, pending: Pending, decision: Decision) {
        let all_here = pending
            .mods
            .iter()
            .all(|m| self.source.ready(vm, pending.server, m));
        if !all_here {
            self.state = State::Waiting(pending, decision);
            return;
        }
        match decision {
            Decision::Run(granted) => self.open(vm, gui, pending, granted),
            Decision::Ask { rows, always } => {
                let request = ConsentRequest::new(&pending.name, &pending.address, &rows);
                self.prompt.ask(vm, gui, &request);
                self.stats.prompts += 1;
                self.state = State::Asking(pending, rows, always);
            }
            Decision::Off(_) => self.state = State::Off,
        }
    }

    fn asking(
        &mut self,
        vm: &mut Vm,
        gui: &Gui,
        pending: Pending,
        rows: Vec<AskRow>,
        always: Grants,
    ) {
        let Some(answer) = self.prompt.poll(vm, gui) else {
            self.state = State::Asking(pending, rows, always);
            return;
        };
        match answer {
            ConsentAnswer::Allow { ticked, remember } => {
                let (granted, decided) = policy_answer(always, &rows, &ticked);
                if remember
                    && let Err(error) =
                        self.choices
                            .merge(pending.server, &pending.address, &decided)
                {
                    vm.warn(format!(
                        "server scripts: the choice could not be saved ({error})"
                    ));
                }
                if granted == Grants(1) {
                    self.status = Some(Status::Declined);
                    self.state = State::Off;
                } else {
                    self.open(vm, gui, pending, granted);
                }
            }
            ConsentAnswer::NotNow => {
                self.status = Some(Status::Declined);
                self.state = State::Off;
            }
            ConsentAnswer::Never => {
                if let Err(error) =
                    self.choices
                        .set(pending.server, &pending.address, Remembered::Never)
                {
                    vm.warn(format!(
                        "server scripts: the choice could not be saved ({error})"
                    ));
                }
                self.status = Some(Status::Declined);
                self.state = State::Off;
            }
        }
    }

    /// Starts the realm: the files read, the guest instanced, the controls made. Only here does a realm come to exist.
    fn open(&mut self, vm: &mut Vm, gui: &Gui, pending: Pending, allowed: Grants) {
        self.state = State::Off;
        let unavailable = |realms: &mut Realms, vm: &mut Vm, why: &str| {
            vm.warn(format!("server scripts: {why}; they are not run"));
            realms.status = Some(Status::Unavailable);
        };
        let Some(factory) = &self.runner else {
            return unavailable(self, vm, "this game cannot run them");
        };
        let names: Vec<&str> = pending.requested.iter().map(|t| t.name()).collect();
        let grants = (self.grant_set)(&names, allowed);
        if grants.grants() == Grants(1) {
            return unavailable(self, vm, "no ability they asked for is available");
        }
        let mut files: Vec<(String, Arc<[u8]>)> = Vec::new();
        for offered in &pending.mods {
            let Some(mine) = self.source.content(vm, pending.server, offered) else {
                return unavailable(self, vm, "their files are not all here");
            };
            files.extend(mine);
        }
        let content = ServerContent::new(files);
        let runner = match factory.open(self.settings.instance()) {
            Ok(runner) => runner,
            Err(trap) => {
                return unavailable(self, vm, &format!("the guest did not start ({trap})"));
            }
        };
        let view = RealmView {
            address: pending.address.clone(),
            name: pending.name.clone(),
            tiers: ABILITIES
                .into_iter()
                .filter(|t| grants.grants().has(*t))
                .map(|tier| TierToggle { tier, on: true })
                .collect(),
        };
        let Some(mut realm) = Realm::new(vm, gui, grants.grants(), view) else {
            return unavailable(self, vm, "a realm is running already");
        };
        let images = Images {
            readers: pending
                .mods
                .iter()
                .map(|m| (m.id.clone(), self.source.images(vm, pending.server, m)))
                .collect(),
            read: RefCell::default(),
            bytes: Cell::new(0),
        };
        realm.set_content_check(Box::new(move |path| images.get(path)));
        let Some(table) = vm.extension::<ClientRpc>().map(|rpc| rpc.table().clone()) else {
            realm.teardown(vm, gui);
            return unavailable(self, vm, "there is no connection");
        };
        let (effects, record) = Recording::new(table);
        let recording = effects.clone();
        let mods: Vec<ClientMod> = pending
            .mods
            .iter()
            .map(|m| ClientMod {
                id: m.id.clone(),
                entry: m.entry.clone(),
            })
            .collect();
        let session = Session::open(runner, effects, grants, &content, &mods, self.settings);
        let tiers: Vec<Tier> = ABILITIES
            .into_iter()
            .filter(|t| grants.grants().has(*t))
            .collect();
        self.status = Some(Status::Allowed(tiers));
        self.stats.opened += 1;
        self.state = State::Live(Box::new(Live {
            session,
            recording,
            realm,
            record,
            replies: Rc::default(),
            server: pending.server,
            address: pending.address,
            warned_send: false,
        }));
    }

    // ---- a running realm

    fn run(
        &mut self,
        vm: &mut Vm,
        gui: &Gui,
        rpc: &ClientRpc,
        inbox: Option<&ScriptInbox>,
        frame: Frame,
    ) {
        let State::Live(mut live) = std::mem::replace(&mut self.state, State::Off) else {
            return;
        };
        // What the GUI did for the realm since its last frame (its layouts, its image decodes) is the realm's time.
        live.session.charge(gui.take_client_script_owed());
        // In: the server's values and calls, the answers to the realm's calls, and the player's input.
        if let Some(inbox) = inbox {
            for value in inbox.take_values(frame.epoch) {
                live.session.message(&value);
            }
        }
        if let Some(realm_inbox) = vm.extension::<RealmInbox>() {
            for call in realm_inbox.take() {
                if torque_client_script_format::net::check_rpc_name(&call.rpc).is_ok() {
                    live.session.rpc(&call.rpc, &reply_value(&call.values));
                }
            }
        }
        let answers: Vec<_> = live.replies.borrow_mut().drain(..).collect();
        for (id, ok, value) in answers {
            live.session.reply(id, ok, &value);
        }
        let batch = gui.take_client_script_batch();
        if !batch.is_empty() {
            live.session.ui_events(&batch);
        }
        let mut ended = None;
        for action in gui.take_client_script_actions() {
            match action {
                LayerAction::Revoke(tier) => {
                    live.session.revoke(tier);
                }
                LayerAction::Hide(_) => {}
                // "Turn off for this server" outweighs a Shift+Escape in the same frame.
                LayerAction::TurnOff => ended = Some(LayerAction::TurnOff),
                LayerAction::Stop => {
                    ended.get_or_insert(LayerAction::Stop);
                }
            }
        }
        if let Some(action) = ended {
            if action == LayerAction::TurnOff
                && let Err(error) = self
                    .choices
                    .set(live.server, &live.address, Remembered::Never)
            {
                vm.warn(format!(
                    "server scripts: the choice could not be saved ({error})"
                ));
            }
            end(vm, gui, *live);
            return;
        }
        // The player's revocations take the controls away before the realm runs again.
        let revoked = std::mem::take(&mut live.record.borrow_mut().revoked);
        if !revoke(vm, gui, &mut live, &revoked) {
            end(vm, gui, *live);
            return;
        }
        {
            let Live {
                session,
                recording,
                realm,
                ..
            } = &mut *live;
            let mut applying = Applying {
                recording,
                realm,
                vm,
                gui,
                now_us: frame.now_us,
            };
            session.frame_with(frame.now_us, frame.target, &mut applying);
        }
        // Out: what the realm asked for that waited for its ui ops.
        let keep = self.apply(vm, gui, rpc, &mut live);
        self.state = if keep {
            State::Live(live)
        } else {
            end(vm, gui, *live);
            State::Off
        };
    }

    /// Carries out what the frame recorded besides its `ui` ops (applied during the frame). `false` when the realm is over.
    fn apply(&mut self, vm: &mut Vm, gui: &Gui, rpc: &ClientRpc, live: &mut Live) -> bool {
        let record = std::mem::take(&mut *live.record.borrow_mut());
        for (_, line) in &record.logs {
            vm.print(format!("[scripts from {}] {line}", live.address).as_bytes());
        }
        self.stats.refused += record.refused_sends + record.refused_calls;
        let alive = record.stopped.is_none() && !record.torn_down;
        if let Some(why) = &record.stopped {
            vm.warn(format!(
                "server scripts from {} stopped: {why}",
                live.address
            ));
        }
        if !alive || !revoke(vm, gui, live, &record.revoked) {
            // A flush whose ui failed sends nothing, and a stopped realm answers no call.
            return false;
        }
        self.send_out(vm, rpc, live, record.outgoing);
        true
    }

    fn send_out(
        &mut self,
        vm: &mut Vm,
        rpc: &ClientRpc,
        live: &mut Live,
        outgoing: VecDeque<Outgoing>,
    ) {
        let replies = Rc::clone(&live.replies);
        let fail = |id: u32, why: &str| {
            replies
                .borrow_mut()
                .push_back((id, false, Value::String(why.as_bytes().to_vec())));
        };
        let mut sent = 0u64;
        let mut dropped = false;
        let linked = rpc.with_link(|link| {
            for item in outgoing {
                match item {
                    Outgoing::Send(value) => {
                        let queued = ScriptUp::of(&value).ok().and_then(|message| {
                            let handle = link.rpc::<ScriptUp, ()>(UP).ok()?;
                            link.notify(&handle, message).ok()
                        });
                        if queued.is_some() {
                            sent += 1;
                        } else {
                            dropped = true;
                        }
                    }
                    Outgoing::Call { id, name, args } => {
                        let replies = Rc::clone(&live.replies);
                        let done = move |result: Result<Vec<torque_ghosts::Value>, _>| {
                            let entry = match result {
                                Ok(values) => (id, true, reply_value(&values)),
                                Err(error) => {
                                    (id, false, Value::String(error_text(&error).into_bytes()))
                                }
                            };
                            replies.borrow_mut().push_back(entry);
                        };
                        if let Err(error) = link.call_script(&name, &args, &NoGhosts, done) {
                            fail(id, &error_text(&error));
                        }
                    }
                }
            }
        });
        if linked.is_none() {
            // No link takes them: the connection is going.
            dropped = true;
        }
        self.stats.sent += sent;
        if dropped && !live.warned_send {
            live.warned_send = true;
            vm.warn(format!(
                "server scripts from {}: a message was not sent (too many, too large, or no connection)",
                live.address
            ));
        }
    }

    fn send_status(&mut self, rpc: &ClientRpc) {
        let Some(status) = &self.status else {
            return;
        };
        let sent = rpc.with_link(|link| {
            let handle = link.rpc::<ScriptStatus, ()>(STATUS).ok()?;
            Some(link.notify(&handle, ScriptStatus::of(status)))
        });
        match sent {
            // Tried once; a busy link is tried again next frame, anything else is given up.
            Some(Some(Err(
                torque_engine::rpc::RpcError::Busy | torque_engine::rpc::RpcError::RateLimited,
            ))) => {}
            Some(_) => self.status = None,
            None => {}
        }
    }
}

/// Takes `tiers` from the realm's controls: `false` when that stopped it.
fn revoke(vm: &mut Vm, gui: &Gui, live: &mut Live, tiers: &[Tier]) -> bool {
    for tier in tiers {
        if let Err(violation) = live.realm.revoke(vm, gui, *tier) {
            vm.warn(format!(
                "server scripts from {} stopped: {violation}",
                live.address
            ));
            return false;
        }
    }
    true
}

/// Closes a realm: its instance thrown away and everything it made removed.
fn end(vm: &mut Vm, gui: &Gui, live: Live) {
    let Live { session, realm, .. } = live;
    drop(session.close());
    realm.teardown(vm, gui);
}

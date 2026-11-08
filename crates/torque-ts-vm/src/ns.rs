//! Namespaces and packages. A namespace is a table of functions; a package adds a layer on top of a
//! namespace while it is active. Entries are shared objects that are updated in place when a function is
//! redefined, so a call site that is already bound to an entry sees the new definition.
//!
//! No `RefCell` borrow of an [`Entry`] may be held while script code runs: the running code may redefine
//! the very function it is part of. Callers copy what they need out of the entry first.
//!
//! `Parent::` follows TGE's linked namespace objects, not the layer a function was declared
//! in: see [`Holder`].
//!
//! A namespace may have a class parent (`Namespace::classLinkTo`): every walk (`lookup`, `Parent::`, the
//! namespace list of a warning) continues from a namespace's linked layers into its class parent's, and
//! stops after [`MAX_NAMESPACE_DEPTH`] namespaces. The walks count their work (`take_work`), which the VM
//! charges to the step budget.
//!
//! Follows Torque3D's MIT-licensed `console/consoleInternal.cpp` (`Namespace`) (Copyright (c) 2012 GarageGames, LLC);
//! the code here is this crate's own.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::code::CodeBlock;
use crate::text::fold;
use crate::vm::NativeFn;

/// The most packages that can be active at once; further activations are refused (TGE's `MaxActivePackages`:
/// the 513th prints `ActivatePackage(<name>) failed - Max package limit reached: 512`).
pub const MAX_ACTIVE_PACKAGES: usize = 512;

/// The most namespaces one walk visits (the namespace itself and its class ancestors). TGE has
/// no limit and hangs on a cycle, which a script can build (a ScriptObject named `SimObject` links
/// SimObject under ScriptObject); real chains are under ten deep.
pub const MAX_NAMESPACE_DEPTH: usize = 64;

/// A refused [`Namespaces::link_class`]: the child already has another class parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkConflict {
    pub child: Vec<u8>,
    pub old: Vec<u8>,
    pub new: Vec<u8>,
}

impl LinkConflict {
    /// The refusal's message (`Namespace::classLinkTo`, printed as an error).
    pub fn message(&self) -> String {
        format!(
            "Error: cannot change namespace parent linkage for {} from {} to {}.",
            String::from_utf8_lossy(&self.child),
            String::from_utf8_lossy(&self.old),
            String::from_utf8_lossy(&self.new)
        )
    }
}

/// Identifies a package that has declared at least one function.
pub type PackageId = u32;

#[derive(Clone)]
pub enum Callable {
    Script {
        block: Rc<CodeBlock>,
        /// Index of the first operand of the `FuncDecl`; 0 when the declaration has no body.
        offset: usize,
    },
    Native {
        min: i32,
        max: i32,
        usage: Rc<str>,
        f: NativeFn,
    },
}

pub struct Entry {
    pub callable: Callable,
    /// The name as it was first declared (printed by `dump`).
    pub name: Vec<u8>,
    /// Folded namespace name; empty for the global namespace.
    pub namespace: Rc<[u8]>,
    /// The package layer the entry was declared in, `None` for the base layer.
    pub package: Option<PackageId>,
}

pub type EntryRef = Rc<RefCell<Entry>>;

/// A function as `dump` lists it: its declared name and its usage.
pub type Listed = (Vec<u8>, Rc<str>);

type Layer = crate::namehash::NameMap<Vec<u8>, EntryRef>;

/// One namespace: its base functions and one layer per package that declared functions in it.
#[derive(Default)]
struct Table {
    base: Layer,
    /// Keyed by the package's id (made here, in declaration order: no script chooses it).
    packages: crate::idhash::IdMap<PackageId, Layer>,
    /// The spelling the namespace was first named with (printed in messages).
    display: Vec<u8>,
    /// The folded name of the class parent, if linked.
    parent: Option<Vec<u8>>,
    /// The active packages that have a layer here, top first, as of the tables' [`generation`](Namespaces::generation)
    /// it holds: a lookup walks them instead of every active package (a server can have dozens active, and the busiest
    /// namespaces have layers of a few).
    linked: RefCell<Option<(u64, Rc<[PackageId]>)>>,
}

/// Where the function that is running sat in its namespace's chain when it was called. TGE
/// swaps entry lists between namespace objects when packages change, so a running function keeps
/// pointing at the namespace object it was called through, and `Parent::` goes to that object's
/// *current* parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    /// The function's layer was the top of the chain: `Parent::` finds every layer but the current top.
    Top,
    /// Its layer sat directly below this active package: `Parent::` finds nothing once that package is
    /// inactive, otherwise the layers strictly below the layer now directly below it.
    Below(PackageId),
    /// Its layer was not linked into the chain (a call site still bound to an entry of a package that
    /// has been deactivated): `Parent::` finds nothing.
    Detached,
}

/// The result of [`Namespaces::activate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    Activated,
    AlreadyActive,
    /// No function was ever declared in a package of that name.
    NotFound,
    /// [`MAX_ACTIVE_PACKAGES`] packages are active (checked before anything else, as TGE does).
    LimitReached,
}

/// Whether a file (its code block's name) is one a [`DeclareScope`] applies to.
type AppliesFn = Rc<dyn Fn(&[u8]) -> bool>;

/// A package that declarations go into without saying so, and the files it applies to
/// ([`Namespaces::set_declare_package`]).
#[derive(Clone)]
pub struct DeclareScope {
    package: Vec<u8>,
    applies: AppliesFn,
}

impl DeclareScope {
    /// Declarations go into `package` when `applies` says the file (its name as the code block has it) is one of the scope's.
    pub fn new(package: &[u8], applies: impl Fn(&[u8]) -> bool + 'static) -> DeclareScope {
        DeclareScope {
            package: package.to_vec(),
            applies: Rc::new(applies),
        }
    }
}

#[derive(Default)]
pub struct Namespaces {
    /// How many packages can be active at once, when not [`MAX_ACTIVE_PACKAGES`] ([`set_max_active`](Self::set_max_active)).
    max_active: Option<usize>,
    /// Folded namespace name -> its layers.
    tables: crate::namehash::NameMap<Vec<u8>, Table>,
    /// Folded package name -> id. A package exists once a function has been declared in it (or a declaration in it was refused:
    /// [`Namespaces::touch`]).
    package_ids: crate::namehash::NameMap<Vec<u8>, PackageId>,
    /// Package id -> the spelling it was first declared with.
    package_names: Vec<Vec<u8>>,
    /// Active packages in activation order (the last one is on top).
    active: Vec<PackageId>,
    /// The package a function declaration with no package of its own goes into, for the files it applies to
    /// ([`set_declare_package`](Self::set_declare_package)).
    declare_package: Option<DeclareScope>,
    /// Layers visited and packages scanned by the walks since the last `take_work`.
    work: Cell<u64>,
    /// Counts the changes of the tables ([`generation`](Self::generation)).
    generation: u64,
    /// The answers of [`lookup`](Self::lookup) since the tables last changed ([`LookupMemo`]).
    memo: RefCell<LookupMemo>,
}

/// What [`Namespaces::lookup`] answered for a namespace and a name as they were spelled, and the work its walk counted,
/// while the tables stay at one [`generation`](Namespaces::generation): a walk's answer and work depend on nothing
/// else, so a repeated lookup gives the same entry and counts the same work without walking. Keyed by the spellings'
/// bytes (length-prefixed) through the keyed name hash. Scripts choose names, so the memo is bounded: a lookup whose
/// namespace and name together pass [`LookupMemo::NAME_BYTES`] is not kept (it walks every time, as before the memo),
/// and the memo is dropped whole when the tables change, when it holds [`LookupMemo::MAX`] answers, or when a key would
/// take its keys past [`LookupMemo::MAX_BYTES`].
#[derive(Default)]
struct LookupMemo {
    generation: u64,
    answers: crate::namehash::NameMap<Vec<u8>, (Option<EntryRef>, u64)>,
    /// The bytes of the keys held.
    bytes: usize,
    key: Vec<u8>,
    /// The last prepared key may be kept.
    keep: bool,
}

impl LookupMemo {
    const MAX: usize = 4096;
    /// The longest namespace and name together a lookup the memo keeps may have (real ones are a few dozen bytes).
    const NAME_BYTES: usize = 256;
    /// The most bytes the memo's keys take together.
    const MAX_BYTES: usize = 256 << 10;

    /// The key of `(namespace, name)` in `self.key`, after making sure the memo belongs to `generation`; false (and no
    /// key) when the lookup is not one the memo keeps.
    fn prepare(&mut self, generation: u64, namespace: &[u8], name: &[u8]) -> bool {
        self.prepare_tagged(generation, &[], namespace, name)
    }

    /// [`prepare`](Self::prepare) for another kind of walk: a non-empty `tag` of a fixed length per kind leads the key,
    /// behind eight 0xff bytes (no plain key starts so: its first eight bytes are a namespace's length).
    fn prepare_tagged(
        &mut self,
        generation: u64,
        tag: &[u8],
        namespace: &[u8],
        name: &[u8],
    ) -> bool {
        if self.generation != generation || self.answers.len() >= Self::MAX {
            self.forget();
            self.generation = generation;
        }
        self.key.clear();
        self.keep = namespace.len().saturating_add(name.len()) <= Self::NAME_BYTES;
        if !self.keep {
            return false;
        }
        if !tag.is_empty() {
            self.key.extend_from_slice(&[0xff; 8]);
            self.key.extend_from_slice(tag);
        }
        self.key
            .extend_from_slice(&(namespace.len() as u64).to_le_bytes());
        self.key.extend_from_slice(namespace);
        self.key.extend_from_slice(name);
        true
    }

    /// The answer of the walk for the last prepared key (nothing when it is not one the memo keeps).
    fn remember(&mut self, found: Option<EntryRef>, work: u64) {
        if !self.keep {
            return;
        }
        if self.bytes.saturating_add(self.key.len()) > Self::MAX_BYTES {
            self.forget();
        }
        self.bytes += self.key.len();
        let key = self.key.clone();
        self.answers.insert(key, (found, work));
    }

    fn forget(&mut self) {
        self.answers.clear();
        self.bytes = 0;
    }
}

impl Namespaces {
    /// Declares a function in the given layer. An existing entry of the same name in that layer is
    /// updated in place, so call sites bound to it see the new definition.
    pub fn declare(
        &mut self,
        namespace: &[u8],
        package: Option<&[u8]>,
        name: &[u8],
        callable: Callable,
    ) -> EntryRef {
        let ns = fold(namespace);
        let pkg = package.map(|p| self.intern_package(p));
        let table = self.table_mut(namespace);
        let layer = match pkg {
            None => &mut table.base,
            Some(p) => table.packages.entry(p).or_default(),
        };
        let key = fold(name);
        if let Some(existing) = layer.get(&key) {
            // `declare` never runs while the entry is borrowed: callers copy out of an entry before
            // running script code.
            existing.borrow_mut().callable = callable;
            return Rc::clone(existing);
        }
        let entry = Rc::new(RefCell::new(Entry {
            callable,
            name: name.to_vec(),
            namespace: Rc::from(ns),
            package: pkg,
        }));
        layer.insert(key, Rc::clone(&entry));
        entry
    }

    /// `Namespace::find(namespace, package)` as a function declaration runs it before anything else: the package's layer in
    /// the namespace is made, empty, so the package exists ([`is_package`](Self::is_package)) even when the declaration is then
    /// skipped (one that would replace a native, or a name the declare policy refuses). Nothing is declared.
    pub fn touch(&mut self, namespace: &[u8], package: Option<&[u8]>) {
        let pkg = package.map(|p| self.intern_package(p));
        let table = self.table_mut(namespace);
        if let Some(p) = pkg {
            table.packages.entry(p).or_default();
        }
    }

    fn intern_package(&mut self, name: &[u8]) -> PackageId {
        let key = fold(name);
        if let Some(id) = self.package_ids.get(&key) {
            return *id;
        }
        let id = PackageId::try_from(self.package_names.len()).unwrap_or(PackageId::MAX);
        self.package_names.push(name.to_vec());
        self.package_ids.insert(key, id);
        id
    }

    fn table_mut(&mut self, namespace: &[u8]) -> &mut Table {
        self.generation = self.generation.wrapping_add(1);
        self.tables.entry(fold(namespace)).or_insert_with(|| Table {
            display: namespace.to_vec(),
            ..Table::default()
        })
    }

    /// The layers of a namespace that are linked right now, top first: active packages that have a
    /// layer in it (newest first), then the base layer. Only a namespace that has package layers scans
    /// the active list.
    fn chain<'a>(
        &'a self,
        table: &'a Table,
    ) -> impl Iterator<Item = (Option<PackageId>, &'a Layer)> {
        let linked = (!table.packages.is_empty()).then(|| self.linked(table));
        let count = linked.as_ref().map_or(0, |ids| ids.len());
        (0..count)
            .filter_map(move |i| {
                let p = *linked.as_ref()?.get(i)?;
                Some((Some(p), table.packages.get(&p)?))
            })
            .chain(std::iter::once((None, &table.base)))
    }

    fn add_work(&self, units: u64) {
        self.work.set(self.work.get().saturating_add(units));
    }

    /// The work counted since the last call: layers visited and active packages scanned. The VM charges
    /// it to the step budget (`vm::WORK_PER_STEP`).
    pub fn take_work(&self) -> u64 {
        self.work.replace(0)
    }

    /// Calls `visit` on every linked layer of `namespace`, then of its class parent, and so on, top
    /// first, until `visit` returns true or [`MAX_NAMESPACE_DEPTH`] namespaces have been visited.
    ///
    /// Every walk folds and hashes the name of `namespace`, which a script chose (a named ScriptObject or game
    /// datablock looks its methods up under its own name, up to the string cap): charged by its length. The
    /// engine keeps a namespace as a string-table pointer; a handle interned once, in the object, would cost
    /// nothing per call, and is the cleaner fix for later.
    fn walk<'a>(&'a self, namespace: &[u8], mut visit: impl FnMut(&'a Table, &'a Layer) -> bool) {
        self.add_work(namespace.len() as u64);
        let mut next = crate::text::with_folded(namespace, |key| self.tables.get(key));
        for _ in 0..MAX_NAMESPACE_DEPTH {
            let Some(table) = next else {
                return;
            };
            // The same layers in the same order as `chain`, and the same work.
            if !table.packages.is_empty() {
                self.add_work(self.active.len() as u64);
                for package in self.linked(table).iter() {
                    if let Some(layer) = table.packages.get(package) {
                        self.add_work(1);
                        if visit(table, layer) {
                            return;
                        }
                    }
                }
            }
            self.add_work(1);
            if visit(table, &table.base) {
                return;
            }
            next = table.parent.as_deref().and_then(|p| self.tables.get(p));
        }
    }

    /// The active packages with a layer in `table`, top first (`chain`'s package layers), kept until the tables change.
    fn linked(&self, table: &Table) -> Rc<[PackageId]> {
        if let Some((generation, ids)) = table.linked.borrow().as_ref()
            && *generation == self.generation
        {
            return Rc::clone(ids);
        }
        let ids: Rc<[PackageId]> = self
            .active
            .iter()
            .rev()
            .filter(|p| table.packages.contains_key(p))
            .copied()
            .collect();
        *table.linked.borrow_mut() = Some((self.generation, Rc::clone(&ids)));
        ids
    }

    /// The function `name` in `namespace`, searching its linked layers top down and then its class
    /// ancestors' (TGE's `Namespace::lookup`).
    pub fn lookup(&self, namespace: &[u8], name: &[u8]) -> Option<EntryRef> {
        {
            let mut memo = self.memo.borrow_mut();
            let kept = memo.prepare(self.generation, namespace, name);
            let memo = &*memo;
            if kept && let Some((found, work)) = memo.answers.get(memo.key.as_slice()) {
                self.add_work(*work);
                return found.clone();
            }
        }
        let before = self.work.get();
        let found = self.walk_lookup(namespace, name);
        let work = self.work.get().saturating_sub(before);
        self.memo.borrow_mut().remember(found.clone(), work);
        found
    }

    /// [`lookup`](Self::lookup) by the walk.
    fn walk_lookup(&self, namespace: &[u8], name: &[u8]) -> Option<EntryRef> {
        crate::text::with_folded(name, |key| {
            let mut found = None;
            self.walk(namespace, |_, layer| {
                found = layer.get(key).cloned();
                found.is_some()
            });
            found
        })
    }

    /// Every function `namespace`'s chain finds (its linked layers top down, then its class ancestors'),
    /// once per name, with its usage (`""` for a script function), sorted ignoring case: TGE's
    /// `Namespace::getEntryList`, which `dump()` prints. The walk is bounded by [`MAX_NAMESPACE_DEPTH`]
    /// and charges one unit per entry it visits and one per entry it sorts.
    pub fn chain_functions(&self, namespace: &[u8]) -> Vec<Listed> {
        let mut seen: HashMap<&[u8], Listed> = HashMap::new();
        self.walk(namespace, |_, layer| {
            for (key, entry) in layer {
                self.add_work(1);
                if !seen.contains_key(key.as_slice()) {
                    let entry = entry.borrow();
                    let usage = match &entry.callable {
                        Callable::Native { usage, .. } => Rc::clone(usage),
                        Callable::Script { .. } => Rc::from(""),
                    };
                    seen.insert(key.as_slice(), (entry.name.clone(), usage));
                }
            }
            false
        });
        let mut out: Vec<(&[u8], Listed)> = seen.into_iter().collect();
        self.add_work(out.len() as u64);
        // Folded names are distinct, so the order is total.
        out.sort_by(|a, b| a.0.cmp(b.0));
        out.into_iter().map(|(_, entry)| entry).collect()
    }

    /// The names, as first declared, of the script functions of every namespace and package layer (natives left
    /// out), sorted and once each. What a game's scripts defined once its start-up has run: a read-only listing
    /// for hosts.
    pub fn script_function_names(&self) -> Vec<Vec<u8>> {
        let mut names: Vec<Vec<u8>> = Vec::new();
        for table in self.tables.values() {
            for layer in std::iter::once(&table.base).chain(table.packages.values()) {
                for entry in layer.values() {
                    let entry = entry.borrow();
                    if matches!(entry.callable, Callable::Script { .. }) {
                        names.push(entry.name.clone());
                    }
                }
            }
        }
        names.sort();
        names.dedup();
        names
    }

    /// The natives registered in the base layers, as `(namespace, name, min, max)` (the namespace and name as first
    /// written, the global namespace empty; `min` and `max` count the function's own name as argument 0, as the
    /// console records do, and a `max` of 0 means no limit), sorted. A read-only listing for hosts and tests.
    pub fn native_signatures(&self) -> Vec<(Vec<u8>, Vec<u8>, i32, i32)> {
        let mut out = Vec::new();
        for table in self.tables.values() {
            for entry in table.base.values() {
                let entry = entry.borrow();
                if let Callable::Native { min, max, .. } = &entry.callable {
                    out.push((table.display.clone(), entry.name.clone(), *min, *max));
                }
            }
        }
        out.sort();
        out
    }

    /// The usage text of every native registered in the base layers (the empty ones left out), as `(namespace, name,
    /// usage)`, sorted. A read-only listing for hosts and tests.
    pub fn native_usages(&self) -> Vec<(Vec<u8>, Vec<u8>, String)> {
        let mut out = Vec::new();
        for table in self.tables.values() {
            for entry in table.base.values() {
                let entry = entry.borrow();
                if let Callable::Native { usage, .. } = &entry.callable
                    && !usage.is_empty()
                {
                    out.push((table.display.clone(), entry.name.clone(), usage.to_string()));
                }
            }
        }
        out.sort();
        out
    }

    /// One line per native registered in the base layers, sorted: `namespace::name min..max usage`, the global
    /// namespace without the `namespace::`, a `max` of `*` for no limit (the counts include the function's own name, as
    /// in [`Namespaces::native_signatures`]), the usage text with its backslashes and line breaks escaped (a native with
    /// none ends after the counts). The text of a host's whole native surface, for a snapshot test.
    pub fn native_listing(&self) -> Vec<String> {
        let mut out = Vec::new();
        for table in self.tables.values() {
            for entry in table.base.values() {
                let entry = entry.borrow();
                let Callable::Native {
                    min, max, usage, ..
                } = &entry.callable
                else {
                    continue;
                };
                let mut line = String::new();
                if !table.display.is_empty() {
                    line.push_str(&String::from_utf8_lossy(&table.display));
                    line.push_str("::");
                }
                line.push_str(&String::from_utf8_lossy(&entry.name));
                if *max == 0 {
                    line.push_str(&format!(" {min}..*"));
                } else {
                    line.push_str(&format!(" {min}..{max}"));
                }
                if !usage.is_empty() {
                    line.push(' ');
                    for c in usage.chars() {
                        match c {
                            '\\' => line.push_str(r"\\"),
                            '\n' => line.push_str(r"\n"),
                            '\r' => line.push_str(r"\r"),
                            c => line.push(c),
                        }
                    }
                }
                out.push(line);
            }
        }
        out.sort();
        out
    }

    /// Replaces the usage text of the native `name` in `namespace`'s base layer; false when there is no such native.
    pub fn set_native_usage(&mut self, namespace: &[u8], name: &[u8], text: &str) -> bool {
        let Some(table) = self.tables.get(fold(namespace).as_slice()) else {
            return false;
        };
        let Some(entry) = table.base.get(&fold(name)) else {
            return false;
        };
        if let Callable::Native { usage, .. } = &mut entry.borrow_mut().callable {
            *usage = Rc::from(text);
            return true;
        }
        false
    }

    /// Whether `namespace`'s base layer itself holds `name`.
    pub fn has_own(&self, namespace: &[u8], name: &[u8]) -> bool {
        self.tables
            .get(fold(namespace).as_slice())
            .is_some_and(|t| t.base.contains_key(&fold(name)))
    }

    /// Whether declaring a script function `name` in `namespace` (in `package`, if any) would replace a
    /// native (a dialect may refuse such a declaration silently, with the packages unlinked):
    /// the name is looked up in the BASE layers of the namespace and its class parents (no active package
    /// takes part), and a native entry found first ends it. A packaged declaration looks in its package's
    /// own namespace, which has no parent and holds script functions only, so a packaged method can
    /// override a native; a global function is also looked up in the plain global namespace, so a
    /// packaged global function cannot.
    pub fn shadows_native(&self, namespace: &[u8], package: Option<&[u8]>, name: &[u8]) -> bool {
        let key = fold(name);
        (package.is_none() && self.base_is_native(namespace, &key))
            || (namespace.is_empty() && self.base_is_native(b"", &key))
    }

    /// Whether the nearest base-layer entry called `key` (folded) along the class chain of `namespace` is
    /// a native.
    fn base_is_native(&self, namespace: &[u8], key: &[u8]) -> bool {
        let mut next = self.tables.get(fold(namespace).as_slice());
        for _ in 0..MAX_NAMESPACE_DEPTH {
            let Some(table) = next else {
                return false;
            };
            self.add_work(1);
            if let Some(entry) = table.base.get(key) {
                return matches!(entry.borrow().callable, Callable::Native { .. });
            }
            next = table.parent.as_deref().and_then(|p| self.tables.get(p));
        }
        false
    }

    /// Where `entry`'s layer sits in its namespace's chain right now; record it when the function is
    /// called and pass it to [`lookup_parent`](Self::lookup_parent) later.
    pub fn holder(&self, entry: &Entry) -> Holder {
        let Some(table) = self.tables.get(&*entry.namespace) else {
            return Holder::Detached;
        };
        let mut above = None;
        for (package, _) in self.chain(table) {
            if package == entry.package {
                return above.map_or(Holder::Top, Holder::Below);
            }
            above = package;
        }
        Holder::Detached
    }

    /// What `Parent::name` finds from a function that lives in `namespace` (already folded) and was
    /// called from `holder`: the layers below that position, continuing into the class ancestors.
    pub fn lookup_parent(&self, namespace: &[u8], holder: Holder, name: &[u8]) -> Option<EntryRef> {
        let mut tag = [0u8; 5];
        match holder {
            Holder::Top => tag[0] = 1,
            Holder::Below(p) => {
                tag[0] = 2;
                tag[1..].copy_from_slice(&p.to_le_bytes());
            }
            Holder::Detached => tag[0] = 3,
        }
        {
            let mut memo = self.memo.borrow_mut();
            let kept = memo.prepare_tagged(self.generation, &tag, namespace, name);
            let memo = &*memo;
            if kept && let Some((found, work)) = memo.answers.get(memo.key.as_slice()) {
                self.add_work(*work);
                return found.clone();
            }
        }
        let before = self.work.get();
        let found = self.walk_parent(namespace, holder, name);
        let work = self.work.get().saturating_sub(before);
        self.memo.borrow_mut().remember(found.clone(), work);
        found
    }

    /// [`lookup_parent`](Self::lookup_parent) by the walk.
    fn walk_parent(&self, namespace: &[u8], holder: Holder, name: &[u8]) -> Option<EntryRef> {
        let skip = self.parent_skip(namespace, holder)?;
        let key = fold(name);
        let mut seen = 0;
        let mut found = None;
        self.walk(namespace, |_, layer| {
            seen += 1;
            if seen > skip {
                found = layer.get(&key).cloned();
            }
            found.is_some()
        });
        found
    }

    /// How many layers of the walk from `namespace` a `Parent::` call from `holder` skips.
    fn parent_skip(&self, namespace: &[u8], holder: Holder) -> Option<usize> {
        let table = self.tables.get(namespace)?;
        match holder {
            Holder::Top => Some(1),
            Holder::Below(package) => self
                .chain(table)
                .position(|(p, _)| p == Some(package))
                .map(|at| at + 2),
            Holder::Detached => None,
        }
    }

    /// The name of the namespace a `Parent::` call from `holder` starts its lookup in (TGE's
    /// `thisNamespace->mParent`): the table of the first layer after the skipped ones, whichever
    /// namespace the function is finally found in. It is printed when a native found through
    /// `Parent::` is called with the wrong number of arguments.
    pub fn parent_scope(&self, namespace: &[u8], holder: Holder) -> Option<Vec<u8>> {
        let skip = self.parent_skip(namespace, holder)?;
        let mut seen = 0;
        let mut scope = None;
        self.walk(namespace, |table, _| {
            seen += 1;
            if seen > skip {
                scope = Some(table.display.clone());
            }
            scope.is_some()
        });
        scope
    }

    /// The package `backtrace` prints in front of a running function (the package of the namespace
    /// object below the one the function was called through). TGE swaps entry
    /// lists when packages change, so the object a function was called through holds the layer at the
    /// function's [`Holder`] position, and the object below it carries that layer's package: for `Top`
    /// the package of the current top layer, for `Below(p)` the package of the layer now directly below
    /// `p`; nothing for the base layer, an unlinked `p`, or `Detached`. So a function called before a
    /// package was activated prints that package once it is on top (`[Pkg]onStart`).
    pub fn frame_package(&self, namespace: &[u8], holder: Holder) -> Option<&[u8]> {
        let table = self.tables.get(namespace)?;
        // Charged like a `walk` over the namespace: the scan of the active packages is hostile-sized.
        self.add_work(1);
        if !table.packages.is_empty() {
            self.add_work(self.active.len() as u64);
        }
        let mut chain = self.chain(table).map(|(package, _)| package);
        let package = match holder {
            Holder::Top => chain.next()?,
            Holder::Below(above) => {
                chain.position(|p| p == Some(above))?;
                chain.next()?
            }
            Holder::Detached => None,
        }?;
        self.package_names.get(package as usize).map(Vec::as_slice)
    }

    /// `Namespace::classLinkTo`: makes `parent` the class parent of `child` (both created when missing).
    /// A child that already has another parent keeps it; linking to the same parent again is
    /// fine. No cycle check: TGE has none, and every walk stops at [`MAX_NAMESPACE_DEPTH`].
    pub fn link_class(&mut self, child: &[u8], parent: &[u8]) -> Result<(), LinkConflict> {
        let parent_key = fold(parent);
        self.table_mut(parent);
        let old = self.table_mut(child).parent.clone();
        match old {
            Some(old) if old != parent_key => Err(LinkConflict {
                child: self.display(child),
                old: self.display(&old),
                new: self.display(parent),
            }),
            _ => {
                self.table_mut(child).parent = Some(parent_key);
                Ok(())
            }
        }
    }

    /// The spelling a namespace was first named with (the given text when it does not exist).
    pub fn display(&self, namespace: &[u8]) -> Vec<u8> {
        self.tables
            .get(fold(namespace).as_slice())
            .map_or_else(|| namespace.to_vec(), |t| t.display.clone())
    }

    /// [`display`](Self::display) written over `out` (its buffer kept).
    pub fn display_into(&self, namespace: &[u8], out: &mut Vec<u8>) {
        let shown = crate::text::with_folded(namespace, |key| {
            self.tables.get(key).map(|t| {
                crate::vars::refill(out, &t.display);
            })
        });
        if shown.is_none() {
            crate::vars::refill(out, namespace);
        }
    }

    /// [`display`](Self::display) for an already folded namespace name, without a copy; None when the
    /// namespace does not exist.
    pub fn display_ref(&self, folded: &[u8]) -> Option<&[u8]> {
        self.tables.get(folded).map(|t| t.display.as_slice())
    }

    /// The names along the chain as the warning after an unknown method prints them: one per linked
    /// layer (a package layer repeats its namespace's name), joined with ` -> `.
    pub fn chain_names(&self, namespace: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        self.walk(namespace, |table, _| {
            if !out.is_empty() {
                out.extend_from_slice(b" -> ");
            }
            out.extend_from_slice(&table.display);
            false
        });
        out
    }

    /// The spelling of package `id` as it was first declared (None for an id no package has).
    pub fn package_name_of(&self, id: PackageId) -> Option<&[u8]> {
        self.package_names.get(id as usize).map(Vec::as_slice)
    }

    /// The spelling a package is printed with: the one it was declared with, else the given one.
    pub fn package_name(&self, package: &[u8]) -> Vec<u8> {
        self.package_ids
            .get(fold(package).as_slice())
            .and_then(|id| self.package_names.get(*id as usize))
            .cloned()
            .unwrap_or_else(|| package.to_vec())
    }

    /// Makes a function declaration that names no package of its own go into `scope`'s package (made if it is new), when the
    /// file the declaration is in is one the scope applies to, until this is called again: how a host runs a file as one
    /// package without the file saying so. A declaration that names a package keeps it, and one in another file (a file the
    /// scripts `exec`) is declared as usual. Returns the scope that was set, so a caller that nests puts it back afterwards.
    pub fn set_declare_package(&mut self, scope: Option<DeclareScope>) -> Option<DeclareScope> {
        if let Some(scope) = &scope {
            self.intern_package(&scope.package);
        }
        std::mem::replace(&mut self.declare_package, scope)
    }

    /// The package a function declared in the file `file` (the code block's name) goes into when it names none
    /// ([`set_declare_package`](Self::set_declare_package)).
    pub fn declare_package_for(&self, file: Option<&[u8]>) -> Option<&[u8]> {
        let scope = self.declare_package.as_ref()?;
        (scope.applies)(file?).then_some(scope.package.as_slice())
    }

    /// How many packages can be active at once.
    pub fn max_active(&self) -> usize {
        self.max_active.unwrap_or(MAX_ACTIVE_PACKAGES)
    }

    /// Sets how many packages can be active at once.
    pub fn set_max_active(&mut self, max: usize) {
        self.max_active = Some(max);
    }

    /// Puts a package on top of the active stack. The checks run in TGE's order: the limit,
    /// then "already active", then "no such package".
    pub fn activate(&mut self, package: &[u8]) -> Activation {
        if self.active.len() >= self.max_active() {
            return Activation::LimitReached;
        }
        let Some(id) = self.package_ids.get(fold(package).as_slice()).copied() else {
            return Activation::NotFound;
        };
        if self.active.contains(&id) {
            return Activation::AlreadyActive;
        }
        self.active.push(id);
        self.generation = self.generation.wrapping_add(1);
        Activation::Activated
    }

    /// Removes a package from the active stack; the layers above it and below it stay linked.
    pub fn deactivate(&mut self, package: &[u8]) {
        if let Some(id) = self.package_ids.get(fold(package).as_slice()).copied() {
            self.active.retain(|p| *p != id);
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// A number that changes with every change of the tables: a declaration (also one that replaces a function in place), a
    /// package's layer made, a class linked, a package activated or deactivated.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The active packages in activation order (the last one is on top), as they were declared.
    pub fn active_packages(&self) -> Vec<Vec<u8>> {
        (0..self.active.len())
            .filter_map(|i| self.active_package(i).map(<[u8]>::to_vec))
            .collect()
    }

    /// How many packages are active.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// The `index`th active package in activation order (0 is the first one activated), as declared.
    pub fn active_package(&self, index: usize) -> Option<&[u8]> {
        let id = self.active.get(index)?;
        self.package_names.get(*id as usize).map(Vec::as_slice)
    }

    /// Whether any function has been declared in a package of this name (or a declaration in it was refused).
    pub fn is_package(&self, package: &[u8]) -> bool {
        self.package_ids.contains_key(fold(package).as_slice())
    }
}

#[cfg(test)]
mod memo_tests {
    use std::rc::Rc;

    use super::{Callable, LookupMemo, Namespaces};

    fn native() -> Callable {
        Callable::Native {
            min: 0,
            max: 0,
            usage: Rc::from(""),
            f: Rc::new(|_, _| crate::vm::Native::Void),
        }
    }

    fn held(ns: &Namespaces) -> (usize, usize) {
        let memo = ns.memo.borrow();
        let bytes: usize = memo.answers.keys().map(Vec::len).sum();
        assert_eq!(bytes, memo.bytes, "the memo counts the bytes it holds");
        (memo.answers.len(), bytes)
    }

    /// SECURITY (resource cap): long names a script chose are looked up (and found) without the memo keeping them, and
    /// a flood of short distinct ones keeps the memo under its byte cap; lookups give the walk's answers either way.
    #[test]
    fn hostile_names_keep_the_lookup_memo_bounded() {
        let mut ns = Namespaces::default();
        let long_ns = vec![b'n'; 100_000];
        let long_name = vec![b'f'; 100_000];
        ns.declare(&long_ns, None, &long_name, native());
        ns.declare(b"Short", None, b"f", native());
        for i in 0..2_000u32 {
            let mut name = long_name.clone();
            name.extend_from_slice(&i.to_le_bytes());
            assert!(ns.lookup(&long_ns, &name).is_none());
            assert!(ns.lookup(&long_ns, &long_name).is_some());
            assert!(ns.lookup(b"short", &name).is_none());
        }
        let (_, bytes) = held(&ns);
        assert_eq!(bytes, 0, "no long key is kept");
        // Short distinct names: as many as the memo can count, each kept, the bytes capped.
        for i in 0..50_000u32 {
            let name = format!("{i:0>200}");
            assert!(ns.lookup(b"Short", name.as_bytes()).is_none());
            assert!(ns.lookup(b"SHORT", b"F").is_some());
            if i % 97 == 0 {
                let (count, bytes) = held(&ns);
                assert!(
                    count <= LookupMemo::MAX && bytes <= LookupMemo::MAX_BYTES,
                    "{count} keys, {bytes} bytes"
                );
            }
        }
        let (count, _) = held(&ns);
        assert!(count > 100, "short names are kept ({count})");
    }
}

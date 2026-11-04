//! LALR(1) parse tables built from a grammar description, the way yacc builds them: LR(0) states, lookaheads by
//! propagation (the dragon book's algorithm 4.63), shift/reduce conflicts settled by precedence and
//! associativity (a rule takes the precedence of its `%prec` token, else of the last terminal of its right-hand
//! side; no precedence on either side means shift), reduce/reduce conflicts by the earlier rule. The same
//! grammar gives the same automaton as yacc's: the same accepted programs, the same token at which an error is
//! found, and the same moments at which rules are reduced (which decide the line numbers the nodes get). Like
//! yacc's tables, a state's row has a default reduction for the tokens it has no action for, so on a bad token
//! the parser makes the same reductions as yacc's before it reports the error.

use std::collections::HashMap;

/// A terminal (token kind) or a nonterminal, by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sym {
    T(u8),
    N(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    Left,
    Right,
    NonAssoc,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub lhs: u8,
    pub rhs: Vec<Sym>,
    /// `%prec`: the terminal whose precedence the rule takes.
    pub prec: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct Grammar {
    /// Terminal 0 is the end of input. At most 128 terminals.
    pub terminals: usize,
    pub nonterminals: usize,
    /// Rule 0 must be the start rule `S -> start` (accepted at end of input).
    pub rules: Vec<Rule>,
    /// Per terminal: precedence level (higher binds tighter) and associativity.
    pub prec: Vec<Option<(u8, Assoc)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Error,
    Shift(u16),
    Reduce(u16),
    Accept,
}

/// The tables a driver needs: per state, the action for each terminal and the goto for each nonterminal,
/// and the states that reduce without reading a token ([`Tables::default_reduce`]).
#[derive(Debug, Clone)]
pub struct Tables {
    pub terminals: usize,
    pub nonterminals: usize,
    pub action: Vec<Action>,
    pub goto: Vec<u16>,
    /// The state's only possible action is this reduction: yacc's driver reduces without reading a token.
    pub default_reduce: Vec<Option<u16>>,
    pub rule_len: Vec<u16>,
    pub rule_lhs: Vec<u8>,
    /// Shift/reduce conflicts resolved by default (shift) and reduce/reduce conflicts, for the tests.
    pub conflicts: Vec<String>,
}

impl Tables {
    pub fn states(&self) -> usize {
        self.default_reduce.len()
    }

    /// The action of `state` on `terminal` (an error for a number that is no terminal).
    pub fn action(&self, state: u16, terminal: u8) -> Action {
        if usize::from(terminal) >= self.terminals {
            return Action::Error;
        }
        self.action
            .get(usize::from(state) * self.terminals + usize::from(terminal))
            .copied()
            .unwrap_or(Action::Error)
    }

    /// The state `state` goes to after reducing to `nonterminal` (`u16::MAX` for none).
    pub fn goto(&self, state: u16, nonterminal: u8) -> u16 {
        if usize::from(nonterminal) >= self.nonterminals {
            return u16::MAX;
        }
        self.goto
            .get(usize::from(state) * self.nonterminals + usize::from(nonterminal))
            .copied()
            .unwrap_or(u16::MAX)
    }
}

type Item = (u16, u16); // (rule, dot)
type Set = u128;

/// The lookahead set of one terminal.
fn bit(t: u8) -> Set {
    1u128.checked_shl(u32::from(t)).unwrap_or(0)
}

struct Builder<'g> {
    g: &'g Grammar,
    by_lhs: Vec<Vec<u16>>,
    nullable: Vec<bool>,
    first: Vec<Set>,
}

impl<'g> Builder<'g> {
    fn new(g: &'g Grammar) -> Builder<'g> {
        let mut by_lhs = vec![Vec::new(); g.nonterminals];
        for (i, r) in g.rules.iter().enumerate() {
            if let Some(list) = by_lhs.get_mut(usize::from(r.lhs)) {
                list.push(i as u16);
            }
        }
        let mut b = Builder {
            g,
            by_lhs,
            nullable: vec![false; g.nonterminals],
            first: vec![0; g.nonterminals],
        };
        b.compute_first();
        b
    }

    fn first(&self, n: u8) -> Set {
        self.first.get(usize::from(n)).copied().unwrap_or(0)
    }

    fn nullable(&self, n: u8) -> bool {
        self.nullable.get(usize::from(n)).copied().unwrap_or(false)
    }

    fn rule(&self, rule: u16) -> Option<&'g Rule> {
        self.g.rules.get(usize::from(rule))
    }

    /// The rules of a nonterminal.
    fn rules_of(&self, n: u8) -> &[u16] {
        self.by_lhs.get(usize::from(n)).map_or(&[], Vec::as_slice)
    }

    fn compute_first(&mut self) {
        let g = self.g;
        loop {
            let mut changed = false;
            for r in &g.rules {
                let lhs = usize::from(r.lhs);
                let mut all_nullable = true;
                let mut set: Set = 0;
                for s in &r.rhs {
                    match *s {
                        Sym::T(t) => {
                            set |= bit(t);
                            all_nullable = false;
                        }
                        Sym::N(n) => {
                            set |= self.first(n);
                            if !self.nullable(n) {
                                all_nullable = false;
                            }
                        }
                    }
                    if !all_nullable {
                        break;
                    }
                }
                if let Some(first) = self.first.get_mut(lhs)
                    && *first | set != *first
                {
                    *first |= set;
                    changed = true;
                }
                if all_nullable
                    && let Some(nullable) = self.nullable.get_mut(lhs)
                    && !*nullable
                {
                    *nullable = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// FIRST of a symbol string followed by the lookahead set `follow`.
    fn first_of(&self, syms: &[Sym], follow: Set) -> Set {
        let mut set: Set = 0;
        for s in syms {
            match *s {
                Sym::T(t) => return set | bit(t),
                Sym::N(n) => {
                    set |= self.first(n);
                    if !self.nullable(n) {
                        return set;
                    }
                }
            }
        }
        set | follow
    }

    fn next_sym(&self, (rule, dot): Item) -> Option<Sym> {
        self.rule(rule)?.rhs.get(usize::from(dot)).copied()
    }

    /// The LR(0) closure of a kernel.
    fn closure0(&self, kernel: &[Item]) -> Vec<Item> {
        let mut items: Vec<Item> = kernel.to_vec();
        let mut seen = vec![false; self.g.nonterminals];
        let mut i = 0;
        while let Some(&item) = items.get(i) {
            if let Some(Sym::N(n)) = self.next_sym(item)
                && let Some(flag) = seen.get_mut(usize::from(n))
                && !*flag
            {
                *flag = true;
                for &r in self.rules_of(n) {
                    items.push((r, 0));
                }
            }
            i += 1;
        }
        items
    }

    /// The LR(1) closure of items with lookahead sets.
    fn closure1(&self, kernel: &[(Item, Set)]) -> Vec<(Item, Set)> {
        let mut items: Vec<(Item, Set)> = kernel.to_vec();
        let mut index: HashMap<Item, usize> = items
            .iter()
            .enumerate()
            .map(|(i, (it, _))| (*it, i))
            .collect();
        let mut work: Vec<usize> = (0..items.len()).collect();
        while let Some(i) = work.pop() {
            let Some(&((rule, dot), la)) = items.get(i) else {
                continue;
            };
            let Some(Sym::N(n)) = self.next_sym((rule, dot)) else {
                continue;
            };
            let beta = self
                .rule(rule)
                .and_then(|r| r.rhs.get(usize::from(dot) + 1..))
                .unwrap_or(&[]);
            let la_new = self.first_of(beta, la);
            for &r in self.rules_of(n) {
                match index.get(&(r, 0)) {
                    Some(&j) => {
                        if let Some(item) = items.get_mut(j)
                            && item.1 | la_new != item.1
                        {
                            item.1 |= la_new;
                            work.push(j);
                        }
                    }
                    None => {
                        index.insert((r, 0), items.len());
                        work.push(items.len());
                        items.push(((r, 0), la_new));
                    }
                }
            }
        }
        items
    }

    fn rule_prec(&self, rule: u16) -> Option<(u8, Assoc)> {
        let r = self.rule(rule)?;
        let t = r.prec.or_else(|| {
            r.rhs.iter().rev().find_map(|s| match s {
                Sym::T(t) => Some(*t),
                Sym::N(_) => None,
            })
        })?;
        self.g.prec.get(usize::from(t)).copied().flatten()
    }
}

/// The transitions of a state.
fn transitions_of(transitions: &[Vec<(Sym, u16)>], state: usize) -> &[(Sym, u16)] {
    transitions.get(state).map_or(&[], Vec::as_slice)
}

/// The lookahead set of a kernel item.
fn la_at(la: &mut [Vec<Set>], state: usize, item: usize) -> Option<&mut Set> {
    la.get_mut(state)?.get_mut(item)
}

/// Builds the tables. The grammar is fixed, so a malformed one is a programming error found by the
/// tests; this returns an error string rather than panicking.
pub fn build(g: &Grammar) -> Result<Tables, String> {
    if g.terminals > 128 || g.nonterminals > 255 || g.rules.is_empty() {
        return Err("grammar too large or empty".into());
    }
    let b = Builder::new(g);
    // LR(0) states.
    let mut kernels: Vec<Vec<Item>> = vec![vec![(0, 0)]];
    let mut ids: HashMap<Vec<Item>, u16> = HashMap::new();
    ids.insert(vec![(0, 0)], 0);
    let mut transitions: Vec<Vec<(Sym, u16)>> = Vec::new();
    let mut s = 0;
    while let Some(kernel) = kernels.get(s) {
        let closure = b.closure0(kernel);
        let mut by_sym: Vec<(Sym, Vec<Item>)> = Vec::new();
        for &(rule, dot) in &closure {
            if let Some(sym) = b.next_sym((rule, dot)) {
                match by_sym.iter_mut().find(|(x, _)| *x == sym) {
                    Some((_, v)) => v.push((rule, dot + 1)),
                    None => by_sym.push((sym, vec![(rule, dot + 1)])),
                }
            }
        }
        let mut trans = Vec::new();
        for (sym, mut kernel) in by_sym {
            kernel.sort_unstable();
            let id = match ids.get(&kernel) {
                Some(&id) => id,
                None => {
                    let id = u16::try_from(kernels.len()).map_err(|_| "too many states")?;
                    ids.insert(kernel.clone(), id);
                    kernels.push(kernel);
                    id
                }
            };
            trans.push((sym, id));
        }
        transitions.push(trans);
        s += 1;
    }
    let goto_of = |state: usize, sym: Sym| {
        transitions_of(&transitions, state)
            .iter()
            .find(|(x, _)| *x == sym)
            .map(|(_, t)| *t)
    };
    // Lookaheads of kernel items: spontaneous ones and propagation links.
    const DUMMY: u8 = 127;
    let mut la: Vec<Vec<Set>> = kernels.iter().map(|k| vec![0; k.len()]).collect();
    if let Some(end) = la_at(&mut la, 0, 0) {
        *end = bit(0); // end of input
    }
    let mut links: Vec<((usize, usize), (usize, usize))> = Vec::new();
    for (state, kernel) in kernels.iter().enumerate() {
        for (ki, &item) in kernel.iter().enumerate() {
            for ((rule, dot), set) in b.closure1(&[(item, bit(DUMMY))]) {
                let Some(sym) = b.next_sym((rule, dot)) else {
                    continue;
                };
                let Some(target) = goto_of(state, sym) else {
                    continue;
                };
                let target = usize::from(target);
                let moved = (rule, dot + 1);
                let Some(ti) = kernels
                    .get(target)
                    .and_then(|k| k.iter().position(|x| *x == moved))
                else {
                    continue;
                };
                if let Some(slot) = la_at(&mut la, target, ti) {
                    *slot |= set & !bit(DUMMY);
                }
                if set & bit(DUMMY) != 0 {
                    links.push(((state, ki), (target, ti)));
                }
            }
        }
    }
    loop {
        let mut changed = false;
        for &((s1, i1), (s2, i2)) in &links {
            let add = la_at(&mut la, s1, i1).map_or(0, |x| *x);
            if let Some(slot) = la_at(&mut la, s2, i2)
                && *slot | add != *slot
            {
                *slot |= add;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Actions.
    let nt = g.terminals;
    let nn = g.nonterminals;
    let states = kernels.len();
    let mut action = vec![Action::Error; states * nt];
    let mut goto = vec![u16::MAX; states * nn];
    let mut default_reduce = vec![None; states];
    let mut conflicts = Vec::new();
    for (state, kernel) in kernels.iter().enumerate() {
        let lookaheads = la.get(state).map_or(&[][..], Vec::as_slice);
        let kernel: Vec<(Item, Set)> = kernel
            .iter()
            .copied()
            .zip(lookaheads.iter().copied())
            .collect();
        let items = b.closure1(&kernel);
        let Some(row) = action.get_mut(state * nt..(state + 1) * nt) else {
            continue;
        };
        for &(sym, target) in transitions_of(&transitions, state) {
            match sym {
                Sym::T(t) => {
                    if let Some(slot) = row.get_mut(usize::from(t)) {
                        *slot = Action::Shift(target);
                    }
                }
                Sym::N(n) => {
                    if let Some(slot) = goto.get_mut(state * nn + usize::from(n)) {
                        *slot = target;
                    }
                }
            }
        }
        let mut reductions: Vec<u16> = Vec::new();
        for &((rule, dot), set) in &items {
            if b.rule(rule).map(|r| r.rhs.len()) != Some(usize::from(dot)) {
                continue;
            }
            for (t, slot) in row.iter_mut().enumerate() {
                if set & bit(t as u8) == 0 {
                    continue;
                }
                if rule == 0 {
                    *slot = Action::Accept;
                    continue;
                }
                *slot = match *slot {
                    Action::Error => Action::Reduce(rule),
                    Action::Accept => Action::Accept,
                    Action::Reduce(other) => {
                        conflicts.push(format!(
                            "state {state}: reduce/reduce {other}/{rule} on {t}"
                        ));
                        Action::Reduce(other.min(rule))
                    }
                    Action::Shift(to) => {
                        match (g.prec.get(t).copied().flatten(), b.rule_prec(rule)) {
                            (Some((tp, assoc)), Some((rp, _))) => {
                                if rp > tp {
                                    Action::Reduce(rule)
                                } else if rp < tp {
                                    Action::Shift(to)
                                } else {
                                    match assoc {
                                        Assoc::Left => Action::Reduce(rule),
                                        Assoc::Right => Action::Shift(to),
                                        Assoc::NonAssoc => Action::Error,
                                    }
                                }
                            }
                            _ => {
                                conflicts
                                    .push(format!("state {state}: shift/reduce {rule} on {t}"));
                                Action::Shift(to)
                            }
                        }
                    }
                };
            }
            if rule != 0 && !reductions.contains(&rule) {
                reductions.push(rule);
            }
        }
        let shifts = transitions_of(&transitions, state)
            .iter()
            .any(|(s, _)| matches!(s, Sym::T(_)));
        if !shifts
            && reductions.len() == 1
            && let Some(slot) = default_reduce.get_mut(state)
        {
            *slot = reductions.first().copied();
        }
        // yacc's default reduction: a token with no action of its own takes the state's default reduction
        // instead of being an error, so an erroneous token is reported only after the reductions it allows
        // (the reduced statements are kept). A default reduction (yacc's `yydef`, `yyex`) is set in every state
        // with a reduction except where that reduction is of an empty rule and the state also shifts; in a state
        // with several reductions it is the one with the most tokens (ties to the earlier rule).
        let count = |rule: u16| row.iter().filter(|a| **a == Action::Reduce(rule)).count();
        let default = match reductions.as_slice() {
            [] => None,
            [only] => {
                let empty = b.rule(*only).is_none_or(|r| r.rhs.is_empty());
                (!empty || !shifts).then_some(*only)
            }
            several => several
                .iter()
                .copied()
                .max_by_key(|&r| (count(r), std::cmp::Reverse(r))),
        };
        if let Some(rule) = default {
            for slot in row.iter_mut() {
                if *slot == Action::Error {
                    *slot = Action::Reduce(rule);
                }
            }
        }
    }
    Ok(Tables {
        terminals: nt,
        nonterminals: nn,
        action,
        goto,
        default_reduce,
        rule_len: g.rules.iter().map(|r| r.rhs.len() as u16).collect(),
        rule_lhs: g.rules.iter().map(|r| r.lhs).collect(),
        conflicts,
    })
}

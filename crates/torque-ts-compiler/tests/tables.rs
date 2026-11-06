//! The parser's tables, pinned: a hash of every syntax's actions, gotos, default reductions and conflicts, as the builder made
//! them before its closure found its items by rule. A change to the grammar changes it on purpose (and this number with it).

use torque_ts_compiler::lalr::Action;
use torque_ts_compiler::parser::{Syntax, tables_for};

#[test]
fn every_syntax_s_tables_are_the_ones_pinned() {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |v: u64| h = (h ^ v).wrapping_mul(0x0100_0000_01b3);
    for base in [Syntax::TORQUE3D, Syntax::TGE14] {
        for do_while in [false, true] {
            for for_omitted_parts in [false, true] {
                let syntax = Syntax {
                    do_while,
                    for_omitted_parts,
                    ..base
                };
                let t = tables_for(&syntax).expect("the grammar builds");
                eat(t.states() as u64);
                for a in &t.action {
                    eat(match a {
                        Action::Error => 0,
                        Action::Accept => 1,
                        Action::Shift(s) => 0x1_0000 | u64::from(*s),
                        Action::Reduce(r) => 0x2_0000 | u64::from(*r),
                    });
                }
                for g in &t.goto {
                    eat(u64::from(*g));
                }
                for d in &t.default_reduce {
                    eat(d.map_or(u64::MAX, u64::from));
                }
                for c in &t.conflicts {
                    c.bytes().for_each(|b| eat(u64::from(b)));
                }
            }
        }
    }
    assert_eq!(h, 0xd28f_9d7d_0b4c_f082, "tables hash {h:#018x}");
}

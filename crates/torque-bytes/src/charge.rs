//! The work-budget callbacks: a decoder or a walk is told how many units of work it is about to do and asks the callback whether
//! the budget can pay.

use std::cell::Cell;
use std::fmt;

/// The work callback of a decoder: told how many units of work are about to be done (one unit is one byte of wire input, and at
/// least one for an element that takes fewer), it answers whether the budget can pay. A `false` stops the work.
pub type ChargeFn<'a> = dyn FnMut(u64) -> bool + 'a;

/// A [`ChargeFn`] borrowed for one call.
pub type Charge<'a> = &'a mut dyn FnMut(u64) -> bool;

/// The budget could not pay: what [`pay`] returns, for the caller's error type to take (`From<OutOfBudget>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfBudget;

impl fmt::Display for OutOfBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("work budget")
    }
}

impl std::error::Error for OutOfBudget {}

/// Pays `units` to `charge`.
///
/// # Errors
/// [`OutOfBudget`] when the budget cannot pay.
pub fn pay(charge: &mut ChargeFn, units: u64) -> Result<(), OutOfBudget> {
    if charge(units) {
        Ok(())
    } else {
        Err(OutOfBudget)
    }
}

/// A charge callback that pays for everything: for tests and for callers with no budget.
pub fn free_charge() -> impl FnMut(u64) -> bool {
    |_| true
}

/// A charge callback that pays out of `units` and refuses once they are spent.
pub fn budget_charge(units: u64) -> impl FnMut(u64) -> bool {
    let budget = Cell::new(Some(units));
    move |cost| spend(&budget, cost)
}

/// A charge callback that pays out of `budget` (`None`: no bound) and refuses once it is spent. What is left stays in the
/// cell, so the decoders of one datagram (its head, its events, its records) can draw on one budget.
pub fn cell_charge(budget: &Cell<Option<u64>>) -> impl FnMut(u64) -> bool + '_ {
    move |cost| spend(budget, cost)
}

/// Takes `cost` out of `budget`, or refuses (taking nothing) when it has less.
fn spend(budget: &Cell<Option<u64>>, cost: u64) -> bool {
    match budget.get() {
        None => true,
        Some(left) => left.checked_sub(cost).is_some_and(|left| {
            budget.set(Some(left));
            true
        }),
    }
}

#[cfg(test)]
#[path = "charge_tests.rs"]
mod tests;

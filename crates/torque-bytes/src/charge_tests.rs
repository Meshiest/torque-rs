use super::*;

#[test]
fn a_budget_refuses_what_it_cannot_pay_in_full() {
    let mut charge = budget_charge(10);
    assert_eq!(pay(&mut charge, 4), Ok(()));
    assert_eq!(pay(&mut charge, 7), Err(OutOfBudget));
    // The refusal took nothing: the six left still pay.
    assert_eq!(pay(&mut charge, 6), Ok(()));
    assert_eq!(pay(&mut charge, 1), Err(OutOfBudget));
    assert_eq!(pay(&mut charge, 0), Ok(()));

    let cell = Cell::new(Some(5));
    assert!(!cell_charge(&cell)(6));
    assert_eq!(cell.get(), Some(5));
    assert!(cell_charge(&cell)(5));
    assert_eq!(cell.get(), Some(0));
    assert!(!cell_charge(&cell)(1));

    let unbounded = Cell::new(None);
    assert!(cell_charge(&unbounded)(u64::MAX));
    assert!(free_charge()(u64::MAX));
}

#[test]
fn the_error_reads_as_the_budget() {
    assert_eq!(OutOfBudget.to_string(), "work budget");
}

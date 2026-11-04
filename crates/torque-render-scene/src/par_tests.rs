use super::*;

#[test]
fn results_come_back_in_order_split_or_not() {
    let items: Vec<usize> = (0..1000).collect();
    let split = map(&items, |_| MIN_WORK_A_THREAD, |i| i * 3);
    assert_eq!(split, items.iter().map(|i| i * 3).collect::<Vec<_>>());
    let small = map(&items[..3], |_| 1, |i| i + 1);
    assert_eq!(small, vec![1, 2, 3]);
    let none: Vec<usize> = map(&[] as &[usize], |_| 1, |i| *i);
    assert!(none.is_empty());
    // Uneven weights: still every item once, in order.
    let uneven = map(&items, |i| if *i == 10 { 1_000_000 } else { 1 }, |i| *i);
    assert_eq!(uneven, items);
    let owned = map_owned(items.clone(), |_| MIN_WORK_A_THREAD, |i| i * 2);
    assert_eq!(owned, items.iter().map(|i| i * 2).collect::<Vec<_>>());
    let owned_uneven = map_owned(
        items.clone(),
        |i| if *i > 990 { 1_000_000 } else { 1 },
        |i| i,
    );
    assert_eq!(owned_uneven, items);
}

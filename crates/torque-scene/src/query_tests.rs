use super::*;

/// A found object as an index would give it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    object: u32,
}

impl SearchItem for Entry {
    fn object(&self) -> u32 {
        self.object
    }
}

fn entry(n: u32) -> Entry {
    Entry { object: n + 1 }
}

/// `n` objects, numbered 1 to `n`.
fn entries(n: u32) -> Vec<Entry> {
    (1..=n).map(|object| Entry { object }).collect()
}

fn free() -> impl FnMut(u64) -> bool {
    |_| true
}

#[test]
fn the_search_boxes_are_the_centre_and_what_each_call_adds() {
    assert_eq!(
        radius_box([1.0, 2.0, 3.0], 0.5),
        [0.5, 1.5, 2.5, 1.5, 2.5, 3.5]
    );
    assert_eq!(
        size_box([1.0, 2.0, 3.0], [2.0, 4.0, 6.0]),
        [0.0, 0.0, 0.0, 2.0, 4.0, 6.0]
    );
    assert_eq!(
        radii_box([0.0; 3], [1.0, 2.0, 3.0]),
        [-1.0, -2.0, -3.0, 1.0, 2.0, 3.0]
    );
    // `containerFindFirst` takes the absolute value of each radius, `radii_box` does not.
    assert_eq!(
        find_box([0.0; 3], [-1.0, 2.0, -3.0]),
        [-1.0, -2.0, -3.0, 1.0, 2.0, 3.0]
    );
    assert_eq!(
        radii_box([0.0; 3], [-1.0, 2.0, -3.0]),
        [1.0, -2.0, 3.0, -1.0, 2.0, -3.0]
    );
}

#[test]
fn a_distance_is_to_the_centre_of_the_box() {
    // A 1 x 1 x 1 box centred at (3, 0, 0), seen from the origin: 3, whatever its size.
    assert_eq!(
        center_distance([2.5, -0.5, -0.5], [3.5, 0.5, 0.5], [0.0; 3]),
        3.0
    );
    // 3-4-5.
    assert_eq!(
        center_distance([2.0, 3.0, -1.0], [4.0, 5.0, 1.0], [0.0; 3]),
        5.0
    );
    // A point inside: the distance to the centre, not zero.
    assert_eq!(center_distance([-1.0; 3], [1.0; 3], [1.0, 0.0, 0.0]), 1.0);
    // Each axis counts: the centre is (1, 1, 1), the point is 4 above it.
    assert_eq!(center_distance([0.0; 3], [2.0; 3], [1.0, 1.0, 5.0]), 4.0);
    assert_eq!(center_distance([0.0; 3], [2.0; 3], [1.0, 5.0, 1.0]), 4.0);
    assert_eq!(center_distance([0.0; 3], [2.0; 3], [5.0, 1.0, 1.0]), 4.0);
}

#[test]
fn the_radius_distance_takes_the_smallest_side_off_and_stops_at_zero() {
    let (min, max) = ([2.0, -1.0, -2.0], [4.0, 1.0, 2.0]); // centre (3, 0, 0), sides 2, 2, 4
    assert_eq!(radius_distance(min, max, [0.0; 3]), 1.0);
    assert_eq!(radius_distance(min, max, [3.0, 0.0, 0.0]), 0.0);
    assert!(radius_distance([f32::NAN; 3], max, [0.0; 3]).is_nan());
}

#[test]
fn the_radius_filter_measures_to_the_nearest_face_and_keeps_only_what_is_strictly_inside() {
    let (min, max) = ([2.0, -1.0, -1.0], [4.0, 1.0, 1.0]);
    assert_eq!(box_distance_squared(min, max, [0.0; 3]), 4.0);
    assert_eq!(box_distance_squared(min, max, [3.0, 0.0, 0.0]), 0.0);
    assert_eq!(box_distance_squared(min, max, [0.0, 3.0, 0.0]), 4.0 + 4.0);
    assert_eq!(
        box_distance_squared(min, max, [6.0, 3.0, 4.0]),
        4.0 + 4.0 + 9.0
    );
    // The radius is strict: a box at exactly the radius is out, one inside it is in.
    assert!(!within_radius(4.0, 2.0));
    assert!(within_radius(3.99, 2.0));
    assert!(!within_radius(f32::NAN, 2.0));
}

#[test]
fn the_comparator_is_one_minus_one_or_zero_and_zero_for_a_nan() {
    assert_eq!(compare_distances(2.0, 1.0), 1);
    assert_eq!(compare_distances(1.0, 2.0), -1);
    assert_eq!(compare_distances(1.0, 1.0), 0);
    assert_eq!(compare_distances(f32::NAN, 1.0), 0);
    assert_eq!(compare_distances(1.0, f32::NAN), 0);
}

#[test]
fn a_search_sorts_by_distance_keeps_the_found_order_for_ties_and_walks_with_its_cursor() {
    // Five objects on the x axis at distances 3, 1, 2, 1, 3 from the origin, as unit boxes.
    let at = |object: u32| {
        let x = [3.0, 1.0, 2.0, 1.0, 3.0][object as usize - 1];
        ([x - 0.5, -0.5, -0.5], [x + 0.5, 0.5, 0.5])
    };
    let found: Vec<Entry> = (0..5).map(entry).collect();
    let mut search = Search::new();
    search
        .start([0.0; 3], found.clone(), &at, &mut free())
        .expect("paid");
    let order: Vec<u32> = search.entries().iter().map(|e| e.object).collect();
    // The plain sort is stable: equal distances stay in the order they were found.
    assert_eq!(order, [2, 4, 3, 1, 5]);
    assert_eq!(search.len(), 5);
    // The cursor starts before the first and ends at the count.
    let alive = |_: Entry| true;
    assert_eq!(search.current(&alive), None);
    let mut seen = Vec::new();
    while let Some(e) = search.next(&alive) {
        assert_eq!(search.current(&alive), Some(e));
        seen.push(e.object);
    }
    assert_eq!(seen, order);
    assert_eq!(search.next(&alive), None);
    assert_eq!(search.current(&alive), None);
    // Once used up the cursor stays at the count, however often it is asked.
    assert_eq!(search.next(&alive), None);
    assert_eq!(search.cursor, 5);
    // A new search starts over.
    search
        .start([0.0; 3], found[..2].to_vec(), &at, &mut free())
        .expect("paid");
    assert_eq!(search.len(), 2);
    assert_eq!(search.current(&alive), None);
}

#[test]
fn a_search_skips_an_object_that_died_and_the_distance_call_reads_zero_for_it() {
    let at = |object: u32| {
        let x = object as f32;
        ([x - 0.5, -0.5, -0.5], [x + 0.5, 0.5, 0.5])
    };
    let found: Vec<Entry> = (0..3).map(entry).collect();
    let mut search = Search::new();
    search
        .start([0.0; 3], found, &at, &mut free())
        .expect("paid");
    let alive = |e: Entry| e.object != 2;
    let mut seen = Vec::new();
    while let Some(e) = search.next(&alive) {
        seen.push(e.object);
    }
    assert_eq!(seen, vec![1, 3], "object 2 died and is skipped");
    // On a dead entry nothing is current.
    let mut again = search.clone();
    again.clear();
    assert_eq!(again.len(), 0);
    let mut on_two = Search::new();
    let all = |_: Entry| true;
    on_two
        .start(
            [0.0; 3],
            (0..3).map(entry).collect(),
            &|object: u32| {
                let x = object as f32;
                ([x - 0.5, -0.5, -0.5], [x + 0.5, 0.5, 0.5])
            },
            &mut free(),
        )
        .expect("paid");
    on_two.next(&all);
    on_two.next(&all);
    assert_eq!(on_two.current(&all).map(|e| e.object), Some(2));
    assert_eq!(on_two.current(&alive), None);
}

#[test]
fn an_unpayable_sort_leaves_the_list_empty() {
    let at = |_: u32| ([0.0; 3], [1.0; 3]);
    let found: Vec<Entry> = (0..4).map(entry).collect();
    let mut search = Search::new();
    let mut broke = |_: u64| false;
    assert_eq!(
        search.start([0.0; 3], found, &at, &mut broke),
        Err(Exhausted)
    );
    assert!(search.is_empty());
}

/// SECURITY (resource cap): the candidates are paid for before their distances are worked out, so a list of any
/// length that cannot be paid for costs nothing, and one over [`MAX_SEARCH_RESULTS`] is cut before it is walked.
#[test]
fn the_candidates_are_paid_for_before_anything_is_worked_out_and_the_list_is_cut() {
    let at = |_: u32| -> ([f32; 3], [f32; 3]) { panic!("no distance may be worked out unpaid") };
    let mut search = Search::new();
    let mut broke = |_: u64| false;
    assert_eq!(
        search.start([0.0; 3], entries(3), &at, &mut broke),
        Err(Exhausted)
    );
    // One object listed past the cap more than a million times: the first payment is for the cut list.
    let one = entries(1)[0];
    let over = vec![one; MAX_SEARCH_RESULTS + 10];
    let mut asked = Vec::new();
    let mut refuse = |units: u64| {
        asked.push(units);
        false
    };
    let mut search = Search::new();
    let flat = |_: u32| ([0.0; 3], [1.0; 3]);
    assert_eq!(
        search.start([0.0; 3], over, &flat, &mut refuse),
        Err(Exhausted)
    );
    assert_eq!(asked, vec![CANDIDATE_WORK * MAX_SEARCH_RESULTS as u64]);
}

#[test]
fn the_find_list_hands_out_the_objects_in_order_and_ends() {
    let mut list = FindList::new();
    assert_eq!(list.next_object(), None);
    list.start(vec![7, 5, 9]);
    assert_eq!(list.next_object(), Some(7));
    assert_eq!(list.next_object(), Some(5));
    list.start(vec![1]);
    assert_eq!(list.next_object(), Some(1));
    assert_eq!(list.next_object(), None);
    assert_eq!(list.next_object(), None);
}

#[test]
fn the_find_list_keeps_at_most_the_result_cap() {
    let mut list = FindList::new();
    list.start(vec![1; MAX_SEARCH_RESULTS + 5]);
    let mut n = 0usize;
    while list.next_object().is_some() {
        n += 1;
    }
    assert_eq!(n, MAX_SEARCH_RESULTS);
}

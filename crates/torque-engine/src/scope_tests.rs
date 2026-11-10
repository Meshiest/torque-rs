use super::*;

/// A script engine with `n` connections, and their objects.
fn connections(n: usize) -> (Vm, Vec<ObjId>) {
    let mut vm = Vm::new();
    vm.register_class(CONNECTION_CLASS, "SimObject", &[]);
    let made = (0..n)
        .map(|_| {
            vm.new_object(CONNECTION_CLASS.as_bytes(), &[])
                .expect("an object")
                .expect("a connection")
        })
        .collect();
    (vm, made)
}

#[test]
fn a_hide_reaches_the_connection_it_names_or_every_one() {
    let (vm, c) = connections(2);
    let scope = ScopeControl::default();
    assert!(!scope.is_hidden(5, Some(c[0])));
    assert_eq!(scope.hide(&vm, 5, Audience::Connection(c[0])), Ok(true));
    assert_eq!(scope.hide(&vm, 5, Audience::Connection(c[0])), Ok(false));
    assert!(scope.is_hidden(5, Some(c[0])));
    assert!(!scope.is_hidden(5, Some(c[1])));
    assert!(!scope.is_hidden(5, None));
    assert_eq!(scope.hide(&vm, 5, Audience::Everyone), Ok(true));
    assert!(scope.is_hidden(5, Some(c[1])));
    assert!(scope.is_hidden(5, None));
    // Showing one audience leaves the other's hide.
    assert!(scope.show(5, Audience::Everyone));
    assert!(!scope.show(5, Audience::Everyone));
    assert!(scope.is_hidden(5, Some(c[0])));
    assert!(!scope.is_hidden(5, Some(c[1])));
    assert!(scope.show(5, Audience::Connection(c[0])));
    assert!(!scope.is_hidden(5, Some(c[0])));
    assert_eq!(scope.hidden_count(Audience::Connection(c[0])), 0);
}

#[test]
fn only_a_connection_can_be_an_audience() {
    let (mut vm, c) = connections(1);
    let scope = ScopeControl::default();
    vm.register_class("Crate", "SimObject", &[]);
    let other = vm
        .new_object(b"Crate", &[])
        .expect("an object")
        .expect("a crate");
    // Neither an object of another class nor one that is not there is a connection.
    for not in [other, 9_999_999] {
        assert_eq!(
            scope.hide(&vm, 5, Audience::Connection(not)),
            Err(HideError::NotAConnection(not))
        );
    }
    assert_eq!(scope.hidden_count(Audience::Connection(other)), 0);
    assert_eq!(scope.hide(&vm, 5, Audience::Connection(c[0])), Ok(true));
}

#[test]
fn a_connections_stamp_moves_only_for_changes_that_may_concern_it() {
    let (vm, c) = connections(2);
    let scope = ScopeControl::default();
    let (a, b) = (scope.stamp_for(Some(c[0])), scope.stamp_for(Some(c[1])));
    scope.hide(&vm, 9, Audience::Connection(c[0])).unwrap();
    let a1 = scope.stamp_for(Some(c[0]));
    assert!(a1 > a);
    assert_eq!(scope.stamp_for(Some(c[1])), b, "another connection's hide");
    // A hide that changes nothing moves nothing; one from everyone moves every connection's.
    scope.hide(&vm, 9, Audience::Connection(c[0])).unwrap();
    assert!(!scope.show(9, Audience::Connection(c[1])));
    assert_eq!(scope.stamp_for(Some(c[0])), a1);
    scope.hide(&vm, 4, Audience::Everyone).unwrap();
    assert!(scope.stamp_for(Some(c[0])) > a1);
    assert!(scope.stamp_for(Some(c[1])) > b);
    assert!(scope.stamp_for(None) > 0);
    // It never goes back, even when a connection's last hide is shown and a new one made.
    let before = scope.stamp_for(Some(c[0]));
    scope.show(9, Audience::Connection(c[0]));
    scope.hide(&vm, 10, Audience::Connection(c[0])).unwrap();
    assert!(scope.stamp_for(Some(c[0])) > before);
}

#[test]
fn a_deleted_object_and_a_gone_connection_take_their_entries() {
    let (mut vm, c) = connections(1);
    let scope = ScopeControl::default();
    scope.hide(&vm, 1, Audience::Everyone).unwrap();
    scope.hide(&vm, 1, Audience::Connection(c[0])).unwrap();
    scope.hide(&vm, 2, Audience::Connection(c[0])).unwrap();
    scope.forget_object(1);
    assert!(!scope.is_hidden(1, Some(c[0])));
    assert!(scope.is_hidden(2, Some(c[0])));
    assert_eq!(scope.hidden_count(Audience::Everyone), 0);
    vm.delete_object(c[0]).expect("deleted");
    scope.sweep(&vm);
    assert!(!scope.is_hidden(2, Some(c[0])));
    assert_eq!(scope.hidden.borrow().count, 0);
}

#[test]
fn an_audience_cannot_take_the_room_of_the_others_and_a_refusal_is_counted() {
    let (vm, c) = connections(2);
    let scope = ScopeControl::default();
    for obj in 0..MAX_PER_AUDIENCE as u32 {
        assert_eq!(scope.hide(&vm, obj, Audience::Connection(c[0])), Ok(true));
    }
    let stamp = scope.stamp_for(Some(c[0]));
    // The connection has its share; it is refused, and nothing changes.
    let over = MAX_PER_AUDIENCE as u32;
    assert_eq!(
        scope.hide(&vm, over, Audience::Connection(c[0])),
        Err(HideError::Full)
    );
    assert!(!scope.is_hidden(over, Some(c[0])));
    assert_eq!(scope.stamp_for(Some(c[0])), stamp);
    assert_eq!(scope.refused(), 1);
    // An object it holds already is no growth, and the others' room is whole.
    assert_eq!(scope.hide(&vm, 0, Audience::Connection(c[0])), Ok(false));
    assert_eq!(scope.hide(&vm, over, Audience::Connection(c[1])), Ok(true));
    assert_eq!(scope.hide(&vm, over, Audience::Everyone), Ok(true));
    assert_eq!(scope.refused(), 1);
}

#[test]
fn a_hide_past_the_pool_is_refused_and_changes_nothing() {
    let (vm, _) = connections(1);
    let scope = ScopeControl::default();
    scope.hidden.borrow_mut().count = MAX_HIDDEN;
    assert_eq!(scope.hide(&vm, 1, Audience::Everyone), Err(HideError::Full));
    assert!(!scope.is_hidden(1, None));
    assert_eq!(scope.refused(), 1);
}

#[test]
fn a_server_gets_a_fresh_control_only_when_a_plugin_asks() {
    let plain = crate::EngineBuilder::new().build().unwrap();
    let mut vm = Vm::new();
    plain.replay(ServerStage::Classes, &mut vm).unwrap();
    assert!(ScopeControl::of(&vm).is_none());

    let mut builder = crate::EngineBuilder::new();
    builder.net().scope_control().scope_control();
    let engine = builder.build().unwrap();
    let mut first = Vm::new();
    engine.replay(ServerStage::Classes, &mut first).unwrap();
    ScopeControl::of(&first)
        .unwrap()
        .hide(&first, 3, Audience::Everyone)
        .unwrap();
    let mut second = Vm::new();
    engine.replay(ServerStage::Classes, &mut second).unwrap();
    assert!(!ScopeControl::of(&second).unwrap().is_hidden(3, None));
}

#[test]
fn each_plugin_hides_and_shows_its_own_and_the_others_hides_stay() {
    let (vm, c) = connections(1);
    let scope = ScopeControl::default();
    assert_eq!(
        scope.hide_as(&vm, "one", 7, Audience::Connection(c[0])),
        Ok(true)
    );
    assert_eq!(
        scope.hide_as(&vm, "two", 7, Audience::Connection(c[0])),
        Ok(true)
    );
    assert_eq!(
        scope.hide_as(&vm, "two", 7, Audience::Connection(c[0])),
        Ok(false)
    );
    assert!(scope.is_hidden_by_other("one", 7, Some(c[0])));
    assert!(scope.is_hidden_by_other("two", 7, Some(c[0])));
    assert!(!scope.is_hidden_by_other("three", 8, Some(c[0])));
    // The plugin that lifts its hide leaves the other's; the object is shown when both have.
    assert!(scope.show_as("one", 7, Audience::Connection(c[0])));
    assert!(!scope.show_as("one", 7, Audience::Connection(c[0])));
    assert!(scope.is_hidden(7, Some(c[0])));
    assert!(!scope.is_hidden_by_other("two", 7, Some(c[0])));
    assert_eq!(scope.hidden_count(Audience::Connection(c[0])), 1);
    assert!(scope.show_as("two", 7, Audience::Connection(c[0])));
    assert!(!scope.is_hidden(7, Some(c[0])));
    assert_eq!(scope.hidden_count(Audience::Connection(c[0])), 0);
    // Hidden from everyone by one plugin is hidden for every connection, and other for the rest.
    scope.hide_as(&vm, "one", 9, Audience::Everyone).unwrap();
    assert!(scope.is_hidden_by_other("two", 9, Some(c[0])));
    assert!(!scope.is_hidden_by_other("one", 9, Some(c[0])));
}

#[test]
fn a_reader_is_told_what_the_other_plugins_hid_and_showed_since_its_place() {
    let (vm, c) = connections(2);
    let scope = ScopeControl::default();
    let start = scope.log_end();
    scope
        .hide_as(&vm, "mine", 1, Audience::Connection(c[0]))
        .unwrap();
    scope
        .hide_as(&vm, "theirs", 2, Audience::Connection(c[1]))
        .unwrap();
    scope.hide_as(&vm, "theirs", 3, Audience::Everyone).unwrap();
    scope.show_as("theirs", 2, Audience::Connection(c[1]));
    let mut seen = Vec::new();
    let next = scope.changes_since("mine", start, &mut seen).unwrap();
    assert_eq!(
        seen,
        [
            (2, Audience::Connection(c[1])),
            (3, Audience::Everyone),
            (2, Audience::Connection(c[1]))
        ]
    );
    // Nothing new since.
    seen.clear();
    assert_eq!(scope.changes_since("mine", next, &mut seen), Ok(next));
    assert!(seen.is_empty());
    // A reader far behind is told so, and where to start again.
    for obj in 0..(MAX_LOG as u32 + 10) {
        scope
            .hide_as(&vm, "theirs", 1_000 + obj % 10, Audience::Everyone)
            .unwrap();
        scope.show_as("theirs", 1_000 + obj % 10, Audience::Everyone);
    }
    assert_eq!(
        scope.changes_since("mine", start, &mut seen),
        Err(scope.log_end())
    );
}

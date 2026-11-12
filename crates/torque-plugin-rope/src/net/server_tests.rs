use super::*;

/// `count` live objects of `vm`, made by a script.
fn objects(vm: &mut Vm, count: usize) -> Vec<ObjId> {
    let script = format!("for (%i = 0; %i < {count}; %i++) new ScriptObject(\"fx_target_\" @ %i);");
    vm.eval(script.as_bytes()).expect("the objects");
    (0..count)
        .map(|i| {
            vm.objects()
                .find(format!("fx_target_{i}").as_bytes())
                .expect("made")
        })
        .collect()
}

#[test]
fn a_flood_of_fresh_targets_cannot_reset_a_held_clients_second() {
    let mut vm = torque_game::testing::vm();
    let targets = objects(&mut vm, FX_TRACKED + 500);
    let budget = FxBudget::default();
    let held = targets[0];
    for _ in 0..FX_PER_SECOND {
        assert!(matches!(budget.spend(&vm, held), Spend::Send));
    }
    assert!(matches!(budget.spend(&vm, held), Spend::Drop(true)));
    // More live targets than the table holds, in the same second: the held client is still over its budget, and the
    // targets past the table's room are refused, not let through.
    let mut sent = 0;
    for (n, &target) in targets[1..].iter().enumerate() {
        sent += usize::from(matches!(budget.spend(&vm, target), Spend::Send));
        if n % 64 == 0 {
            assert!(
                matches!(budget.spend(&vm, held), Spend::Drop(false)),
                "after {n} other targets"
            );
        }
    }
    assert_eq!(sent, FX_TRACKED - 1);
}

//! What the host lets through of a client's calls to the server commands plugins declared: bursts, refills, argument checks,
//! the connections and commands apart, and the console lines.

use torque_engine::PluginId;
use torque_engine::command::{ArgType, CommandRefusal, CommandSpec, ServerCommand};

use super::{Admission, CommandTable, ConnCommands, REPORT_TICKS};

fn table(commands: &[(&str, CommandSpec)]) -> CommandTable {
    let commands: Vec<ServerCommand> = commands
        .iter()
        .map(|(name, spec)| ServerCommand {
            function: format!("serverCmd{name}"),
            plugin: PluginId::new("test"),
            spec: spec.clone(),
        })
        .collect();
    CommandTable::new(&commands)
}

fn args(list: &[&str]) -> Vec<Vec<u8>> {
    list.iter().map(|a| a.as_bytes().to_vec()).collect()
}

fn runs(admission: &Admission) -> bool {
    *admission == Admission::Run
}

#[test]
fn a_thousand_calls_in_one_tick_run_at_most_the_burst() {
    let table = table(&[(
        "Ping",
        CommandSpec::new().arg("n", ArgType::Int).rate(10, 7),
    )]);
    let mut conn = ConnCommands::default();
    let ran = (0..1000)
        .filter(|_| runs(&conn.admit(&table, 5, b"serverCmdPing", &args(&["1"]), 40)))
        .count();
    assert_eq!(ran, 7);
    assert_eq!(conn.drops("Ping"), 993);
    assert_eq!(
        conn.drops("serverCmdPING"),
        993,
        "with the prefix, without case"
    );
}

#[test]
fn the_bucket_refills_by_the_rate_over_600_ticks() {
    let table = table(&[(
        "Ping",
        CommandSpec::new().arg("n", ArgType::Int).rate(10, 3),
    )]);
    let mut conn = ConnCommands::default();
    let mut ran = 0;
    // Drain the burst, then call once a tick for 600 ticks.
    for _ in 0..3 {
        assert!(runs(&conn.admit(
            &table,
            5,
            b"serverCmdPing",
            &args(&["1"]),
            0
        )));
    }
    for tick in 1..=600 {
        ran += usize::from(runs(&conn.admit(
            &table,
            5,
            b"serverCmdPing",
            &args(&["1"]),
            tick,
        )));
    }
    // 19.2 seconds at 10 a second.
    assert_eq!(ran, 192);
    assert_eq!(conn.drops("Ping"), 600 - 192);
}

#[test]
fn a_call_with_the_wrong_arguments_never_runs_and_is_counted() {
    let spec = CommandSpec::new()
        .arg("n", ArgType::Int)
        .optional("note", ArgType::Text)
        .max_arg_bytes(8)
        .rate(1000, 1000);
    let table = table(&[("Ping", spec)]);
    let mut conn = ConnCommands::default();
    let mut judge = |list: &[&str]| conn.admit(&table, 5, b"serverCmdPing", &args(list), 0);
    assert!(runs(&judge(&["3"])));
    assert!(runs(&judge(&["3", "hello"])));
    let refused = |admission: Admission| match admission {
        Admission::Refused { reason, .. } => reason,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        refused(judge(&["3", "a", "b"])),
        CommandRefusal::TooManyArguments { given: 3, max: 2 }
    );
    assert_eq!(
        refused(judge(&[])),
        CommandRefusal::TooFewArguments {
            given: 0,
            needed: 1
        }
    );
    assert_eq!(
        refused(judge(&["3", "123456789"])),
        CommandRefusal::ArgumentTooLong {
            number: 2,
            len: 9,
            max: 8
        }
    );
    assert_eq!(
        refused(judge(&["three"])),
        CommandRefusal::BadArgument {
            number: 1,
            expected: ArgType::Int
        }
    );
    assert_eq!(conn.drops("Ping"), 4);
}

#[test]
fn a_malformed_flood_spends_the_bucket_too() {
    let table = table(&[("Ping", CommandSpec::new().arg("n", ArgType::Int).rate(1, 4))]);
    let mut conn = ConnCommands::default();
    for _ in 0..4 {
        conn.admit(&table, 5, b"serverCmdPing", &args(&["junk"]), 10);
    }
    // The four tokens went to malformed calls: a good one now is over the rate.
    assert!(matches!(
        conn.admit(&table, 5, b"serverCmdPing", &args(&["1"]), 10),
        Admission::Refused {
            reason: CommandRefusal::RateLimited,
            ..
        }
    ));
}

#[test]
fn commands_and_connections_have_buckets_of_their_own() {
    let table = table(&[
        ("Ping", CommandSpec::new().arg("n", ArgType::Int).rate(1, 2)),
        ("Pong", CommandSpec::new().arg("n", ArgType::Int).rate(1, 2)),
    ]);
    let (mut a, mut b) = (ConnCommands::default(), ConnCommands::default());
    for _ in 0..2 {
        assert!(runs(&a.admit(
            &table,
            1,
            b"serverCmdPing",
            &args(&["1"]),
            0
        )));
    }
    assert!(!runs(&a.admit(
        &table,
        1,
        b"serverCmdPing",
        &args(&["1"]),
        0
    )));
    // Another command of the same connection, and the same command of another connection, are untouched.
    assert!(runs(&a.admit(
        &table,
        1,
        b"serverCmdPong",
        &args(&["1"]),
        0
    )));
    assert!(runs(&b.admit(
        &table,
        2,
        b"serverCmdPing",
        &args(&["1"]),
        0
    )));
    assert_eq!(
        (a.drops("Ping"), a.drops("Pong"), b.drops("Ping")),
        (1, 0, 0)
    );
}

#[test]
fn a_command_no_plugin_declared_is_not_limited() {
    let table = table(&[("Ping", CommandSpec::new().rate(1, 1))]);
    let mut conn = ConnCommands::default();
    for tick in 0..2000 {
        assert_eq!(
            conn.admit(
                &table,
                1,
                b"serverCmdOther",
                &args(&["a", "b", "c"]),
                tick / 100
            ),
            Admission::Unlimited
        );
    }
    assert_eq!(conn.drops("Other"), 0);
    // The name is looked up without case, as scripts do.
    assert!(runs(&conn.admit(&table, 1, b"SERVERCMDPING", &[], 0)));
}

#[test]
fn a_flood_prints_one_line_a_second_with_the_count_since_the_last() {
    let table = table(&[("Ping", CommandSpec::new().rate(0, 1))]);
    let mut conn = ConnCommands::default();
    assert!(runs(&conn.admit(&table, 9, b"serverCmdPing", &[], 0)));
    let mut lines = Vec::new();
    // A refused call on each of 100 ticks.
    for tick in 1..=100u64 {
        if let Admission::Refused {
            report: Some(line), ..
        } = conn.admit(&table, 9, b"serverCmdPing", &[], tick)
        {
            lines.push((tick, line));
        }
    }
    let ticks: Vec<u64> = lines.iter().map(|(tick, _)| *tick).collect();
    let expected: Vec<u64> = (0..4).map(|n| 1 + n * REPORT_TICKS).collect();
    assert_eq!(ticks, expected);
    assert_eq!(
        lines[0].1,
        "Client 9 serverCmdPing: 1 call(s) refused (too many calls)."
    );
    assert!(
        lines[1].1.contains(&format!("{} call(s)", REPORT_TICKS)),
        "{}",
        lines[1].1
    );
}

/// What a test caller holds.
struct Holds(&'static [&'static str]);

impl torque_engine::permissions::PermissionView for Holds {
    fn has_permission(&self, permission: &str) -> bool {
        self.0.contains(&permission)
    }
}

#[test]
fn a_command_with_a_permission_runs_only_for_a_caller_who_holds_it() {
    let table = table(&[(
        "Mute",
        CommandSpec::new()
            .arg("n", ArgType::Int)
            .requires("chat.mute")
            .rate(4, 8),
    )]);
    let mut conn = ConnCommands::default();
    let call = |conn: &mut ConnCommands, held: &Holds| {
        conn.admit_for(&table, 5, b"serverCmdMute", &args(&["1"]), 0, held)
    };
    for held in [Holds(&[]), Holds(&["chat.kick"])] {
        assert!(
            matches!(
                call(&mut conn, &held),
                Admission::Refused {
                    reason: CommandRefusal::NotPermitted,
                    ..
                }
            ),
            "a caller without it is refused"
        );
    }
    assert!(runs(&call(&mut conn, &Holds(&["chat.mute"]))));
    // Without a permission to hold, the plain entry point refuses too, and a command that asks for none is unaffected.
    assert!(matches!(
        conn.admit(&table, 5, b"serverCmdMute", &args(&["1"]), 0),
        Admission::Refused {
            reason: CommandRefusal::NotPermitted,
            ..
        }
    ));
    let open = self::table(&[("Ping", CommandSpec::new().arg("n", ArgType::Int))]);
    assert!(runs(&conn.admit_for(
        &open,
        5,
        b"serverCmdPing",
        &args(&["1"]),
        0,
        &Holds(&[])
    )));
    // The refusal is counted like any other.
    assert_eq!(conn.drops("Mute"), 3);
}

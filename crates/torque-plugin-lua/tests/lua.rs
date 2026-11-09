//! The Lua extension linked into an engine: the API surface, the sandbox, the limits, realms, mods and nesting.
//!
//! The same cases run on the web (`just lua-web-check`): a browser build of the engine with the page's Lua module
//! (`common::lua` is that build's `WebLuaPlugin`), so these are the checks of both.

// By its path, so that the web check, which builds this file from its own crate, finds it too.
#[macro_use]
#[path = "common/mod.rs"]
mod common;

use common::{Stopwatch, client, console, engine, eval, files, global, run, script, server};
use torque_engine::{EngineBuilder, ServerStage};
use torque_mods::{Mods, ModsPlugin};
use torque_ts_vm::vm::VmError;

/// The TorqueScript the shared scripts expect before they run.
const FIXTURES: &str =
    "new ScriptObject(Thing) { health = 10; };\nfunction tsDouble(%x) { return %x * 2; }";

/// Runs `test` on a thread with `kib` KiB of stack (on the web, on the page's own).
#[cfg(not(target_arch = "wasm32"))]
fn on_stack(kib: usize, test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(kib << 10)
        .spawn(test)
        .expect("the thread starts")
        .join()
        .expect("the test passes");
}

#[cfg(target_arch = "wasm32")]
fn on_stack(_kib: usize, test: impl FnOnce()) {
    test();
}

fn mod_toml(id: &str, extra: &str) -> Vec<u8> {
    format!("id = \"{id}\"\nversion = \"1.0.0\"\n{extra}").into_bytes()
}

cases! {
    fn the_shared_basics_script_passes() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        eval(&mut vm, FIXTURES).unwrap();
        console(&mut vm);
        assert_eq!(
            run(&mut vm, &files, "scripts/basics.lua", &script("basics.lua")),
            Ok(true)
        );
        let log = console(&mut vm);
        assert_eq!(global(&vm, "$Test::Result"), "ok", "{log}");
        assert!(log.contains("basics ran 3"), "{log}");
        // What the script defined, called from TorqueScript.
        assert_eq!(eval(&mut vm, "return luaAdd(2, 3);").unwrap(), "5");
        assert_eq!(
            eval(&mut vm, "return Thing.luaGreet(\"Ann\");").unwrap(),
            "hello Ann from Thing"
        );
        assert_eq!(eval(&mut vm, "return luaNothing();").unwrap(), "");
        assert_eq!(global(&vm, "$Test::FromLua"), "7");
        assert_eq!(eval(&mut vm, "return Thing.health;").unwrap(), "25");
    }

    fn the_shared_sandbox_script_passes() {
        let engine = engine();
        let files = files(&[("main.cs", b"")]);
        let mut vm = server(&engine, &files);
        eval(&mut vm, FIXTURES).unwrap();
        console(&mut vm);
        assert_eq!(
            run(
                &mut vm,
                &files,
                "scripts/sandbox.lua",
                &script("sandbox.lua")
            ),
            Ok(true)
        );
        let log = console(&mut vm);
        assert_eq!(global(&vm, "$Test::Result"), "ok", "{log}");
        assert!(files.get("out.cs").is_none(), "nothing was written");
    }

    fn a_script_error_is_printed_and_the_engine_goes_on() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        assert_eq!(
            run(&mut vm, &files, "bad.lua", "error('broken on purpose')"),
            Ok(true)
        );
        let log = console(&mut vm);
        assert!(log.contains("broken on purpose"), "{log}");
        assert_eq!(run(&mut vm, &files, "syntax.lua", "local = ="), Ok(true));
        assert!(console(&mut vm).contains("syntax.lua"));
        let recurse = "local function f() return f() + 1 end f()";
        assert_eq!(run(&mut vm, &files, "recurse.lua", recurse), Ok(true));
        assert!(console(&mut vm).contains("stack overflow"));
        assert_eq!(eval(&mut vm, "return 1 + 1;").unwrap(), "2");
    }

    fn compiled_lua_is_never_loaded() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        // A chunk of Luau bytecode: its version byte first.
        files.insert(
            "compiled.lua",
            b"\x06\x03\x02\x00ts.setGlobal('$Test::Ran', 1)",
        );
        assert_eq!(vm.exec_file(b"compiled.lua", false), Ok(true));
        assert!(console(&mut vm).contains("compiled Lua is never loaded"));
        assert_eq!(global(&vm, "$Test::Ran"), "");
    }

    // Limits.

    fn a_runaway_loop_stops_at_the_step_limit() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        vm.set_step_limit(Some(200_000));
        for (name, source) in [
            ("loop.lua", "while true do end"),
            // pcall cannot catch its way past the limit.
            (
                "pcall.lua",
                "while true do pcall(function() while true do end end) end",
            ),
            (
                "pcall-then-return.lua",
                "pcall(function() while true do end end) ts.setGlobal('$Test::After', 1)",
            ),
            // Nor can a coroutine.
            (
                "coroutine.lua",
                "local co = coroutine.wrap(function() while true do coroutine.yield() end end) while true do co() end",
            ),
            (
                "coroutine-error.lua",
                "while true do coroutine.resume(coroutine.create(function() while true do end end)) end",
            ),
            (
                "xpcall.lua",
                "while true do xpcall(function() while true do end end, function() while true do end end) end",
            ),
        ] {
            let ran = run(&mut vm, &files, name, source);
            assert_eq!(ran, Err(VmError::StepLimit), "{name}");
            console(&mut vm);
        }
        assert_eq!(
            global(&vm, "$Test::After"),
            "",
            "the code after the caught loop never ran"
        );
        // The engine and the realm go on.
        assert_eq!(eval(&mut vm, "return 6 * 7;").unwrap(), "42");
        assert_eq!(
            run(
                &mut vm,
                &files,
                "after.lua",
                "ts.setGlobal('$Test::Fine', 1)"
            ),
            Ok(true)
        );
        assert_eq!(global(&vm, "$Test::Fine"), "1");
    }

    fn the_limit_holds_across_torque_script_and_lua() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        vm.set_step_limit(Some(200_000));
        // A Lua callback that loops forever, called from TorqueScript.
        assert_eq!(
            run(
                &mut vm,
                &files,
                "define.lua",
                "ts.define('luaSpin', function() while true do end end)"
            ),
            Ok(true)
        );
        assert_eq!(
            eval(&mut vm, "luaSpin(); $Test::Reached = 1;"),
            Err(VmError::StepLimit)
        );
        assert_eq!(global(&vm, "$Test::Reached"), "");
        // A TorqueScript loop called from Lua.
        eval(&mut vm, "function tsSpin() { while (1) {} }").unwrap();
        assert_eq!(
            run(
                &mut vm,
                &files,
                "spin.lua",
                "ts.call('tsSpin') ts.setGlobal('$Test::Reached', 1)"
            ),
            Err(VmError::StepLimit)
        );
        assert_eq!(global(&vm, "$Test::Reached"), "");
        assert_eq!(eval(&mut vm, "return 2;").unwrap(), "2");
    }

    fn without_a_step_limit_the_default_budget_stops_a_runaway_loop() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        vm.set_step_limit(None);
        assert_eq!(
            run(&mut vm, &files, "loop.lua", "while true do end"),
            Err(VmError::StepLimit)
        );
        assert_eq!(eval(&mut vm, "return 3;").unwrap(), "3");
    }

    fn an_allocation_bomb_hits_the_memory_cap() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        vm.set_global("$Pref::Lua::MemoryLimit", "16");
        vm.set_step_limit(Some(500_000_000));
        for (name, source) in [
            ("rep.lua", "local s = string.rep('x', 1e9)"),
            ("doubling.lua", "local s = 'x' while true do s = s .. s end"),
            (
                "tables.lua",
                "local t = {} for i = 1, 1e9 do t[i] = { i, i, i, i } end",
            ),
            ("buffer.lua", "local b = buffer.create(1e9)"),
        ] {
            assert_eq!(run(&mut vm, &files, name, source), Ok(true), "{name}");
            let log = console(&mut vm);
            assert!(log.contains("memory"), "{name}: {log}");
        }
        // The engine and the realm go on.
        assert_eq!(eval(&mut vm, "return 5;").unwrap(), "5");
        assert_eq!(
            run(
                &mut vm,
                &files,
                "after.lua",
                "ts.setGlobal('$Test::Fine', #string.rep('a', 1000))"
            ),
            Ok(true)
        );
        assert_eq!(global(&vm, "$Test::Fine"), "1000");
    }

    // Realms.

    fn server_and_client_realms_are_apart() {
        let mut builder = EngineBuilder::new();
        builder
            .server_script()
            .function("", "serverOnly", &[], "", || "server".to_string());
        let engine = builder
            .add_plugins((ModsPlugin::new(), common::lua()))
            .build()
            .unwrap();
        let files = files(&[]);
        let mut server_vm = server(&engine, &files);
        let mut client_vm = client(&engine, &files);
        let probe = "shared = (shared or 0) + 1 \
                     ts.setGlobal('$Test::Realm', ts.realm .. ' ' .. tostring(SERVER) .. ' ' .. tostring(CLIENT) .. ' ' .. shared) \
                     ts.setGlobal('$Test::Native', ts.call('serverOnly'))";
        assert_eq!(run(&mut server_vm, &files, "probe.lua", probe), Ok(true));
        assert_eq!(run(&mut client_vm, &files, "probe.lua", probe), Ok(true));
        assert_eq!(run(&mut server_vm, &files, "probe.lua", probe), Ok(true));
        assert_eq!(global(&server_vm, "$Test::Realm"), "server true false 2");
        assert_eq!(global(&client_vm, "$Test::Realm"), "client false true 1");
        assert_eq!(global(&server_vm, "$Test::Native"), "server");
        assert_eq!(
            global(&client_vm, "$Test::Native"),
            "",
            "a client cannot reach a server's native"
        );
        assert!(console(&mut client_vm).contains("serverOnly: Unknown command."));
    }

    fn a_realm_goes_with_its_script_engine() {
        let engine = engine();
        let files = files(&[]);
        for _ in 0..3 {
            let mut vm = server(&engine, &files);
            assert_eq!(
                run(
                    &mut vm,
                    &files,
                    "a.lua",
                    "count = (count or 0) + 1 ts.setGlobal('$Test::Count', count)"
                ),
                Ok(true)
            );
            assert_eq!(
                global(&vm, "$Test::Count"),
                "1",
                "each script engine has a fresh realm"
            );
        }
    }

    // Nesting.

    fn torque_script_and_lua_call_each_other_deeply() {
        // A script engine runs on a thread of at least 1 MiB: recursion through Lua must stop there with an error.
        on_stack(1024, || {
            let engine = engine();
            let files = files(&[]);
            let mut vm = server(&engine, &files);
            eval(
                &mut vm,
                "function tsDown(%n) { if (%n <= 0) return \"bottom\"; return luaDown(%n - 1) @ \"\"; }",
            )
            .unwrap();
            assert_eq!(
                run(
                    &mut vm,
                    &files,
                    "down.lua",
                    "ts.define('luaDown', function(n) return ts.call('tsDown', n) end)"
                ),
                Ok(true)
            );
            // TS -> Lua -> TS -> Lua ...
            assert_eq!(eval(&mut vm, "return tsDown(12);").unwrap(), "bottom");
            console(&mut vm);
            // Past the call depth: the run stops with an error, and nothing crashes.
            assert_eq!(
                eval(&mut vm, "return tsDown(1000);"),
                Err(VmError::CallDepth)
            );
            // Lua that recurses into itself through TorqueScript, started from Lua.
            assert_eq!(
                run(
                    &mut vm,
                    &files,
                    "up.lua",
                    "ts.define('luaUp', function(n) return ts.call('luaUp', n + 1) end) ts.call('luaUp', 0)"
                ),
                Err(VmError::CallDepth)
            );
            assert_eq!(eval(&mut vm, "return tsDown(3);").unwrap(), "bottom");
        });
    }

    // Mods.

    fn a_mod_s_lua_runs_at_its_stage_and_switching_the_mod_off_stops_it() {
        let engine = engine();
        let files = files(&[
            ("mods/greeter/mod.toml", &mod_toml("greeter", "")),
            (
                "mods/greeter/server.lua",
                b"local words = require('lib.words')\n\
                  ts.setGlobal('$Test::Stage', 'server ' .. words.hello)\n\
                  ts.define('luaGreet', function(who) return words.hello .. ', ' .. who end)\n",
            ),
            (
                "mods/greeter/lib/words.lua",
                b"loads = (loads or 0) + 1\nreturn { hello = 'Hello', loads = loads }\n",
            ),
            ("mods/other/mod.toml", &mod_toml("other", "[scripts]\nserver = \"start.lua\"")),
            (
                "mods/other/start.lua",
                b"local ok = pcall(require, 'lib.words')\nts.setGlobal('$Test::Other', tostring(ok) .. ' ' .. tostring(loads))\n",
            ),
        ]);
        let mut vm = server(&engine, &files);
        let log = console(&mut vm);
        assert_eq!(global(&vm, "$Test::Stage"), "server Hello", "{log}");
        assert_eq!(
            global(&vm, "$Test::Other"),
            "false nil",
            "another mod cannot require this mod's modules or see its globals"
        );
        assert_eq!(
            eval(&mut vm, "return luaGreet(\"Bo\");").unwrap(),
            "Hello, Bo"
        );
        let mods = Mods::of(&vm).expect("the mods are there");
        mods.disable(&mut vm, "greeter").expect("switched off");
        console(&mut vm);
        assert_eq!(
            eval(&mut vm, "return luaGreet(\"Bo\");").unwrap(),
            "",
            "its function went with it"
        );
        assert!(console(&mut vm).contains("luaGreet"));
        mods.enable(&mut vm, "greeter").expect("switched on");
        assert_eq!(
            eval(&mut vm, "return luaGreet(\"Bo\");").unwrap(),
            "Hello, Bo"
        );
    }

    fn a_mod_with_a_hyphen_in_its_id_defines_functions_and_loses_them_when_switched_off() {
        let engine = engine();
        let files = files(&[
            ("mods/bigger-bombs/mod.toml", &mod_toml("bigger-bombs", "")),
            (
                "mods/bigger-bombs/server.lua",
                b"ts.define('boom', function() return 'big' end)
",
            ),
        ]);
        let mut vm = server(&engine, &files);
        let log = console(&mut vm);
        assert_eq!(eval(&mut vm, "return boom();").unwrap(), "big", "{log}");
        assert_eq!(
            eval(&mut vm, "return getModField(\"bigger-bombs\", \"package\");").unwrap(),
            "Mod_bigger_bombs"
        );
        let mods = Mods::of(&vm).expect("the mods are there");
        mods.disable(&mut vm, "bigger-bombs").expect("switched off");
        console(&mut vm);
        assert_eq!(
            eval(&mut vm, "return boom();").unwrap(),
            "",
            "its function went with it"
        );
    }

    fn a_mod_s_lua_needs_the_extension() {
        let engine = EngineBuilder::new()
            .add_plugins(ModsPlugin::new())
            .build()
            .unwrap();
        let files = files(&[
            ("mods/greeter/mod.toml", &mod_toml("greeter", "")),
            (
                "mods/greeter/server.lua",
                b"ts.setGlobal('$Test::Stage', 1)",
            ),
        ]);
        let mut vm = server(&engine, &files);
        assert!(console(&mut vm).contains("no script language"));
        assert_eq!(global(&vm, "$Test::Stage"), "");
    }

    fn lua_a_server_sent_never_runs_or_loads() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = client(&engine, &files);
        let mods = Mods::of(&vm).expect("the mods are there");
        let evil = b"ts.setGlobal('$Test::Pwned', 1)".to_vec();
        let sent = vec![
            ("evil.lua".to_string(), evil.clone()),
            ("scripts/EVIL.LUA".to_string(), evil.clone()),
            ("scripts/evil.luau".to_string(), evil.clone()),
            ("served.lua".to_string(), b"return 'served'".to_vec()),
            ("art/pic.png".to_string(), b"png".to_vec()),
        ];
        // Mounted as content (untrusted): never run, and never loaded by `require`.
        assert_eq!(mods.mount_server_files("server:1", sent), Ok(5));
        console(&mut vm);
        for path in ["evil.lua", "scripts/EVIL.LUA", "scripts/evil.luau"] {
            assert!(mods.is_server_file(path), "{path}");
            assert_eq!(vm.exec_file(path.as_bytes(), false), Ok(false), "{path}");
            assert!(console(&mut vm).contains("came from a server"), "{path}");
        }
        // Nor can a local script require it.
        assert_eq!(
            run(
                &mut vm,
                &files,
                "probe.lua",
                "ts.setGlobal('$Test::Lib', tostring((pcall(require, 'served'))))"
            ),
            Ok(true)
        );
        assert_eq!(global(&vm, "$Test::Lib"), "false");
        assert_eq!(global(&vm, "$Test::Pwned"), "");
        assert!(mods.is_server_file("art/pic.png"));
    }

    fn a_mod_runs_in_its_own_realm_per_side() {
        let engine = engine();
        let files = files(&[
            ("mods/both/mod.toml", &mod_toml("both", "")),
            (
                "mods/both/server.lua",
                b"ts.setGlobal('$Test::Side', ts.realm)",
            ),
            (
                "mods/both/client.lua",
                b"ts.setGlobal('$Test::Side', ts.realm)",
            ),
        ]);
        let server_vm = server(&engine, &files);
        let client_vm = client(&engine, &files);
        assert_eq!(global(&server_vm, "$Test::Side"), "server");
        assert_eq!(global(&client_vm, "$Test::Side"), "client");
        let _ = ServerStage::ALL;
    }

    fn lua_a_server_mounted_never_runs_in_a_server_realm_or_a_mod() {
        let engine = engine();
        let files = files(&[
            ("mods/m/mod.toml", &mod_toml("m", "")),
            (
                "mods/m/server.lua",
                b"function probe() return tostring((pcall(require, 'served'))) end",
            ),
        ]);
        let mut vm = server(&engine, &files);
        let mods = Mods::of(&vm).expect("the mods are there");
        let sent = vec![
            (
                "served.lua".to_string(),
                b"ts.setGlobal('$Test::Pwned', 1) return 1".to_vec(),
            ),
            (
                "mods/m/served.lua".to_string(),
                b"ts.setGlobal('$Test::Pwned', 1) return 1".to_vec(),
            ),
        ];
        assert_eq!(mods.mount_server_files("peer", sent), Ok(2));
        assert_eq!(vm.exec_file(b"served.lua", false), Ok(false));
        assert_eq!(
            run(
                &mut vm,
                &files,
                "probe.lua",
                "ts.setGlobal('$Test::Lib', tostring((pcall(require, 'served'))))"
            ),
            Ok(true)
        );
        assert_eq!(global(&vm, "$Test::Lib"), "false");
        assert_eq!(global(&vm, "$Test::Pwned"), "");
    }

    fn two_mods_defining_one_name_each_keep_their_own() {
        let engine = engine();
        let files = files(&[
            ("main.cs", b"function shared() { return \"base\"; }"),
            ("mods/a/mod.toml", &mod_toml("a", "")),
            (
                "mods/a/server.lua",
                b"ts.define('shared', function() return 'A' end)",
            ),
            ("mods/b/mod.toml", &mod_toml("b", "load_after = [\"a\"]")),
            (
                "mods/b/server.lua",
                b"ts.define('shared', function() return 'B' end)",
            ),
            // A TorqueScript mod on top, reaching the one below with `Parent::`.
            ("mods/c/mod.toml", &mod_toml("c", "load_after = [\"b\"]")),
            (
                "mods/c/server.cs",
                b"function shared() { return \"C>\" @ Parent::shared(); }",
            ),
        ]);
        let mut vm = server(&engine, &files);
        let shared = |vm: &mut torque_ts_vm::vm::Vm| eval(vm, "return shared();").unwrap();
        assert_eq!(shared(&mut vm), "C>B", "{}", console(&mut vm));
        let mods = Mods::of(&vm).unwrap();
        mods.disable(&mut vm, "b").unwrap();
        assert_eq!(shared(&mut vm), "C>A", "b off: a's answers again");
        mods.enable(&mut vm, "b").unwrap();
        assert_eq!(shared(&mut vm), "C>B");
        mods.disable(&mut vm, "a").unwrap();
        assert_eq!(shared(&mut vm), "C>B", "a off: b's still answers");
        mods.disable(&mut vm, "b").unwrap();
        assert_eq!(shared(&mut vm), "C>base", "both off: the game's own");
        mods.disable(&mut vm, "c").unwrap();
        assert_eq!(shared(&mut vm), "base");
        mods.enable(&mut vm, "a").unwrap();
        assert_eq!(shared(&mut vm), "A");
    }

    fn the_call_policy_holds_whatever_names_the_function() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        eval(&mut vm, FIXTURES).unwrap();
        for (what, code) in [
            (
                "SimObject::call",
                "ts.call('SimObject::call', ts.object('Thing'), 'save', 'out.cs')",
            ),
            (
                "FileObject::openForWrite",
                "ts.call('FileObject::openForWrite', ts.object('Thing'), 'out.cs')",
            ),
            (
                "SimObject::save",
                "ts.call('SimObject::save', ts.object('Thing'), 'out.cs')",
            ),
            ("Foo::exec", "ts.call('Foo::exec', 'main.cs')"),
            (
                "SimObject::schedule",
                "ts.call('SimObject::schedule', ts.object('Thing'), 0, 'save', 'out.cs')",
            ),
            (
                "schedule, any position",
                "ts.call('schedule', 0, 0, 'echo', 'exec')",
            ),
            (
                "method schedule",
                "ts.method('Thing', 'schedule', 0, 'call', 'save')",
            ),
            ("method call", "ts.object('Thing'):call('save', 'out.cs')"),
        ] {
            let source = format!(
                "local ok = pcall(function() {code} end) ts.setGlobal('$Test::Allowed', tostring(ok))"
            );
            assert_eq!(
                run(&mut vm, &files, "policy.lua", &source),
                Ok(true),
                "{what}"
            );
            let log = console(&mut vm);
            assert_eq!(global(&vm, "$Test::Allowed"), "false", "{what}: {log}");
            // A ScriptObject has no `call` method to refuse: calling the missing one fails on its own.
            if what != "method call" {
                assert!(log.contains("from a script language"), "{what}: {log}");
            }
        }
        assert!(files.get("out.cs").is_none(), "nothing was written");
    }

    fn bulk_builtins_are_charged_by_the_bytes_they_handle() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        vm.set_step_limit(Some(20_000));
        for (name, source) in [
            (
                "rep.lua",
                "while true do local s = string.rep('x', 16 * 1024 * 1024) end",
            ),
            (
                "rep-small.lua",
                "while true do local s = string.rep('x', 512 * 1024) end",
            ),
            (
                "fill.lua",
                "local b = buffer.create(1024 * 1024) while true do buffer.fill(b, 0, 1) end",
            ),
            (
                "copy.lua",
                "local a, b = buffer.create(1000000), buffer.create(1000000) while true do buffer.copy(b, 0, a) end",
            ),
            (
                "concat.lua",
                "local t = {} for i = 1, 100 do t[i] = ('x'):rep(1000) end while true do local s = table.concat(t) end",
            ),
            (
                "find.lua",
                "local s = ('a'):rep(500000) while true do string.find(s, 'b', 1, true) end",
            ),
            (
                "match.lua",
                "local s = ('a'):rep(200000) while true do s:match('a*a*b') end",
            ),
            (
                "table-find.lua",
                "local t = table.create(100000, 1) while true do table.find(t, 2) end",
            ),
            (
                "insert-front.lua",
                "local t = table.create(100000, 1) while true do table.insert(t, 1, 0) table.remove(t, 1) end",
            ),
            (
                "plain-find-needle.lua",
                "local s, n = ('a'):rep(65536), ('a'):rep(32768) .. 'b' while true do s:find(n, 1, true) end",
            ),
            (
                "balanced.lua",
                "local s = ('('):rep(65536) while true do s:find('%b()') end",
            ),
            (
                "split-needle.lua",
                "local s, n = ('a'):rep(65536), ('a'):rep(32768) .. 'b' while true do s:split(n) end",
            ),
            (
                "next-sparse.lua",
                "local t = table.create(100000) while true do next(t) end",
            ),
            (
                "clear-sparse.lua",
                "local t = table.create(100000) while true do table.clear(t) end",
            ),
            (
                "pcall-error-loop.lua",
                "while true do pcall(error, 'x') end",
            ),
            (
                "refused-ts-loop.lua",
                "while true do pcall(ts.call, 'exec', 'main.cs') end",
            ),
            (
                "unpack-past-length.lua",
                "while true do pcall(unpack, {}, 1, 7000) end",
            ),
            (
                "pairs-iterator-sparse.lua",
                "local t = table.create(100000) t[100000] = 1 local f = pairs(t) while true do f(t) end",
            ),
            (
                "rep-text-count.lua",
                "while true do local s = string.rep('x', '1e7') end",
            ),
            (
                "create-text-size.lua",
                "while true do local t = table.create('100000') end",
            ),
            (
                "clone-sparse.lua",
                "local t = table.create(100000) while true do local c = table.clone(t) end",
            ),
        ] {
            let started = Stopwatch::start();
            assert_eq!(
                run(&mut vm, &files, name, source),
                Err(VmError::StepLimit),
                "{name}"
            );
            let took = started.seconds();
            assert!(took < 2.0, "{name} took {took} s");
            console(&mut vm);
        }
    }

    fn garbage_is_collected_before_the_memory_cap_refuses() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        vm.set_global("$Pref::Lua::MemoryLimit", "16");
        vm.set_step_limit(Some(100_000_000));
        let source = "local n = 0 \
                      for i = 1, 60 do local s = string.rep('x', 5 * 1024 * 1024) n = n + #s end \
                      for i = 1, 60 do local s = string.rep('y', 3 * 1024 * 1024) .. i n = n + #s end \
                      ts.setGlobal('$Test::Made', n)";
        assert_eq!(run(&mut vm, &files, "temporaries.lua", source), Ok(true));
        let log = console(&mut vm);
        assert_ne!(global(&vm, "$Test::Made"), "", "{log}");
        // The cap is still hard.
        assert_eq!(
            run(
                &mut vm,
                &files,
                "bomb.lua",
                "local s = string.rep('x', 64 * 1024 * 1024)"
            ),
            Ok(true)
        );
        assert!(console(&mut vm).contains("memory"));
    }

    fn ordinary_loops_over_tables_and_strings_fit_the_budget() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        for limit in [Some(5_000_000), None] {
            vm.set_step_limit(limit);
            let source = "local t = {} for i = 1, 100000 do table.insert(t, i) end \
                          local s = string.rep('ab', 50000) local n = 0 \
                          for i = 1, #s do if s:sub(i, i) == 'a' then n += s:byte(i) end end \
                          for i = 1, 1000 do table.remove(t) end \
                          ts.setGlobal('$Test::Done', #t .. ' ' .. n)";
            assert_eq!(
                run(&mut vm, &files, "ordinary.lua", source),
                Ok(true),
                "{limit:?}"
            );
            let log = console(&mut vm);
            assert_eq!(
                global(&vm, "$Test::Done"),
                "99000 4850000",
                "{limit:?}: {log}"
            );
            vm.set_global("$Test::Done", "");
        }
    }

    fn code_valued_fields_and_bind_cmd_are_refused() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        eval(&mut vm, FIXTURES).unwrap();
        for (what, code) in [
            ("command", "ts.object('Thing').command = 'quit();'"),
            (
                "altCommand",
                "assert(ts.setField('Thing', 'altCommand', 'quit();'))",
            ),
            (
                "validate in another case",
                "ts.object('Thing').VALIDATE = 'quit();'",
            ),
            (
                "setFieldValue",
                "ts.call('SimObject::setFieldValue', 'Thing', 'command', 'quit();')",
            ),
            (
                "setFieldValue method",
                "ts.object('Thing'):setFieldValue('escapeCommand', 'quit();')",
            ),
            (
                "bindCmd",
                "ts.call('ActionMap::bindCmd', 'Thing', 'keyboard', 'a', 'quit();', '')",
            ),
            (
                "bindCmd method",
                "ts.method('Thing', 'bindCmd', 'keyboard', 'a', 'quit();', '')",
            ),
        ] {
            let source = format!(
                "local ok = pcall(function() {code} end) ts.setGlobal('$Test::Allowed', tostring(ok))"
            );
            assert_eq!(
                run(&mut vm, &files, "fields.lua", &source),
                Ok(true),
                "{what}"
            );
            let log = console(&mut vm);
            assert_eq!(global(&vm, "$Test::Allowed"), "false", "{what}: {log}");
        }
        for field in ["command", "altCommand", "validate", "escapeCommand"] {
            assert_eq!(
                eval(&mut vm, &format!("return Thing.{field};")).unwrap(),
                "",
                "{field}"
            );
        }
        // Other fields are written as before.
        assert_eq!(
            run(
                &mut vm,
                &files,
                "ok.lua",
                "ts.object('Thing').label = 'quit();'"
            ),
            Ok(true)
        );
        assert_eq!(eval(&mut vm, "return Thing.label;").unwrap(), "quit();");
    }

    fn a_code_field_cannot_be_reached_by_its_name_s_spelling() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        eval(&mut vm, FIXTURES).unwrap();
        // An index long enough that the joined name is cut at 255 bytes: what is left still starts with `command`.
        let long = format!(
            "assert(ts.setField('Thing', 'c', 'quit();', 'ommand{}'))",
            "x".repeat(300)
        );
        for (what, code) in [
            (
                "name and index join",
                "assert(ts.setField('Thing', 'comm', 'x', 'and'))".to_string(),
            ),
            (
                "a NUL in the name",
                "ts.object('Thing')['command\0x'] = 'quit();'".to_string(),
            ),
            (
                "a NUL hiding a suffix",
                "assert(ts.setField('Thing', 'x\0', 'quit();'))".to_string(),
            ),
            (
                "a NUL in the index",
                "assert(ts.setField('Thing', 'command', 'quit();', '\0'))".to_string(),
            ),
            (
                "command0",
                "ts.object('Thing').command0 = 'quit();'".to_string(),
            ),
            (
                "command[0]",
                "assert(ts.setField('Thing', 'command', 'quit();', 0))".to_string(),
            ),
            (
                "mixed case",
                "ts.object('Thing').CoMmAnD = 'quit();'".to_string(),
            ),
            (
                "an element by setFieldValue",
                "ts.object('Thing'):setFieldValue('command[0]', 'quit();')".to_string(),
            ),
            (
                "a NUL by setFieldValue",
                "ts.object('Thing'):setFieldValue('x\0command', 'quit();')".to_string(),
            ),
            ("a long joined name", long),
        ] {
            let source = format!(
                "local ok = pcall(function() {code} end) ts.setGlobal('$Test::Allowed', tostring(ok))"
            );
            assert_eq!(
                run(&mut vm, &files, "names.lua", &source),
                Ok(true),
                "{what}"
            );
            let log = console(&mut vm);
            assert_eq!(global(&vm, "$Test::Allowed"), "false", "{what}: {log}");
        }
        for field in ["command", "command0", "x", "command[0]"] {
            assert_eq!(
                eval(
                    &mut vm,
                    &format!("return Thing.getFieldValue(\"{field}\");")
                )
                .unwrap(),
                "",
                "{field}"
            );
        }
    }

    fn backtracking_patterns_are_bounded() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        // Too many backtracking quantifiers: refused before the search.
        assert_eq!(
            run(
                &mut vm,
                &files,
                "many.lua",
                "local ok, e = pcall(string.find, ('a'):rep(10), '.-.-.-.-.-.-.-b') ts.setGlobal('$Test::Ok', tostring(ok) .. ' ' .. tostring(e))"
            ),
            Ok(true)
        );
        let refused = global(&vm, "$Test::Ok");
        assert!(
            refused.starts_with("false") && refused.contains("more than 6"),
            "{refused}"
        );
        // Fewer, on a long subject: the matcher's steps are charged, so the step limit stops it in bounded time.
        for limit in [20_000, 1_000_000] {
            vm.set_step_limit(Some(limit));
            let started = Stopwatch::start();
            assert_eq!(
                run(
                    &mut vm,
                    &files,
                    "slow.lua",
                    "local x = ('a'):rep(3000):find('.-.-.-.-.-b')"
                ),
                Err(VmError::StepLimit)
            );
            let took = started.seconds();
            assert!(took < 3.0, "{limit}: {took} s");
            console(&mut vm);
        }
        // Ordinary patterns still work: escapes and sets are not quantifiers.
        vm.set_step_limit(Some(5_000_000));
        assert_eq!(
            run(
                &mut vm,
                &files,
                "ok.lua",
                "local k, v = ('  name = a-b  '):match('^%s*(%w+)%s*=%s*([%w%-]+)%s*$') ts.setGlobal('$Test::Kv', k .. ' ' .. v)"
            ),
            Ok(true)
        );
        assert_eq!(global(&vm, "$Test::Kv"), "name a-b", "{}", console(&mut vm));
    }

    fn a_field_spec_is_read_as_the_engine_reads_it() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        eval(&mut vm, FIXTURES).unwrap();
        let long = format!("c[ommand{}]", "x".repeat(300));
        for spec in [
            "comm[and]",
            "COMM[AND]",
            "com[mand]",
            "command[0]",
            "c[ommand[0]]",
            "comm[and",
            r"x\0",
            long.as_str(),
        ] {
            for code in [
                format!("ts.call('SimObject::setFieldValue', 'Thing', '{spec}', 'quit();')"),
                format!("ts.method('Thing', 'setFieldValue', '{spec}', 'quit();')"),
            ] {
                let source = format!(
                    "local ok = pcall(function() {code} end) ts.setGlobal('$Test::Allowed', tostring(ok))"
                );
                assert_eq!(
                    run(&mut vm, &files, "spec.lua", &source),
                    Ok(true),
                    "{spec}"
                );
                let log = console(&mut vm);
                assert_eq!(
                    global(&vm, "$Test::Allowed"),
                    "false",
                    "{spec}: {code}: {log}"
                );
                assert!(log.contains("from a script language"), "{spec}: {log}");
            }
        }
    }

    fn optional_items_count_as_backtracking() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        let source = "local ok, e = pcall(string.match, ('a'):rep(40), 'a?a?a?a?a?a?a?a?aaaaaaaa') \
                      ts.setGlobal('$Test::Ok', tostring(ok) .. ' ' .. tostring(e)) \
                      ts.setGlobal('$Test::Few', tostring(('ab'):match('a?b?')))";
        assert_eq!(run(&mut vm, &files, "optional.lua", source), Ok(true));
        let refused = global(&vm, "$Test::Ok");
        assert!(
            refused.starts_with("false") && refused.contains("more than 6"),
            "{refused}"
        );
        assert_eq!(global(&vm, "$Test::Few"), "ab");
    }

    fn set_data_field_s_name_and_index_are_checked_joined_in_either_form() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        eval(&mut vm, FIXTURES).unwrap();
        let long = format!("ommand{}", "x".repeat(300));
        for (slot, array) in [
            ("comm", "and"),
            ("COMM", "AND"),
            ("command", "0"),
            ("c", long.as_str()),
            (r"comm\0", "and"),
            ("x", r"\0"),
        ] {
            for code in [
                format!("ts.method('Thing', 'setDataField', '{slot}', '{array}', 'quit();')"),
                format!("ts.call('SimObject::setDataField', 'Thing', '{slot}', '{array}', 'quit();')"),
                format!("ts.call('setDataField', 'Thing', '{slot}', '{array}', 'quit();')"),
            ] {
                let source = format!(
                    "local ok = pcall(function() {code} end) ts.setGlobal('$Test::Allowed', tostring(ok))"
                );
                assert_eq!(
                    run(&mut vm, &files, "data.lua", &source),
                    Ok(true),
                    "{code}"
                );
                let log = console(&mut vm);
                assert_eq!(global(&vm, "$Test::Allowed"), "false", "{code}: {log}");
                assert!(log.contains("from a script language"), "{code}: {log}");
            }
        }
    }

    fn queued_and_bound_calls_are_held_to_the_policy() {
        let engine = engine();
        let files = files(&[]);
        let mut vm = server(&engine, &files);
        eval(&mut vm, FIXTURES).unwrap();
        eval(&mut vm, "function tsMark(%v) { $marked = %v; }").unwrap();
        for (what, code) in [
            (
                "schedule setFieldValue",
                "ts.call('schedule', 0, 0, 'setFieldValue', 'Thing', 'command', 'quit();')",
            ),
            (
                "schedule a method setFieldValue",
                "ts.object('Thing'):schedule(0, 'setFieldValue', 'command', 'quit();')",
            ),
            (
                "static schedule setDataField",
                "ts.call('SimObject::schedule', 'Thing', 0, 'setDataField', 'comm', 'and', 'quit();')",
            ),
            (
                "schedule a schedule",
                "ts.call('schedule', 0, 0, 'schedule', 0, 0, 'exec', 'main.cs')",
            ),
            (
                "bind exec",
                "ts.call('ActionMap::bind', 'Thing', 'keyboard', 'a', 'exec')",
            ),
            (
                "bindObj eval",
                "ts.call('ActionMap::bindObj', 'Thing', 'keyboard', 'a', 'eval', 'Thing')",
            ),
        ] {
            let source = format!(
                "local ok = pcall(function() {code} end) ts.setGlobal('$Test::Allowed', tostring(ok))"
            );
            assert_eq!(
                run(&mut vm, &files, "queue.lua", &source),
                Ok(true),
                "{what}"
            );
            let log = console(&mut vm);
            assert_eq!(global(&vm, "$Test::Allowed"), "false", "{what}: {log}");
        }
        vm.advance_time(10);
        assert_eq!(eval(&mut vm, "return Thing.command;").unwrap(), "");
        // An ordinary queued call runs when it fires.
        assert_eq!(
            run(
                &mut vm,
                &files,
                "ok.lua",
                "ts.call('schedule', 0, 0, 'tsMark', 'fired')"
            ),
            Ok(true)
        );
        vm.advance_time(10);
        assert_eq!(global(&vm, "$marked"), "fired");
    }
}

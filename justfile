set windows-shell := ["powershell", "-NoProfile", "-Command"]

# Kept in step with `rust-version` in Cargo.toml and MSRV in .github/workflows/ci.yml.
msrv := "1.88"

# The build folder: CARGO_TARGET_DIR's, else the workspace's (the site goes into its `site` folder)
web_target := env_var_or_default("CARGO_TARGET_DIR", justfile_directory() / "target")

# Where the web check's tools (puppeteer-core) are installed; exported so the recipes need no shell-specific syntax
export TORQUE_WEB_TOOLS := web_target / "web-tools" / "node"

# The examples' recipes, one justfile per example: `just examples <example> <recipe>` (see examples/justfile)
mod examples

default:
    @just --list

# Every local recipe builds with `--all-features`, as CI does: one feature set means one copy of each crate in the target folder
build:
    cargo build --workspace --all-features

# Every test (cargo-nextest, one process per test) and then the doc tests, which nextest does not run
test:
    cargo nextest run --workspace --all-features --no-fail-fast
    cargo test --workspace --all-features --doc

# The quick profile of `.config/nextest.toml`: leaves out the fuzz, golden, hostile and sweep tests and the wire_golden program
test-quick:
    cargo nextest run --workspace --all-features --no-fail-fast --profile quick

# One crate's tests, and the tests whose name holds `filter`: `just t torque-server ghosting`
t crate *filter:
    cargo nextest run -p {{crate}} --all-features --no-fail-fast {{filter}}

# One crate type-checks, tests and benches included
c crate:
    cargo check -p {{crate}} --all-targets --all-features

# One crate's clippy, with warnings as errors
clip crate:
    cargo clippy -p {{crate}} --all-targets --all-features -- -D warnings

# The TrueType sweep under Miri (needs `rustup +nightly component add miri`): fontdue rasterizes with unchecked writes, which
# only Miri or a sanitizer can see go wrong. `TORQUE_FUZZ_TTF` sets the case count.
miri-ttf:
    cargo +nightly miri test -p torque-fonts --lib --ignore-rust-version a_short_hostile_sweep

check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# API reference (rustdoc)
doc:
    cargo doc --workspace --no-deps

# The guide (mdBook), into target/book
book:
    mdbook build book

# The script reference (book/src/reference/script): every console function, method, class and pref of an engine of
# `DefaultPlugins` (and of the opt-in plugins it covers), read from the script engines it makes. Commit what it writes
script-reference:
    cargo run -p torque-script-reference

# Writes the script reference again and fails when that changed a page or added one (CI's check that the pages are current)
script-reference-check: script-reference
    git add --intent-to-add book/src/reference
    git diff --exit-code book/src/reference

# Serve the guide with live reload
book-serve:
    mdbook serve book --open

# Each web example played alone in its own folder, the guide in book/ and the API reference in api/ (needs the wasm32
# target, Node and mdbook); for GitHub Pages (.github/workflows/pages.yml)
# The website into the target folder's `site`: the gallery of the web examples, the guide and the API reference
site:
    node web/site.mjs

# The browser build of the app crate type-checks with its peer-to-peer feature (wasm32-unknown-unknown). A rustc wrapper
# (sccache) is turned off for it, set in the cargo config or in RUSTC_WRAPPER: the wrapper spawns rustc with web-sys's full
# feature list on one command line, which is longer than Windows allows, and cargo's own response-file fallback never runs
wasm-check $RUSTC_WRAPPER="":
    cargo check --config "build.rustc-wrapper=''" -p torque-app --target wasm32-unknown-unknown --features p2p

# Serve the site on http://127.0.0.1:{{port}}/torque-rs/, as GitHub Pages does (build it first with `site`)
site-serve port="8080":
    node web/serve.mjs "{{web_target}}/site" {{port}} /torque-rs/

# Headless Chrome from the cursor guard loads the gallery under /torque-rs/ (no console error, every link HTTP 200), the
# guide's and the API docs' front pages, and each card's game to its world; pictures of the gallery at a desktop's and a
# phone's width go into the target folder's `site-check`
# The site's check (build it first with `site`)
site-check:
    node web/guard-scan.mjs
    npm install --prefix "{{web_target}}/web-tools/node" --no-audit --no-fund --silent --save-exact puppeteer-core@24.43.1
    node web/site-check.mjs "{{web_target}}/site" "{{web_target}}/site-check"

# The web pages' loading display, settings, script bridge, browser host, connection codes and WebTransport join, tested in Node (no browser)
web-unit:
    node --test web/loading.test.mjs web/settings.test.mjs web/bridge.test.mjs web/host.test.mjs web/files.test.mjs web/codes.test.mjs web/invites.test.mjs web/webtransport.test.mjs

# Fails when any script in the repository could open a browser other than through `launchGuarded` (web/guard.mjs), whose pages
# cannot take the pointer lock: a lock confines the real cursor in headless Chrome on Windows
web-guard:
    node web/guard-scan.mjs

# `web-guard`, then the guard's test: a page that asks for the pointer lock on click never gets it. Every check recipe runs it
# first
web-guard-test: web-guard
    npm install --prefix "{{web_target}}/web-tools/node" --no-audit --no-fund --silent --save-exact puppeteer-core@24.43.1
    node web/guard.test.mjs

# The size of each web example's module (`game_bg.wasm` of the level demo, the arena and the vehicle demo, built with their files
# fetched): `just web-size --out /somewhere/outside/the/repo/sizes.json` writes the sizes,
# `just web-size --compare /somewhere/outside/the/repo/sizes.json` gives each one's change in percent
# against that file, and `--limit 1` also fails on more than 1% or on a module the file lacks
web-size *args:
    node web/size.mjs {{args}}

# The invite links' connection alone, with no game and no build, in headless Chrome and headless Firefox (Firefox's
# defaults, mDNS names included; installed where `web/guard.mjs` looks, or `FIREFOX` names it): a small page runs
# `web/invites.js`, the joiner presses Connect, the reply code is pasted 30 s later (a joiner's browser gives up on a host
# it cannot reach yet after a few seconds; a Firefox host's addresses answer only once it has the reply), and a message
# must go each way. One browser with two tabs, two browsers, and each browser hosting the other; then a Firefox invite
# whose candidates are all refused fails, and its connection details are printed and checked. No STUN server is
# contacted. Runs the guard's test in Firefox first
web-invite-link-check: web-guard-test
    node web/guard.test.mjs firefox
    node web/invite-link-check.mjs chrome same 30
    node web/invite-link-check.mjs firefox same 30
    node web/invite-link-check.mjs firefox two 30
    node web/invite-link-check.mjs chrome,firefox two 30
    node web/invite-link-check.mjs firefox,chrome two 30
    node web/invite-link-check.mjs firefox fail 0

# The determinism check: the trace-hash scenarios (the figure on foot and the buggy, each a 600-tick scripted run of a server and a
# predicting client, hashed tick by tick, in each network protocol; and the Rapier world's drop of bodies onto a terrain and an interior) run natively twice and as wasm32-wasip1 under Node's WASI (no browser), and every line
# they print must be equal, and equal to crates/torque-trace-hash/expected.txt. Needs `rustup target add wasm32-wasip1`; `just trace-hash --release` runs the release profile
trace-hash *flags:
    node web/trace-hash.mjs {{flags}}

# The suites that must pass in both network protocols: every example's tests, and the rope, mods and props plugins' suites
net_matrix_packages := "-p torque-example-minimal -p torque-example-minimal-engine -p torque-example-character -p torque-example-level-demo -p torque-example-fps -p torque-example-vehicle-demo -p torque-example-mods-demo -p torque-plugin-rope -p torque-mods -p torque-plugin-props"

# `just test` (the whole workspace, every feature) with the engine's own protocol named, whatever the shell's
# `TORQUE_NET_PROTOCOL` says: the plugin groups' default. Extra flags go to cargo test (`--release`)
test-engine *flags:
    $env:TORQUE_NET_PROTOCOL = "engine"; cargo test -j4 --workspace --all-features --no-fail-fast {{flags}}

# `just test` (the whole workspace, every feature) with Torque's protocol: `TORQUE_NET_PROTOCOL` makes the plugin groups turn
# on Torque's protocol, so a test that follows the protocol runs its Torque half and one that pins the engine's protocol (with
# `set_protocol`) keeps it. A test that fails here only is a stale expectation, a missing pin or a bug of Torque's protocol.
# No package is left out: no test needs a window or a GPU (the examples' pictures are the `*-shots` recipes). `--no-fail-fast`
# lists every failing test suite. Extra flags go to cargo test (`--release`)
test-tge *flags:
    $env:TORQUE_NET_PROTOCOL = "tge"; cargo test -j4 --workspace --all-features --no-fail-fast {{flags}}

# The network matrix: the examples' headless runs and the suites above, once with Torque's protocol and once with the engine's
# (`TORQUE_NET_PROTOCOL` picks the protocol the plugin groups turn on; a game's `[net] protocol` and the tests that pin one
# keep theirs); each run must end with 0 failures. Torque's half also runs the whole workspace (`test-tge`; `just test` is the
# engine's), so a test of any crate that does not follow the protocol shows here. Extra flags go to cargo test (`--release`)
net-matrix *flags:
    $env:TORQUE_NET_PROTOCOL = "tge"; cargo test -j4 {{net_matrix_packages}} --all-features {{flags}}; if ($LASTEXITCODE -ne 0) { exit 1 }; just examples headless; if ($LASTEXITCODE -ne 0) { exit 1 }
    $env:TORQUE_NET_PROTOCOL = "engine"; cargo test -j4 {{net_matrix_packages}} --all-features {{flags}}; if ($LASTEXITCODE -ne 0) { exit 1 }; just examples headless; if ($LASTEXITCODE -ne 0) { exit 1 }
    just test-tge {{flags}}

# Writes the C header of the native extension ABI (crates/torque-plugin-abi/include/torque_plugin.h) with cbindgen
c-header:
    cargo run --manifest-path crates/torque-plugin-abi/header-gen/Cargo.toml

# Fails when the checked-in C header is not what the ABI makes (CI runs the same check)
c-header-check:
    cargo run --manifest-path crates/torque-plugin-abi/header-gen/Cargo.toml -- --check

# The Lua extension (crates/torque-plugin-lua) built on its own as a release library, with the C++ runtime linked in so the
# library alone is what a plugins folder needs; it builds Luau's C++, so it needs a C++ compiler (the engine never does). It goes
# into the workspace's build folder (`release/`), where the examples look for it, not the extension's own workspace's
lua:
    $env:RUSTFLAGS = "-C target-feature=+crt-static"; cargo build --release --manifest-path crates/torque-plugin-lua/Cargo.toml --target-dir "{{web_target}}"

# The Lua extension's tests, clippy, formatting and licenses (its own workspace)
lua-test:
    cargo test --manifest-path crates/torque-plugin-lua/Cargo.toml
    cargo clippy --manifest-path crates/torque-plugin-lua/Cargo.toml --all-targets -- -D warnings
    cargo fmt --manifest-path crates/torque-plugin-lua/Cargo.toml --check
    cargo deny --manifest-path crates/torque-plugin-lua/Cargo.toml check

# The Lua extension as a browser build's WebAssembly module, <target>/lua-web/torque_lua.wasm (web/lua-build.mjs): the same Rust
# and Luau as `lua`, compiled with the WASI SDK's clang. Needs `rustup target add wasm32-wasip1` and a WASI SDK release unpacked
# anywhere, its folder named by WASI_SDK_PATH; a page loads it with web/lua.js (`node web/build.mjs <example> --lua`)
lua-web:
    node web/lua-build.mjs

# The client-script guest, <target>/lua-web/torque_lua_client.wasm (web/lua-build.mjs --client): the Lua extension's crate with
# its `client-script` feature built as the realm's WebAssembly module (crates/torque-client-script-format names its exports).
# Needs what `lua-web` needs
lua-client-web:
    node web/lua-build.mjs --client

# The client-script realm's tests: its escape suite, its fuel, memory and tier cases, natively (the Lua extension's workspace)
lua-client-test:
    cargo test --manifest-path crates/torque-plugin-lua/Cargo.toml --features client-script --test client_script -- --nocapture

# The Lua tests on the web: the cases of `lua-test` run natively, then built for a browser with the page's Lua module and run
# under Node and in headless Chrome from the cursor guard (web/lua-check.mjs). Needs what `lua-web` needs and Chrome
lua-web-check:
    node web/guard-scan.mjs
    node --test web/lua.test.mjs
    cargo test --manifest-path crates/torque-plugin-lua/Cargo.toml --test lua
    node web/lua-build.mjs --check
    node "{{web_target}}/lua-web/check/lua-node.mjs"
    npm install --prefix "{{web_target}}/web-tools/node" --no-audit --no-fund --silent --save-exact puppeteer-core@24.43.1
    node web/lua-check.mjs "{{web_target}}/lua-web/check"

# The client-script host's tests (crates/torque-client-script-host): its own, then every case of `cases` under Wasmtime with
# the guest `lua-client-web` builds (the deadline cases, memory traps, the strike rule, grants and revocation,
# teardown, the frame budget), clippy and formatting. Needs what `lua-web` needs
client-script-host-test:
    node web/lua-build.mjs --client
    $env:TORQUE_CLIENT_GUEST = "{{web_target}}/lua-web/torque_lua_client.wasm"; $env:TORQUE_CLIENT_GUEST_REQUIRED = "1"; cargo test -p torque-client-script-host --features wasmtime,cases -- --nocapture
    cargo clippy -p torque-client-script-host --features wasmtime,cases --all-targets -- -D warnings
    cargo clippy -p torque-client-script-host --all-targets -- -D warnings
    cargo fmt -p torque-client-script-host -p torque-test-client-script-web-check --check

# The client-script glue's tests (crates/torque-plugin-client-script: consent, messages), the choices-file and content rules in
# torque-mods, the format's, the GUI's prompt and the layout plugin's realm containers, with clippy and formatting. No guest needed
client-script-test:
    cargo test -p torque-client-script-format
    cargo test -p torque-plugin-client-script
    cargo test -p torque-mods client_script
    cargo test -p torque-gui client_script
    cargo test -p torque-test-app --test gui_layout_realm --test client_script_end_to_end
    cargo clippy -p torque-client-script-format -p torque-plugin-client-script -p torque-mods -p torque-gui -p torque-plugin-gui-layout -p torque-test-app --all-targets -- -D warnings
    cargo fmt -p torque-client-script-format -p torque-plugin-client-script -p torque-mods -p torque-gui -p torque-plugin-gui-layout -p torque-test-app --check

# The SHA-256 of the realm guest `lua-client-web` built, and the line a release adds to ACCEPTED_CLIENT_GUESTS (the engine runs
# only guests it lists)
client-script-guest-hash:
    cargo run -p torque-client-script-host --example guest-hash -- "{{web_target}}/lua-web/torque_lua_client.wasm"

# The client-script host's cases on the web: built for a browser (tests/web/client-script-check), each realm's guest
# in a Worker of web/client-script.js, run under Node and in headless Chrome from the cursor guard
# (web/client-script-check.mjs). Needs what `lua-web` needs and Chrome
client-script-web-check:
    node web/guard-scan.mjs
    node --test web/client-script.test.mjs
    node web/lua-build.mjs --client --check
    node "{{web_target}}/lua-web/client-check/client-script-node.mjs"
    npm install --prefix "{{web_target}}/web-tools/node" --no-audit --no-fund --silent --save-exact puppeteer-core@24.43.1
    node web/client-script-check.mjs "{{web_target}}/lua-web/client-check"

# The guest `lua-client-web` built must be one this build accepts (ACCEPTED_CLIENT_GUESTS); prints its hash
client-script-guest-check: lua-client-web
    cargo run -p torque-client-script-host --example guest-hash -- --check "{{web_target}}/lua-web/torque_lua_client.wasm"

# The client-script realm's fuzz and authority suites at their full counts (message decoder 1,000,000 cases, Lua-to-arguments
# converter 300,000, rich text 300,000, 100,000 spoofing frames; the format's props reader 300,000, the guest's layout-prop converter
# 300,000, 60,000 random container ops against the layout plugin): the consent, message, authority and end-to-end tests and the
# GUI's, run natively with TORQUE_CLIENT_SCRIPT_FUZZ set. No guest needed
client-script-fuzz:
    $env:TORQUE_CLIENT_SCRIPT_FUZZ = "1"; cargo test -p torque-plugin-client-script -- --nocapture
    $env:TORQUE_CLIENT_SCRIPT_FUZZ = "1"; cargo test -p torque-gui client_script -- --nocapture
    $env:TORQUE_CLIENT_SCRIPT_FUZZ = "1"; cargo test -p torque-client-script-format -- --nocapture
    $env:TORQUE_CLIENT_SCRIPT_FUZZ = "1"; cargo test -p torque-test-app --test gui_layout_realm --test client_script_end_to_end -- --nocapture
    $env:TORQUE_CLIENT_SCRIPT_FUZZ = "1"; cargo test --manifest-path crates/torque-plugin-lua/Cargo.toml --features client-script --test client_script hostile_lua -- --nocapture

# The time a realm costs a frame (crates/torque-plugin-client-script/tests/bench.rs): the real guest under Wasmtime in a client's
# GUI, counting the canvas's drawing of the realm's controls, with a font and without one: a hostile realm at its caps within 3 ms of
# the scene alone at p99, an idle realm with 1,024 controls within 0.3 ms at p50. `tiers` are the tiers granted (`ui`, `net`). A timing run: run it with nothing else loading the machine. Needs what `lua-web` needs
client-script-bench tiers="ui,net":
    node web/lua-build.mjs --client
    $env:TORQUE_CLIENT_SCRIPT_BENCH_TIERS = "{{tiers}}"; $env:TORQUE_CLIENT_GUEST = "{{web_target}}/lua-web/torque_lua_client.wasm"; $env:TORQUE_CLIENT_GUEST_REQUIRED = "1"; cargo test --profile client-script-check -p torque-plugin-client-script --test bench -- --ignored --nocapture

# The client-script realm's hardening suite, one command: the guest built and its hash listed, the escape, fuel and memory suites
# natively and under Wasmtime (whose cases also hold every case's parity number), the fuzz and authority suites at full counts,
# the same cases under Node and in headless Chrome (the same parity numbers), and the bench for `ui` and `net`. Needs what `lua-web`
# needs and Chrome; takes a while, and the bench is a timing run (see `client-script-bench`)
client-script-check: client-script-guest-check lua-client-test client-script-host-test client-script-test client-script-fuzz client-script-web-check (client-script-bench "ui,net")

# The Wasmtime gate for client scripts (crates/torque-client-script-host): the Lua module of `lua-web` run natively under
# Wasmtime against the Lua extension's shared cases, an epoch-interruption check, and the costs (compile, instantiate, host
# call, Luau step, release binary size; `cold` adds the cold build time, built from nothing). Needs what `lua-web` needs
realm-gate cold="":
    node web/lua-build.mjs
    cargo run --release --jobs 4 -p torque-client-script-host --features wasmtime --example realm-gate -- "{{web_target}}/lua-web/torque_lua.wasm" {{ if cold == "cold" { "--cold" } else { "" } }}

# The server crate's dependency tree has no wgpu, winit, naga, cpal, drawing, GUI, audio or client crates (CI runs the same check)
server-deps:
    $tree = cargo tree -p torque-app-server --all-features --prefix none; if ($LASTEXITCODE -ne 0) { exit 1 }; $bad = $tree | ForEach-Object { ($_ -split ' ')[0] } | Select-String -Pattern '^(wgpu|winit|naga|cpal|raw-window-handle|torque-(render|gui|audio|client|fonts))'; if ($bad) { $bad; exit 1 }

# Licenses, advisories, bans and sources of the dependency tree (cargo-deny)
deny:
    cargo deny check

# Packages a game crate (a folder with a bundle.toml, such as examples/mods-demo) for this computer: its binaries built in
# release with the plugins its game.toml keeps, its game folder, its extensions, the license texts of everything it links and a
# manifest of hashes, scanned for local paths, then its headless check run from the bundle folder (`--out <dir>`, `--no-check`;
# `--server` for the dedicated-server variant, which must link no renderer, window or audio crate and import no GPU, window or
# audio system library (the `server-deps` recipe is the crate check for torque-app-server); `--archive` also writes a zip (Windows)
# or a gzipped tar; `--twice` builds two archives from clean folders and compares their SHA-256, to check that a game builds
# reproducibly;
# `just bundle --verify <bundle folder>` checks a bundle against its manifest; every file of the game folder is opened by its
# loader first and a refused one fails the bundle (`--no-validate` skips that; `just bundle --validate <game crate>` does only
# that, with nothing built); `--precompile` ships the `.cs` and `.gui` scripts compiled in place of their source, in TGE 1.4's format with `--classic-scripts`)
bundle *args:
    cargo run -p torque-bundle --release -- {{args}}

# The checks on a game's assets and compiled scripts: the mods demo's game folder opens with nothing refused, a planted bad
# asset is reported, and the trace world run from a compiled script hashes as it does from its source
bundle-content-check:
    cargo test -p torque-bundle --test content
    cargo test -p torque-trace-hash --test precompiled

# Builds everything with the oldest supported Rust (install that toolchain with rustup first)
msrv:
    cargo +{{msrv}} check --workspace --all-targets --all-features

# The interior builder's verification at its full counts on 20 generated brush maps, then on 20 generated glTF and OBJ meshes
# (closed, open, authored hulls, volumes, a 200,344-triangle terrain cut into pieces): 100,000 surface points, 1,000,000
# segments and 1,000,000 BSP points each, read back through the importer and the collision; every one 0 mismatches
dif-verify:
    cargo run --release -p torque-interior-builder --features verify --example dif-verify
    cargo run --release -p torque-interior-builder --features verify --example dif-mesh-verify
    $env:DIF_VERIFY_SHARE = "1"; cargo test --release -p torque-example-assets the_arena_interior -- --nocapture

# The glTF import's pass condition: 20 generated glTF files read back as `.dts` (equal node, mesh, vertex and sequence
# counts; vertices within 1e-4; poses within 1e-3 at rest and 10 times a sequence), the kit's shapes through `.glb` giving the
# same `.dts` bytes, and 300,000 fuzz cases of the reader with 0 panics. The arena's shots and fps tests with the `.glb`
# shapes: `ARENA_SHAPES=glb` before `just examples fps shots` and `cargo nextest run -p torque-example-fps`.
gltf-verify:
    cargo test -p torque-import --test fixtures -- --nocapture
    cargo test -p torque-example-assets every_shape_through_a_glb_makes_the_same_dts
    $env:TORQUE_IMPORT_FUZZ_CASES = "300000"; cargo test --release -p torque-import --lib the_reader_never_panics -- --nocapture

# What CI checks, in one go (rustdoc with warnings as errors is part of it)
ci: check test deny book
    just examples headless
    cargo doc --workspace --no-deps --all-features --config "build.rustdocflags=['-D', 'warnings']"

# The script engine's benches: the VM's workloads with the Torque3D executor and with the classic one (calls, loops,
# strings, objects, numbers to text, fields, host calls, schedule, eval, exec, compile), then calls across the native
# extension ABI. Timing runs take the bench lock (see the agents' rules). `just bench-script mimalloc` runs the VM
# bench with mimalloc as the allocator (crates/torque-ts-vm/bench-alloc, a build of its own with a C compiler: a
# trial, never part of the workspace)
bench-script allocator="":
    cargo run --release --jobs 4 {{ if allocator == "mimalloc" { "--manifest-path crates/torque-ts-vm/bench-alloc/Cargo.toml" } else { "-p torque-ts-vm --example bench" } }}
    cargo run --release --jobs 4 {{ if allocator == "mimalloc" { "--manifest-path crates/torque-ts-vm/bench-alloc/Cargo.toml" } else { "-p torque-ts-vm --example bench" } }} -- --classic
    cargo bench --jobs 4 -p torque-plugin-loader --bench native_call

# The particles' cost a frame through the GPU particle pool (`--cpu`: the CPU path's quads instead) at `--count` particles alive,
# `--quality` and `--size` (100,000 at High and 1920x1080 by default): the pool's pass, the draws' GPU time, the scene side's CPU
# and the draw's `begin: particles` mark. A timing run: take the bench lock (see the agents' rules)
particles-profile *args:
    cargo run --release --jobs 4 -p torque-render-hd --example particles-profile -- {{args}}

# The VM bench with the system allocator and then with mimalloc, one after the other, in the `dist` profile, to see what
# mimalloc gives (adopt it in the examples' and the benches' binaries at 20% or better on the strings or objects workloads).
# Takes the bench lock
bench-mimalloc:
    cargo run --profile dist --jobs 4 -p torque-ts-vm --example bench
    cargo run --profile dist --jobs 4 --manifest-path crates/torque-ts-vm/bench-alloc/Cargo.toml

# The hostile-mod fuzz at its full 100,000 cases (the default of `just test` is 2,000)
fuzz-mods:
    $env:TORQUE_FUZZ_MODS = "100000"; cargo nextest run -p torque-mods --test fuzz --release

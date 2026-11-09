set windows-shell := ["powershell", "-NoProfile", "-Command"]

# Kept in step with `rust-version` in Cargo.toml and MSRV in .github/workflows/ci.yml.
msrv := "1.88"

# The build folder: CARGO_TARGET_DIR's, else the workspace's (the site goes into its `site` folder)
web_target := env_var_or_default("CARGO_TARGET_DIR", justfile_directory() / "target")

default:
    @just --list

build:
    cargo build --workspace

test:
    cargo test --workspace --all-features

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

# The determinism check: the trace-hash scenarios (the figure on foot and the buggy, each a 600-tick scripted run of a server and a
# predicting client, hashed tick by tick, in each network protocol; and the Rapier world's drop of bodies onto a terrain and an interior) run natively twice and as wasm32-wasip1 under Node's WASI (no browser), and every line
# they print must be equal, and equal to crates/torque-trace-hash/expected.txt. Needs `rustup target add wasm32-wasip1`; `just trace-hash --release` runs the release profile
trace-hash *flags:
    node web/trace-hash.mjs {{flags}}

# The suites that must pass in both network protocols: every example's tests, and the rope, mods and props plugins' suites
net_matrix_packages := "-p torque-example-minimal -p torque-example-minimal-engine -p torque-example-character -p torque-example-level-demo -p torque-example-fps -p torque-example-vehicle-demo -p torque-example-mods-demo -p torque-plugin-rope -p torque-mods -p torque-plugin-props"

# The network matrix: the examples' headless runs and the suites above, once with Torque's protocol and once with the engine's
# (`TORQUE_NET_PROTOCOL` picks the protocol the plugin groups turn on; a game's `[net] protocol` and the tests that pin one
# keep theirs); each run must end with 0 failures. Extra flags go to cargo test (`--release`)
net-matrix *flags:
    $env:TORQUE_NET_PROTOCOL = "tge"; cargo test -j4 {{net_matrix_packages}} {{flags}}; if ($LASTEXITCODE -ne 0) { exit 1 }; just examples/headless; if ($LASTEXITCODE -ne 0) { exit 1 }
    $env:TORQUE_NET_PROTOCOL = "engine"; cargo test -j4 {{net_matrix_packages}} {{flags}}; if ($LASTEXITCODE -ne 0) { exit 1 }; just examples/headless; if ($LASTEXITCODE -ne 0) { exit 1 }

# What `shots-diff` accepts as the same: renders are not bit-stable from run to run, so a picture whose differing pixels are off by
# at most one level in a channel and number at most 32 passes as "near" (listed apart from the exact matches); anything more fails
shots_tolerance := "--max-delta 1 --max-pixels 32"

# Every example's offscreen pictures, each recipe's into its own folder below `dir` (the character, level demo, arena and vehicle
# demo shots, the arena's name and menu shots, the HUD shots and `showcase --examples`), to compare with `shots-diff` after a
# change. Not drawn: the clips (they need ffmpeg), the race track pictures and `showcase`'s front-page grid. Run it on the same
# machine and GPU as the folder it is compared with
shots-all dir="target/shots":
    just examples/character-shots "{{absolute_path(dir)}}/character"
    just examples/level-demo-shots "{{absolute_path(dir)}}/level-demo"
    just examples/level-demo-valley-shots "{{absolute_path(dir)}}/level-demo-valley"
    just examples/level-demo-swim-shots "{{absolute_path(dir)}}/level-demo-swim"
    just examples/level-demo-switch-shots "{{absolute_path(dir)}}/level-demo-switch"
    just examples/fps-shots "{{absolute_path(dir)}}/fps"
    just examples/fps-menu-shots "{{absolute_path(dir)}}/fps-menu"
    just examples/fps-name-shots "{{absolute_path(dir)}}/fps-name"
    just examples/fps-motion-shots "{{absolute_path(dir)}}/fps-motion"
    just examples/fps-polish-shots "{{absolute_path(dir)}}/fps-polish"
    just examples/fps-polish2-shots "{{absolute_path(dir)}}/fps-polish2"
    just examples/fps-hitbox-shots "{{absolute_path(dir)}}/fps-hitbox"
    just examples/vehicle-demo-shots "{{absolute_path(dir)}}/vehicle-demo"
    just examples/vehicle-demo-name-shots "{{absolute_path(dir)}}/vehicle-demo-name"
    just examples/vehicle-demo-switch-shots "{{absolute_path(dir)}}/vehicle-demo-switch"
    just examples/hud-shots "{{absolute_path(dir)}}/hud"
    just examples/showcase "{{absolute_path(dir)}}/showcase" --examples

# Compares two folders drawn by `shots-all`: per picture whether the sizes match, the differing pixels and the largest channel
# difference; the exit code is 1 when any picture differs. Left out: the top right corner of every picture (160x40 px), where the
# corner readout shows the frame rate measured on the clock, which differs from run to run. Pictures off by one level on at most 32
# pixels (`shots_tolerance`) pass as near and are listed apart from the exact matches. Works on the folder of a single example too
shots-diff a b:
    cargo run -p torque-example-showcase --release --bin shots-diff -- --ignore-corner 160x40 {{shots_tolerance}} "{{absolute_path(a)}}" "{{absolute_path(b)}}"

# `shots-diff` on one example's folder: `example` is the folder name below the `shots-all` dir (for instance `vehicle-demo`), `a`
# is the dir a run drew into and `b` the baseline dir
shots-diff-one example a b:
    just shots-diff "{{a}}/{{example}}" "{{b}}/{{example}}"

# Writes the C header of the native extension ABI (crates/torque-plugin-abi/include/torque_plugin.h) with cbindgen
c-header:
    cargo run --manifest-path crates/torque-plugin-abi/header-gen/Cargo.toml

# Fails when the checked-in C header is not what the ABI makes (CI runs the same check)
c-header-check:
    cargo run --manifest-path crates/torque-plugin-abi/header-gen/Cargo.toml -- --check

# The sample native extension (examples/native-extension), built on its own as a library
native-ext-sample:
    cargo build --manifest-path examples/native-extension/Cargo.toml

# The Lua extension (crates/torque-plugin-lua) built on its own as a release library, with the C++ runtime linked in so the
# library alone is what a plugins folder needs; it builds Luau's C++, so it needs a C++ compiler (the engine never does)
lua:
    $env:RUSTFLAGS = "-C target-feature=+crt-static"; cargo build --release --manifest-path crates/torque-plugin-lua/Cargo.toml

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

# The Wasmtime gate for client scripts (crates/torque-client-script-host): the Lua module of `lua-web` run natively under
# Wasmtime against the Lua extension's shared cases, an epoch-interruption check, and the costs (compile, instantiate, host
# call, Luau step, release binary size; `cold` adds the cold build time, built from nothing). Needs what `lua-web` needs
realm-gate cold="":
    node web/lua-build.mjs
    cargo run --release --jobs 4 -p torque-client-script-host --features wasmtime --example realm-gate -- "{{web_target}}/lua-web/torque_lua.wasm" {{ if cold == "cold" { "--cold" } else { "" } }}

# The sample extension's picture compiled in (`<dir>/compiled`) and loaded as a library (`<dir>/loaded`), then shots-diff
native-ext-shots $TORQUE_NATIVE_EXT_SHOTS="target/native-ext-shots":
    cargo test -p torque-plugin-loader --features render --test pictures
    just shots-diff "{{TORQUE_NATIVE_EXT_SHOTS}}/compiled" "{{TORQUE_NATIVE_EXT_SHOTS}}/loaded"

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
# `just bundle --verify <bundle folder>` checks a bundle against its manifest)
bundle *args:
    cargo run -p torque-bundle --release -- {{args}}

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
# shapes: `ARENA_SHAPES=glb` before `just examples/fps-shots` and `cargo nextest run -p torque-example-fps`.
gltf-verify:
    cargo test -p torque-import --test fixtures -- --nocapture
    cargo test -p torque-example-assets every_shape_through_a_glb_makes_the_same_dts
    $env:TORQUE_IMPORT_FUZZ_CASES = "300000"; cargo test --release -p torque-import --lib the_reader_never_panics -- --nocapture

# What CI checks, in one go (rustdoc with warnings as errors is part of it)
ci: check test deny book
    just examples/headless
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

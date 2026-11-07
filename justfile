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

check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# API reference (rustdoc)
doc:
    cargo doc --workspace --no-deps

# The guide (mdBook), into target/book
book:
    mdbook build book

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

# Pictures that differ between two runs of the same build, as `--skip` switches for `shots-diff`:
# vehicle-demo/hill.png: 2 pixels off by one level among the hills (rounding noise from run to run)
shots_flapping := "--skip vehicle-demo/hill.png"

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
# difference; the exit code is 1 when any picture differs. Left out, by name: the top right corner of every picture (160x40 px),
# where the corner readout shows the frame rate measured on the clock, which differs from run to run; and the pictures in
# `shots_flapping`, each with its reason
shots-diff a b:
    cargo run -p torque-example-showcase --release --bin shots-diff -- --ignore-corner 160x40 {{shots_flapping}} "{{absolute_path(a)}}" "{{absolute_path(b)}}"

# Licenses, advisories, bans and sources of the dependency tree (cargo-deny)
deny:
    cargo deny check

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

set windows-shell := ["powershell", "-NoProfile", "-Command"]

# Kept in step with `rust-version` in Cargo.toml and MSRV in .github/workflows/ci.yml.
msrv := "1.88"

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

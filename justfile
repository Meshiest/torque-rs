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

# The interior builder's verification at its full counts on 20 generated brush maps: 100,000 surface points, 1,000,000
# segments and 1,000,000 BSP points each, read back through the importer and the collision; every map 0 mismatches
dif-verify:
    cargo run --release -p torque-interior-builder --features verify --example dif-verify

# What CI checks, in one go (rustdoc with warnings as errors is part of it)
ci: check test deny book
    just examples/headless
    cargo doc --workspace --no-deps --all-features --config "build.rustdocflags=['-D', 'warnings']"

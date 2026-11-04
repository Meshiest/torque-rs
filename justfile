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

# The headless examples of the umbrella crate
examples:
    cargo run -p torque --example script
    cargo run -p torque --example shape
    cargo run -p torque --example collide
    cargo run -p torque --example net
    cargo run -p torque-example --bin example-server -- --port 0 --ticks 64

# The example dedicated server: the bundled game on 127.0.0.1:28000 (`--ticks N` stops it after N ticks)
example-server *args:
    cargo run -p torque-example --bin example-server -- {{args}}

# The example client: connects to the example server and prints the ghosts it is sent
example-client *args:
    cargo run -p torque-example --bin example-client -- {{args}}

# The example games' asset kit (shapes and textures made from code), written into `dir`
example-assets dir="target/example-assets":
    cargo run -p torque-example-assets -- {{dir}}

# Licences, advisories, bans and sources of the dependency tree (cargo-deny)
deny:
    cargo deny check

# Builds everything with the oldest supported Rust (install that toolchain with rustup first)
msrv:
    cargo +{{msrv}} check --workspace --all-targets --all-features

# What CI checks, in one go (rustdoc with warnings as errors is part of it)
ci: check test examples deny book
    cargo doc --workspace --no-deps --all-features --config "build.rustdocflags=['-D', 'warnings']"

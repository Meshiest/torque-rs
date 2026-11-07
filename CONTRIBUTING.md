# Contributing to torque-rs

Thank you for helping. This file says how to build the project, what a change must pass, and the rules the code
follows.

## Building

You need Rust (see `rust-version` in [Cargo.toml](Cargo.toml) for the oldest supported release),
[just](https://github.com/casey/just), and for the guide [mdBook](https://rust-lang.github.io/mdBook/). The dependency
check uses [cargo-deny](https://github.com/EmbarkStudios/cargo-deny). On Linux the `torque-audio/device` feature needs
the ALSA headers (`libasound2-dev` on Debian and Ubuntu).

```
just build      # build the workspace
just test       # run the tests (all features)
just check      # rustfmt and clippy, warnings as errors
just doc        # API reference
just book       # the guide, into target/book
just examples/headless   # the headless examples and a short run of the minimal server
just deny       # licences, advisories and sources of the dependencies
just msrv       # build with the oldest supported Rust
just ci         # everything CI checks
```

## What a change must pass

CI (`.github/workflows/ci.yml`) runs on Linux and Windows and fails on any of:

- `cargo fmt --all --check`;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`, with the lints in `[workspace.lints]`;
- `cargo test --workspace --all-features` and the examples;
- `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings` (no broken or private intra-doc links);
- the mdBook build;
- `cargo check` with the MSRV;
- `cargo deny check`.

The `natives` tests compare the natives each example's and the app layer's script engines have with the snapshot files under
`tests/natives/`. A change that adds, removes or alters a native fails them; `TORQUE_BLESS=1 cargo test --workspace
--all-features --test natives` rewrites the snapshot files, and the diff is part of the change.

Run `just ci` before opening a pull request. Keep a pull request to one topic, and add a line to the `Unreleased`
section of [CHANGELOG.md](CHANGELOG.md) when the change is visible to users of the crates.

## Code rules

- **No `unsafe`.** The workspace forbids it. The one exception is torque-render-core's pipeline cache, whose single
  `unsafe` block (wgpu's `Device::create_pipeline_cache`) has a `SAFETY` comment saying why it is sound; that crate denies
  `unsafe_code` instead of forbidding it, and expects it (`#[expect(unsafe_code)]`) on that one function only.
- **Hostile input never panics.** Files (scripts, compiled scripts, shapes, maps, images, fonts, sounds) and network
  packets come from places the program does not control. A decoder returns a value or an error; every count read from
  input is checked against the bytes that remain and a named cap before anything is allocated for it; work that grows
  with the input is bounded or charged to a budget. Each crate's top-level documentation states its rules; follow them,
  and add a fuzz case or a test for every new reader.
- **Games extend the engine through its registries and hooks**, not by forking engine code. A behaviour a game may
  want to change is a hook with the default behaviour built in.
- **Tools are build-time features.** A shipping build must compile without them.
- **New crates** join the workspace with `[lints] workspace = true`, inherit `[workspace.package]`, and give their own
  `description`, `keywords` and `categories`. Crates that need a GPU go behind a feature of the `torque` umbrella crate
  that is off by default. Their tests that need a GPU adapter return early when none is found (CI gives Linux a
  software Vulkan driver and Windows has WARP, so they do run there).
- **Documentation**: every public item says what it is for. Module documentation says what is in the module and what
  it guarantees.

## Licence and credits

torque-rs is MIT-licensed ([LICENSE](LICENSE)). By contributing you agree that your contribution is licensed under the
same terms, and you confirm you have the right to contribute it.

Some formats and behaviours follow Torque3D's MIT-licensed source. When code follows a Torque3D file, name the file
and its copyright line in the module documentation, and make sure the copyright holder is listed in [NOTICE](NOTICE).
Do not contribute code taken from sources whose licence does not allow it.

## Reporting security issues

Do not open a public issue for a vulnerability; see [SECURITY.md](SECURITY.md).

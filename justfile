set default-list
set ignore-comments

set shell := ["bash", "-euo", "pipefail", "-c"]

wasm_target := "wasm32-unknown-unknown"

# Run all CI jobs (except msrv, which needs its own toolchain: `devenv --profile msrv shell -- just msrv`).
ci: fmt clippy test test-wasm features doc deny package

# Check formatting.
fmt:
    cargo fmt --all --check

# Run clippy natively (default, no default and all features) and for wasm (default and all features; needs the wasm C toolchain from devenv).
clippy:
    cargo clippy --workspace --all-targets --locked -- --deny warnings
    cargo clippy -p imgpipe --no-default-features --locked -- --deny warnings
    cargo clippy --workspace --all-targets --all-features --locked -- --deny warnings
    cargo clippy -p imgpipe --target {{ wasm_target }} --locked -- --deny warnings
    cargo clippy -p imgpipe --target {{ wasm_target }} --all-features --locked -- --deny warnings

# Run the tests natively (default features, overlay, all features).
test:
    cargo test --workspace --locked
    cargo test -p imgpipe --features overlay --locked
    cargo test --workspace --all-features --locked

# Run imgpipe's integration tests (goldens, steps, codecs) in wasm under Node.
test-wasm:
    cargo test -p imgpipe --target {{ wasm_target }} --features overlay --test golden --test steps --test codecs --test overlay --test adversarial --locked
    cargo test -p imgpipe --target {{ wasm_target }} --features webp-libwebp,png-optimize,wasm-libc-shim --test codecs --locked

# cargo-hack rewrites manifests while it runs: don't run other cargo commands at the same time.
# No --locked: --no-dev-deps drops the dev-dependencies, which changes the lock file while it runs.
# Check every feature on its own, natively and for wasm.
features:
    cargo hack check -p imgpipe --each-feature --no-dev-deps
    cargo hack check -p imgpipe --each-feature --no-dev-deps --target {{ wasm_target }}

# Build the docs as docs.rs would (all features, warnings are errors).
[env("RUSTDOCFLAGS", "--deny warnings")]
doc:
    cargo doc --workspace --no-deps --all-features --locked

# Check dependencies (advisories, licences, sources).
deny:
    cargo deny --locked check

# `cargo publish` without uploading: catches files missing from the packages
# and dev-dependencies that `cargo publish` strips.
[doc("Build both crates from their packaged sources.")]
package:
    cargo publish --dry-run --workspace --locked

# Check with the minimum supported Rust version (run it in the msrv profile: `devenv --profile msrv shell -- just msrv`).
msrv:
    cargo check --workspace --all-features --all-targets --locked

# Regenerate imgpipe's golden images (natively); review the diff before committing.
update-goldens:
    UPDATE_GOLDENS=1 cargo test -p imgpipe --features overlay --test golden --test overlay

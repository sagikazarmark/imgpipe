# imgpipe

[![ci](https://img.shields.io/github/actions/workflow/status/sagikazarmark/imgpipe/ci.yaml?style=flat-square&label=ci)](https://github.com/sagikazarmark/imgpipe/actions/workflows/ci.yaml)
[![openssf scorecard](https://api.securityscorecards.dev/projects/github.com/sagikazarmark/imgpipe/badge?style=flat-square&label=openssf%20scorecard)](https://securityscorecards.dev/viewer/?uri=github.com/sagikazarmark/imgpipe)

A linear image pipeline for native targets and `wasm32-unknown-unknown`:
decode untrusted bytes, run an ordered list of serializable steps (resize,
crop, pad, trim, rotate, flip, adjust, sharpen, text), encode one or more
outputs.

## Crates

| Crate | Description | |
| ----- | ----------- | - |
| [`imgpipe`](crates/imgpipe) | The library: decoding, steps, text overlay, encoders, and the WebAssembly notes | [![crates.io](https://img.shields.io/crates/v/imgpipe?style=flat-square)](https://crates.io/crates/imgpipe) [![docs.rs](https://img.shields.io/docsrs/imgpipe?style=flat-square)](https://docs.rs/imgpipe) |
| [`imgpipe-cli`](crates/imgpipe-cli) | The `imgpipe` command-line tool: run, validate and explain pipeline files | [![crates.io](https://img.shields.io/crates/v/imgpipe-cli?style=flat-square)](https://crates.io/crates/imgpipe-cli) |

## Development

The development environment is managed by [devenv](https://devenv.sh): it provides the Rust toolchain with the
wasm target, the wasm C toolchain for the C codecs, `wasm-bindgen-test-runner` and Node for the wasm tests,
the cargo tools CI uses and [just](https://just.systems).

Each CI job is a [just](justfile) recipe. List them with `just`, and run them all with:

```shell
just ci
```

The MSRV job runs `just msrv` in the `msrv` devenv profile, which replaces the stable toolchain with the
`rust-version` in `Cargo.toml`:

```shell
devenv --profile msrv shell -- just msrv
```

See the individual crate READMEs for crate-specific tests (e.g. the golden images of `imgpipe`).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

# imgpipe-cli

[![ci](https://img.shields.io/github/actions/workflow/status/sagikazarmark/imgpipe/ci.yaml?style=flat-square&label=ci)](https://github.com/sagikazarmark/imgpipe/actions/workflows/ci.yaml)
[![openssf scorecard](https://api.securityscorecards.dev/projects/github.com/sagikazarmark/imgpipe/badge?style=flat-square&label=openssf%20scorecard)](https://securityscorecards.dev/viewer/?uri=github.com/sagikazarmark/imgpipe)
[![crates.io](https://img.shields.io/crates/v/imgpipe-cli?style=flat-square)](https://crates.io/crates/imgpipe-cli)

`imgpipe`, a command-line tool that runs, validates and explains
[imgpipe](https://crates.io/crates/imgpipe) pipeline files: decode images,
run an ordered list of steps (resize, crop, pad, trim, rotate, flip, adjust,
sharpen, text), encode one or more outputs.

## Installation

Requires Rust **1.89 or later** and a C compiler (see below).

```shell
cargo install imgpipe-cli
```

### Features and licences

The binary is built with imgpipe's `png`, `jpeg`, `webp`, `gif`, `overlay`,
`schemars` and `png-optimize` features. `png-optimize` compiles libdeflate
(C), so installing needs a C compiler. The JPEG encoder is based in part on
the work of the Independent JPEG Group.

## Usage

```shell
imgpipe run -c pipeline.toml in.jpg -o out.webp            # one output, format from the extension
imgpipe run -c pipeline.toml 'raw/*.jpg' --out-dir dist/   # every [[outputs]] entry
imgpipe run -c pipeline.toml in.jpg -o out.png --font anton=Anton-Regular.ttf --var caption=hi
imgpipe validate -c pipeline.toml --input 4000x3000        # parameters, encoders, memory
imgpipe explain -c pipeline.toml                           # resolved steps and sizes
imgpipe schema > pipeline.schema.json                      # JSON Schema for editor completion
imgpipe --help
```

A pipeline file (TOML, or JSON by extension):

```toml
version = 1

[[steps]]
op = "resize"
max_long_side = 1600

[[steps]]
op = "sharpen"
amount = 0.3

[[outputs]]
name = "{stem}/full.{hash8}.webp"   # {stem} {ext} {w} {h} {hash8}
format = "webp"
lossless = true

[[outputs]]
name = "{stem}/thumb-{w}x{h}.png"
format = "png"
tail = [{ op = "resize", max_long_side = 320 }]
```

`run` processes inputs in parallel (`--jobs`), prints one line per output (or
JSON with `--format json`) and exits non-zero if any input failed. With `-o`,
the first `[[outputs]]` entry of the output's format, if any, supplies its
extra steps and encoder settings.

See the [imgpipe README](https://crates.io/crates/imgpipe) for the steps,
their parameters and defaults.

## Development

See the [repository README](https://github.com/sagikazarmark/imgpipe#development) for the shared checks.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

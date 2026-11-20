# imgpipe

[![ci](https://img.shields.io/github/actions/workflow/status/sagikazarmark/imgpipe/ci.yaml?style=flat-square&label=ci)](https://github.com/sagikazarmark/imgpipe/actions/workflows/ci.yaml)
[![openssf scorecard](https://api.securityscorecards.dev/projects/github.com/sagikazarmark/imgpipe/badge?style=flat-square&label=openssf%20scorecard)](https://securityscorecards.dev/viewer/?uri=github.com/sagikazarmark/imgpipe)
[![crates.io](https://img.shields.io/crates/v/imgpipe?style=flat-square)](https://crates.io/crates/imgpipe)
[![docs.rs](https://img.shields.io/docsrs/imgpipe?style=flat-square)](https://docs.rs/imgpipe)

A linear image pipeline for native targets and `wasm32-unknown-unknown`:
decode untrusted bytes, run an ordered list of serializable steps (resize,
crop, pad, trim, rotate, flip, adjust, sharpen, text), encode one or more
outputs. The core does no I/O; callers bring bytes, fonts and a place to put
the results. The default features are pure Rust, but the default `jpeg`
feature carries an attribution duty: programs that ship it must say "this
software is based in part on the work of the Independent JPEG Group" in
their documentation (see [Features](#features)).

- Pixels are 8-bit sRGB with straight alpha at rest; filtering steps work in
  16-bit linear light with premultiplied alpha.
- A pipeline is validated before it runs: parameters, predicted sizes and peak
  memory against a budget.
- Pipelines are plain data (TOML or JSON), with an optional JSON Schema.
- Running is synchronous and single-threaded: no async runtime, no threads,
  the same on a server and in a browser worker.
- The text overlay (HarfRust shaping, UAX #14 line breaking, tiny-skia drawing)
  lays out in a resolution-independent space, so a preview and a full-size
  export break lines identically.

Version 0.1 (unreleased). Until 1.0, breaking changes bump the minor version
(0.1 → 0.2). See the crate docs ("Stability") for which types are
`#[non_exhaustive]` and which are plain data.

## Installation

Requires Rust **1.89 or later**.

```toml
[dependencies]
imgpipe = "0.1"
```

To draw text, enable `overlay`. Pipeline files are plain Serde data: the examples
below read them with [`toml`](https://docs.rs/toml).

```toml
[dependencies]
imgpipe = { version = "0.1", features = ["overlay"] }
toml = "1"
```

### Features

| Feature | Default | What it adds | C code | Licence notes |
| --- | --- | --- | --- | --- |
| `png` | on | PNG decode and encode (`png`) | no | MIT/Apache-2.0 |
| `jpeg` | on | JPEG decode (`image`) and encode (`jpeg-encoder`) | no | `jpeg-encoder` is `(MIT OR Apache-2.0) AND IJG`: its DCT is ported from IJG code. Binaries must say "this software is based in part on the work of the Independent JPEG Group" in their documentation |
| `webp` | on | WebP decode (lossy and lossless) and lossless encode (`image-webp`) | no | MIT/Apache-2.0 |
| `gif` | off | GIF decode, including animations | no | MIT/Apache-2.0 |
| `overlay` | off | Laying out and drawing text: `FontRegistry`, `TextRenderer`, `draw_onto`, and running `text` steps (`harfrust`, `tiny-skia`, `unicode-linebreak`). Without it, text steps still parse but fail validation | no | MIT, BSD-3-Clause, Apache-2.0 |
| `schemars` | off | `schemars::JsonSchema` for pipeline files | no | MIT |
| `png-optimize` | off | Smaller PNGs through oxipng when `optimize` is set | yes: libdeflate | oxipng MIT, libdeflate MIT |
| `jpeg-mozjpeg` | off | Progressive, trellis-quantized JPEG through MozJPEG when `optimize` is set. Native only: on wasm it does nothing. libjpeg reports errors by unwinding, which the encoder turns into an `Error`; a binary built with `panic = "abort"` aborts on such an error instead | yes: MozJPEG, statically linked | `IJG AND BSD-3-Clause AND Zlib`: binaries need the IJG sentence above and the BSD-3-Clause notices of libjpeg-turbo and MozJPEG |
| `webp-libwebp` | off | Lossy (and better lossless) WebP encoding through libwebp | yes: libwebp, statically linked | libwebp is BSD-3-Clause with Google's patent grant: binaries must reproduce its copyright notice, conditions and disclaimer |
| `wasm-libc-shim` | off | `malloc`, `calloc`, `realloc`, `free` and `qsort` for the C codecs on wasm (see below) | no | |

Features are additive. The C features need a C compiler (and `nasm` for
MozJPEG's x86 SIMD; without it MozJPEG builds in plain C).

## Usage

### A pipeline from TOML

```rust,no_run
use imgpipe::config::PipelineFile;
use imgpipe::{
    Ctx, DecodeOptions, EncodeOptions, Encoders, Format, Limits, MemoryBudget, decode,
};

let file: PipelineFile = toml::from_str(
    r#"
version = 1

[[steps]]
op = "resize"
max_long_side = 1600

[[steps]]
op = "sharpen"
amount = 0.3
"#,
)?;
// One set of size limits for decoding and for every step's output.
let limits = Limits::default();
let input = decode(&std::fs::read("in.jpg")?, &DecodeOptions::default().with_limits(limits))?;
let cx = Ctx::new()
    .with_limits(limits)
    .with_budget(MemoryBudget::bytes(512 << 20)); // what this process may spend on pixels
let output = file.pipeline.run(input, &cx)?;
let webp = Encoders::builtin().encode(&output, &EncodeOptions::new(Format::Webp).lossless(true))?;
std::fs::write("out.webp", webp)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Pipeline::validate` returns the plan (sizes, peak memory) or every problem
at once. `Pipeline::run` validates first and refuses to start when the input
or any step's predicted output is outside `Ctx::limits`, or the predicted
peak exceeds `Ctx::budget`, so hostile parameters (a 4-billion-pixel pad, an
upscale to 100 000 px) are errors, never panics or aborts. The defaults are
`Limits::default()` (16 383 px a side, libwebp's maximum, and 100 million
pixels; `decode` uses the same) and `MemoryBudget::DEFAULT` (2 GiB).

Lengths in `crop` and `pad` are integers for pixels or strings for a
percentage of the image's width or height: `rect = [10, 10, "50%", "50%"]`.
A bare float such as `0.5` is an error (JSON writers print `1.0` as `1`, so a
float can't tell a fraction from a pixel count).

#### Defaults worth knowing

- `resize` with `fit = "pad"` and `pad` fill with opaque **white** unless
  `background`/`color` says otherwise (`"#00000000"` is transparent).
- `resize` filters in linear light (`linear = false` for sRGB values).
- JPEG has no alpha: transparent pixels are flattened onto white.
- `lossless` defaults to true for PNG and GIF and false otherwise, so a
  `format = "webp"` output asks for lossy WebP, which needs libwebp (the
  `webp-libwebp` feature or a platform encoder).
- GIF frame delays of 10 ms or less are read as 100 ms, as browsers show
  them.
- Built-in encoders never write metadata (EXIF, XMP, ICC profiles).

### Several outputs

A pipeline file's `[[outputs]]` entries are `variants::Variant`s:
`run_variants` runs each one's extra `tail` steps on the shared result,
encodes it and hands it to a `Sink`. `MemorySink` collects the bytes;
`variants::FsSink::new(dir)` writes files below `dir`, and a rendered name
that is absolute or contains `..` or `.` is an error, never a write outside
it.

```rust
use imgpipe::config::PipelineFile;
use imgpipe::variants::{MemorySink, run_variants};
use imgpipe::{Ctx, Encoders, Frames, Rgba8};

let file: PipelineFile = toml::from_str(
    r#"
version = 1

[[steps]]
op = "pad"
top = "25%"                      # a caption band, 25% of the height

[[outputs]]
name = "{stem}/full.{hash8}.png"   # {stem} {ext} {w} {h} {hash8}
format = "png"

[[outputs]]
name = "{stem}/thumb-{w}x{h}.webp"
format = "webp"
lossless = true
tail = [{ op = "resize", max_long_side = 32 }]
"#,
)?;
let cx = Ctx::new();
let base = file.pipeline.run(Frames::still(Rgba8::filled(64, 48, [40, 60, 90, 255])), &cx)?;
let mut sink = MemorySink::default(); // or FsSink::new("dist")
let rendered = run_variants(&base, "photo", &file.outputs, &cx, &Encoders::builtin(), &mut sink)?;
assert_eq!((rendered[0].width, rendered[0].height), (64, 60));
assert_eq!(rendered[1].name, "photo/thumb-32x30.webp");
assert_eq!(sink.outputs.len(), 2);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Errors

Everything returns `imgpipe::Error`, which is `Send + Sync + 'static` and
`#[non_exhaustive]` (match the cases you handle, then `_`):

```rust
use imgpipe::{Ctx, Error, Frames, Pipeline, Rgba8, Step, StepError};
use imgpipe::steps::Rotate;

let pipeline = Pipeline::new(vec![Step::Rotate(Rotate::new(45))]);
let frames = Frames::still(Rgba8::filled(4, 4, [0; 4]));
match pipeline.run(frames, &Ctx::new()) {
    Ok(_) => unreachable!(),
    // Bad parameters or sizes: nothing ran. One diagnostic per problem.
    Err(Error::Invalid(problems)) => {
        assert_eq!(problems[0].to_string(), "step 0 (rotate): degrees must be a multiple of 90, got 45");
    }
    // The plan needs more memory than `Ctx::budget`.
    Err(Error::Budget { needed, budget }) => eprintln!("needs {needed} of {budget} bytes"),
    // A step failed while running; `source` says why (a `TextError` for text).
    Err(Error::Step { index, source: StepError::Text(e), .. }) => eprintln!("box in step {index}: {e}"),
    Err(e) => eprintln!("{e}"),
}
```

`decode` fails with `Error::Decode` (not an image) or `Error::Limit` (too
large), `Encoders::encode` with `Error::Unsupported` (no encoder for that
format in this build) or `Error::Encode`, and `FsSink` with `Error::Io` (it
carries the `std::io::Error` and the path) or `Error::OutputName`.

### Text

A `text` step draws boxes with fonts from the `Ctx` (`Ctx::with_fonts`,
feature `overlay`); `{name}` placeholders are filled from `Ctx::with_vars` (a
`HashMap` or `BTreeMap` of strings) in one pass (unknown names stay as
written; `{{` and `}}` are literal braces):

```toml
[[steps]]
op = "text"

[[steps.boxes]]
rect = [0.05, 0.7, 0.9, 0.25]          # x, y, w, h as fractions of the image
text = "{caption}"
fonts = ["anton"]                      # ids registered in the FontRegistry
size = { fit = { min = 0.04, max = 0.12 } }   # or { fixed = 0.08 }; fractions of the image height
stroke = { color = "#000000", width = 0.08 }  # width as a fraction of the font size
valign = "bottom"
uppercase = true
```

The text types (`TextStep`, `TextBox`, ...) are in every build, so such a
file parses everywhere; without `overlay`, `validate` rejects the step
("this build does not have the `overlay` feature").

#### Without a pipeline

```rust,no_run
# #[cfg(feature = "overlay")]
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use imgpipe::overlay::{FontRegistry, FontSize, Stroke, TextBox, TextRenderer, VAlign, draw_onto};
use imgpipe::{Color, Rgba8};

let mut fonts = FontRegistry::new();
fonts.register("anton", std::fs::read("Anton-Regular.ttf")?)?;
// Variable fonts are pinned to one instance:
// fonts.register_instance("oswald-bold", bytes, &[("wght", 700.0)])?;

let caption = TextBox {
    lang: Some("en".into()),
    stroke: Some(Stroke::new(Color::BLACK, 0.08)),
    valign: VAlign::Bottom,
    uppercase: true,
    ..TextBox::new(
        [0.05, 0.7, 0.9, 0.25],
        "Hello, world",
        ["anton"],
        FontSize::Fit { min: 0.04, max: 0.12 },
    )
};

let mut img = Rgba8::filled(800, 600, [40, 60, 90, 255]);
let aspect = img.width() as f32 / img.height() as f32;
let mut renderer = TextRenderer::new(); // keep it: it caches shaping and outlines
// Arguments come in one order: box, layout, fonts, then the frame.
let layout = renderer.layout(&caption, &fonts, aspect)?;
assert!(!layout.overflow && layout.missing.is_empty());

// Redraw just this box, e.g. in an editor (straight-alpha RGBA for its rect):
let _layer = renderer.rasterize(&caption, &layout, &fonts, aspect, img.height());

draw_onto(&mut img, [(&caption, &layout)], &fonts, &mut renderer)?;
# Ok(())
# }
# #[cfg(not(feature = "overlay"))]
# fn main() {}
```

The layout depends only on the box, the aspect ratio and the fonts, never on
the pixel size. `TextLayout::pixel_bounds` gives the pixels a box can touch at
a given height, and `TextRenderer::rasterize` draws it into a pixmap of just
that size.

### A custom encoder

Every encoder has an `EncoderId`, and `Encoders::register` puts your own (for
example a browser's built-in encoder) ahead of the built-in ones:

```rust
use imgpipe::{EncodeOptions, Encoder, EncoderId, Encoders, Format, Frames, Result};

struct BrowserAvif; // would call the platform's encoder

impl Encoder for BrowserAvif {
    fn id(&self) -> EncoderId {
        EncoderId::new("browser-avif", "1")
    }
    fn supports(&self, opts: &EncodeOptions) -> bool {
        opts.format == Format::Avif
    }
    fn encode(&self, frames: &Frames, _opts: &EncodeOptions) -> Result<Vec<u8>> {
        Ok(frames.first().data().to_vec()) // stand-in for the real thing
    }
}

let mut encoders = Encoders::builtin();
encoders.register(Box::new(BrowserAvif));
let opts: EncodeOptions = "avif".parse::<Format>().map(EncodeOptions::new)?;
assert_eq!(encoders.find(&opts).unwrap().id().name, "browser-avif");
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Format` also has `from_extension` and `from_mime`, and displays as `PNG`,
`JPEG`, `WebP`, ...

See the [API documentation](https://docs.rs/imgpipe) for details.

## WebAssembly

- Everything builds for `wasm32-unknown-unknown`. Nothing spawns threads.
- Build with `-C target-feature=+simd128`: tiny-skia and fast_image_resize use
  SIMD on wasm only with it.
- `Pipeline::run` is synchronous; in a browser, run it in a worker.
- `Encoder` and `overlay::Vars` are bounded by `MaybeSend`, which is
  `Send + Sync` natively and nothing on wasm, so a browser encoder can hold
  JavaScript handles.
- `jpeg-mozjpeg` does nothing on wasm (libjpeg reports errors by unwinding
  through C), so JPEG stays on `jpeg-encoder`. `FsSink` is native only.

The C codecs (`webp-libwebp`, `png-optimize`) also build for wasm, with:

1. A C compiler that targets wasm32 and an archiver, plus libc headers, set
   through `CC_wasm32_unknown_unknown`, `AR_wasm32_unknown_unknown` and
   `CFLAGS_wasm32_unknown_unknown` (for example clang, `llvm-ar` and
   wasi-libc's headers). Compile libwebp without threads:
   `-UWEBP_USE_THREAD -U_THREAD_SAFE`.
2. A libc for the five functions the codecs call: `malloc`, `calloc`,
   `realloc`, `free` and `qsort`. `wasm32-unknown-unknown` has none. Enable
   `wasm-libc-shim` in the crate that builds the final `.wasm` to get them,
   implemented over Rust's allocator. The symbols are global, so leave the
   feature off if anything else in the binary already defines them, and never
   enable it from a library. Without the shim or another definition, linking
   fails with `undefined symbol: malloc` (and the others).

## Command-line tool

[`imgpipe-cli`](https://crates.io/crates/imgpipe-cli) (binary `imgpipe`) runs, validates and explains pipeline files:

```shell
imgpipe run -c pipeline.toml in.jpg -o out.webp            # one output, format from the extension
imgpipe run -c pipeline.toml 'raw/*.jpg' --out-dir dist/   # every [[outputs]] entry
imgpipe run -c pipeline.toml in.jpg -o out.png --font anton=Anton-Regular.ttf --var caption=hi
imgpipe validate -c pipeline.toml --input 4000x3000        # parameters, encoders, memory
imgpipe explain -c pipeline.toml                           # resolved steps and sizes
imgpipe schema > pipeline.schema.json                      # JSON Schema for editor completion
imgpipe --help
```

## Development

See the [repository README](https://github.com/sagikazarmark/imgpipe#development) for the shared checks.

### Tests

A bare `cargo test` skips `tests/overlay.rs`, which needs the `overlay`
feature (and `png`); run tests with `--features overlay` or
`--all-features`, as `just test` does:

```shell
cargo test --all-features       # everything (the C codecs need a C compiler)
cargo test --features overlay   # pure Rust, including the text tests
```

### Golden images

Golden images live in `tests/golden`. Comparisons allow small SIMD rounding
differences, so the same goldens pass in wasm. After an intended change,
regenerate them natively and review the diff:

```shell
just update-goldens
```

### WebAssembly tests

The integration tests also run in wasm under Node, with
`wasm-bindgen-test-runner` as the runner and `+simd128` (see
`.cargo/config.toml`):

```shell
just test-wasm
```

Testing the C codecs in wasm (`--features webp-libwebp,png-optimize,wasm-libc-shim --test codecs`)
needs the wasm C toolchain described under [WebAssembly](#webassembly); the devenv shell provides it.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

The test fonts in `tests/fonts` (Anton, Oswald) are under the SIL Open Font
License 1.1; their licences are next to them.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

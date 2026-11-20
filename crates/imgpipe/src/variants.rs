//! Variants: several encoded outputs from one pipeline result.
//!
//! A [`Variant`] is one `[[outputs]]` entry of a pipeline file (see
//! [`PipelineFile`](crate::config::PipelineFile)): a file name template,
//! optional extra steps and the encoder settings. [`run_variants`] encodes
//! each into a [`Sink`] and returns a [`RenderedVariant`] per output.

use crate::encode::{EncodeFields, EncodeOptions, Encoders, Format};
#[cfg(not(target_arch = "wasm32"))]
use crate::error::Error;
use crate::error::Result;
use crate::frame::Frames;
use crate::pipeline::{Ctx, Pipeline};
use crate::steps::Step;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One output: optional extra steps (`tail`) from the full-resolution
/// result, then encoding.
///
/// Serialized as a pipeline file's `[[outputs]]` entry: `name`, the
/// [`EncodeOptions`] fields (`format`, `quality`, `lossless`, `optimize`,
/// written exactly as `EncodeOptions` writes them) and `tail`, a list of
/// steps.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    /// File name template: `{stem}` (the input's name), `{ext}` (the
    /// format's extension), `{w}` and `{h}` (the encoded size in px) and
    /// `{hash8}` (see [`content_hash`]). Substituted once: values are never
    /// expanded again, and unknown `{...}` stay as written.
    pub name: String,
    /// Steps run on the base result for this output only.
    pub tail: Pipeline,
    /// How to encode.
    pub encode: EncodeOptions,
}

impl Variant {
    /// A variant named by the template `name`, encoded with `encode`,
    /// without extra steps.
    pub fn new(name: impl Into<String>, encode: EncodeOptions) -> Self {
        Self {
            name: name.into(),
            tail: Pipeline::default(),
            encode,
        }
    }

    /// Sets the extra steps run for this output only.
    pub fn with_tail(mut self, steps: Vec<Step>) -> Self {
        self.tail = Pipeline::new(steps);
        self
    }
}

// The `[[outputs]]` entry as written: `EncodeFields` plus `name` and `tail`
// (not `#[serde(flatten)]`, which can't deny unknown fields). The doc comment
// below is the JSON Schema description.
/// One `[[outputs]]` entry: a file name template, how to encode, and optional
/// extra steps for this output only.
#[derive(Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, rename = "Variant")]
struct VariantFields {
    /// Template: `{stem}`, `{ext}`, `{w}`, `{h}`, `{hash8}`.
    name: String,
    /// Output format.
    format: Format,
    /// 1–100 for lossy formats; the encoder's default when unset.
    #[serde(default)]
    quality: Option<u8>,
    /// Lossless encoding where the format offers both (WebP). Defaults to
    /// true for PNG and GIF, false otherwise.
    #[serde(default)]
    lossless: Option<bool>,
    /// Smaller files for more time (PNG: oxipng, with `png-optimize`; JPEG:
    /// MozJPEG, with `jpeg-mozjpeg`).
    #[serde(default)]
    optimize: bool,
    /// Extra steps for this output, run on the full pipeline result.
    #[serde(default)]
    tail: Vec<Step>,
}

impl<'de> Deserialize<'de> for Variant {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let f = VariantFields::deserialize(d)?;
        let encode = EncodeFields {
            format: f.format,
            quality: f.quality,
            lossless: f.lossless,
            optimize: f.optimize,
        };
        Ok(Variant::new(f.name, encode.into()).with_tail(f.tail))
    }
}

impl Serialize for Variant {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Fields<'a> {
            name: &'a str,
            #[serde(flatten)]
            encode: EncodeFields,
            #[serde(skip_serializing_if = "<[Step]>::is_empty")]
            tail: &'a [Step],
        }
        Fields {
            name: &self.name,
            encode: self.encode.into(),
            tail: &self.tail.steps,
        }
        .serialize(s)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for Variant {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Variant".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        VariantFields::json_schema(g)
    }
}

/// Where encoded outputs go. Synchronous: a browser collects into a
/// [`MemorySink`] and stores from there.
pub trait Sink {
    /// Stores one encoded output under its rendered name. A sink that does
    /// I/O can return its `std::io::Error` with `?`, as [an `Error::Io`](crate::Error::Io).
    fn write(&mut self, name: &str, bytes: Vec<u8>) -> Result<()>;
}

/// Keeps outputs in memory (tests, browser).
#[derive(Default, Debug)]
pub struct MemorySink {
    /// (name, bytes) in the order written.
    pub outputs: Vec<(String, Vec<u8>)>,
}

impl Sink for MemorySink {
    fn write(&mut self, name: &str, bytes: Vec<u8>) -> Result<()> {
        self.outputs.push((name.to_owned(), bytes));
        Ok(())
    }
}

/// Writes outputs under a directory, creating parent directories.
///
/// Names must be relative paths of plain components (`a/b.png`): an empty or
/// absolute name, or one with `..`, `.`, a root or a drive prefix, is an
/// [`Error::OutputName`] and nothing is written, so a name template (or a
/// `{stem}` from an untrusted file name) cannot reach outside `root`.
/// Symlinks that already exist inside `root` are followed; keep `root` to
/// yourself.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
pub struct FsSink {
    /// The directory names are relative to.
    pub root: std::path::PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FsSink {
    /// A sink writing below `root`.
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

/// `name` as a path inside a sink's root, or why it is not one.
#[cfg(not(target_arch = "wasm32"))]
fn relative_path(name: &str) -> Result<&std::path::Path> {
    use std::path::{Component, Path};
    let path = Path::new(name);
    let plain = path.components().all(|c| matches!(c, Component::Normal(_)));
    if name.is_empty() || path.is_absolute() || !plain {
        return Err(Error::OutputName(name.to_owned()));
    }
    Ok(path)
}

#[cfg(not(target_arch = "wasm32"))]
impl Sink for FsSink {
    fn write(&mut self, name: &str, bytes: Vec<u8>) -> Result<()> {
        let io = |path: &std::path::Path| {
            let path = path.to_owned();
            move |source| Error::Io {
                path: Some(path),
                source,
            }
        };
        let path = self.root.join(relative_path(name)?);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&path, bytes).map_err(io(&path))
    }
}

/// What one variant produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct RenderedVariant {
    /// The rendered file name.
    pub name: String,
    /// Width in px.
    pub width: u32,
    /// Height in px.
    pub height: u32,
    /// Encoded size.
    pub bytes: usize,
}

impl RenderedVariant {
    /// What encoding `frames` to `bytes` under `name` produced (for
    /// callers that encode a variant themselves).
    pub fn new(name: impl Into<String>, frames: &Frames, bytes: usize) -> Self {
        let info = frames.info();
        Self {
            name: name.into(),
            width: info.width,
            height: info.height,
            bytes,
        }
    }
}

/// Fills a name template; see [`Variant::name`].
pub(crate) fn render_name(
    template: &str,
    stem: &str,
    ext: &str,
    w: u32,
    h: u32,
    bytes: &[u8],
) -> String {
    let mut out = String::with_capacity(template.len() + stem.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let value = after.find('}').and_then(|close| {
            let v = match &after[..close] {
                "stem" => stem.to_owned(),
                "ext" => ext.to_owned(),
                "w" => w.to_string(),
                "h" => h.to_string(),
                "hash8" => format!("{:08x}", (content_hash(bytes) >> 32) as u32),
                _ => return None,
            };
            Some((v, close))
        });
        match value {
            Some((v, close)) => {
                out.push_str(&v);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The hash behind `{hash8}`: 64-bit FNV-1a of `bytes`; `{hash8}` is its
/// upper 32 bits as 8 lowercase hex digits.
///
/// Stable: it will not change within a major version, so names made with it
/// (by a template, or by callers naming related files the same way) stay
/// valid. For cache-friendly names, not a security property.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Runs every variant from `base` (never from another variant) and writes
/// the encoded results to `sink`, in order. Stops at the first error.
pub fn run_variants(
    base: &Frames,
    stem: &str,
    variants: &[Variant],
    cx: &Ctx<'_>,
    encoders: &Encoders,
    sink: &mut dyn Sink,
) -> Result<Vec<RenderedVariant>> {
    let mut outputs = Vec::with_capacity(variants.len());
    for v in variants {
        let tailed;
        let frames = if v.tail.steps.is_empty() {
            base
        } else {
            tailed = v.tail.run(base.clone(), cx)?;
            &tailed
        };
        let bytes = encoders.encode(frames, &v.encode)?;
        let info = frames.info();
        let name = render_name(
            &v.name,
            stem,
            v.encode.format.extension(),
            info.width,
            info.height,
            &bytes,
        );
        outputs.push(RenderedVariant::new(name.clone(), frames, bytes.len()));
        sink.write(&name, bytes)?;
    }
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_name_substitutes_once() {
        let name = |t: &str, stem: &str| render_name(t, stem, "png", 640, 480, b"x");
        assert_eq!(name("{stem}-{w}x{h}.{ext}", "cat"), "cat-640x480.png");
        // A stem that looks like a placeholder is not expanded again.
        assert_eq!(name("{stem}.{ext}", "{w}{ext}"), "{w}{ext}.png");
        // Unknown and unbalanced braces stay as written.
        assert_eq!(name("{nope}/{stem", "a"), "{nope}/{stem");
        assert_eq!(name("{nope}/{{stem}}/{w", "a"), "{nope}/{a}/{w");
        let hash = format!("{:08x}", (content_hash(b"x") >> 32) as u32);
        assert_eq!(name("{hash8}", "a"), hash);
    }

    #[test]
    fn content_hash_is_fnv1a() {
        // Published FNV-1a 64 test vectors: names built on it must not move.
        assert_eq!(content_hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(content_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn variants_serialize_as_outputs_entries() {
        let v: Variant = toml::from_str(
            "name = \"{stem}.webp\"\nformat = \"webp\"\nquality = 80\n\
             tail = [{ op = \"resize\", max_long_side = 10 }]\n",
        )
        .unwrap();
        assert_eq!(v.encode, EncodeOptions::new(Format::Webp).quality(80));
        assert_eq!(v.tail.steps.len(), 1);
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(
            json,
            r##"{"name":"{stem}.webp","format":"webp","quality":80,"tail":[{"op":"resize","max_long_side":10,"fit":"contain","filter":"lanczos3","linear":true,"allow_upscale":false,"background":"#ffffff"}]}"##
        );
        assert_eq!(serde_json::from_str::<Variant>(&json).unwrap(), v);
        let err = toml::from_str::<Variant>("name = \"a\"\nformat = \"png\"\nqualty = 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("qualty"), "{err}");
    }
}

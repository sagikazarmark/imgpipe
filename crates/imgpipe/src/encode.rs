//! Encoders: a registry of format encoders, each behind its own feature.
//! Platforms can register their own (e.g. a browser's built-in WebP or AVIF
//! encoder) ahead of the built-in ones.
//!
//! # A custom encoder
//!
//! ```
//! use imgpipe::{EncodeOptions, Encoder, EncoderId, Encoders, Format, Frames, Result};
//!
//! /// Writes raw RGBA bytes, standing in for a platform codec.
//! struct RawAvif;
//!
//! impl Encoder for RawAvif {
//!     fn id(&self) -> EncoderId {
//!         EncoderId::new("raw-avif", "1")
//!     }
//!     fn supports(&self, opts: &EncodeOptions) -> bool {
//!         opts.format == Format::Avif
//!     }
//!     fn encode(&self, frames: &Frames, _opts: &EncodeOptions) -> Result<Vec<u8>> {
//!         Ok(frames.first().data().to_vec())
//!     }
//! }
//!
//! let mut encoders = Encoders::builtin();
//! encoders.register(Box::new(RawAvif)); // ahead of the built-in ones
//! let frames = Frames::still(imgpipe::Rgba8::filled(2, 2, [255, 0, 0, 255]));
//! let bytes = encoders.encode(&frames, &EncodeOptions::new(Format::Avif))?;
//! assert_eq!(bytes.len(), 16);
//! # Ok::<(), imgpipe::Error>(())
//! ```

use crate::MaybeSend;
use crate::error::{Error, Result};
use crate::frame::{Frames, Rgba8};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// An image file format. Whether it can be encoded depends on the features
/// (and registered platform encoders), see [`Encoders`].
///
/// Written in pipeline files by its lowercase name (`png`, `jpeg`, `webp`,
/// `gif`, `avif`). `Display` gives the usual spelling (`WebP`); `FromStr`
/// takes either, in any case, and `jpg` for JPEG.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Format {
    /// PNG.
    Png,
    /// JPEG.
    Jpeg,
    /// WebP, lossy or lossless.
    Webp,
    /// GIF (decoding only for now).
    Gif,
    /// AVIF (no built-in codec; for platform encoders).
    Avif,
}

impl Format {
    /// Every format, in declaration order.
    pub const ALL: [Format; 5] = [
        Format::Png,
        Format::Jpeg,
        Format::Webp,
        Format::Gif,
        Format::Avif,
    ];

    /// The usual file extension, without the dot (`jpg` for JPEG).
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
            Format::Gif => "gif",
            Format::Avif => "avif",
        }
    }

    /// The MIME type, e.g. `image/png`.
    pub fn mime(self) -> &'static str {
        match self {
            Format::Png => "image/png",
            Format::Jpeg => "image/jpeg",
            Format::Webp => "image/webp",
            Format::Gif => "image/gif",
            Format::Avif => "image/avif",
        }
    }

    /// The format for a file extension (`png`, `jpg`, `jpeg`, ...), with or
    /// without the leading dot, in any case.
    pub fn from_extension(ext: &str) -> Option<Format> {
        let ext = ext.strip_prefix('.').unwrap_or(ext);
        if ext.eq_ignore_ascii_case("jpeg") {
            return Some(Format::Jpeg);
        }
        Self::ALL
            .into_iter()
            .find(|f| f.extension().eq_ignore_ascii_case(ext))
    }

    /// The format for a MIME type (`image/png`, ...), in any case and with
    /// any parameters (`; charset=...`) ignored.
    pub fn from_mime(mime: &str) -> Option<Format> {
        let essence = mime.split(';').next().unwrap_or("").trim();
        Self::ALL
            .into_iter()
            .find(|f| f.mime().eq_ignore_ascii_case(essence))
    }

    /// Lossless unless asked otherwise: PNG and GIF have nothing else.
    fn lossless_by_default(self) -> bool {
        matches!(self, Format::Png | Format::Gif)
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Webp => "WebP",
            Format::Gif => "GIF",
            Format::Avif => "AVIF",
        })
    }
}

/// [`Format`]'s `FromStr` error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown image format {0:?}; use png, jpeg, webp, gif or avif")]
pub struct UnknownFormat(String);

impl std::str::FromStr for Format {
    type Err = UnknownFormat;
    /// `png`, `jpeg` (or `jpg`), `webp`, `gif`, `avif`, in any case.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("jpg") {
            return Ok(Format::Jpeg);
        }
        Self::ALL
            .into_iter()
            .find(|f| f.to_string().eq_ignore_ascii_case(s))
            .ok_or_else(|| UnknownFormat(s.to_owned()))
    }
}

/// What to encode to. [`EncodeOptions::new`] gives the defaults; the
/// setters change one field each.
///
/// Serialized as in a pipeline file's `[[outputs]]` entries: `format`, and
/// `quality`, `lossless` and `optimize` when they differ from the defaults
/// (`lossless` defaults to true for PNG and GIF, false otherwise).
///
/// Built-in encoders never write metadata (EXIF, XMP, ICC profiles).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "EncodeFields", into = "EncodeFields")]
#[non_exhaustive]
pub struct EncodeOptions {
    /// Output format.
    pub format: Format,
    /// 1–100 for lossy formats; the encoder's default when unset (85 for
    /// the built-in JPEG and WebP encoders).
    pub quality: Option<u8>,
    /// Lossless encoding (always for PNG and GIF; optional for WebP).
    pub lossless: bool,
    /// Spend more time for a smaller file where an encoder can: PNG through
    /// oxipng (feature `png-optimize`, about 10% smaller and 2-3x slower),
    /// JPEG through MozJPEG (feature `jpeg-mozjpeg`, native only; about 20%
    /// smaller at the same quality number and 5x slower; its errors unwind,
    /// so a `panic = "abort"` binary aborts on one). Without the feature it
    /// is ignored.
    pub optimize: bool,
}

// The serialized form of `EncodeOptions`, with the same fields as a
// `Variant`'s. The doc comment below is the JSON Schema description.
/// How to encode: a format and its settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields, rename = "EncodeOptions")]
pub(crate) struct EncodeFields {
    /// Output format.
    pub format: Format,
    /// 1–100 for lossy formats; the encoder's default when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
    /// Lossless encoding where the format offers both (WebP). Defaults to
    /// true for PNG and GIF, false otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lossless: Option<bool>,
    /// Smaller files for more time (PNG: oxipng, with `png-optimize`; JPEG:
    /// MozJPEG, with `jpeg-mozjpeg`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optimize: bool,
}

impl From<EncodeFields> for EncodeOptions {
    fn from(f: EncodeFields) -> Self {
        let mut o = EncodeOptions::new(f.format).optimize(f.optimize);
        o.quality = f.quality;
        if let Some(lossless) = f.lossless {
            o.lossless = lossless;
        }
        o
    }
}

impl From<EncodeOptions> for EncodeFields {
    fn from(o: EncodeOptions) -> Self {
        EncodeFields {
            format: o.format,
            quality: o.quality,
            lossless: (o.lossless != o.format.lossless_by_default()).then_some(o.lossless),
            optimize: o.optimize,
        }
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for EncodeOptions {
    fn schema_name() -> Cow<'static, str> {
        "EncodeOptions".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        EncodeFields::json_schema(g)
    }
}

impl EncodeOptions {
    /// Defaults for `format`: encoder-default quality, lossless for PNG and
    /// GIF, not optimized.
    pub fn new(format: Format) -> Self {
        Self {
            format,
            quality: None,
            lossless: format.lossless_by_default(),
            optimize: false,
        }
    }

    /// Sets [`quality`](Self::quality).
    pub fn quality(mut self, q: u8) -> Self {
        self.quality = Some(q);
        self
    }

    /// Sets [`optimize`](Self::optimize).
    pub fn optimize(mut self, optimize: bool) -> Self {
        self.optimize = optimize;
        self
    }

    /// Sets [`lossless`](Self::lossless).
    pub fn lossless(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        self
    }
}

/// Identifies an encoder implementation, for cache keys and diagnostics.
/// `Display` gives `name version`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct EncoderId {
    /// Implementation name, e.g. `libwebp`.
    pub name: Cow<'static, str>,
    /// Implementation version, so output changes invalidate caches.
    pub version: Cow<'static, str>,
}

impl EncoderId {
    /// An id from static strings. For names known only at run time (a
    /// browser's codec version), build the struct with owned strings.
    pub const fn new(name: &'static str, version: &'static str) -> Self {
        Self {
            name: Cow::Borrowed(name),
            version: Cow::Borrowed(version),
        }
    }
}

impl std::fmt::Display for EncoderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.name, self.version)
    }
}

/// A format encoder. Implement it to plug in a platform codec (e.g. the
/// browser's) and add it with [`Encoders::register`]; see the
/// [module docs](self) for an example.
pub trait Encoder: MaybeSend {
    /// Which implementation this is.
    fn id(&self) -> EncoderId;
    /// Whether this encoder handles these options (format, lossless).
    fn supports(&self, opts: &EncodeOptions) -> bool;
    /// Encodes `frames`; only called with options it [`supports`](Self::supports),
    /// and never with an image that has no pixels.
    fn encode(&self, frames: &Frames, opts: &EncodeOptions) -> Result<Vec<u8>>;
}

/// Encoders in priority order: the first that supports a request wins.
pub struct Encoders {
    list: Vec<Box<dyn Encoder>>,
}

impl std::fmt::Debug for Encoders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.list.iter().map(|e| e.id().to_string()))
            .finish()
    }
}

impl Encoders {
    /// No encoders at all.
    pub fn empty() -> Self {
        Self { list: Vec::new() }
    }

    /// Every encoder compiled into this build.
    pub fn builtin() -> Self {
        #[allow(unused_mut)]
        let mut e = Self::empty();
        #[cfg(feature = "png-optimize")]
        e.list.push(Box::new(png_impl::OxipngEncoder));
        #[cfg(feature = "png")]
        e.list.push(Box::new(png_impl::PngEncoder));
        #[cfg(all(feature = "jpeg-mozjpeg", not(target_arch = "wasm32")))]
        e.list.push(Box::new(jpeg_impl::MozjpegEncoder));
        #[cfg(feature = "jpeg")]
        e.list.push(Box::new(jpeg_impl::JpegEncoder));
        #[cfg(feature = "webp-libwebp")]
        e.list.push(Box::new(libwebp_impl::LibwebpEncoder));
        #[cfg(feature = "webp")]
        e.list.push(Box::new(webp_impl::WebpLosslessEncoder));
        e
    }

    /// Adds an encoder ahead of all existing ones.
    pub fn register(&mut self, encoder: Box<dyn Encoder>) {
        self.list.insert(0, encoder);
    }

    /// The first encoder that supports `opts`.
    pub fn find(&self, opts: &EncodeOptions) -> Option<&dyn Encoder> {
        self.list
            .iter()
            .find(|e| e.supports(opts))
            .map(|e| e.as_ref())
    }

    /// Every encoder, in priority order.
    pub fn ids(&self) -> Vec<EncoderId> {
        self.list.iter().map(|e| e.id()).collect()
    }

    /// Encodes with the first encoder that supports `opts`;
    /// [`Error::Unsupported`] when none does, [`Error::Encode`] for an image
    /// with no pixels.
    pub fn encode(&self, frames: &Frames, opts: &EncodeOptions) -> Result<Vec<u8>> {
        let encoder = self.find(opts).ok_or_else(|| {
            let kind = if opts.lossless { "lossless" } else { "lossy" };
            let hint = match (opts.format, opts.lossless) {
                (Format::Webp, false) => {
                    " (lossy WebP needs libwebp: a platform encoder or the webp-libwebp feature)"
                }
                _ => "",
            };
            Error::Unsupported(format!(
                "no {kind} {} encoder in this build{hint}",
                opts.format
            ))
        })?;
        let info = frames.info();
        if info.width == 0 || info.height == 0 {
            return Err(Error::Encode(format!(
                "a {}x{} image has no pixels",
                info.width, info.height
            )));
        }
        encoder.encode(frames, opts)
    }
}

/// Stills only for now; animated output arrives with the GIF encoder.
#[cfg_attr(
    not(any(feature = "png", feature = "jpeg", feature = "webp")),
    allow(dead_code)
)]
fn still(frames: &Frames) -> Result<&Rgba8> {
    if frames.is_animated() {
        return Err(Error::Unsupported(
            "animated output (GIF encoding comes later)".into(),
        ));
    }
    Ok(frames.first())
}

#[cfg(feature = "png")]
mod png_impl {
    use super::*;

    pub(super) fn encode_png(img: &Rgba8, compression: png::Compression) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, img.width(), img.height());
        // Opaque images are written as RGB: a quarter smaller before compression.
        let opaque = img.is_opaque();
        enc.set_color(if opaque {
            png::ColorType::Rgb
        } else {
            png::ColorType::Rgba
        });
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(compression);
        let mut writer = enc
            .write_header()
            .map_err(|e| Error::Encode(e.to_string()))?;
        let data: Vec<u8> = if opaque {
            img.data()
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect()
        } else {
            img.data().to_vec()
        };
        writer
            .write_image_data(&data)
            .map_err(|e| Error::Encode(e.to_string()))?;
        writer.finish().map_err(|e| Error::Encode(e.to_string()))?;
        Ok(out)
    }

    pub(super) struct PngEncoder;

    impl Encoder for PngEncoder {
        fn id(&self) -> EncoderId {
            EncoderId::new("png", "0.18")
        }
        fn supports(&self, opts: &EncodeOptions) -> bool {
            opts.format == Format::Png
        }
        fn encode(&self, frames: &Frames, _opts: &EncodeOptions) -> Result<Vec<u8>> {
            encode_png(still(frames)?, png::Compression::High)
        }
    }

    /// PNG, then a lossless oxipng pass (level 2), when `optimize` is asked for.
    /// oxipng links libdeflate (C): native, or wasm with a wasm32 C compiler in
    /// `CC_wasm32_unknown_unknown` (e.g. clang with wasi-libc headers).
    #[cfg(feature = "png-optimize")]
    pub(super) struct OxipngEncoder;

    #[cfg(feature = "png-optimize")]
    impl Encoder for OxipngEncoder {
        fn id(&self) -> EncoderId {
            EncoderId::new("oxipng", "10.2")
        }
        fn supports(&self, opts: &EncodeOptions) -> bool {
            opts.format == Format::Png && opts.optimize
        }
        fn encode(&self, frames: &Frames, _opts: &EncodeOptions) -> Result<Vec<u8>> {
            // Start from the plain encoder's best: oxipng keeps its input when it
            // can't beat it, so optimizing never makes a file larger.
            let png = encode_png(still(frames)?, png::Compression::High)?;
            let options = oxipng::Options::from_preset(2);
            oxipng::optimize_from_memory(&png, &options).map_err(|e| Error::Encode(e.to_string()))
        }
    }
}

#[cfg(feature = "jpeg")]
mod jpeg_impl {
    use super::*;

    /// RGB with transparency flattened onto white, plus the size as JPEG
    /// allows it.
    fn rgb_over_white(img: &Rgba8) -> Result<(Vec<u8>, u16, u16)> {
        let (w, h) = (u16::try_from(img.width()), u16::try_from(img.height()));
        let (Ok(w), Ok(h)) = (w, h) else {
            return Err(Error::Encode("JPEG is limited to 65535 px per side".into()));
        };
        let rgb = img
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let a = p[3] as u32;
                let over_white = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
                [over_white(p[0]), over_white(p[1]), over_white(p[2])]
            })
            .collect();
        Ok((rgb, w, h))
    }

    fn quality(opts: &EncodeOptions) -> u8 {
        opts.quality.unwrap_or(85).clamp(1, 100)
    }

    /// Baseline JPEG; transparency is flattened onto white. Below q90 chroma
    /// is subsampled 4:2:0, from q90 it is kept at full resolution (4:4:4).
    pub(super) struct JpegEncoder;

    impl Encoder for JpegEncoder {
        fn id(&self) -> EncoderId {
            EncoderId::new("jpeg-encoder", "0.7")
        }
        fn supports(&self, opts: &EncodeOptions) -> bool {
            opts.format == Format::Jpeg && !opts.lossless
        }
        fn encode(&self, frames: &Frames, opts: &EncodeOptions) -> Result<Vec<u8>> {
            let (rgb, w, h) = rgb_over_white(still(frames)?)?;
            let mut out = Vec::new();
            let enc = jpeg_encoder::Encoder::new(&mut out, quality(opts));
            enc.encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
                .map_err(|e| Error::Encode(e.to_string()))?;
            Ok(out)
        }
    }

    /// MozJPEG, when `optimize` is asked for: progressive scans, optimized
    /// Huffman tables and trellis quantization (its defaults). Same quality
    /// scale and chroma subsampling as [`JpegEncoder`]; no metadata (JFIF
    /// header only), like it. Native only: libjpeg reports errors by
    /// unwinding through C, which is caught here and becomes an error, so
    /// only with `panic = "unwind"`; with `panic = "abort"` the process
    /// aborts on a MozJPEG error.
    #[cfg(all(feature = "jpeg-mozjpeg", not(target_arch = "wasm32")))]
    pub(super) struct MozjpegEncoder;

    #[cfg(all(feature = "jpeg-mozjpeg", not(target_arch = "wasm32")))]
    impl Encoder for MozjpegEncoder {
        fn id(&self) -> EncoderId {
            EncoderId::new("mozjpeg", "0.10")
        }
        fn supports(&self, opts: &EncodeOptions) -> bool {
            opts.format == Format::Jpeg && !opts.lossless && opts.optimize
        }
        fn encode(&self, frames: &Frames, opts: &EncodeOptions) -> Result<Vec<u8>> {
            let (rgb, w, h) = rgb_over_white(still(frames)?)?;
            let q = quality(opts);
            let run = || -> std::io::Result<Vec<u8>> {
                let mut c = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
                c.set_size(w.into(), h.into());
                c.set_quality(q.into());
                // jpeg-encoder's rule: 4:2:0 below q90, 4:4:4 from q90.
                let px = if q < 90 { (2, 2) } else { (1, 1) };
                c.set_chroma_sampling_pixel_sizes(px, px);
                let mut c = c.start_compress(Vec::new())?;
                c.write_scanlines(&rgb)?;
                c.finish()
            };
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
                Ok(Ok(out)) => Ok(out),
                Ok(Err(e)) => Err(Error::Encode(format!("mozjpeg: {e}"))),
                Err(panic) => Err(Error::Encode(format!(
                    "mozjpeg: {}",
                    panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("encoder failed")
                ))),
            }
        }
    }
}

#[cfg(feature = "webp")]
mod webp_impl {
    use super::*;

    /// Lossless WebP only; lossy WebP needs libwebp.
    pub(super) struct WebpLosslessEncoder;

    impl Encoder for WebpLosslessEncoder {
        fn id(&self) -> EncoderId {
            EncoderId::new("image-webp", "0.2")
        }
        fn supports(&self, opts: &EncodeOptions) -> bool {
            opts.format == Format::Webp && opts.lossless
        }
        fn encode(&self, frames: &Frames, _opts: &EncodeOptions) -> Result<Vec<u8>> {
            let img = still(frames)?;
            let mut out = Vec::new();
            image_webp::WebPEncoder::new(&mut out)
                .encode(
                    img.data(),
                    img.width(),
                    img.height(),
                    image_webp::ColorType::Rgba8,
                )
                .map_err(|e| Error::Encode(e.to_string()))?;
            Ok(out)
        }
    }
}

/// Lossy and lossless WebP through libwebp (C, statically linked). On wasm
/// it needs a C compiler for wasm32 in `CC_wasm32_unknown_unknown` (e.g.
/// clang with wasi-libc headers in `CFLAGS_wasm32_unknown_unknown`) plus a
/// few libc functions, e.g. from the `wasm-libc-shim` feature.
#[cfg(feature = "webp-libwebp")]
mod libwebp_impl {
    use super::*;

    pub(super) struct LibwebpEncoder;

    impl Encoder for LibwebpEncoder {
        fn id(&self) -> EncoderId {
            EncoderId::new("libwebp", "1.5")
        }
        fn supports(&self, opts: &EncodeOptions) -> bool {
            opts.format == Format::Webp
        }
        fn encode(&self, frames: &Frames, opts: &EncodeOptions) -> Result<Vec<u8>> {
            use libwebp_sys as w;
            let img = still(frames)?;
            let fail = |what: &str| Error::Encode(format!("libwebp: {what}"));
            let q = if opts.lossless {
                75.0
            } else {
                opts.quality.unwrap_or(85).clamp(1, 100) as f32
            };
            let mut config = w::WebPConfig::new_with_preset(w::WebPPreset::WEBP_PRESET_DEFAULT, q)
                .map_err(|_| fail("config"))?;
            config.lossless = opts.lossless as i32;
            // Keep the colour of fully transparent pixels: lossless means exact.
            config.exact = opts.lossless as i32;
            config.method = 4;
            let mut pic = w::WebPPicture::new().map_err(|_| fail("picture"))?;
            pic.use_argb = 1;
            pic.width = img.width() as i32;
            pic.height = img.height() as i32;
            // Zeroed, not uninit: WebPMemoryWriterInit sets `mem`, `size` and
            // `max_size` but not `pad`, and `assume_init` on a struct with an
            // uninitialised field is undefined behaviour.
            let mut writer = std::mem::MaybeUninit::<w::WebPMemoryWriter>::zeroed();
            // The advanced API writes no metadata (EXIF, XMP, ICC) unless asked.
            let bytes = unsafe {
                w::WebPMemoryWriterInit(writer.as_mut_ptr());
                let mut writer = writer.assume_init();
                if w::WebPPictureImportRGBA(&mut pic, img.data().as_ptr(), pic.width * 4) == 0 {
                    w::WebPPictureFree(&mut pic);
                    return Err(fail("import"));
                }
                pic.writer = Some(w::WebPMemoryWrite);
                pic.custom_ptr = (&mut writer as *mut w::WebPMemoryWriter).cast();
                let ok = w::WebPEncode(&config, &mut pic) != 0;
                let out = ok.then(|| std::slice::from_raw_parts(writer.mem, writer.size).to_vec());
                w::WebPPictureFree(&mut pic);
                w::WebPMemoryWriterClear(&mut writer);
                out
            };
            bytes.ok_or_else(|| fail("encode"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_names_extensions_and_mime_types() {
        for f in Format::ALL {
            assert_eq!(f.to_string().parse::<Format>(), Ok(f));
            assert_eq!(f.to_string().to_lowercase().parse::<Format>(), Ok(f));
            assert_eq!(Format::from_extension(f.extension()), Some(f));
            assert_eq!(Format::from_mime(f.mime()), Some(f));
        }
        assert_eq!("jpg".parse::<Format>(), Ok(Format::Jpeg));
        assert_eq!(Format::Webp.to_string(), "WebP");
        assert_eq!(Format::from_extension(".JPEG"), Some(Format::Jpeg));
        assert_eq!(Format::from_extension("tiff"), None);
        assert_eq!(Format::from_mime("Image/WebP; q=0.9"), Some(Format::Webp));
        assert!("bmp".parse::<Format>().is_err());
    }

    #[test]
    fn encode_options_serialize_like_an_outputs_entry() {
        let png: EncodeOptions = serde_json::from_str(r#"{"format": "png"}"#).unwrap();
        assert_eq!(png, EncodeOptions::new(Format::Png));
        assert!(png.lossless);
        assert_eq!(serde_json::to_string(&png).unwrap(), r#"{"format":"png"}"#);
        let webp = EncodeOptions::new(Format::Webp).quality(80).lossless(true);
        let json = serde_json::to_string(&webp).unwrap();
        assert_eq!(json, r#"{"format":"webp","quality":80,"lossless":true}"#);
        assert_eq!(serde_json::from_str::<EncodeOptions>(&json).unwrap(), webp);
        let err =
            serde_json::from_str::<EncodeOptions>(r#"{"format": "png", "strip_metadata": true}"#)
                .unwrap_err()
                .to_string();
        assert!(err.contains("strip_metadata"), "{err}");
    }

    #[test]
    fn missing_encoder_errors_name_the_format() {
        let frames = Frames::still(Rgba8::filled(1, 1, [0; 4]));
        let err = Encoders::empty()
            .encode(&frames, &EncodeOptions::new(Format::Webp))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no lossy WebP encoder"), "{err}");
        assert_eq!(format!("{:?}", Encoders::empty()), "[]");
    }
}

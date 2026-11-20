//! Decoding untrusted bytes: pure-Rust decoders, explicit limits, EXIF
//! orientation applied, colour profiles converted to sRGB.

use crate::encode::Format;
use crate::error::{Error, Result};
#[cfg(feature = "gif")]
use crate::frame::Frame;
use crate::frame::{Frames, Rgba8};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::io::Cursor;

/// Hard size limits for untrusted images: checked by [`decode()`] before and
/// during decoding, and by [`Pipeline::validate`](crate::Pipeline::validate)
/// (through [`Ctx::limits`](crate::Ctx::limits)) for the input and every
/// step's output, so no step can grow an image past them.
///
/// Pass the same value to both to keep them consistent: anything the decoder
/// accepts can then run through a pipeline that does not enlarge it.
///
/// `#[non_exhaustive]`: start from [`Limits::default`] and assign the fields
/// you want to change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Widest image accepted, px. [`decode()`] checks the stored size and,
    /// when it applies an EXIF quarter turn, the turned size too.
    pub max_width: u32,
    /// Tallest image accepted, px (checked like `max_width`).
    pub max_height: u32,
    /// Most animation frames accepted.
    pub max_frames: u32,
    /// Summed over all frames, not per frame.
    pub max_total_pixels: u64,
    /// Bytes the decoder itself may allocate: its own buffers and the image
    /// it decodes, in the source's pixel format.
    ///
    /// Not counted are the copies [`decode()`] makes afterwards: conversion
    /// to 8-bit RGBA (none when the source already is) and then the EXIF
    /// rotation, together at most two RGBA buffers (8 bytes per pixel) at a
    /// time, bounded through [`max_total_pixels`](Self::max_total_pixels).
    /// Colour-profile conversion works in place. Decoding only: a
    /// pipeline's memory is bounded by [`MemoryBudget`](crate::MemoryBudget).
    pub max_alloc: u64,
}

impl Limits {
    /// The largest side libwebp can encode (`WEBP_MAX_DIMENSION`), and the
    /// default `max_width`/`max_height`: images within the defaults can be
    /// encoded in every built-in format (JPEG allows 65 535, PNG more).
    pub const DEFAULT_MAX_SIDE: u32 = 16_383;
}

impl Default for Limits {
    /// 16 383 × 16 383 px ([`DEFAULT_MAX_SIDE`](Self::DEFAULT_MAX_SIDE)), 500
    /// frames, 100 million pixels over all frames (400 MB of RGBA), 512 MiB
    /// of decoder allocations.
    fn default() -> Self {
        Self {
            max_width: Self::DEFAULT_MAX_SIDE,
            max_height: Self::DEFAULT_MAX_SIDE,
            max_frames: 500,
            max_total_pixels: 100_000_000,
            max_alloc: 512 * 1024 * 1024,
        }
    }
}

/// How to decode; the default applies [`Limits::default`] and the EXIF
/// orientation. `#[non_exhaustive]`: start from [`DecodeOptions::default`]
/// and use the setters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// Size and memory limits.
    pub limits: Limits,
    /// Rotate/flip by the EXIF orientation tag (the tag itself is dropped).
    pub apply_orientation: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            apply_orientation: true,
        }
    }
}

impl DecodeOptions {
    /// Sets [`limits`](Self::limits).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Sets [`apply_orientation`](Self::apply_orientation).
    pub fn with_apply_orientation(mut self, apply: bool) -> Self {
        self.apply_orientation = apply;
        self
    }
}

/// Detected container format of `bytes`, if it is one this build decodes.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    Some(match sniff_image(bytes)? {
        ImageFormat::Png => Format::Png,
        ImageFormat::Jpeg => Format::Jpeg,
        ImageFormat::WebP => Format::Webp,
        ImageFormat::Gif => Format::Gif,
        ImageFormat::Avif => Format::Avif,
        _ => return None,
    })
}

/// `image`'s view of the format, for the decoders; never public, so
/// imgpipe's API doesn't depend on the `image` crate's version.
fn sniff_image(bytes: &[u8]) -> Option<ImageFormat> {
    let format = image::guess_format(bytes).ok()?;
    format.reading_enabled().then_some(format)
}

/// Decodes a still image or an animation (GIF, when the `gif` feature is on).
///
/// GIF frame delays of 10 ms or less become **100 ms**, as browsers show
/// them ([`Frame::delay_ms`](crate::Frame::delay_ms) has the result).
pub fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Frames> {
    let format = sniff_image(bytes)
        .ok_or_else(|| Error::Decode("unknown or unsupported image format".into()))?;
    #[cfg(feature = "gif")]
    if format == ImageFormat::Gif {
        return decode_gif(bytes, opts);
    }
    decode_still(bytes, format, opts)
}

fn image_limits(l: &Limits) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(l.max_width);
    limits.max_image_height = Some(l.max_height);
    limits.max_alloc = Some(l.max_alloc);
    limits
}

fn check_size(w: u32, h: u32, frames: u64, l: &Limits) -> Result<()> {
    if w == 0 || h == 0 {
        return Err(Error::Decode("image has no pixels".into()));
    }
    if w > l.max_width || h > l.max_height {
        return Err(Error::Limit(format!(
            "{w}x{h} exceeds {}x{}",
            l.max_width, l.max_height
        )));
    }
    let total = (w as u64 * h as u64).saturating_mul(frames);
    if total > l.max_total_pixels {
        return Err(Error::Limit(format!(
            "{total} pixels exceed the limit of {}",
            l.max_total_pixels
        )));
    }
    Ok(())
}

fn decode_still(bytes: &[u8], format: ImageFormat, opts: &DecodeOptions) -> Result<Frames> {
    use image::metadata::Orientation;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(image_limits(&opts.limits));
    let mut decoder = reader.into_decoder().map_err(decode_err)?;

    // Dimensions come from the header, before any pixel is allocated.
    let (w, h) = decoder.dimensions();
    check_size(w, h, 1, &opts.limits)?;

    let orientation = if opts.apply_orientation {
        decoder.orientation().ok()
    } else {
        None
    };
    // The image that comes out is turned by the orientation: with a quarter
    // turn and asymmetric limits (e.g. 4000 x 2000), check that too.
    let turned = matches!(
        orientation,
        Some(
            Orientation::Rotate90
                | Orientation::Rotate270
                | Orientation::Rotate90FlipH
                | Orientation::Rotate270FlipH
        )
    );
    if turned {
        check_size(h, w, 1, &opts.limits)?;
    }
    let icc = decoder.icc_profile().ok().flatten();
    let image = DynamicImage::from_decoder(decoder).map_err(decode_err)?;
    // 16-bit and float sources become 8-bit here, before the orientation, so
    // a rotation copies 4 bytes per pixel, not up to 16.
    let mut image = DynamicImage::ImageRgba8(image.into_rgba8());
    if let Some(o) = orientation {
        image.apply_orientation(o);
    }
    let mut rgba = image.into_rgba8();
    if let Some(icc) = icc {
        to_srgb(&icc, rgba.as_mut());
    }
    let (w, h) = rgba.dimensions();
    Ok(Frames::still(
        Rgba8::from_raw(w, h, rgba.into_raw()).ok_or_else(sized)?,
    ))
}

/// Pixels converted per [`to_srgb`] chunk: 16 KiB of RGBA scratch.
const PROFILE_CHUNK: usize = 4096;

/// Converts RGBA pixels described by an ICC profile to sRGB in place, in
/// chunks (no copy of the image). RGB profiles convert colour; grey
/// profiles convert the grey level (taken from the red channel, which
/// equals the others in a decoded grey image) to sRGB, keeping alpha.
///
/// Any other profile (e.g. CMYK, whose pixels the JPEG decoder has already
/// converted naively), and any profile moxcms cannot read or build a
/// transform from (truncated, missing tags, an unsupported connection
/// space), leaves the pixels as they are: they are treated as sRGB, as
/// browsers treat a broken profile, rather than making the image
/// undecodable.
fn to_srgb(icc: &[u8], rgba: &mut [u8]) {
    use moxcms::{ColorProfile, DataColorSpace, Layout, TransformOptions};
    let Ok(src) = ColorProfile::new_from_slice(icc) else {
        return;
    };
    let dst = ColorProfile::new_srgb();
    let options = TransformOptions::default();
    match src.color_space {
        DataColorSpace::Rgb => {
            let Ok(t) = src.create_transform_8bit(Layout::Rgba, &dst, Layout::Rgba, options) else {
                return;
            };
            let mut scratch = [0u8; PROFILE_CHUNK * 4];
            for chunk in rgba.chunks_mut(PROFILE_CHUNK * 4) {
                let src = &mut scratch[..chunk.len()];
                src.copy_from_slice(chunk);
                if t.transform(src, chunk).is_err() {
                    // Only on a length mismatch, which the chunking rules out.
                    chunk.copy_from_slice(src);
                }
            }
        }
        DataColorSpace::Gray => {
            let Ok(t) = src.create_transform_8bit(Layout::Gray, &dst, Layout::Rgb, options) else {
                return;
            };
            let mut grey = [0u8; PROFILE_CHUNK];
            let mut rgb = [0u8; PROFILE_CHUNK * 3];
            for chunk in rgba.chunks_mut(PROFILE_CHUNK * 4) {
                let px = chunk.as_chunks_mut::<4>().0;
                let (grey, rgb) = (&mut grey[..px.len()], &mut rgb[..px.len() * 3]);
                for (g, p) in grey.iter_mut().zip(px.iter()) {
                    *g = p[0];
                }
                if t.transform(grey, rgb).is_ok() {
                    for (p, c) in px.iter_mut().zip(rgb.as_chunks::<3>().0) {
                        p[..3].copy_from_slice(c);
                    }
                }
            }
        }
        _ => {}
    }
}

#[cfg(feature = "gif")]
fn decode_gif(bytes: &[u8], opts: &DecodeOptions) -> Result<Frames> {
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;

    let mut decoder = GifDecoder::new(Cursor::new(bytes)).map_err(decode_err)?;
    decoder
        .set_limits(image_limits(&opts.limits))
        .map_err(decode_err)?;
    let (w, h) = decoder.dimensions();
    check_size(w, h, 1, &opts.limits)?;

    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.map_err(decode_err)?;
        if frames.len() as u32 >= opts.limits.max_frames {
            return Err(Error::Limit(format!(
                "more than {} frames",
                opts.limits.max_frames
            )));
        }
        check_size(w, h, frames.len() as u64 + 1, &opts.limits)?;
        let (num, den) = frame.delay().numer_denom_ms();
        let ms = num.checked_div(den).unwrap_or(0);
        // Browsers show delays of 10 ms or less as 100 ms.
        let delay_ms = if ms <= 10 { 100 } else { ms };
        let buf = frame.into_buffer();
        let (fw, fh) = buf.dimensions();
        frames.push(Frame {
            image: Rgba8::from_raw(fw, fh, buf.into_raw()).ok_or_else(sized)?,
            delay_ms,
        });
    }
    if frames.is_empty() {
        return Err(Error::Decode("GIF has no frames".into()));
    }
    // `image` composes every frame onto the logical screen, so they share a
    // size; a decoder that ever broke that would be an error, not a panic.
    Frames::new(frames, None).map_err(|e| Error::Decode(e.to_string()))
}

/// The decoder returned a buffer that does not match its dimensions.
fn sized() -> Error {
    Error::Decode("decoder returned a wrong-sized buffer".into())
}

fn decode_err(e: image::ImageError) -> Error {
    match e {
        image::ImageError::Limits(l) => Error::Limit(l.to_string()),
        other => Error::Decode(other.to_string()),
    }
}

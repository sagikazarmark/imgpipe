//! sRGB ↔ linear light, premultiplied alpha, and colour values.
//!
//! Filtering steps convert to 16-bit linear light with premultiplied alpha,
//! work there and convert back: filtering sRGB values darkens images, and
//! filtering straight alpha pulls colour out of transparent pixels.
//!
//! The transfer functions use the `libm` crate, not `f32::powf`: std's
//! `powf` calls the platform's libm natively (glibc, Apple's libm, ...) and
//! compiler-builtins on wasm, and those differ in the last bit, which moved
//! some 8-bit results by one (`linear16_to_srgb8(43837)` was 213 natively and
//! 214 in wasm). With `libm` the conversion tables are identical on every
//! target, so the browser and a server produce the same pixels.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::OnceLock;

/// sRGB decoding in f64, for the tables: exact enough that rounding to 16
/// (or 8) bits is never decided by the last bit of the arithmetic.
fn srgb_to_linear_f64(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        libm::pow((v + 0.055) / 1.055, 2.4)
    }
}

/// sRGB encoding in f64, for the tables.
fn linear_to_srgb_f64(v: f64) -> f64 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * libm::pow(v, 1.0 / 2.4) - 0.055
    }
}

/// sRGB-encoded u8 → linear u16.
fn to_linear_table() -> &'static [u16; 256] {
    static TABLE: OnceLock<[u16; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0u16; 256];
        for (i, v) in t.iter_mut().enumerate() {
            *v = (srgb_to_linear_f64(i as f64 / 255.0) * 65535.0).round() as u16;
        }
        t
    })
}

/// Linear u16 → sRGB-encoded u8 (64 KiB, built on first use).
fn to_srgb_table() -> &'static [u8] {
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=u16::MAX)
            .map(|i| (linear_to_srgb_f64(i as f64 / 65535.0) * 255.0).round() as u8)
            .collect()
    })
}

/// An 8-bit sRGB value in 16-bit linear light (0..=65535), as the
/// filtering steps convert it. The same on every target.
pub fn srgb8_to_linear16(v: u8) -> u16 {
    to_linear_table()[v as usize]
}

/// A 16-bit linear-light value as 8-bit sRGB, as the filtering steps convert
/// it back. The same on every target.
pub fn linear16_to_srgb8(v: u16) -> u8 {
    to_srgb_table()[v as usize]
}

/// sRGB transfer function, decoding: encoded 0..1 → linear light 0..1.
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        libm::powf((v + 0.055) / 1.055, 2.4)
    }
}

/// sRGB transfer function, encoding: linear light 0..1 → encoded 0..1.
pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * libm::powf(v, 1.0 / 2.4) - 0.055
    }
}
/// One RGBA8 sRGB straight pixel → RGBA16 linear premultiplied.
#[inline]
fn premul_px(lin: &[u16; 256], p: &[u8; 4]) -> [u16; 4] {
    let a = p[3] as u32;
    let c = |v: u8| ((lin[v as usize] as u32 * a + 127) / 255) as u16;
    [c(p[0]), c(p[1]), c(p[2]), (a * 257) as u16]
}

/// One RGBA16 linear premultiplied pixel → RGBA8 sRGB straight.
#[inline]
fn unpremul_px(srgb: &[u8], p: [u16; 4]) -> [u8; 4] {
    let a = p[3] as u32;
    if a == 0 {
        return [0; 4];
    }
    let c = |v: u16| srgb[((v as u32 * 65535 + a / 2) / a).min(65535) as usize];
    [c(p[0]), c(p[1]), c(p[2]), ((a + 128) / 257) as u8]
}

/// RGBA8 sRGB straight → RGBA16 linear premultiplied (alpha scaled to 16 bits).
pub(crate) fn to_linear_premul(rgba: &[u8]) -> Vec<u16> {
    let lin = to_linear_table();
    let mut out = Vec::with_capacity(rgba.len());
    for p in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&premul_px(lin, p));
    }
    out
}

/// [`to_linear_premul`] as native-endian bytes, the layout
/// `fast_image_resize` takes for 16-bit images, without a `u16` copy first.
pub(crate) fn to_linear_premul_ne_bytes(rgba: &[u8]) -> Vec<u8> {
    let lin = to_linear_table();
    let mut out = Vec::with_capacity(rgba.len() * 2);
    for p in rgba.as_chunks::<4>().0 {
        for v in premul_px(lin, p) {
            out.extend_from_slice(&v.to_ne_bytes());
        }
    }
    out
}

/// RGBA16 linear premultiplied → RGBA8 sRGB straight.
pub(crate) fn from_linear_premul(rgba16: &[u16], out: &mut [u8]) {
    let srgb = to_srgb_table();
    for (p, o) in rgba16
        .as_chunks::<4>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<4>().0.iter_mut())
    {
        *o = unpremul_px(srgb, *p);
    }
}

/// [`from_linear_premul`] from native-endian bytes.
pub(crate) fn from_linear_premul_ne_bytes(bytes: &[u8], out: &mut [u8]) {
    let srgb = to_srgb_table();
    for (p, o) in bytes
        .as_chunks::<8>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<4>().0.iter_mut())
    {
        let v = |i: usize| u16::from_ne_bytes([p[2 * i], p[2 * i + 1]]);
        *o = unpremul_px(srgb, [v(0), v(1), v(2), v(3)]);
    }
}

/// An sRGB colour with straight alpha, written `#rrggbb` or `#rrggbbaa`.
///
/// Parse one with [`Color::parse`] or [`str::parse`]; `From<[u8; 4]>` and
/// `From<Color> for [u8; 4]` convert to and from RGBA bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub [u8; 4]);

impl Color {
    /// Opaque white.
    pub const WHITE: Color = Color([255, 255, 255, 255]);
    /// Opaque black.
    pub const BLACK: Color = Color([0, 0, 0, 255]);
    /// Fully transparent.
    pub const TRANSPARENT: Color = Color([0, 0, 0, 0]);

    /// Parses `#rrggbb` or `#rrggbbaa`: exactly 6 or 8 hex digits, either
    /// case, after the `#`.
    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.strip_prefix('#')?.as_bytes();
        if !matches!(hex.len(), 6 | 8) || !hex.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let digit = |c: u8| match c {
            b'0'..=b'9' => c - b'0',
            _ => (c | 0x20) - b'a' + 10,
        };
        let byte = |i: usize| (digit(hex[i]) << 4) | digit(hex[i + 1]);
        let alpha = if hex.len() == 8 { byte(6) } else { 255 };
        Some(Color([byte(0), byte(2), byte(4), alpha]))
    }
}

/// [`Color`]'s `FromStr` error: the text is not `#rrggbb` or `#rrggbbaa`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid colour {0:?}; use #rrggbb or #rrggbbaa")]
pub struct ParseColorError(String);

impl std::str::FromStr for Color {
    type Err = ParseColorError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Color::parse(s).ok_or_else(|| ParseColorError(s.to_owned()))
    }
}

impl From<[u8; 4]> for Color {
    fn from(rgba: [u8; 4]) -> Self {
        Color(rgba)
    }
}

impl From<Color> for [u8; 4] {
    fn from(c: Color) -> Self {
        c.0
    }
}

impl std::fmt::Display for Color {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [r, g, b, a] = self.0;
        if a == 255 {
            write!(f, "#{r:02x}{g:02x}{b:02x}")
        } else {
            write!(f, "#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for Color {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Color".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "pattern": "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_round_trip_is_lossless_for_opaque_pixels() {
        let src: Vec<u8> = (0..=255u8).flat_map(|v| [v, 255 - v, v / 2, 255]).collect();
        let lin = to_linear_premul(&src);
        let mut back = vec![0; src.len()];
        from_linear_premul(&lin, &mut back);
        assert_eq!(src, back);
    }

    #[test]
    fn transparent_pixels_become_zero() {
        let lin = to_linear_premul(&[200, 100, 50, 0]);
        assert_eq!(&lin[..3], &[0, 0, 0]);
        let mut back = [9; 4];
        from_linear_premul(&lin, &mut back);
        assert_eq!(back, [0, 0, 0, 0]);
    }

    #[test]
    fn colour_parsing() {
        assert_eq!(Color::parse("#ff8000"), Some(Color([255, 128, 0, 255])));
        assert_eq!(Color::parse("#ff800080"), Some(Color([255, 128, 0, 128])));
        assert_eq!(Color::parse("ff8000"), None);
        assert_eq!(Color::parse("#ff80"), None);
        assert_eq!(Color([1, 2, 3, 255]).to_string(), "#010203");
        assert_eq!(
            Color::parse("#AbCdEf"),
            Some(Color([0xab, 0xcd, 0xef, 255]))
        );
        // Only hex digits: `from_str_radix` alone accepted a sign.
        for bad in [
            "#+f+f+f",
            "#-1-1-1",
            "#ff80 0",
            "#ff800g",
            "#ff8000ff00",
            "#é1234",
        ] {
            assert_eq!(Color::parse(bad), None, "{bad:?}");
        }
        assert_eq!("#ff8000".parse::<Color>(), Ok(Color([255, 128, 0, 255])));
        let err = "red".parse::<Color>().unwrap_err().to_string();
        assert!(err.contains("#rrggbb"), "{err}");
        assert_eq!(Color::from([1, 2, 3, 4]), Color([1, 2, 3, 4]));
        assert_eq!(<[u8; 4]>::from(Color::WHITE), [255; 4]);
    }
}

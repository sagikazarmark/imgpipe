//! Step parameters and dispatch. Each step is a plain function over frames.
//!
//! The parameter structs mirror the pipeline file format and have public
//! fields (see the crate docs on stability). Those with a [`Default`] match
//! an empty TOML table, so `..Default::default()` fills in exactly what a
//! file would leave out.
//!
//! Defaults worth knowing: [`Resize::background`] and [`Pad::color`] are
//! opaque **white**, and `resize` filters in linear light.

mod adjust;
mod geometry;
mod resize;
mod sharpen;

use crate::color::Color;
use crate::error::{Diagnostic, StepError};
use crate::frame::{FrameInfo, Frames};
use crate::overlay::TextStep;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One pipeline step. Steps run exactly in the order written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "op", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Step {
    /// `op = "resize"`: see [`Resize`].
    Resize(Resize),
    /// `op = "crop"`: see [`Crop`].
    Crop(Crop),
    /// `op = "pad"`: see [`Pad`].
    Pad(Pad),
    /// `op = "trim"`: see [`Trim`].
    Trim(Trim),
    /// `op = "rotate"`: see [`Rotate`].
    Rotate(Rotate),
    /// `op = "flip"`: see [`Flip`].
    Flip(Flip),
    /// `op = "adjust"`: see [`Adjust`].
    Adjust(Adjust),
    /// `op = "sharpen"`: see [`Sharpen`].
    Sharpen(Sharpen),
    /// `op = "text"`: draws text boxes, see [`TextStep`]. Fonts come from
    /// the [`Ctx`](crate::Ctx). Parses in every build; running it needs the
    /// `overlay` feature, and validation rejects it without.
    Text(TextStep),
}

/// A length: pixels or a percentage of the image's width or height.
///
/// Written as an integer for pixels (`40`) or a string ending in `%` for a
/// percentage (`"25%"`, `"12.5%"`), in TOML and JSON alike. A bare float such
/// as `0.25` is rejected: JSON writers (JavaScript's among them) print `1.0`
/// as `1`, so a float can't reliably tell a fraction from a pixel count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    /// Pixels, e.g. `40`.
    Px(u32),
    /// A percentage 0–100 of the dimension, e.g. `"25%"` is `Percent(25.0)`.
    Percent(f32),
}

impl Length {
    pub(crate) fn resolve(self, total: u32) -> u32 {
        match self {
            Length::Px(px) => px,
            Length::Percent(p) => (p as f64 * total as f64 / 100.0).round().max(0.0) as u32,
        }
    }

    fn check(self, name: &str) -> Result<(), String> {
        match self {
            Length::Percent(p) if !p.is_finite() || !(0.0..=100.0).contains(&p) => Err(format!(
                "{name} must be a pixel count or a percentage from 0% to 100%"
            )),
            _ => Ok(()),
        }
    }
}

impl Default for Length {
    fn default() -> Self {
        Length::Px(0)
    }
}

impl std::fmt::Display for Length {
    /// `40` or `25%`, as written in a pipeline file.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Length::Px(px) => write!(f, "{px}"),
            Length::Percent(p) => write!(f, "{p}%"),
        }
    }
}

impl Serialize for Length {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Length::Px(px) => s.serialize_u32(*px),
            Length::Percent(_) => s.collect_str(self),
        }
    }
}

/// `"12.5%"` → 12.5: digits, optionally a dot and more digits, then `%`.
fn parse_percent(s: &str) -> Option<f32> {
    let number = s.strip_suffix('%')?;
    let (int, frac) = number.split_once('.').unwrap_or((number, "0"));
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || !digits(frac) {
        return None;
    }
    number.parse().ok()
}

impl<'de> Deserialize<'de> for Length {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{Error, Unexpected, Visitor};
        struct LengthVisitor;
        impl Visitor<'_> for LengthVisitor {
            type Value = Length;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(r#"a pixel count (40) or a percentage string ("25%")"#)
            }
            fn visit_u64<E: Error>(self, v: u64) -> Result<Length, E> {
                u32::try_from(v)
                    .map(Length::Px)
                    .map_err(|_| E::invalid_value(Unexpected::Unsigned(v), &self))
            }
            fn visit_i64<E: Error>(self, v: i64) -> Result<Length, E> {
                u32::try_from(v)
                    .map(Length::Px)
                    .map_err(|_| E::invalid_value(Unexpected::Signed(v), &self))
            }
            fn visit_f64<E: Error>(self, v: f64) -> Result<Length, E> {
                Err(E::custom(format_args!(
                    "invalid length {v}: write a pixel count as an integer (40) or a fraction \
                     as a percentage string (\"{}%\")",
                    (v * 1e6).round() / 1e4
                )))
            }
            fn visit_str<E: Error>(self, v: &str) -> Result<Length, E> {
                parse_percent(v)
                    .map(Length::Percent)
                    .ok_or_else(|| E::invalid_value(Unexpected::Str(v), &self))
            }
        }
        d.deserialize_any(LengthVisitor)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for Length {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Length".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Pixels (an integer) or a percentage of the dimension (a string such as \"25%\").",
            "oneOf": [
                { "type": "integer", "minimum": 0, "maximum": u32::MAX },
                { "type": "string", "pattern": "^[0-9]+(\\.[0-9]+)?%$" }
            ]
        })
    }
}

/// How [`Resize`] fits the image into a `width` × `height` box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Fit {
    /// Fit inside the box, keeping the aspect ratio.
    #[default]
    Contain,
    /// Fill the box, keeping the aspect ratio; the overflow is cropped (centred).
    Cover,
    /// Stretch to exactly the box.
    Exact,
    /// Fit inside the box, then centre on a `background` canvas of exactly the box.
    Pad,
}

/// Resampling filter for [`Resize`], sharpest first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Filter {
    /// Lanczos, 3 lobes (the default): sharp, slight ringing.
    #[default]
    Lanczos3,
    /// Catmull-Rom cubic: sharp, less ringing.
    CatmullRom,
    /// Mitchell-Netravali cubic: softer, no visible ringing.
    Mitchell,
    /// Hamming-windowed sinc.
    Hamming,
    /// Bilinear (triangle).
    Bilinear,
    /// Box: averages the source pixels each output pixel covers.
    Box,
}

fn yes() -> bool {
    true
}

/// Scale the image. `width`/`height` give a box; `max_long_side` caps the
/// longer side. Proportional modes never upscale unless `allow_upscale`;
/// `cover`, `exact` and `pad` always produce exactly `width` × `height`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Resize {
    /// Optional step id, used in plans and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Box width, px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Box height, px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Cap on the longer side, px (instead of `width`/`height`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_long_side: Option<u32>,
    /// How to fit the box when both `width` and `height` are set.
    #[serde(default)]
    pub fit: Fit,
    /// Resampling filter.
    #[serde(default)]
    pub filter: Filter,
    /// Filter in linear light (true, the default) or on sRGB values (false).
    #[serde(default = "yes")]
    pub linear: bool,
    /// Let proportional modes make the image larger.
    #[serde(default)]
    pub allow_upscale: bool,
    /// Canvas colour for `fit = "pad"`. Defaults to opaque **white**; use
    /// `"#00000000"` for a transparent canvas.
    #[serde(default = "white")]
    pub background: Color,
}

/// Same defaults as an empty TOML table (`linear` on, white background).
impl Default for Resize {
    fn default() -> Self {
        Self {
            id: None,
            width: None,
            height: None,
            max_long_side: None,
            fit: Fit::default(),
            filter: Filter::default(),
            linear: true,
            allow_upscale: false,
            background: Color::WHITE,
        }
    }
}

/// Keep only `rect` (x, y, w, h), each in pixels or a percentage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Crop {
    /// Optional step id, used in plans and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// x, y, w, h; percentages are of the width (x, w) or height (y, h).
    pub rect: [Length; 4],
}

impl Crop {
    /// A crop to `rect` (x, y, w, h) without an id.
    pub fn new(rect: [Length; 4]) -> Self {
        Self { id: None, rect }
    }
}

fn white() -> Color {
    Color::WHITE
}

/// Add borders. Percentages are of the height (top, bottom) or width (left,
/// right).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Pad {
    /// Optional step id, used in plans and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Border above.
    #[serde(default)]
    pub top: Length,
    /// Border on the right.
    #[serde(default)]
    pub right: Length,
    /// Border below.
    #[serde(default)]
    pub bottom: Length,
    /// Border on the left.
    #[serde(default)]
    pub left: Length,
    /// Border colour. Defaults to opaque **white**.
    #[serde(default = "white")]
    pub color: Color,
}

/// No borders, white colour: an empty TOML table.
impl Default for Pad {
    fn default() -> Self {
        Self {
            id: None,
            top: Length::default(),
            right: Length::default(),
            bottom: Length::default(),
            left: Length::default(),
            color: Color::WHITE,
        }
    }
}

fn default_trim_threshold() -> u8 {
    10
}

/// Remove uniform borders. On animations, one rectangle for all frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Trim {
    /// Optional step id, used in plans and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Largest per-channel difference from the corner colour that still
    /// counts as border (default 10).
    #[serde(default = "default_trim_threshold")]
    pub threshold: u8,
    /// Border pixels to keep around the content.
    #[serde(default)]
    pub padding: u32,
}

/// Same defaults as an empty TOML table (threshold 10, no padding).
impl Default for Trim {
    fn default() -> Self {
        Self {
            id: None,
            threshold: default_trim_threshold(),
            padding: 0,
        }
    }
}

/// Rotate clockwise by a multiple of 90 degrees.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Rotate {
    /// Optional step id, used in plans and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// A multiple of 90; negative turns counter-clockwise.
    pub degrees: i32,
}

impl Rotate {
    /// A rotation by `degrees` without an id.
    pub fn new(degrees: i32) -> Self {
        Self { id: None, degrees }
    }
}

/// Which way [`Flip`] mirrors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Axis {
    /// Mirror left to right.
    Horizontal,
    /// Mirror top to bottom.
    Vertical,
}

/// Mirror the image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Flip {
    /// Optional step id, used in plans and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Mirror direction.
    pub axis: Axis,
}

impl Flip {
    /// A flip along `axis` without an id.
    pub fn new(axis: Axis) -> Self {
        Self { id: None, axis }
    }
}

fn one() -> f32 {
    1.0
}

/// Colour adjustments on sRGB values, like the CSS filters of the same name.
/// All default to 1 (no change); applied in this order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Adjust {
    /// Optional step id, used in plans and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Multiplies every channel; 0 is black.
    #[serde(default = "one")]
    pub brightness: f32,
    /// Scales the distance from mid-grey; 0 is flat grey.
    #[serde(default = "one")]
    pub contrast: f32,
    /// Scales the distance from the pixel's luma; 0 is greyscale.
    #[serde(default = "one")]
    pub saturation: f32,
    /// Values above 1 brighten mid-tones.
    #[serde(default = "one")]
    pub gamma: f32,
}

/// No change: every factor 1, as an empty TOML table.
impl Default for Adjust {
    fn default() -> Self {
        Self {
            id: None,
            brightness: 1.0,
            contrast: 1.0,
            saturation: 1.0,
            gamma: 1.0,
        }
    }
}

fn default_sigma() -> f32 {
    1.0
}
fn default_amount() -> f32 {
    0.5
}

/// Unsharp mask in linear light.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Sharpen {
    /// Optional step id, used in plans and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Blur radius (standard deviation) in pixels (default 1).
    #[serde(default = "default_sigma")]
    pub sigma: f32,
    /// How much of the difference from the blur is added back (default 0.5).
    #[serde(default = "default_amount")]
    pub amount: f32,
    /// Differences below this (in 8-bit units) are left alone.
    #[serde(default)]
    pub threshold: u8,
}

/// Same defaults as an empty TOML table (sigma 1, amount 0.5, threshold 0).
impl Default for Sharpen {
    fn default() -> Self {
        Self {
            id: None,
            sigma: default_sigma(),
            amount: default_amount(),
            threshold: 0,
        }
    }
}

/// What a step will do to the frame size, and the memory it needs.
pub(crate) struct StepEffect {
    pub output: FrameInfo,
    /// Working memory beyond the input and output frames.
    pub scratch_bytes: u64,
}

impl Step {
    /// The `op` name, as written in pipeline files.
    pub fn op(&self) -> &'static str {
        match self {
            Step::Resize(_) => "resize",
            Step::Crop(_) => "crop",
            Step::Pad(_) => "pad",
            Step::Trim(_) => "trim",
            Step::Rotate(_) => "rotate",
            Step::Flip(_) => "flip",
            Step::Adjust(_) => "adjust",
            Step::Sharpen(_) => "sharpen",
            Step::Text(_) => "text",
        }
    }

    /// The step's optional `id`.
    pub fn id(&self) -> Option<&str> {
        match self {
            Step::Resize(s) => s.id.as_deref(),
            Step::Crop(s) => s.id.as_deref(),
            Step::Pad(s) => s.id.as_deref(),
            Step::Trim(s) => s.id.as_deref(),
            Step::Rotate(s) => s.id.as_deref(),
            Step::Flip(s) => s.id.as_deref(),
            Step::Adjust(s) => s.id.as_deref(),
            Step::Sharpen(s) => s.id.as_deref(),
            Step::Text(s) => s.id.as_deref(),
        }
    }

    /// Checks parameters and predicts the output size. Trim depends on the
    /// pixels, so its prediction is the input size (an upper bound).
    pub(crate) fn effect(&self, index: usize, input: FrameInfo) -> Result<StepEffect, Diagnostic> {
        let err = |m: String| Diagnostic::step(index, self.op(), m);
        let px = input.width as u64 * input.height as u64;
        let same = |scratch_bytes| StepEffect {
            output: input,
            scratch_bytes,
        };
        match self {
            Step::Resize(p) => {
                let plan = resize::plan(p, input.width, input.height).map_err(err)?;
                let (width, height) = plan.canvas.unwrap_or((plan.width, plan.height));
                let out = FrameInfo {
                    width,
                    height,
                    ..input
                };
                let scratch = resize::scratch_bytes(p, &plan, input.width, input.height);
                Ok(StepEffect {
                    output: out,
                    scratch_bytes: scratch,
                })
            }
            Step::Crop(p) => {
                let (_, _, w, h) =
                    geometry::crop_rect(p, input.width, input.height).map_err(err)?;
                Ok(StepEffect {
                    output: FrameInfo {
                        width: w,
                        height: h,
                        ..input
                    },
                    scratch_bytes: 0,
                })
            }
            Step::Pad(p) => {
                for (name, l) in [
                    ("top", p.top),
                    ("right", p.right),
                    ("bottom", p.bottom),
                    ("left", p.left),
                ] {
                    l.check(name).map_err(err)?;
                }
                let (w, h, _, _) = geometry::pad_size(p, input.width, input.height).map_err(err)?;
                Ok(StepEffect {
                    output: FrameInfo {
                        width: w,
                        height: h,
                        ..input
                    },
                    scratch_bytes: 0,
                })
            }
            Step::Trim(_) => Ok(same(0)),
            Step::Rotate(p) => {
                let quarter = geometry::quarter_turns(p.degrees).map_err(err)?;
                let (w, h) = if quarter % 2 == 1 {
                    (input.height, input.width)
                } else {
                    (input.width, input.height)
                };
                Ok(StepEffect {
                    output: FrameInfo {
                        width: w,
                        height: h,
                        ..input
                    },
                    scratch_bytes: 0,
                })
            }
            Step::Flip(_) => Ok(same(0)),
            Step::Adjust(p) => {
                for (name, v) in [
                    ("brightness", p.brightness),
                    ("contrast", p.contrast),
                    ("saturation", p.saturation),
                ] {
                    if !v.is_finite() || v < 0.0 {
                        return Err(err(format!("{name} must be a finite number ≥ 0")));
                    }
                }
                if !p.gamma.is_finite() || p.gamma <= 0.0 {
                    return Err(err("gamma must be a finite number > 0".into()));
                }
                Ok(same(0))
            }
            Step::Sharpen(p) => {
                if !p.sigma.is_finite() || p.sigma <= 0.0 || p.sigma > 50.0 {
                    return Err(err("sigma must be between 0 and 50".into()));
                }
                if !p.amount.is_finite() || p.amount < 0.0 {
                    return Err(err("amount must be a finite number ≥ 0".into()));
                }
                // Linear copy, blurred copy and one blur pass buffer of one frame.
                Ok(same(px.saturating_mul(8 * 3)))
            }
            Step::Text(p) => {
                if !cfg!(feature = "overlay") {
                    return Err(err(
                        "text steps need imgpipe's `overlay` feature, which this build does not have"
                            .into(),
                    ));
                }
                for (i, b) in p.boxes.iter().enumerate() {
                    b.check().map_err(|m| err(format!("box {i}: {m}")))?;
                }
                // One box at a time is drawn into a layer of at most the frame.
                Ok(same(px.saturating_mul(4)))
            }
        }
    }

    /// Runs the step. Parameters were checked by `effect`.
    pub(crate) fn apply(&self, frames: Frames, cx: &crate::Ctx<'_>) -> Result<Frames, StepError> {
        let _ = cx;
        let result = match self {
            Step::Resize(p) => resize::apply(p, frames),
            Step::Crop(p) => geometry::crop(p, frames),
            Step::Pad(p) => geometry::pad(p, frames),
            Step::Trim(p) => geometry::trim(p, frames),
            Step::Rotate(p) => geometry::rotate(p, frames),
            Step::Flip(p) => geometry::flip(p, frames),
            Step::Adjust(p) => adjust::apply(p, frames),
            Step::Sharpen(p) => sharpen::apply(p, frames),
            #[cfg(feature = "overlay")]
            Step::Text(p) => return text(p, frames, cx),
            // Validation rejects text steps without the feature.
            #[cfg(not(feature = "overlay"))]
            Step::Text(_) => return Err(StepError::Other("no `overlay` feature".into())),
        };
        result.map_err(StepError::Other)
    }
}

#[cfg(feature = "overlay")]
fn text(p: &TextStep, frames: Frames, cx: &crate::Ctx<'_>) -> Result<Frames, StepError> {
    use crate::overlay::{TextRenderer, draw_onto, interpolate};
    let fonts = cx.fonts.ok_or(StepError::NoFonts)?;
    let info = frames.info();
    let aspect = info.width as f32 / info.height as f32;
    let mut renderer = TextRenderer::new();
    let mut boxes = Vec::with_capacity(p.boxes.len());
    let no_vars = std::collections::HashMap::<String, String>::new();
    let vars: &dyn crate::overlay::Vars = cx.vars.unwrap_or(&no_vars);
    for b in &p.boxes {
        let mut b = b.clone();
        // Always, so `{{` and `}}` mean the same with or without vars.
        b.text = interpolate(&b.text, vars);
        let layout = renderer.layout(&b, fonts, aspect)?;
        boxes.push((b, layout));
    }
    // Layout is per frame size, which all frames share: compute once, draw on each.
    frames.map_images(|mut img| {
        draw_onto(
            &mut img,
            boxes.iter().map(|(b, l)| (b, l)),
            fonts,
            &mut renderer,
        )?;
        Ok(img)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_integers_or_percent_strings() {
        let parse = |json: &str| serde_json::from_str::<[Length; 4]>(json);
        assert_eq!(
            parse(r#"[0, 10, "50%", "12.5%"]"#).unwrap(),
            [
                Length::Px(0),
                Length::Px(10),
                Length::Percent(50.0),
                Length::Percent(12.5)
            ]
        );
        // What JavaScript writes for a full-image crop is unambiguous now.
        assert_eq!(
            parse(r#"[0, 0, "100%", "100%"]"#).unwrap()[2],
            Length::Percent(100.0)
        );
        let err = parse("[0, 0, 0.5, 1]").unwrap_err().to_string();
        assert!(err.contains("\"50%\""), "{err}");
        let err = parse("[0, 0, 0.07, 1]").unwrap_err().to_string();
        assert!(err.contains("\"7%\""), "{err}");
        for bad in [
            r#"[0, 0, "50", 1]"#,
            r#"[0, 0, "-5%", 1]"#,
            r#"[0, 0, "1e2%", 1]"#,
            r#"[0, 0, ".5%", 1]"#,
            r#"[0, 0, "nan%", 1]"#,
            "[0, 0, -1, 1]",
            "[0, 0, 4294967296, 1]",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        // TOML: integers and strings, floats rejected with a hint.
        #[derive(Deserialize)]
        struct T {
            #[allow(dead_code)]
            l: Length,
        }
        assert!(toml::from_str::<T>("l = \"25%\"").is_ok());
        let err = toml::from_str::<T>("l = 0.25").err().unwrap().to_string();
        assert!(err.contains("\"25%\""), "{err}");
        // Round trip.
        let ls = [Length::Px(3), Length::Percent(33.3)];
        let json = serde_json::to_string(&ls).unwrap();
        assert_eq!(json, r#"[3,"33.3%"]"#);
        assert_eq!(serde_json::from_str::<[Length; 2]>(&json).unwrap(), ls);
        assert_eq!(Length::Percent(25.0).resolve(50), 13);
        assert!(Length::Percent(100.5).check("x").is_err());
    }

    #[test]
    fn text_steps_parse_in_every_build() {
        let step: Step = toml::from_str(
            "op = \"text\"\n[[boxes]]\nrect = [0.0, 0.0, 1.0, 0.2]\ntext = \"hi\"\n\
             fonts = [\"a\"]\nsize = { fixed = 0.1 }\n",
        )
        .unwrap();
        let info = FrameInfo {
            width: 10,
            height: 10,
            frames: 1,
        };
        let overlay = cfg!(feature = "overlay");
        match step.effect(0, info) {
            Ok(_) => assert!(overlay),
            Err(d) => {
                assert!(!overlay, "{d}");
                assert!(d.message.contains("`overlay` feature"), "{d}");
            }
        }
    }

    #[test]
    fn defaults_match_an_empty_table() {
        let t = |op: &str| -> Step { toml::from_str(&format!("op = \"{op}\"")).unwrap() };
        assert_eq!(t("pad"), Step::Pad(Pad::default()));
        assert_eq!(t("trim"), Step::Trim(Trim::default()));
        assert_eq!(t("adjust"), Step::Adjust(Adjust::default()));
        assert_eq!(t("sharpen"), Step::Sharpen(Sharpen::default()));
        assert_eq!(
            toml::from_str::<Step>("op = \"resize\"\nmax_long_side = 1").unwrap(),
            Step::Resize(Resize {
                max_long_side: Some(1),
                ..Default::default()
            })
        );
    }
}

//! Text overlay: shaping, line breaking, fitting and drawing of text boxes.
//!
//! The plain-data types ([`TextStep`], [`TextBox`], [`FontSize`],
//! [`Stroke`], [`HAlign`], [`VAlign`], [`TextError`]) and [`interpolate`]
//! are in every build, so pipelines with `text` steps parse and serialize
//! everywhere. Laying out and drawing (`FontRegistry`, `TextRenderer`,
//! `TextLayout`, `draw_onto`, ...) need the `overlay` feature; without it,
//! [`Pipeline::validate`](crate::Pipeline::validate) rejects text steps.
//!
//! Layout is resolution-independent: it runs once in a canonical space where
//! the frame is 1000 units tall and `1000 * aspect` wide, with font sizes on
//! a 0.5-unit grid. Drawing only scales that layout, so a line never breaks
//! differently at preview and export resolution. The public measurements in
//! a `TextLayout` (sizes, pen positions, baselines, widths) are in these
//! units: thousandths of the frame height.
//!
//! Two ways in:
//! - [`Step::Text`](crate::Step::Text) with a [`TextStep`] draws boxes in a
//!   pipeline; fonts come from the [`Ctx`](crate::Ctx).
//! - Directly: register fonts in a `FontRegistry`, lay a [`TextBox`] out
//!   with `TextRenderer::layout`, then draw it with `draw_onto` (whole
//!   image) or `TextRenderer::rasterize` (straight-alpha pixels covering
//!   only the text, see `TextLayout::pixel_bounds`) for incremental redraws.
//!
//! Arguments come in one order everywhere: box, layout, fonts, then the
//! frame (aspect ratio, height in px).
//!
//! Only imgpipe's own types appear in this API; tiny-skia and HarfRust are
//! implementation details.

#[cfg(feature = "overlay")]
mod case;
#[cfg(feature = "overlay")]
mod draw;
#[cfg(feature = "overlay")]
mod fit;
#[cfg(feature = "overlay")]
mod shape;

#[cfg(feature = "overlay")]
pub use case::uppercase;
#[cfg(feature = "overlay")]
pub use draw::{PixelRect, RasterizedText, draw_onto, frame_width};

use crate::MaybeSend;
use crate::color::Color;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hash};

#[cfg(feature = "overlay")]
use shape::{ShapeKey, Shaped};

/// Frame height in canonical units.
#[cfg(feature = "overlay")]
pub(crate) const FRAME_H: f32 = 1000.0;
/// Font sizes are chosen on this grid (canonical units).
#[cfg(feature = "overlay")]
pub(crate) const SIZE_STEP: f32 = 0.5;

/// How big the text is. Sizes are fractions of the frame height, so a box
/// renders the same at every resolution. There is deliberately no default.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FontSize {
    /// A fixed size, as a fraction of frame height.
    Fixed(f32),
    /// The largest size between `min` and `max` (fractions of frame height)
    /// at which the text fits the box.
    Fit {
        /// Smallest size; text that does not fit at it overflows.
        min: f32,
        /// Largest size.
        max: f32,
    },
}

/// An outline around the glyphs, drawn under the fill.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    /// Outline colour.
    pub color: Color,
    /// Visible outline width as a fraction of the font size.
    pub width: f32,
}

impl Stroke {
    /// An outline of `color`, `width` × the font size wide.
    pub fn new(color: Color, width: f32) -> Self {
        Self { color, width }
    }
}

/// Horizontal alignment of lines within the box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HAlign {
    /// Flush left.
    Left,
    /// Centred (the default).
    #[default]
    Center,
    /// Flush right.
    Right,
}

/// Vertical alignment of the text block within the box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum VAlign {
    /// At the top.
    Top,
    /// Centred on cap height (the default).
    #[default]
    Middle,
    /// At the bottom.
    Bottom,
}

fn white() -> Color {
    Color::WHITE
}
fn default_line_height() -> f32 {
    1.2
}

/// One text box: where the text goes and how it looks.
///
/// No `Default`: there is no neutral font list or size. [`TextBox::new`]
/// takes those and gives the rest the defaults of a pipeline file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TextBox {
    /// x, y, w, h, normalized to the frame the box is drawn on.
    pub rect: [f32; 4],
    /// May contain `\n` (a mandatory line break) and, in a [`TextStep`],
    /// `{name}` placeholders (see [`interpolate`]; `{{` and `}}` for literal
    /// braces).
    pub text: String,
    /// BCP 47 language tag; decides casing rules (e.g. Turkish dotted İ).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Font ids from the `FontRegistry`, tried in order per character.
    pub fonts: Vec<String>,
    /// Font size, fixed or fitted to the box.
    pub size: FontSize,
    /// Glyph colour (default white).
    #[serde(default = "white")]
    pub fill: Color,
    /// Optional outline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    /// Horizontal alignment.
    #[serde(default)]
    pub align: HAlign,
    /// Vertical alignment.
    #[serde(default)]
    pub valign: VAlign,
    /// Uppercase the text (for `lang`) before shaping.
    #[serde(default)]
    pub uppercase: bool,
    /// Baseline-to-baseline distance as a multiple of the font size (default 1.2).
    #[serde(default = "default_line_height")]
    pub line_height: f32,
    /// Clockwise rotation around the box centre.
    #[serde(default)]
    pub rotation_deg: f32,
}

impl TextBox {
    /// A box with `text` in `fonts` at `size`; everything else as a
    /// pipeline file leaves it: white fill, no outline, centred, line height
    /// 1.2, no uppercasing or rotation.
    pub fn new(
        rect: [f32; 4],
        text: impl Into<String>,
        fonts: impl IntoIterator<Item = impl Into<String>>,
        size: FontSize,
    ) -> Self {
        Self {
            rect,
            text: text.into(),
            lang: None,
            fonts: fonts.into_iter().map(Into::into).collect(),
            size,
            fill: Color::WHITE,
            stroke: None,
            align: HAlign::default(),
            valign: VAlign::default(),
            uppercase: false,
            line_height: default_line_height(),
            rotation_deg: 0.0,
        }
    }

    /// Checks the parameters; the error says which one is wrong.
    pub fn check(&self) -> Result<(), BoxProblem> {
        let [x, y, w, h] = self.rect;
        if ![x, y, w, h].iter().all(|v| v.is_finite())
            || w <= 0.0
            || h <= 0.0
            || x < 0.0
            || y < 0.0
            || x + w > 1.0001
            || y + h > 1.0001
        {
            return Err(BoxProblem::Rect);
        }
        if self.fonts.is_empty() {
            return Err(BoxProblem::NoFonts);
        }
        let ok = |v: f32| v.is_finite() && v > 0.0 && v <= 1.0;
        match self.size {
            FontSize::Fixed(s) if !ok(s) => return Err(BoxProblem::FixedSize),
            FontSize::Fit { min, max } if !ok(min) || !ok(max) || min > max => {
                return Err(BoxProblem::FitSizes);
            }
            _ => {}
        }
        if let Some(s) = self.stroke
            && (!s.width.is_finite() || !(0.0..=0.5).contains(&s.width))
        {
            return Err(BoxProblem::StrokeWidth);
        }
        if !self.line_height.is_finite() || self.line_height <= 0.0 {
            return Err(BoxProblem::LineHeight);
        }
        if !self.rotation_deg.is_finite() {
            return Err(BoxProblem::Rotation);
        }
        Ok(())
    }

    /// Box rectangle in canonical units.
    #[cfg(feature = "overlay")]
    pub(crate) fn units(&self, aspect: f32) -> (f32, f32, f32, f32) {
        let [x, y, w, h] = self.rect;
        (
            x * FRAME_H * aspect,
            y * FRAME_H,
            w * FRAME_H * aspect,
            h * FRAME_H,
        )
    }
}

/// What is wrong with a [`TextBox`]'s parameters ([`TextBox::check`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BoxProblem {
    /// `rect` is not within 0..1 or has no area.
    #[error("rect must be within 0..1 with a positive size")]
    Rect,
    /// `fonts` is empty.
    #[error("at least one font is needed")]
    NoFonts,
    /// A [`FontSize::Fixed`] size outside (0, 1].
    #[error("fixed size must be in (0, 1]")]
    FixedSize,
    /// [`FontSize::Fit`] sizes outside (0, 1], or `min > max`.
    #[error("fit sizes must be in (0, 1] with min ≤ max")]
    FitSizes,
    /// A stroke width outside 0..=0.5.
    #[error("stroke width must be between 0 and 0.5")]
    StrokeWidth,
    /// `line_height` is not a positive number.
    #[error("line_height must be positive")]
    LineHeight,
    /// `rotation_deg` is not finite.
    #[error("rotation must be finite")]
    Rotation,
}

/// Errors from fonts, text layout and drawing.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum TextError {
    /// `FontRegistry::register` got bytes that aren't a font it can read.
    #[error("font {0:?}: not a readable font")]
    UnreadableFont(String),
    /// A box names a font id that isn't registered.
    #[error("unknown font {0:?}")]
    UnknownFont(String),
    /// The box's parameters are invalid.
    #[error(transparent)]
    InvalidBox(#[from] BoxProblem),
    /// Drawing onto an image with no pixels.
    #[error("cannot draw onto an empty image")]
    EmptyImage,
}

/// Pipeline step: draws text boxes onto every frame. Fonts come from
/// [`Ctx::fonts`](crate::Ctx) (feature `overlay`); `{name}` placeholders are
/// filled from [`Ctx::vars`](crate::Ctx) by [`interpolate`], which also
/// turns `{{` and `}}` into literal braces (with or without vars).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TextStep {
    /// Optional step id, used in plans and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The boxes, drawn in order.
    pub boxes: Vec<TextBox>,
}

impl TextStep {
    /// A step drawing `boxes`, without an id.
    pub fn new(boxes: Vec<TextBox>) -> Self {
        Self { id: None, boxes }
    }
}

/// Values for `{name}` placeholders: [`interpolate`] and
/// [`Ctx::with_vars`](crate::Ctx::with_vars) take any of them.
///
/// Implemented for `HashMap` and `BTreeMap` with string keys and values
/// (`String`, `&str`, ...) and for slices of `(key, value)` pairs (the first
/// match wins).
pub trait Vars: MaybeSend {
    /// The value for `name`, if there is one.
    fn var(&self, name: &str) -> Option<&str>;
}

impl<K, V, S> Vars for HashMap<K, V, S>
where
    K: Borrow<str> + Hash + Eq + MaybeSend,
    V: AsRef<str> + MaybeSend,
    S: BuildHasher + MaybeSend,
{
    fn var(&self, name: &str) -> Option<&str> {
        self.get(name).map(AsRef::as_ref)
    }
}

impl<K, V> Vars for BTreeMap<K, V>
where
    K: Borrow<str> + Ord + MaybeSend,
    V: AsRef<str> + MaybeSend,
{
    fn var(&self, name: &str) -> Option<&str> {
        self.get(name).map(AsRef::as_ref)
    }
}

impl<K, V> Vars for [(K, V)]
where
    K: AsRef<str> + MaybeSend,
    V: AsRef<str> + MaybeSend,
{
    fn var(&self, name: &str) -> Option<&str> {
        self.iter()
            .find(|(k, _)| k.as_ref() == name)
            .map(|(_, v)| v.as_ref())
    }
}

/// Fills `{name}` placeholders from `vars`, in one pass over `text`.
///
/// - A name is one or more ASCII letters, digits, `_` or `-`
///   (`[A-Za-z0-9_-]+`).
/// - `{name}` with `name` in `vars` becomes its value. The value is copied
///   as is, never scanned again, so a value containing `{other}` stays
///   literal and the result is at most `text` plus one value per
///   placeholder.
/// - A `{name}` not in `vars`, or braces around anything that is not a
///   name, stay as written.
/// - `{{` and `}}` are escapes for a literal `{` and `}` (`{{name}}` gives
///   `{name}`). A lone `}` stays as written.
///
/// The result never depends on the iteration order of `vars`.
pub fn interpolate<V: Vars + ?Sized>(text: &str, vars: &V) -> String {
    let is_name = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    // Every byte that matters here is ASCII, so slicing next to one of them
    // is always on a char boundary.
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{')
            && let Some(close) = tail[1..].find(['{', '}'])
            && tail.as_bytes()[1 + close] == b'}'
            && is_name(&tail[1..1 + close])
            && let Some(value) = vars.var(&tail[1..1 + close])
        {
            out.push_str(value);
            rest = &tail[close + 2..];
            continue;
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

#[cfg(feature = "overlay")]
pub(crate) struct FontEntry {
    pub font: harfrust::Font,
    pub upem: f32,
    /// Cap height as a fraction of the em.
    pub cap: f32,
}

/// Fonts by id (feature `overlay`). Takes bytes from the caller: it never
/// reads files or fetches, so every platform loads fonts its own way.
#[cfg(feature = "overlay")]
#[derive(Default)]
pub struct FontRegistry {
    fonts: Vec<FontEntry>,
    ids: HashMap<String, usize>,
}

#[cfg(feature = "overlay")]
impl std::fmt::Debug for FontRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ids: Vec<_> = self.ids.iter().collect();
        ids.sort_by_key(|(_, i)| **i);
        f.debug_struct("FontRegistry")
            .field(
                "ids",
                &ids.into_iter().map(|(id, _)| id).collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(feature = "overlay")]
impl FontRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a static font.
    pub fn register(&mut self, id: impl Into<String>, data: Vec<u8>) -> Result<(), TextError> {
        self.register_instance(id, data, &[])
    }

    /// Registers a variable font pinned to one instance, e.g. `[("wght", 700.0)]`.
    /// Pinning keeps advances identical at every size (no optical-size drift).
    pub fn register_instance(
        &mut self,
        id: impl Into<String>,
        data: Vec<u8>,
        axes: &[(&str, f32)],
    ) -> Result<(), TextError> {
        let id = id.into();
        let base =
            harfrust::Font::new(data, 0).ok_or_else(|| TextError::UnreadableFont(id.clone()))?;
        let font = if axes.is_empty() {
            base
        } else {
            base.instance_builder()
                .variations(axes.iter().copied())
                .build()
        };
        let metrics = font.metrics();
        let upem = metrics.units_per_em as f32;
        let cap = metrics
            .cap_height
            .map(|c| c.to_f32() / upem)
            .filter(|c| *c > 0.0)
            .unwrap_or(0.7);
        let index = self.fonts.len();
        self.fonts.push(FontEntry { font, upem, cap });
        self.ids.insert(id, index);
        Ok(())
    }

    /// Whether a font is registered under `id`.
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains_key(id)
    }

    pub(crate) fn resolve(&self, ids: &[String]) -> Result<Vec<usize>, TextError> {
        ids.iter()
            .map(|id| {
                self.ids
                    .get(id)
                    .copied()
                    .ok_or_else(|| TextError::UnknownFont(id.clone()))
            })
            .collect()
    }

    pub(crate) fn entry(&self, index: usize) -> &FontEntry {
        &self.fonts[index]
    }

    pub(crate) fn covers(&self, index: usize, ch: char) -> bool {
        self.fonts[index].font.charmap().map_unicode(ch).is_some()
    }
}

/// A glyph placed in canonical units.
#[cfg(feature = "overlay")]
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PlacedGlyph {
    /// Index into the registry's fonts (in registration order).
    pub font: usize,
    /// Glyph id in that font.
    pub glyph: u32,
    /// Pen position (left edge on the baseline), canonical units.
    pub x: f32,
}

/// One line of a [`TextLayout`].
#[cfg(feature = "overlay")]
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LayoutLine {
    /// Glyphs in visual order.
    pub glyphs: Vec<PlacedGlyph>,
    /// Baseline y, canonical units.
    pub baseline: f32,
    /// Width without trailing whitespace, canonical units.
    pub width: f32,
}

/// The result of laying out one box. Independent of output resolution.
#[cfg(feature = "overlay")]
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TextLayout {
    /// Chosen font size, canonical units (frame height = 1000).
    pub size: f32,
    /// Lines from top to bottom.
    pub lines: Vec<LayoutLine>,
    /// The text does not fit even at the minimum size.
    pub overflow: bool,
    /// Characters no font in the stack covers (drawn as `.notdef`).
    pub missing: Vec<char>,
    /// Box rectangle and rotation the layout was made for, canonical units.
    pub(crate) frame: (f32, f32, f32, f32, f32),
}

#[cfg(feature = "overlay")]
impl TextLayout {
    /// Glyph count per line; equal signatures mean equal line breaks.
    pub fn break_signature(&self) -> Vec<usize> {
        self.lines.iter().map(|l| l.glyphs.len()).collect()
    }
}

/// Lays out and draws boxes, caching shaped text and glyph outlines. Keep
/// one per editor (or per worker) and reuse it across keystrokes. The caches
/// are keyed by font index, so use one renderer per [`FontRegistry`].
#[cfg(feature = "overlay")]
#[derive(Default)]
pub struct TextRenderer {
    shaped: HashMap<ShapeKey, Shaped>,
    pub(crate) outlines: HashMap<(usize, u32), Option<tiny_skia::Path>>,
}

#[cfg(feature = "overlay")]
impl std::fmt::Debug for TextRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextRenderer")
            .field("shaped", &self.shaped.len())
            .field("outlines", &self.outlines.len())
            .finish()
    }
}

/// Cached shaped strings kept before the cache is cleared.
#[cfg(feature = "overlay")]
const SHAPE_CACHE_LIMIT: usize = 64;

#[cfg(feature = "overlay")]
impl TextRenderer {
    /// A renderer with empty caches.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lays out `b` with `fonts` for a frame of the given aspect ratio
    /// (width / height).
    pub fn layout(
        &mut self,
        b: &TextBox,
        fonts: &FontRegistry,
        aspect: f32,
    ) -> Result<TextLayout, TextError> {
        b.check()?;
        let stack = fonts.resolve(&b.fonts)?;
        let text = if b.uppercase {
            uppercase(&b.text, b.lang.as_deref(), |c| {
                stack.iter().any(|&f| fonts.covers(f, c))
            })
        } else {
            b.text.clone()
        };
        let key = ShapeKey {
            text,
            stack: stack.clone(),
        };
        if !self.shaped.contains_key(&key) {
            if self.shaped.len() >= SHAPE_CACHE_LIMIT {
                self.shaped.clear();
            }
            let shaped = shape::shape(&key.text, &stack, fonts);
            self.shaped.insert(key.clone(), shaped);
        }
        let shaped = &self.shaped[&key];
        Ok(fit::layout(shaped, b, aspect, fonts.entry(stack[0]).cap))
    }
}

/// One-off layout without a cache: [`TextRenderer::layout`] on a new renderer.
#[cfg(feature = "overlay")]
pub fn layout_text(
    b: &TextBox,
    fonts: &FontRegistry,
    aspect: f32,
) -> Result<TextLayout, TextError> {
    TextRenderer::new().layout(b, fonts, aspect)
}

#[cfg(test)]
mod tests {
    use super::interpolate;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn substituted_values_are_never_expanded_again() {
        // Every new HashMap has its own hash seed, so iteration order varies
        // between them; the old per-key `replace` gave "{b}" or "X" by chance.
        for _ in 0..64 {
            let v = vars(&[("a", "{b}"), ("b", "X"), ("c", "{c}{c}")]);
            assert_eq!(interpolate("{a}", &v), "{b}");
            assert_eq!(interpolate("{b}{a}", &v), "X{b}");
            assert_eq!(interpolate("{c}{c}{c}", &v), "{c}{c}".repeat(3));
        }
    }

    #[test]
    fn placeholders_escapes_and_leftovers() {
        let v = vars(&[("name", "Ada"), ("top-text", "hi"), ("n_2", "two")]);
        let cases = [
            ("Hello, {name}!", "Hello, Ada!"),
            ("{top-text} {n_2}", "hi two"),
            ("{unknown} stays", "{unknown} stays"),
            ("{not a name} {} {name", "{not a name} {} {name"),
            ("{{name}} is literal", "{name} is literal"),
            ("{{{name}}}", "{Ada}"),
            ("a }} b {{ c", "a } b { c"),
            ("lone } and {", "lone } and {"),
            ("{{name}", "{name}"),
            ("{nam{name}}", "{namAda}"),
            ("árvíz {name} tűrő", "árvíz Ada tűrő"),
            ("{név}", "{név}"),
            ("", ""),
        ];
        for (text, expected) in cases {
            assert_eq!(interpolate(text, &v), expected, "{text:?}");
        }
    }

    #[test]
    fn any_string_map_or_pair_list_provides_vars() {
        let tree: std::collections::BTreeMap<&str, &str> = [("a", "1")].into();
        assert_eq!(interpolate("{a}{b}", &tree), "1{b}");
        let pairs = [("a", "1"), ("a", "2"), ("b", "3")];
        assert_eq!(interpolate("{a}{b}", &pairs[..]), "13");
        let dynamic: &dyn super::Vars = &tree;
        assert_eq!(interpolate("{a}", dynamic), "1");
    }
}

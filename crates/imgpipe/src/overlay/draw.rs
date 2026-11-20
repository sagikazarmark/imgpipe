//! Drawing laid-out text with tiny-skia: one path per box, its stroke
//! (round joins and caps) under its fill. Also the pixel bounds of a
//! layout and drawing into a pixmap that covers only those bounds.

use super::{FRAME_H, FontRegistry, TextBox, TextLayout, TextRenderer};
use crate::frame::Rgba8;
use harfrust::GlyphId;
use harfrust::font::glyph::outline::{DrawMemory, DrawOptions, PathSink};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Point, Stroke, Transform,
};

/// A rectangle of whole pixels on a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct PixelRect {
    /// Left edge, px.
    pub x: u32,
    /// Top edge, px.
    pub y: u32,
    /// Width, px (never 0).
    pub width: u32,
    /// Height, px (never 0).
    pub height: u32,
}

/// One box drawn into a pixmap covering only its [`PixelRect`]: straight
/// alpha RGBA, row-major, `rect.width * rect.height * 4` bytes (what
/// `ImageData` and encoders expect).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RasterizedText {
    /// Where the pixels go on the frame.
    pub rect: PixelRect,
    /// The pixels.
    pub rgba: Vec<u8>,
}

/// Width in px of a frame `height` px tall with the given aspect ratio
/// (width / height), rounded, at least 1. [`TextLayout::pixel_bounds`] and
/// [`TextRenderer::rasterize`] clamp to a frame of this size.
pub fn frame_width(aspect: f32, height: u32) -> u32 {
    (aspect * height as f32).round().max(1.0) as u32
}

impl TextLayout {
    /// Canonical units → pixmap pixels: the box rotation (around the box
    /// centre), then `scale` px per unit, then a shift by (-`dx`, -`dy`) px.
    /// Drawing and [`pixel_bounds`](Self::pixel_bounds) both use it.
    fn transform(&self, scale: f32, dx: f32, dy: f32) -> Transform {
        let (bx, by, bw, bh, rotation) = self.frame;
        let mut transform = Transform::from_scale(scale, scale).post_translate(-dx, -dy);
        if rotation != 0.0 {
            let (cx, cy) = (bx + bw / 2.0, by + bh / 2.0);
            transform = transform.pre_concat(Transform::from_rotate_at(rotation, cx, cy));
        }
        transform
    }

    /// The pixels drawing this layout can touch on a frame `height` px tall
    /// (`aspect` = width / height): glyph extents with room for ascenders
    /// (accented capitals reach about 1.1 em) and descenders, plus the
    /// stroke, rotated like the drawing, padded by a pixel and clamped to the
    /// frame. `None` when there is nothing to draw. `b` is the box the layout
    /// was made from.
    pub fn pixel_bounds(&self, b: &TextBox, aspect: f32, height: u32) -> Option<PixelRect> {
        self.bounds_on(b, frame_width(aspect, height), height)
    }

    /// [`pixel_bounds`](Self::pixel_bounds) on a frame `width` × `height` px.
    fn bounds_on(&self, b: &TextBox, width: u32, height: u32) -> Option<PixelRect> {
        if self.lines.iter().all(|l| l.glyphs.is_empty()) {
            return None;
        }
        let size = self.size;
        let margin = size * b.stroke.map_or(0.0, |s| s.width) + size * 0.1;
        let mut x0 = f32::MAX;
        let mut x1 = f32::MIN;
        for l in &self.lines {
            if let Some(first) = l.glyphs.first() {
                x0 = x0.min(first.x);
                x1 = x1.max(first.x + l.width.max(size * 0.5));
            }
        }
        let first = self.lines.first()?.baseline;
        let last = self.lines.last()?.baseline;
        let (y0, y1) = (first - size * 1.15, last + size * 0.4);
        let (x0, x1, y0, y1) = (x0 - margin, x1 + margin, y0 - margin, y1 + margin);

        let mut corners = [
            Point::from_xy(x0, y0),
            Point::from_xy(x1, y0),
            Point::from_xy(x0, y1),
            Point::from_xy(x1, y1),
        ];
        self.transform(height as f32 / FRAME_H, 0.0, 0.0)
            .map_points(&mut corners);
        let (mut min, mut max) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        for p in corners {
            min = (min.0.min(p.x), min.1.min(p.y));
            max = (max.0.max(p.x), max.1.max(p.y));
        }
        let width = width as f32;
        let px0 = (min.0.floor() - 1.0).clamp(0.0, width) as u32;
        let py0 = (min.1.floor() - 1.0).clamp(0.0, height as f32) as u32;
        let px1 = (max.0.ceil() + 1.0).clamp(0.0, width) as u32;
        let py1 = (max.1.ceil() + 1.0).clamp(0.0, height as f32) as u32;
        (px1 > px0 && py1 > py0).then(|| PixelRect {
            x: px0,
            y: py0,
            width: px1 - px0,
            height: py1 - py0,
        })
    }
}

/// Collects an outline in em units (y up, as fonts are).
struct EmSink {
    pb: PathBuilder,
    scale: f32,
}

impl PathSink for EmSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(x * self.scale, y * self.scale);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(x * self.scale, y * self.scale);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.pb.quad_to(
            cx0 * self.scale,
            cy0 * self.scale,
            x * self.scale,
            y * self.scale,
        );
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let s = self.scale;
        self.pb
            .cubic_to(cx0 * s, cy0 * s, cx1 * s, cy1 * s, x * s, y * s);
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

impl TextRenderer {
    /// A glyph outline in em units, cached.
    fn outline(
        &mut self,
        fonts: &FontRegistry,
        font: usize,
        glyph: u32,
    ) -> Option<tiny_skia::Path> {
        self.outlines
            .entry((font, glyph))
            .or_insert_with(|| {
                let entry = fonts.entry(font);
                let outline = entry.font.outline_glyphs().get(GlyphId::new(glyph))?;
                let mut sink = EmSink {
                    pb: PathBuilder::new(),
                    scale: 1.0 / entry.upem,
                };
                outline
                    .draw(&DrawOptions::new(), DrawMemory::default(), &mut sink)
                    .ok()?;
                sink.pb.finish()
            })
            .clone()
    }

    /// Draws `layout` into `canvas`, which shows the frame region starting at
    /// frame pixel (`dx`, `dy`) with `scale` pixels per canonical unit (the
    /// frame height in px / 1000). `canvas` is premultiplied, as tiny-skia's
    /// pixmaps are; [`demultiply`] converts the result to straight alpha.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_region(
        &mut self,
        canvas: &mut Pixmap,
        layout: &TextLayout,
        b: &TextBox,
        fonts: &FontRegistry,
        scale: f32,
        dx: f32,
        dy: f32,
    ) {
        let size = layout.size;
        let mut pb = PathBuilder::new();
        for line in &layout.lines {
            for g in &line.glyphs {
                let Some(path) = self.outline(fonts, g.font, g.glyph) else {
                    continue;
                };
                // Em units, y up -> canonical units, y down, at the pen position.
                let place = Transform::from_row(size, 0.0, 0.0, -size, g.x, line.baseline);
                if let Some(p) = path.transform(place) {
                    pb.push_path(&p);
                }
            }
        }
        let Some(path) = pb.finish() else { return };
        let transform = layout.transform(scale, dx, dy);

        let mut paint = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        if let Some(s) = b.stroke.filter(|s| s.width > 0.0) {
            let [r, g, bl, a] = s.color.0;
            paint.set_color_rgba8(r, g, bl, a);
            let stroke = Stroke {
                // Strokes are centred on the outline: twice the visible width.
                width: 2.0 * s.width * size,
                line_join: LineJoin::Round,
                line_cap: LineCap::Round,
                ..Stroke::default()
            };
            canvas.stroke_path(&path, &paint, &stroke, transform, None);
        }
        let [r, g, bl, a] = b.fill.0;
        paint.set_color_rgba8(r, g, bl, a);
        canvas.fill_path(&path, &paint, FillRule::Winding, transform, None);
    }

    /// Draws one laid-out box into a pixmap covering only its
    /// [`pixel_bounds`](TextLayout::pixel_bounds) on a frame `height` px
    /// tall, so an editor can redraw one box without touching the rest.
    /// `None` when there is nothing to draw.
    pub fn rasterize(
        &mut self,
        b: &TextBox,
        layout: &TextLayout,
        fonts: &FontRegistry,
        aspect: f32,
        height: u32,
    ) -> Option<RasterizedText> {
        self.rasterize_on(layout, b, fonts, frame_width(aspect, height), height)
    }

    /// [`rasterize`](Self::rasterize) for a frame `width` × `height` px.
    fn rasterize_on(
        &mut self,
        layout: &TextLayout,
        b: &TextBox,
        fonts: &FontRegistry,
        width: u32,
        height: u32,
    ) -> Option<RasterizedText> {
        let rect = layout.bounds_on(b, width, height)?;
        let mut pixmap = Pixmap::new(rect.width, rect.height)?;
        let scale = height as f32 / FRAME_H;
        self.draw_region(
            &mut pixmap,
            layout,
            b,
            fonts,
            scale,
            rect.x as f32,
            rect.y as f32,
        );
        let mut rgba = pixmap.take();
        demultiply(&mut rgba);
        Some(RasterizedText { rect, rgba })
    }
}

/// Draws boxes onto an sRGB straight-alpha image (what the pipeline's text
/// step does), matching a browser that shows each [`TextRenderer::rasterize`]
/// layer as a canvas over the base image: each box is rasterized exactly as
/// `rasterize` does, then composited "source-over" onto the image, in box
/// order, as a browser composites canvases: the layer
/// premultiplied at 8 bits (what `putImageData` stores), then blended on
/// sRGB values (`out = s·a + d·(1 − a)`), rounded once.
///
/// Only pixels a box actually covers change: everything outside the boxes'
/// [`pixel_bounds`](TextLayout::pixel_bounds), and every pixel inside them
/// that the text leaves transparent, stays bit-identical.
///
/// `boxes` are (box, layout) pairs, borrowed: from a `Vec<(TextBox,
/// TextLayout)>` pass `laid.iter().map(|(b, l)| (b, l))`.
pub fn draw_onto<'b>(
    img: &mut Rgba8,
    boxes: impl IntoIterator<Item = (&'b TextBox, &'b TextLayout)>,
    fonts: &FontRegistry,
    renderer: &mut TextRenderer,
) -> Result<(), super::TextError> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err(super::TextError::EmptyImage);
    }
    for (b, layout) in boxes {
        if let Some(layer) = renderer.rasterize_on(layout, b, fonts, w, h) {
            composite(img, &layer);
        }
    }
    Ok(())
}

/// Composites a rasterized layer (straight alpha) onto `img` at its rect,
/// "source-over", like a browser showing the layer's canvas over the base:
///
/// 1. `putImageData` stores the layer premultiplied at 8 bits:
///    `s' = round(s · a / 255)`.
/// 2. The compositor blends premultiplied values in sRGB, without
///    linearizing: `out' = s' + d' · (1 − a)`, `out_a = a + d_a · (1 − a)`.
///
/// The result is stored straight again, rounded once from the exact values.
/// Over an opaque base this is `round(s' + d · (255 − a) / 255)`, exactly
/// what an 8-bit compositor produces. Pixels where the layer is transparent
/// are not touched, and fully opaque layer pixels replace the base.
fn composite(img: &mut Rgba8, layer: &RasterizedText) {
    let r = layer.rect;
    let (row_bytes, stride) = (r.width as usize * 4, img.width() as usize * 4);
    let data = img.data_mut();
    for (y, src) in layer.rgba.chunks_exact(row_bytes).enumerate() {
        let start = (r.y as usize + y) * stride + r.x as usize * 4;
        let dst = &mut data[start..start + row_bytes];
        for (d, s) in dst
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(src.as_chunks::<4>().0)
        {
            *d = over(*d, *s);
        }
    }
}

/// One pixel of [`composite`]: straight-alpha `src` over straight-alpha `dst`.
fn over(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    let sa = src[3] as u32;
    match sa {
        0 => return dst,
        255 => return src,
        _ => {}
    }
    let (da, inv) = (dst[3] as u32, 255 - sa);
    // Alpha in units of 1/255², premultiplied colour in units of 1/255³;
    // the largest value, 255³ + 255³, fits a u32.
    let alpha = sa * 255 + da * inv;
    let mut out = [0; 4];
    for k in 0..3 {
        let premul = (src[k] as u32 * sa + 127) / 255;
        let colour = premul * 255 * 255 + dst[k] as u32 * da * inv;
        out[k] = ((colour + alpha / 2) / alpha).min(255) as u8;
    }
    out[3] = ((alpha + 127) / 255) as u8;
    out
}

/// Premultiplied → straight alpha, as `ImageData` and encoders expect.
pub(crate) fn demultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::over;

    #[test]
    fn over_matches_an_8_bit_compositor_on_opaque_bases() {
        for sa in 0..=255u32 {
            for s in (0..=255u32).step_by(5) {
                for d in (0..=255u32).step_by(15) {
                    let premul = (s * sa + 127) / 255;
                    let expected = ((premul * 255 + d * (255 - sa) + 127) / 255) as u8;
                    let (s8, d8) = (s as u8, d as u8);
                    let got = over([d8, d8, d8, 255], [s8, s8, s8, sa as u8]);
                    assert_eq!(
                        got,
                        [expected, expected, expected, 255],
                        "s={s} a={sa} d={d}"
                    );
                }
            }
        }
    }

    #[test]
    fn transparent_layer_pixels_leave_the_base_alone() {
        assert_eq!(over([200, 100, 50, 7], [9, 9, 9, 0]), [200, 100, 50, 7]);
        assert_eq!(over([200, 100, 50, 7], [9, 8, 7, 255]), [9, 8, 7, 255]);
        // Onto nothing, the layer itself (up to the 8-bit premultiply).
        assert_eq!(over([0, 0, 0, 0], [255, 128, 0, 128]), [255, 128, 0, 128]);
    }
}

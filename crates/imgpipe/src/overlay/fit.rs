//! Greedy line filling, size search on the grid, and rebalancing.

use super::shape::{Segment, Shaped};
use super::{
    FRAME_H, FontSize, HAlign, LayoutLine, PlacedGlyph, SIZE_STEP, TextBox, TextLayout, VAlign,
};
use std::ops::Range;

/// Greedy filling. `None` when one segment alone is wider than `max_width`.
fn greedy(segments: &[Segment], max_width: f32) -> Option<Vec<Range<usize>>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut width = 0.0;
    for (i, seg) in segments.iter().enumerate() {
        if seg.trimmed > max_width {
            return None;
        }
        if i > start && width + seg.trimmed > max_width {
            lines.push(start..i);
            start = i;
            width = 0.0;
        }
        width += seg.width;
        if seg.hard_break {
            lines.push(start..i + 1);
            start = i + 1;
            width = 0.0;
        }
    }
    if start < segments.len() {
        lines.push(start..segments.len());
    }
    Some(lines)
}

/// Splits segments wider than `max_width` between glyphs, for the overflow
/// case only: a word is broken only when it alone is wider than the box.
fn split_oversized(
    segments: &[Segment],
    glyphs: &[super::shape::Glyph],
    max_width: f32,
) -> Vec<Segment> {
    let mut out = Vec::new();
    for seg in segments {
        if seg.trimmed <= max_width {
            out.push(seg.clone());
            continue;
        }
        let mut start = seg.glyphs.start;
        let mut width = 0.0;
        for g in seg.glyphs.clone() {
            let adv = glyphs[g].advance;
            if g > start && width + adv > max_width {
                out.push(Segment {
                    glyphs: start..g,
                    width,
                    trimmed: width,
                    hard_break: false,
                });
                start = g;
                width = 0.0;
            }
            width += adv;
        }
        out.push(Segment {
            glyphs: start..seg.glyphs.end,
            width,
            trimmed: width,
            hard_break: seg.hard_break,
        });
    }
    out
}

/// Narrowest width (≤ `max_width`) that keeps the same line count, so the
/// last line is not left with a lone word. It cannot make the widest line
/// wider, so the text still fits.
fn rebalance(segments: &[Segment], max_width: f32, lines: usize) -> Vec<Range<usize>> {
    let widest = segments.iter().map(|s| s.trimmed).fold(0.0, f32::max);
    let (mut lo, mut hi) = (widest, max_width);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        match greedy(segments, mid) {
            Some(l) if l.len() <= lines => hi = mid,
            _ => lo = mid,
        }
    }
    greedy(segments, hi).expect("hi always fits")
}

fn line_width(segments: &[Segment], range: &Range<usize>) -> (f32, Range<usize>) {
    let segs = &segments[range.clone()];
    let glyphs =
        segs.first().map_or(0, |s| s.glyphs.start)..segs.last().map_or(0, |s| s.glyphs.end);
    let full: f32 = segs.iter().map(|s| s.width).sum();
    let trailing = segs.last().map_or(0.0, |s| s.width - s.trimmed);
    (full - trailing, glyphs)
}

pub(super) fn layout(shaped: &Shaped, b: &TextBox, aspect: f32, cap: f32) -> TextLayout {
    let (bx, by, bw, bh) = b.units(aspect);
    let rotation = b.rotation_deg;
    let stroke = b.stroke.map_or(0.0, |s| s.width);
    // Height of `n` all-caps lines at `size`, and the usable box at `size`.
    let block = |n: usize, size: f32| {
        if n == 0 {
            0.0
        } else {
            cap * size + (n - 1) as f32 * size * b.line_height
        }
    };
    let inner = |size: f32| {
        (
            (bw - 2.0 * stroke * size).max(1.0),
            (bh - 2.0 * stroke * size).max(1.0),
        )
    };
    let fits = |size: f32| {
        let (w, h) = inner(size);
        greedy(&shaped.segments, w / size).is_some_and(|l| block(l.len(), size) <= h)
    };

    let (size, overflow) = match b.size {
        FontSize::Fixed(f) => {
            let size = ((f * FRAME_H) / SIZE_STEP).round().max(1.0) * SIZE_STEP;
            (size, !fits(size))
        }
        FontSize::Fit { min, max } => {
            let lo_k = ((min * FRAME_H) / SIZE_STEP).ceil().max(1.0) as i32;
            let hi_k = ((max * FRAME_H) / SIZE_STEP).floor().max(lo_k as f32) as i32;
            if !fits(lo_k as f32 * SIZE_STEP) {
                (lo_k as f32 * SIZE_STEP, true)
            } else {
                // `fits` is monotone in size: greedy breaking with linearly
                // scaling advances never needs more lines at a smaller size.
                let (mut lo, mut hi) = (lo_k, hi_k);
                while lo < hi {
                    let mid = (lo + hi + 1) / 2;
                    if fits(mid as f32 * SIZE_STEP) {
                        lo = mid
                    } else {
                        hi = mid - 1
                    }
                }
                (lo as f32 * SIZE_STEP, false)
            }
        }
    };

    let (w, _) = inner(size);
    let max_em = w / size;
    let (segments, ranges) = match greedy(&shaped.segments, max_em) {
        Some(lines) if lines.len() > 1 => {
            let lines = rebalance(&shaped.segments, max_em, lines.len());
            (shaped.segments.clone(), lines)
        }
        Some(lines) => (shaped.segments.clone(), lines),
        None => {
            let split = split_oversized(&shaped.segments, &shaped.glyphs, max_em);
            // A single glyph can still be wider than the box: let it overhang.
            let widest = split.iter().map(|s| s.trimmed).fold(max_em, f32::max);
            let lines = greedy(&split, widest).expect("every segment fits the widest");
            (split, lines)
        }
    };

    let n = ranges.len();
    let top = match b.valign {
        VAlign::Top => by + stroke * size,
        VAlign::Middle => by + (bh - block(n, size)) / 2.0,
        VAlign::Bottom => by + bh - stroke * size - block(n, size),
    };
    let lines = ranges
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let (width_em, glyph_range) = line_width(&segments, r);
            let width = width_em * size;
            let x0 = match b.align {
                HAlign::Left => bx + stroke * size,
                HAlign::Center => bx + (bw - width) / 2.0,
                HAlign::Right => bx + bw - stroke * size - width,
            };
            let mut x = x0;
            let glyphs = glyph_range
                .map(|g| {
                    let gl = shaped.glyphs[g];
                    let placed = PlacedGlyph {
                        font: gl.font,
                        glyph: gl.id,
                        x,
                    };
                    x += gl.advance * size;
                    placed
                })
                .collect();
            LayoutLine {
                glyphs,
                baseline: top + cap * size + i as f32 * size * b.line_height,
                width,
            }
        })
        .collect();

    TextLayout {
        size,
        lines,
        overflow,
        missing: shaped.missing.clone(),
        frame: (bx, by, bw, bh, rotation),
    }
}

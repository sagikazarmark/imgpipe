//! Shaping with per-character font fallback, then UAX #14 segments.
//!
//! Shaping happens once per text change; widths are kept in em units, so
//! every fit iteration afterwards is arithmetic.

use super::FontRegistry;
use harfrust::{Buffer, ShapeOptions, ShaperFont};
use unicode_linebreak::{BreakOpportunity, linebreaks};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ShapeKey {
    pub text: String,
    pub stack: Vec<usize>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Glyph {
    pub font: usize,
    pub id: u32,
    /// Advance in em units.
    pub advance: f32,
    /// Byte offset of the glyph's cluster in the text.
    pub cluster: usize,
}

/// Glyphs between two break opportunities.
#[derive(Clone, Debug)]
pub(super) struct Segment {
    pub glyphs: std::ops::Range<usize>,
    /// Width in em, including trailing whitespace.
    pub width: f32,
    /// Width in em without trailing whitespace.
    pub trimmed: f32,
    /// A mandatory break (`\n`) follows.
    pub hard_break: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Shaped {
    pub glyphs: Vec<Glyph>,
    pub segments: Vec<Segment>,
    pub missing: Vec<char>,
}

/// The font for each character: the first in the stack that covers it.
/// Whitespace and control characters stay in the surrounding run's font.
fn assign_fonts(
    text: &str,
    stack: &[usize],
    fonts: &FontRegistry,
    missing: &mut Vec<char>,
) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new(); // (byte start, font)
    let mut current = stack[0];
    for (i, c) in text.char_indices() {
        let font = if c.is_whitespace() || c.is_control() {
            current
        } else if let Some(&f) = stack.iter().find(|&&f| fonts.covers(f, c)) {
            f
        } else {
            if !missing.contains(&c) {
                missing.push(c);
            }
            stack[0]
        };
        if runs.last().is_none_or(|&(_, f)| f != font) {
            runs.push((i, font));
        }
        current = font;
    }
    runs
}

pub(super) fn shape(text: &str, stack: &[usize], fonts: &FontRegistry) -> Shaped {
    let mut missing = Vec::new();
    let runs = assign_fonts(text, stack, fonts, &mut missing);
    let mut glyphs = Vec::new();
    for (k, &(start, font)) in runs.iter().enumerate() {
        let end = runs.get(k + 1).map_or(text.len(), |r| r.0);
        let entry = fonts.entry(font);
        let shaper = ShaperFont::new(&entry.font);
        let mut buffer = Buffer::new();
        buffer.push_str(&text[start..end]);
        buffer.guess_segment_properties();
        if harfrust::shape(&shaper, &mut buffer, ShapeOptions::new()).is_err() {
            continue;
        }
        for (info, pos) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
            glyphs.push(Glyph {
                font,
                id: info.glyph_id,
                advance: pos.x_advance as f32 / entry.upem,
                cluster: start + info.cluster as usize,
            });
        }
    }

    // Left-to-right text: glyph clusters increase with byte offsets.
    let mut segments = Vec::new();
    let mut g = 0;
    for (pos, kind) in linebreaks(text) {
        let first = g;
        while g < glyphs.len() && glyphs[g].cluster < pos {
            g += 1;
        }
        let width: f32 = glyphs[first..g].iter().map(|gl| gl.advance).sum();
        let mut trimmed = width;
        let mut k = g;
        while k > first && text[glyphs[k - 1].cluster..].starts_with(char::is_whitespace) {
            k -= 1;
            trimmed -= glyphs[k].advance;
        }
        segments.push(Segment {
            glyphs: first..g,
            width,
            trimmed,
            hard_break: kind == BreakOpportunity::Mandatory && pos < text.len(),
        });
    }
    Shaped {
        glyphs,
        segments,
        missing,
    }
}

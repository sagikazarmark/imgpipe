//! Crop, pad, trim, rotate and flip: pixel moves without filtering.

use super::{Axis, Crop, Flip, Pad, Rotate, Trim};
use crate::frame::{Frames, Rgba8};

pub(super) fn crop_rect(p: &Crop, w: u32, h: u32) -> Result<(u32, u32, u32, u32), String> {
    for (name, l) in ["x", "y", "w", "h"].iter().zip(p.rect) {
        l.check(name)?;
    }
    let [x, y, cw, ch] = p.rect;
    let (x, y) = (x.resolve(w), y.resolve(h));
    let (cw, ch) = (cw.resolve(w), ch.resolve(h));
    if x >= w || y >= h {
        return Err(format!(
            "crop origin ({x}, {y}) is outside the {w}x{h} image"
        ));
    }
    let cw = cw.min(w - x);
    let ch = ch.min(h - y);
    if cw == 0 || ch == 0 {
        return Err("crop rectangle is empty".into());
    }
    Ok((x, y, cw, ch))
}

pub(crate) fn crop_image(img: &Rgba8, x: u32, y: u32, w: u32, h: u32) -> Rgba8 {
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in y..y + h {
        let r = img.row(row);
        out.extend_from_slice(&r[x as usize * 4..(x + w) as usize * 4]);
    }
    Rgba8::from_raw(w, h, out).expect("sized")
}

pub(super) fn crop(p: &Crop, frames: Frames) -> Result<Frames, String> {
    let info = frames.info();
    let (x, y, w, h) = crop_rect(p, info.width, info.height)?;
    frames.map_images(|img| Ok(crop_image(&img, x, y, w, h)))
}

/// New size and content offset: (width, height, left, top); an error when a
/// side would not fit in a `u32`.
pub(super) fn pad_size(p: &Pad, w: u32, h: u32) -> Result<(u32, u32, u32, u32), String> {
    let (top, bottom) = (p.top.resolve(h), p.bottom.resolve(h));
    let (left, right) = (p.left.resolve(w), p.right.resolve(w));
    let nw = w.checked_add(left).and_then(|v| v.checked_add(right));
    let nh = h.checked_add(top).and_then(|v| v.checked_add(bottom));
    match (nw, nh) {
        (Some(nw), Some(nh)) => Ok((nw, nh, left, top)),
        _ => Err(format!(
            "padding {w}x{h} by {top}/{right}/{bottom}/{left} px overflows"
        )),
    }
}

pub(super) fn pad(p: &Pad, frames: Frames) -> Result<Frames, String> {
    let info = frames.info();
    let (nw, nh, left, top) = pad_size(p, info.width, info.height)?;
    frames.map_images(|img| {
        let mut out =
            Rgba8::try_filled(nw, nh, p.color.0).ok_or("padded image does not fit in memory")?;
        let stride = nw as usize * 4;
        for y in 0..img.height() {
            let start = (top + y) as usize * stride + left as usize * 4;
            out.data_mut()[start..start + img.width() as usize * 4].copy_from_slice(img.row(y));
        }
        Ok(out)
    })
}

/// Border widths (left, top, right, bottom) of uniform colour around `img`.
fn uniform_insets(img: &Rgba8, threshold: u8) -> (u32, u32, u32, u32) {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return (0, 0, 0, 0);
    }
    let reference = img.pixel(0, 0);
    let close = |p: [u8; 4]| {
        p.iter()
            .zip(reference)
            .all(|(a, b)| a.abs_diff(b) <= threshold)
    };
    let row_uniform = |y: u32| (0..w).all(|x| close(img.pixel(x, y)));
    let col_uniform = |x: u32, y0: u32, y1: u32| (y0..y1).all(|y| close(img.pixel(x, y)));

    let mut top = 0;
    while top < h && row_uniform(top) {
        top += 1;
    }
    if top == h {
        return (0, 0, 0, 0); // entirely uniform: nothing to trim to
    }
    let mut bottom = 0;
    while row_uniform(h - 1 - bottom) {
        bottom += 1;
    }
    let (y0, y1) = (top, h - bottom);
    let mut left = 0;
    while col_uniform(left, y0, y1) {
        left += 1;
    }
    let mut right = 0;
    while col_uniform(w - 1 - right, y0, y1) {
        right += 1;
    }
    (left, top, right, bottom)
}

pub(super) fn trim(p: &Trim, frames: Frames) -> Result<Frames, String> {
    let info = frames.info();
    // One rectangle for every frame: the smallest insets of all of them.
    let (mut l, mut t, mut r, mut b) = (u32::MAX, u32::MAX, u32::MAX, u32::MAX);
    for f in frames.frames() {
        let (fl, ft, fr, fb) = uniform_insets(&f.image, p.threshold);
        (l, t, r, b) = (l.min(fl), t.min(ft), r.min(fr), b.min(fb));
    }
    let (l, t) = (l.saturating_sub(p.padding), t.saturating_sub(p.padding));
    let (r, b) = (r.saturating_sub(p.padding), b.saturating_sub(p.padding));
    if (l, t, r, b) == (0, 0, 0, 0) {
        return Ok(frames);
    }
    let (w, h) = (info.width - l - r, info.height - t - b);
    frames.map_images(|img| Ok(crop_image(&img, l, t, w, h)))
}

pub(super) fn quarter_turns(degrees: i32) -> Result<u32, String> {
    if degrees % 90 != 0 {
        return Err(format!("degrees must be a multiple of 90, got {degrees}"));
    }
    Ok((degrees.rem_euclid(360) / 90) as u32)
}

pub(super) fn rotate(p: &Rotate, frames: Frames) -> Result<Frames, String> {
    let turns = quarter_turns(p.degrees)?;
    if turns == 0 {
        return Ok(frames);
    }
    frames.map_images(|img| {
        let (w, h) = (img.width(), img.height());
        let (nw, nh) = if turns % 2 == 1 { (h, w) } else { (w, h) };
        let mut out = Rgba8::new(nw, nh);
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = match turns {
                    1 => (h - 1 - y, x),
                    2 => (w - 1 - x, h - 1 - y),
                    _ => (y, w - 1 - x),
                };
                out.set_pixel(nx, ny, img.pixel(x, y));
            }
        }
        Ok(out)
    })
}

pub(super) fn flip(p: &Flip, frames: Frames) -> Result<Frames, String> {
    frames.map_images(|mut img| {
        let (w, h) = (img.width() as usize, img.height() as usize);
        if w == 0 || h == 0 {
            return Ok(img);
        }
        let data = img.data_mut();
        match p.axis {
            Axis::Horizontal => {
                for row in data.chunks_exact_mut(w * 4) {
                    for x in 0..w / 2 {
                        for c in 0..4 {
                            row.swap(x * 4 + c, (w - 1 - x) * 4 + c);
                        }
                    }
                }
            }
            Axis::Vertical => {
                let stride = w * 4;
                for y in 0..h / 2 {
                    let (top, rest) = data.split_at_mut((h - 1 - y) * stride);
                    top[y * stride..(y + 1) * stride].swap_with_slice(&mut rest[..stride]);
                }
            }
        }
        Ok(img)
    })
}

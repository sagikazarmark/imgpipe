//! Resize with `fast_image_resize`: 16-bit linear light, premultiplied alpha
//! by default; straight 8-bit sRGB when `linear = false`.

use super::{Filter, Fit, Resize};
use crate::color::{from_linear_premul_ne_bytes, to_linear_premul_ne_bytes};
use crate::frame::{Frames, Rgba8};
use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

pub(super) struct ResizePlan {
    pub width: u32,
    pub height: u32,
    /// Crop the source to the target aspect ratio (cover).
    pub cover: bool,
    /// Centre the result on a canvas of this size (pad).
    pub canvas: Option<(u32, u32)>,
}

pub(super) fn plan(p: &Resize, sw: u32, sh: u32) -> Result<ResizePlan, String> {
    let both = p.width.zip(p.height);
    if p.width == Some(0) || p.height == Some(0) || p.max_long_side == Some(0) {
        return Err("sizes must be greater than 0".into());
    }
    if both.is_some() && p.max_long_side.is_some() {
        return Err("use either width and height, or max_long_side, not both".into());
    }
    if p.width.is_none() && p.height.is_none() && p.max_long_side.is_none() {
        return Err("set width, height or max_long_side".into());
    }
    if let Some((w, h)) = both {
        match p.fit {
            Fit::Exact => {
                return Ok(ResizePlan {
                    width: w,
                    height: h,
                    cover: false,
                    canvas: None,
                });
            }
            Fit::Cover => {
                return Ok(ResizePlan {
                    width: w,
                    height: h,
                    cover: true,
                    canvas: None,
                });
            }
            Fit::Contain | Fit::Pad => {}
        }
    } else if p.fit != Fit::Contain {
        return Err("cover, exact and pad need both width and height".into());
    }

    let (sw_f, sh_f) = (sw as f64, sh as f64);
    let mut scale = match (p.width, p.height) {
        (Some(w), Some(h)) => (w as f64 / sw_f).min(h as f64 / sh_f),
        (Some(w), None) => w as f64 / sw_f,
        (None, Some(h)) => h as f64 / sh_f,
        (None, None) => f64::INFINITY,
    };
    if let Some(m) = p.max_long_side {
        scale = scale.min(m as f64 / sw_f.max(sh_f));
    }
    if !p.allow_upscale {
        scale = scale.min(1.0);
    }
    let width = (sw_f * scale).round().max(1.0) as u32;
    let height = (sh_f * scale).round().max(1.0) as u32;
    let canvas = (p.fit == Fit::Pad).then(|| both.expect("checked above"));
    Ok(ResizePlan {
        width,
        height,
        cover: false,
        canvas,
    })
}

/// Working memory of one frame's resize beyond its input and output frames,
/// an upper bound (checked against a counting allocator in
/// `tests/memory.rs`):
///
/// - Linear light: the 16-bit premultiplied source (8 bytes per source
///   pixel) and the 16-bit destination (8 per output pixel).
/// - sRGB: the premultiplied copy of the source `fast_image_resize` makes
///   when it handles alpha (4 per source pixel).
/// - Both: the resizer's buffer between its two passes, at most
///   `max(src_w × dst_h, dst_w × src_h)` pixels (which one depends on the
///   pixel type), and its filter coefficients, a few dozen bytes per source
///   and destination row and column.
/// - `fit = "pad"`: the resized frame before it is centred on the canvas.
pub(super) fn scratch_bytes(p: &Resize, plan: &ResizePlan, sw: u32, sh: u32) -> u64 {
    let (sw, sh) = (sw as u64, sh as u64);
    let (dw, dh) = (plan.width as u64, plan.height as u64);
    let (src_px, dst_px) = (sw * sh, dw * dh);
    let pass = (sw * dh).max(dw * sh);
    let copies = if p.linear {
        src_px
            .saturating_mul(8)
            .saturating_add(dst_px.saturating_mul(8))
            .saturating_add(pass.saturating_mul(8))
    } else {
        src_px
            .saturating_mul(4)
            .saturating_add(pass.saturating_mul(4))
    };
    let coefficients = (sw + sh + dw + dh).saturating_mul(128);
    let centring = if plan.canvas.is_some() {
        dst_px.saturating_mul(4)
    } else {
        0
    };
    copies.saturating_add(coefficients).saturating_add(centring)
}

fn filter(f: Filter) -> FilterType {
    match f {
        Filter::Lanczos3 => FilterType::Lanczos3,
        Filter::CatmullRom => FilterType::CatmullRom,
        Filter::Mitchell => FilterType::Mitchell,
        Filter::Hamming => FilterType::Hamming,
        Filter::Bilinear => FilterType::Bilinear,
        Filter::Box => FilterType::Box,
    }
}

pub(super) fn apply(p: &Resize, frames: Frames) -> Result<Frames, String> {
    let info = frames.info();
    let plan = plan(p, info.width, info.height)?;
    let frames = if (plan.width, plan.height) == (info.width, info.height) && !plan.cover {
        frames
    } else {
        resize_frames(p, &plan, frames)?
    };
    match plan.canvas {
        Some((cw, ch)) => frames.map_images(|img| Ok(centre_on(&img, cw, ch, p.background.0))),
        None => Ok(frames),
    }
}

fn resize_frames(p: &Resize, plan: &ResizePlan, frames: Frames) -> Result<Frames, String> {
    let mut resizer = Resizer::new();
    let mut options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter(p.filter)));
    if plan.cover {
        options = options.fit_into_destination(Some((0.5, 0.5)));
    }
    frames.map_images(|img| {
        if p.linear {
            resize_linear(&mut resizer, &options, &img, plan.width, plan.height)
        } else {
            resize_srgb(&mut resizer, &options, img, plan.width, plan.height)
        }
    })
}

/// `img` centred on a `w`×`h` canvas of `bg`.
fn centre_on(img: &Rgba8, w: u32, h: u32, bg: [u8; 4]) -> Rgba8 {
    let mut out = Rgba8::filled(w, h, bg);
    let (x0, y0) = ((w - img.width()) / 2, (h - img.height()) / 2);
    let stride = w as usize * 4;
    for y in 0..img.height() {
        let start = (y0 + y) as usize * stride + x0 as usize * 4;
        out.data_mut()[start..start + img.width() as usize * 4].copy_from_slice(img.row(y));
    }
    out
}

fn resize_linear(
    resizer: &mut Resizer,
    options: &ResizeOptions,
    img: &Rgba8,
    w: u32,
    h: u32,
) -> Result<Rgba8, String> {
    // Already premultiplied, so the resizer must not multiply again.
    let options = options.clone().use_alpha(false);
    // Straight into the native-endian bytes the resizer takes: one 8-byte
    // per pixel copy of the source, not two.
    let bytes = to_linear_premul_ne_bytes(img.data());
    let src = Image::from_vec_u8(img.width(), img.height(), bytes, PixelType::U16x4)
        .map_err(|e| e.to_string())?;
    let mut dst = Image::new(w, h, PixelType::U16x4);
    resizer
        .resize(&src, &mut dst, &options)
        .map_err(|e| e.to_string())?;
    drop(src);
    let mut out = Rgba8::new(w, h);
    from_linear_premul_ne_bytes(dst.buffer(), out.data_mut());
    Ok(out)
}

fn resize_srgb(
    resizer: &mut Resizer,
    options: &ResizeOptions,
    img: Rgba8,
    w: u32,
    h: u32,
) -> Result<Rgba8, String> {
    // The resizer premultiplies U8x4 itself when `use_alpha` is on (default).
    let (sw, sh) = (img.width(), img.height());
    let src =
        Image::from_vec_u8(sw, sh, img.into_raw(), PixelType::U8x4).map_err(|e| e.to_string())?;
    let mut dst = Image::new(w, h, PixelType::U8x4);
    resizer
        .resize(&src, &mut dst, options)
        .map_err(|e| e.to_string())?;
    Rgba8::from_raw(w, h, dst.into_vec())
        .ok_or_else(|| "resize produced a wrong-sized buffer".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rs(width: Option<u32>, height: Option<u32>, max: Option<u32>, fit: Fit) -> Resize {
        Resize {
            width,
            height,
            max_long_side: max,
            fit,
            linear: true,
            ..Default::default()
        }
    }

    #[test]
    fn plan_sizes() {
        let p = |r: Resize| plan(&r, 1600, 900).map(|p| (p.width, p.height));
        assert_eq!(p(rs(None, None, Some(800), Fit::Contain)), Ok((800, 450)));
        assert_eq!(p(rs(Some(400), None, None, Fit::Contain)), Ok((400, 225)));
        assert_eq!(
            p(rs(Some(400), Some(400), None, Fit::Contain)),
            Ok((400, 225))
        );
        assert_eq!(
            p(rs(Some(400), Some(400), None, Fit::Cover)),
            Ok((400, 400))
        );
        // Never upscale in proportional modes unless asked.
        assert_eq!(p(rs(None, None, Some(3200), Fit::Contain)), Ok((1600, 900)));
        assert!(p(rs(Some(10), None, Some(10), Fit::Contain)).is_ok());
        assert!(p(rs(Some(10), Some(10), Some(10), Fit::Contain)).is_err());
        assert!(p(rs(None, None, None, Fit::Contain)).is_err());
        assert!(p(rs(Some(10), None, None, Fit::Cover)).is_err());
    }

    #[test]
    fn linear_downscale_keeps_average_light() {
        // A 1-px black/white checkerboard averages to 50% *light*, which is
        // sRGB ~188, not 128. Filtering sRGB values would give ~128.
        let mut img = Rgba8::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                img.set_pixel(x, y, [v, v, v, 255]);
            }
        }
        let frames = Frames::still(img);
        let r = Resize {
            max_long_side: Some(8),
            filter: Filter::Box,
            ..rs(None, None, Some(8), Fit::Contain)
        };
        let linear = apply(&r, frames.clone()).unwrap();
        let v = linear.first().pixel(4, 4)[0];
        assert!((185..=191).contains(&v), "linear average {v}");

        let srgb = apply(&Resize { linear: false, ..r }, frames).unwrap();
        let v = srgb.first().pixel(4, 4)[0];
        assert!((125..=130).contains(&v), "sRGB average {v}");
    }

    #[test]
    fn transparent_pixels_do_not_darken_edges() {
        // Left half opaque red, right half transparent black. Straight-alpha
        // filtering would pull black into the red edge.
        let mut img = Rgba8::new(32, 4);
        for y in 0..4 {
            for x in 0..16 {
                img.set_pixel(x, y, [255, 0, 0, 255]);
            }
        }
        let r = rs(Some(8), None, None, Fit::Contain);
        let out = apply(&r, Frames::still(img)).unwrap();
        for x in 0..8 {
            let [red, g, b, a] = out.first().pixel(x, 0);
            if a > 0 {
                assert!(red >= 250 && g == 0 && b == 0, "x={x}: {red} {g} {b} {a}");
            }
        }
    }
}

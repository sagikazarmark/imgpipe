//! Unsharp mask in 16-bit linear light with premultiplied alpha.

use super::Sharpen;
use crate::color::{from_linear_premul, to_linear_premul};
use crate::frame::{Frames, Rgba8};

fn kernel(sigma: f32) -> Vec<f32> {
    let radius = (sigma * 3.0).ceil() as i32;
    let mut k: Vec<f32> = (-radius..=radius)
        // libm, not f32::exp, so native and wasm builds agree (see `color`).
        .map(|i| libm::expf(-(i * i) as f32 / (2.0 * sigma * sigma)))
        .collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

/// One separable blur pass over RGB (alpha is not blurred).
fn blur_pass(src: &[u16], dst: &mut [u16], w: usize, h: usize, k: &[f32], horizontal: bool) {
    let r = (k.len() / 2) as isize;
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 3];
            for (i, kv) in k.iter().enumerate() {
                let d = i as isize - r;
                let (sx, sy) = if horizontal {
                    ((x as isize + d).clamp(0, w as isize - 1) as usize, y)
                } else {
                    (x, (y as isize + d).clamp(0, h as isize - 1) as usize)
                };
                let s = (sy * w + sx) * 4;
                for c in 0..3 {
                    acc[c] += src[s + c] as f32 * kv;
                }
            }
            let o = (y * w + x) * 4;
            for c in 0..3 {
                dst[o + c] = acc[c].round() as u16;
            }
            dst[o + 3] = src[o + 3];
        }
    }
}

pub(super) fn apply(p: &Sharpen, frames: Frames) -> Result<Frames, String> {
    if p.amount == 0.0 {
        return Ok(frames);
    }
    let k = kernel(p.sigma);
    let threshold = p.threshold as f32 * 257.0;
    frames.map_images(|img| {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let orig = to_linear_premul(img.data());
        let mut tmp = vec![0u16; orig.len()];
        let mut blurred = vec![0u16; orig.len()];
        blur_pass(&orig, &mut tmp, w, h, &k, true);
        blur_pass(&tmp, &mut blurred, w, h, &k, false);
        drop(tmp);

        // The result goes into `blurred`, each value read just before it is
        // overwritten, so no third frame-sized buffer is needed.
        for (px, o) in blurred
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(orig.as_chunks::<4>().0)
        {
            let a = o[3] as f32;
            for c in 0..3 {
                let (orig_c, blur_c) = (o[c] as f32, px[c] as f32);
                let diff = orig_c - blur_c;
                px[c] = if diff.abs() > threshold {
                    // Premultiplied colour can never exceed alpha.
                    (orig_c + p.amount * diff).clamp(0.0, a).round() as u16
                } else {
                    o[c]
                };
            }
            px[3] = o[3];
        }
        drop(orig);
        let mut out = Rgba8::new(w as u32, h as u32);
        from_linear_premul(&blurred, out.data_mut());
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_image_is_unchanged_and_edges_gain_contrast() {
        let mut img = Rgba8::filled(16, 4, [100, 100, 100, 255]);
        let flat = apply(
            &Sharpen {
                id: None,
                sigma: 1.0,
                amount: 1.0,
                threshold: 0,
            },
            Frames::still(img.clone()),
        )
        .unwrap();
        assert_eq!(flat.first(), &img);

        for y in 0..4 {
            for x in 8..16 {
                img.set_pixel(x, y, [200, 200, 200, 255]);
            }
        }
        let out = apply(
            &Sharpen {
                id: None,
                sigma: 1.0,
                amount: 1.0,
                threshold: 0,
            },
            Frames::still(img),
        )
        .unwrap();
        assert!(
            out.first().pixel(7, 1)[0] < 100,
            "dark side of the edge gets darker"
        );
        assert!(out.first().pixel(8, 1)[0] > 200, "light side gets lighter");
        assert_eq!(
            out.first().pixel(0, 1)[0],
            100,
            "far from the edge is untouched"
        );
    }
}

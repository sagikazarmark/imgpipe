//! Brightness, contrast, saturation and gamma on sRGB values (CSS-like).

use super::Adjust;
use crate::frame::Frames;

pub(super) fn apply(p: &Adjust, frames: Frames) -> Result<Frames, String> {
    if p.brightness == 1.0 && p.contrast == 1.0 && p.saturation == 1.0 && p.gamma == 1.0 {
        return Ok(frames);
    }
    // Brightness and contrast before saturation, gamma last.
    let tone = |v: f32| ((v * p.brightness) - 0.5) * p.contrast + 0.5;
    let pre: Vec<f32> = (0..256)
        .map(|i| tone(i as f32 / 255.0).clamp(0.0, 1.0))
        .collect();
    let gamma = 1.0 / p.gamma;
    // libm, not f32::powf, so native and wasm builds agree (see `color`).
    let post = |v: f32| (libm::powf(v.clamp(0.0, 1.0), gamma) * 255.0).round() as u8;
    let s = p.saturation;

    frames.map_images(|mut img| {
        for px in img.data_mut().as_chunks_mut::<4>().0 {
            let (r, g, b) = (
                pre[px[0] as usize],
                pre[px[1] as usize],
                pre[px[2] as usize],
            );
            let (r, g, b) = if s == 1.0 {
                (r, g, b)
            } else {
                // Rec. 709 luma weights, as CSS saturate() uses.
                let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                (l + (r - l) * s, l + (g - l) * s, l + (b - l) * s)
            };
            px[0] = post(r);
            px[1] = post(g);
            px[2] = post(b);
        }
        Ok(img)
    })
}

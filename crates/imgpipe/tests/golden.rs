//! Golden images for the filtering steps, checked natively and under
//! `wasm-bindgen-test` with two bounds: the largest per-channel difference
//! and the number of pixels that differ at all.
//!
//! Regenerate (natively): UPDATE_GOLDENS=1 cargo test -p imgpipe --test golden
#![cfg(feature = "png")]

mod common;

use imgpipe::steps::{Adjust, Filter, Length, Pad, Resize, Rotate, Sharpen};
use imgpipe::{Color, Ctx, Frames, Pipeline, Rgba8, Step};

/// A synthetic "photo": gradients, fine stripes and a soft-edged transparent hole.
fn source() -> Rgba8 {
    let (w, h) = (96u32, 64u32);
    let mut img = Rgba8::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let stripe = if (x + y) % 6 < 3 { 40 } else { 0 };
            let r = (x * 255 / w) as u8;
            let g = ((y * 255 / h) as u8).saturating_add(stripe);
            let b = (((x + y) * 255 / (w + h)) as u8) ^ 0x55;
            let (dx, dy) = (x as f32 - 60.0, y as f32 - 30.0);
            let d = (dx * dx + dy * dy).sqrt();
            let a = ((d - 10.0) * 64.0).clamp(0.0, 255.0) as u8;
            img.set_pixel(x, y, [r, g, b, a]);
        }
    }
    img
}

fn run(steps: Vec<Step>) -> Rgba8 {
    Pipeline::new(steps)
        .run(Frames::still(source()), &Ctx::default())
        .unwrap()
        .into_first()
}

macro_rules! golden {
    ($test:ident, $name:literal, $steps:expr) => {
        #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
        #[cfg_attr(not(target_arch = "wasm32"), test)]
        fn $test() {
            common::check($name, golden_png!($name), run($steps), 2);
        }
    };
}

golden!(
    resize_linear,
    "resize-linear",
    vec![Step::Resize(Resize {
        max_long_side: Some(40),
        ..Default::default()
    })]
);
golden!(
    resize_srgb_catmull_rom,
    "resize-srgb-catmullrom",
    vec![Step::Resize(Resize {
        width: Some(50),
        filter: Filter::CatmullRom,
        linear: false,
        ..Default::default()
    })]
);
golden!(
    resize_upscale,
    "resize-upscale",
    vec![Step::Resize(Resize {
        max_long_side: Some(150),
        allow_upscale: true,
        ..Default::default()
    })]
);
golden!(
    sharpen,
    "sharpen",
    vec![Step::Sharpen(Sharpen {
        id: None,
        sigma: 1.2,
        amount: 0.8,
        threshold: 2
    })]
);
golden!(
    adjust,
    "adjust",
    vec![Step::Adjust(Adjust {
        id: None,
        brightness: 1.1,
        contrast: 1.2,
        saturation: 0.6,
        gamma: 1.3
    })]
);
golden!(
    pad_rotate,
    "pad-rotate",
    vec![
        Step::Pad(Pad {
            id: None,
            top: Length::Percent(25.0),
            right: Length::Px(0),
            bottom: Length::Px(4),
            left: Length::Px(0),
            color: Color::WHITE
        }),
        Step::Rotate(Rotate {
            id: None,
            degrees: 270
        }),
    ]
);

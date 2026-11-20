//! Behaviour of the core steps through the public pipeline API.

use imgpipe::steps::{Adjust, Axis, Crop, Flip, Length, Pad, Rotate, Trim};
use imgpipe::{Color, Ctx, Frame, FrameInfo, Frames, Pipeline, Rgba8, Step};

fn run(steps: Vec<Step>, frames: Frames) -> Frames {
    Pipeline::new(steps).run(frames, &Ctx::default()).unwrap()
}

/// `w`×`h` of `bg` with an opaque red rectangle at (x, y, rw, rh).
fn boxed(w: u32, h: u32, bg: [u8; 4], (x, y, rw, rh): (u32, u32, u32, u32)) -> Rgba8 {
    let mut img = Rgba8::filled(w, h, bg);
    for yy in y..y + rh {
        for xx in x..x + rw {
            img.set_pixel(xx, yy, [255, 0, 0, 255]);
        }
    }
    img
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn pad_adds_a_caption_band() {
    let out = run(
        vec![Step::Pad(Pad {
            id: None,
            top: Length::Percent(25.0),
            right: Length::Px(0),
            bottom: Length::Px(0),
            left: Length::Px(0),
            color: Color::WHITE,
        })],
        Frames::still(Rgba8::filled(40, 40, [0, 0, 255, 255])),
    );
    assert_eq!(
        out.info(),
        FrameInfo {
            width: 40,
            height: 50,
            frames: 1
        }
    );
    assert_eq!(out.first().pixel(5, 5), [255, 255, 255, 255]);
    assert_eq!(out.first().pixel(5, 20), [0, 0, 255, 255]);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn crop_accepts_pixels_and_fractions() {
    let img = boxed(100, 50, [0, 0, 0, 255], (10, 10, 20, 20));
    let out = run(
        vec![Step::Crop(Crop {
            id: None,
            rect: [
                Length::Px(10),
                Length::Px(10),
                Length::Percent(20.0),
                Length::Percent(40.0),
            ],
        })],
        Frames::still(img),
    );
    assert_eq!(
        out.info(),
        FrameInfo {
            width: 20,
            height: 20,
            frames: 1
        }
    );
    assert!(
        out.first()
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 0, 0, 255])
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn trim_removes_uniform_borders_with_padding() {
    let img = boxed(100, 80, [255, 255, 255, 255], (20, 10, 30, 40));
    let out = run(
        vec![Step::Trim(Trim {
            id: None,
            threshold: 10,
            padding: 2,
        })],
        Frames::still(img),
    );
    assert_eq!(
        out.info(),
        FrameInfo {
            width: 34,
            height: 44,
            frames: 1
        }
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn trim_uses_one_rectangle_for_all_frames() {
    let f1 = boxed(100, 100, [255, 255, 255, 255], (10, 10, 20, 20));
    let f2 = boxed(100, 100, [255, 255, 255, 255], (60, 60, 20, 20));
    let frames = Frames::new(
        vec![
            Frame {
                image: f1,
                delay_ms: 100,
            },
            Frame {
                image: f2,
                delay_ms: 100,
            },
        ],
        None,
    )
    .unwrap();
    let out = run(
        vec![Step::Trim(Trim {
            id: None,
            threshold: 10,
            padding: 0,
        })],
        frames,
    );
    // The union of both boxes: x 10..80, y 10..80, and every frame the same size.
    assert_eq!(
        out.info(),
        FrameInfo {
            width: 70,
            height: 70,
            frames: 2
        }
    );
    assert_eq!(out.frames()[1].delay_ms, 100);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn full_turns_and_double_flips_are_identities() {
    let img = boxed(7, 5, [10, 20, 30, 255], (1, 1, 2, 3));
    let frames = Frames::still(img.clone());
    let four = (0..4)
        .map(|_| {
            Step::Rotate(Rotate {
                id: None,
                degrees: 90,
            })
        })
        .collect();
    assert_eq!(run(four, frames.clone()).first(), &img);
    let flips = vec![
        Step::Flip(Flip {
            id: None,
            axis: Axis::Horizontal,
        }),
        Step::Flip(Flip {
            id: None,
            axis: Axis::Vertical,
        }),
        Step::Rotate(Rotate {
            id: None,
            degrees: 180,
        }),
    ];
    assert_eq!(run(flips, frames).first(), &img);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn adjust_defaults_change_nothing_and_saturation_zero_is_grey() {
    let img = boxed(8, 8, [30, 140, 220, 255], (0, 0, 4, 4));
    let neutral = Adjust {
        id: None,
        brightness: 1.0,
        contrast: 1.0,
        saturation: 1.0,
        gamma: 1.0,
    };
    assert_eq!(
        run(
            vec![Step::Adjust(neutral.clone())],
            Frames::still(img.clone())
        )
        .first(),
        &img
    );

    let grey = run(
        vec![Step::Adjust(Adjust {
            saturation: 0.0,
            ..neutral
        })],
        Frames::still(img),
    );
    let [r, g, b, _] = grey.first().pixel(6, 6);
    assert!(r == g && g == b, "({r},{g},{b})");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn resize_pad_letterboxes_to_an_exact_size() {
    use imgpipe::steps::{Fit, Resize};
    let src = Rgba8::filled(400, 800, [0, 0, 255, 255]);
    let step = Step::Resize(Resize {
        width: Some(1200),
        height: Some(630),
        fit: Fit::Pad,
        linear: true,
        background: Color::WHITE,
        ..Default::default()
    });
    let out = run(vec![step], Frames::still(src));
    assert_eq!(
        out.info(),
        FrameInfo {
            width: 1200,
            height: 630,
            frames: 1
        }
    );
    assert_eq!(
        out.first().pixel(10, 300),
        [255, 255, 255, 255],
        "letterbox"
    );
    assert_eq!(
        out.first().pixel(600, 300),
        [0, 0, 255, 255],
        "content, centred"
    );
}

/// 64-bit FNV-1a.
fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn colour_tables_are_identical_on_every_target() {
    use imgpipe::color::{linear16_to_srgb8, srgb8_to_linear16};
    // Values where an f32 `powf` rounds the wrong way on some platform: the
    // exact results are 10417.498… and 213.49999…, so these are correctly
    // rounded. (glibc gave 10418 for the first, wasm's libm 214 for the second.)
    assert_eq!(srgb8_to_linear16(111), 10417);
    assert_eq!(linear16_to_srgb8(43837), 213);
    assert_eq!(
        [0, 1, 10, 128, 254, 255].map(srgb8_to_linear16),
        [0, 20, 199, 14_146, 64_952, 65_535]
    );
    assert_eq!(
        [0, 1, 13, 14, 1000, 32768, 65534, 65535].map(linear16_to_srgb8),
        [0, 0, 1, 1, 33, 188, 255, 255]
    );
    let to_linear = fnv((0..=255).flat_map(|v| srgb8_to_linear16(v).to_le_bytes()));
    let to_srgb = fnv((0..=u16::MAX).map(linear16_to_srgb8));
    assert_eq!(
        (to_linear, to_srgb),
        (0xfc9f_2a74_dc5b_7d64, 0x8c01_7252_713a_a969),
        "table hashes (to_linear, to_srgb): {to_linear:#x}, {to_srgb:#x}"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn adjust_gamma_is_identical_on_every_target() {
    // Per-pixel `powf` in Adjust: a hash of a full ramp pins the result.
    let mut img = Rgba8::new(256, 4);
    for x in 0..256 {
        for y in 0..4 {
            let v = x as u8;
            img.set_pixel(x, y, [v, v.wrapping_mul(7), 255 - v, 255]);
        }
    }
    let out = run(
        vec![Step::Adjust(Adjust {
            id: None,
            brightness: 1.05,
            contrast: 1.1,
            saturation: 0.7,
            gamma: 1.37,
        })],
        Frames::still(img),
    );
    let hash = fnv(out.first().data().iter().copied());
    assert_eq!(hash, 0x18f4_254b_a3fa_dce5, "adjust hash {hash:#x}");
}

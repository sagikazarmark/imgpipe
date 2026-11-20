//! Golden-image helpers shared by the integration tests.
#![allow(dead_code)]

use imgpipe::{DecodeOptions, Rgba8, decode};

/// Two bounds, as browser reftests use: the largest per-channel difference
/// and the number of pixels that differ at all.
pub fn assert_close(name: &str, actual: &Rgba8, golden: &Rgba8, max_delta: u8, max_pixels: usize) {
    assert_eq!(
        (actual.width(), actual.height()),
        (golden.width(), golden.height()),
        "{name}: size"
    );
    let mut differing = 0;
    let mut worst = 0;
    for (a, g) in actual
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(golden.data().as_chunks::<4>().0)
    {
        let d = a.iter().zip(g).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
        worst = worst.max(d);
        differing += usize::from(d > 0);
    }
    assert!(
        worst <= max_delta && differing <= max_pixels,
        "{name}: max channel difference {worst} (allowed {max_delta}), {differing} differing pixels (allowed {max_pixels})"
    );
}

/// Compares `actual` with the golden PNG, or rewrites it natively when
/// `UPDATE_GOLDENS` is set. Allows SIMD rounding differences between
/// targets: per-channel difference ≤ `max_delta` on ≤ 0.5% of pixels.
pub fn check(name: &str, golden_png: &[u8], actual: Rgba8, max_delta: u8) {
    #[cfg(not(target_arch = "wasm32"))]
    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        let png = imgpipe::Encoders::builtin()
            .encode(
                &imgpipe::Frames::still(actual),
                &imgpipe::EncodeOptions::new(imgpipe::Format::Png),
            )
            .unwrap();
        std::fs::write(
            format!("{}/tests/golden/{name}.png", env!("CARGO_MANIFEST_DIR")),
            png,
        )
        .unwrap();
        return;
    }
    assert!(
        !golden_png.is_empty(),
        "{name}: no golden yet; run with UPDATE_GOLDENS=1 natively"
    );
    let golden = decode(golden_png, &DecodeOptions::default())
        .unwrap()
        .into_first();
    let tolerance_pixels = (golden.width() * golden.height()) as usize / 200;
    assert_close(name, &actual, &golden, max_delta, tolerance_pixels);
}

#[macro_export]
macro_rules! golden_png {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/golden/",
            $name,
            ".png"
        ))
    };
}

/// A white, black-outlined, all-caps caption that fits its box: the style
/// most overlay tests and goldens use.
#[cfg(feature = "overlay")]
pub fn classic(
    rect: [f32; 4],
    text: impl Into<String>,
    fonts: &[&str],
) -> imgpipe::overlay::TextBox {
    use imgpipe::Color;
    use imgpipe::overlay::{FontSize, HAlign, Stroke, TextBox, VAlign};
    TextBox {
        rect,
        text: text.into(),
        lang: None,
        fonts: fonts.iter().map(|f| f.to_string()).collect(),
        size: FontSize::Fit {
            min: 0.035,
            max: 0.11,
        },
        fill: Color::WHITE,
        stroke: Some(Stroke {
            color: Color::BLACK,
            width: 0.08,
        }),
        align: HAlign::Center,
        valign: VAlign::Middle,
        uppercase: true,
        line_height: 1.2,
        rotation_deg: 0.0,
    }
}

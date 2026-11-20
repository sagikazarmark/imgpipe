//! Text overlay: fit, breaking, fallback, casing, and golden renders.
//! Runs natively and under `wasm-bindgen-test`.
#![cfg(all(feature = "overlay", feature = "png"))]

mod common;

use common::classic;
use imgpipe::overlay::{
    FontRegistry, FontSize, HAlign, Stroke, TextBox, TextLayout, TextRenderer, TextStep, VAlign,
    draw_onto, frame_width,
};
use imgpipe::{Color, Ctx, Frames, Pipeline, Rgba8, Step};
use std::collections::HashMap;

// SIL Open Font License 1.1; the licence texts are next to the fonts.
const ANTON: &[u8] = include_bytes!("fonts/Anton-Regular.ttf");
const OSWALD: &[u8] = include_bytes!("fonts/Oswald[wght].ttf");

fn fonts() -> FontRegistry {
    let mut f = FontRegistry::new();
    f.register("anton", ANTON.to_vec()).unwrap();
    f.register_instance("oswald-bold", OSWALD.to_vec(), &[("wght", 700.0)])
        .unwrap();
    f
}

fn layout(b: &TextBox) -> TextLayout {
    TextRenderer::new().layout(b, &fonts(), 1.0).unwrap()
}

const BOTTOM: [f32; 4] = [0.04, 0.68, 0.92, 0.3];

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn chosen_size_is_the_largest_that_fits() {
    let texts = [
        "One does not simply",
        "When the code works on the first try and you have no idea why it works",
        "Legénységfelülvizsgálatáért ő és ű betűkkel",
        "a b c d e f g h i j k l m n o p",
    ];
    let mut checked = 0;
    for text in texts {
        for w in [0.3, 0.6, 0.92] {
            let b = classic([0.04, 0.6, w, 0.3], text, &["anton"]);
            let l = layout(&b);
            if l.overflow {
                continue; // e.g. a 27-letter word in a narrow box: covered by the split test
            }
            checked += 1;
            let FontSize::Fit { max, .. } = b.size else {
                unreachable!()
            };
            if l.size < max * 1000.0 {
                // One grid step larger must not fit.
                let bigger = TextBox {
                    size: FontSize::Fixed((l.size + 0.5) / 1000.0),
                    ..b.clone()
                };
                assert!(
                    layout(&bigger).overflow,
                    "{text:?} at width {w}: size {} + step still fits",
                    l.size
                );
            }
        }
    }
    assert!(checked >= 10, "only {checked} cases fit");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn newline_is_a_mandatory_break() {
    let l = layout(&classic(BOTTOM, "top\nbottom", &["anton"]));
    assert_eq!(l.lines.len(), 2);
    assert_eq!(l.break_signature()[1], "BOTTOM".len());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn missing_glyphs_fall_back_or_are_reported() {
    let fonts = fonts();
    let anton_only = classic(BOTTOM, "Привет мир", &["anton"]);
    let l = TextRenderer::new()
        .layout(&anton_only, &fonts, 1.0)
        .unwrap();
    assert!(
        l.missing.contains(&'П'),
        "Anton has no Cyrillic: {:?}",
        l.missing
    );

    let with_fallback = classic(BOTTOM, "Привет мир", &["anton", "oswald-bold"]);
    let l = TextRenderer::new()
        .layout(&with_fallback, &fonts, 1.0)
        .unwrap();
    assert!(l.missing.is_empty(), "{:?}", l.missing);
    let used: std::collections::HashSet<usize> = l
        .lines
        .iter()
        .flat_map(|ln| ln.glyphs.iter().map(|g| g.font))
        .collect();
    assert_eq!(
        used.len(),
        1,
        "every Cyrillic glyph comes from the fallback font"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn turkish_uppercase_uses_dotted_capital_i() {
    let mut tr = classic(BOTTOM, "istanbul", &["anton"]);
    tr.lang = Some("tr".into());
    let expected = TextBox {
        uppercase: false,
        ..classic(BOTTOM, "İSTANBUL", &["anton"])
    };
    assert_eq!(
        layout(&tr).break_signature(),
        layout(&expected).break_signature()
    );
    assert_eq!(
        layout(&tr).lines[0].glyphs[0].glyph,
        layout(&expected).lines[0].glyphs[0].glyph
    );
    assert_ne!(
        layout(&tr).lines[0].glyphs[0].glyph,
        layout(&classic(BOTTOM, "istanbul", &["anton"])).lines[0].glyphs[0].glyph,
        "İ and I are different glyphs"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn an_overlong_word_is_split_only_at_the_minimum_size() {
    let b = classic(
        [0.4, 0.1, 0.2, 0.8],
        "Legénységfelülvizsgálatáért",
        &["anton"],
    );
    let l = layout(&b);
    assert!(l.overflow, "does not fit unsplit even at the minimum size");
    assert!(l.lines.len() > 1, "split across lines");
    let box_w = 0.2 * 1000.0;
    assert!(
        l.lines.iter().all(|ln| ln.width <= box_w + 0.01),
        "every piece fits the box width"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rebalancing_avoids_a_lone_last_word() {
    // Fixed size, so the line count is forced; greedy would leave "SEVEN" alone.
    let mut b = classic(
        [0.0, 0.0, 1.0, 1.0],
        "one two three four five six seven",
        &["anton"],
    );
    b.size = FontSize::Fixed(0.12);
    let l = layout(&b);
    assert_eq!(l.lines.len(), 2);
    let (a, c) = (l.lines[0].width, l.lines[1].width);
    assert!(c > a * 0.6, "lines are balanced: {a:.0} vs {c:.0}");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn text_step_draws_with_vars() {
    let fonts = fonts();
    let vars = HashMap::from([("who".to_string(), "me".to_string())]);
    let step = Step::Text(TextStep {
        id: None,
        boxes: vec![classic(BOTTOM, "{who}", &["anton"])],
    });
    let cx = Ctx::new().with_fonts(&fonts).with_vars(&vars);
    let out = Pipeline::new(vec![step.clone()])
        .run(
            Frames::still(Rgba8::filled(200, 200, [0, 0, 255, 255])),
            &cx,
        )
        .unwrap();
    let white = out
        .first()
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| **p == [255, 255, 255, 255])
        .count();
    assert!(white > 50, "caption drawn ({white} white pixels)");

    let err = Pipeline::new(vec![step])
        .run(Frames::still(Rgba8::filled(8, 8, [0; 4])), &Ctx::default())
        .unwrap_err();
    assert!(err.to_string().contains("needs fonts"), "{err}");
}

/// A gradient base with soft diagonal bands, like a photo behind captions.
fn base(w: u32, h: u32) -> Rgba8 {
    let mut img = Rgba8::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let band = if (x + y) / 24 % 2 == 0 { 20 } else { 0 };
            img.set_pixel(
                x,
                y,
                [
                    (60 + x * 120 / w) as u8 + band,
                    (90 + y * 60 / h) as u8,
                    160,
                    255,
                ],
            );
        }
    }
    img
}

fn render(boxes: Vec<TextBox>, w: u32, h: u32) -> Rgba8 {
    let fonts = fonts();
    let mut renderer = TextRenderer::new();
    let aspect = w as f32 / h as f32;
    let laid: Vec<_> = boxes
        .into_iter()
        .map(|b| {
            let l = renderer.layout(&b, &fonts, aspect).unwrap();
            (b, l)
        })
        .collect();
    let mut img = base(w, h);
    draw_onto(
        &mut img,
        laid.iter().map(|(b, l)| (b, l)),
        &fonts,
        &mut renderer,
    )
    .unwrap();
    img
}

macro_rules! golden {
    ($test:ident, $name:literal, $w:expr, $h:expr, $boxes:expr) => {
        #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
        #[cfg_attr(not(target_arch = "wasm32"), test)]
        fn $test() {
            common::check($name, golden_png!($name), render($boxes, $w, $h), 3);
        }
    };
}

golden!(
    golden_classic,
    "text-classic",
    320,
    320,
    vec![
        TextBox {
            valign: VAlign::Top,
            ..classic([0.04, 0.02, 0.92, 0.3], "one does not simply", &["anton"])
        },
        TextBox {
            valign: VAlign::Bottom,
            ..classic(BOTTOM, "write a meme generator in a weekend", &["anton"])
        },
    ]
);
golden!(
    golden_hungarian,
    "text-hungarian",
    320,
    320,
    vec![classic(
        [0.04, 0.5, 0.92, 0.48],
        "Árvíztűrő tükörfúrógép, ő és ű",
        &["anton"]
    )]
);
golden!(
    golden_fallback,
    "text-fallback",
    320,
    180,
    vec![classic(
        [0.05, 0.1, 0.9, 0.8],
        "Привет, Ő!",
        &["anton", "oswald-bold"]
    )]
);
golden!(
    golden_modern,
    "text-modern",
    320,
    240,
    vec![TextBox {
        rect: [0.05, 0.05, 0.9, 0.4],
        text: "When the meeting could have been an email".into(),
        lang: Some("en".into()),
        fonts: vec!["oswald-bold".into()],
        size: FontSize::Fit {
            min: 0.03,
            max: 0.09
        },
        fill: Color([17, 17, 17, 255]),
        stroke: None,
        align: HAlign::Left,
        valign: VAlign::Top,
        uppercase: false,
        line_height: 1.2,
        rotation_deg: 0.0,
    }]
);
golden!(
    golden_rotated,
    "text-rotated",
    320,
    320,
    vec![TextBox {
        rotation_deg: 12.5,
        ..classic([0.2, 0.35, 0.6, 0.3], "change my mind", &["anton"])
    }]
);

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn preview_and_export_resolutions_render_the_same_text() {
    use imgpipe::steps::Resize;
    let fonts = fonts();
    let text = Step::Text(TextStep {
        id: None,
        boxes: vec![classic(
            BOTTOM,
            "When the code works on the first try and you have no idea why",
            &["anton"],
        )],
    });
    let cx = Ctx::new().with_fonts(&fonts);
    let render = |px: u32| {
        Pipeline::new(vec![text.clone()])
            .run(
                Frames::still(Rgba8::filled(px, px, [60, 90, 160, 255])),
                &cx,
            )
            .unwrap()
    };

    // The layout only sees the aspect ratio, so breaks are equal by construction.
    let layout = |_px: u32| {
        TextRenderer::new()
            .layout(
                &classic(
                    BOTTOM,
                    "When the code works on the first try and you have no idea why",
                    &["anton"],
                ),
                &fonts,
                1.0,
            )
            .unwrap()
    };
    assert_eq!(
        layout(400).break_signature(),
        layout(1600).break_signature()
    );

    // And the pixels agree: the 1600 px export, scaled down, matches the 400 px preview.
    // A box filter on sRGB values reproduces tiny-skia's coverage antialiasing.
    let preview = render(400).into_first();
    let export = Pipeline::new(vec![Step::Resize(Resize {
        width: Some(400),
        filter: imgpipe::steps::Filter::Box,
        linear: false,
        ..Default::default()
    })])
    .run(render(1600), &Ctx::default())
    .unwrap()
    .into_first();
    let diff: u64 = preview
        .data()
        .iter()
        .zip(export.data())
        .map(|(a, b)| a.abs_diff(*b) as u64)
        .sum();
    let mean = diff as f64 / preview.data().len() as f64;
    assert!(mean < 1.0, "mean channel difference {mean:.2}");
}

/// Rotated, stroked, multi-line boxes with accents and descenders, some
/// reaching past the frame edge.
/// A transparent frame with one box drawn on it (straight alpha).
fn draw_full(layout: &TextLayout, b: &TextBox, fonts: &FontRegistry, w: u32, h: u32) -> Rgba8 {
    let mut img = Rgba8::new(w, h);
    draw_onto(&mut img, [(b, layout)], fonts, &mut TextRenderer::new()).unwrap();
    img
}

fn incremental_cases() -> Vec<TextBox> {
    vec![
        TextBox {
            rotation_deg: -18.0,
            ..classic(
                [0.3, 0.35, 0.4, 0.3],
                "Árvíztűrő\ntükörfúrógép ŐŰ",
                &["anton"],
            )
        },
        TextBox {
            rect: [0.05, 0.5, 0.6, 0.45],
            text: "gjpqy Ágy\nquite jumpy Привет".into(),
            lang: None,
            fonts: vec!["oswald-bold".into()],
            size: FontSize::Fit {
                min: 0.03,
                max: 0.2,
            },
            fill: Color([250, 200, 20, 255]),
            stroke: Some(Stroke {
                color: Color([20, 20, 120, 200]),
                width: 0.25,
            }),
            align: HAlign::Left,
            valign: VAlign::Bottom,
            uppercase: false,
            line_height: 0.9,
            rotation_deg: 45.0,
        },
        TextBox {
            rotation_deg: 133.0,
            ..classic([0.0, 0.0, 0.5, 0.4], "corner case\nqueue", &["anton"])
        },
        TextBox {
            size: FontSize::Fixed(0.3),
            rotation_deg: 7.3,
            ..classic([0.6, 0.7, 0.4, 0.3], "far too wide", &["anton"])
        },
    ]
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn pixel_bounds_contain_every_drawn_pixel() {
    let fonts = fonts();
    let mut renderer = TextRenderer::new();
    for aspect in [4.0 / 3.0, 0.6] {
        for b in incremental_cases() {
            let layout = renderer.layout(&b, &fonts, aspect).unwrap();
            assert!(layout.lines.len() > 1 || b.text == "far too wide");
            for h in [120, 333, 800] {
                let w = frame_width(aspect, h);
                let r = layout.pixel_bounds(&b, aspect, h).unwrap();
                assert!(
                    r.x + r.width <= w && r.y + r.height <= h,
                    "{r:?} within {w}x{h}"
                );
                let full = draw_full(&layout, &b, &fonts, w, h);
                let data = full.data();
                let mut drawn = 0;
                for y in 0..h {
                    for x in 0..w {
                        let inside =
                            x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
                        let a = data[((y * w + x) * 4 + 3) as usize];
                        drawn += usize::from(a != 0);
                        assert!(
                            inside || a == 0,
                            "{:?} at {h}px: pixel ({x}, {y}) is outside {r:?}",
                            b.text
                        );
                    }
                }
                assert!(drawn > 0, "{:?} at {h}px draws something", b.text);
            }
        }
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rasterize_matches_a_full_frame_draw() {
    let fonts = fonts();
    let mut renderer = TextRenderer::new();
    let (aspect, h) = (4.0 / 3.0, 333);
    let w = frame_width(aspect, h);
    for b in incremental_cases() {
        let layout = renderer.layout(&b, &fonts, aspect).unwrap();
        let px = renderer.rasterize(&b, &layout, &fonts, aspect, h).unwrap();
        let r = px.rect;
        assert_eq!(r, layout.pixel_bounds(&b, aspect, h).unwrap());
        assert_eq!(px.rgba.len(), (r.width * r.height * 4) as usize);
        let full = draw_full(&layout, &b, &fonts, w, h).data().to_vec();
        // tiny-skia's anti-aliasing can round a few edge pixels differently
        // at another offset; everything else is identical.
        let differing = (0..r.height)
            .flat_map(|y| (0..r.width).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let i = ((y * r.width + x) * 4) as usize;
                let j = (((r.y + y) * w + r.x + x) * 4) as usize;
                px.rgba[i..i + 4] != full[j..j + 4]
            })
            .count();
        let area = (r.width * r.height) as usize;
        assert!(
            differing <= (area / 1000).max(4),
            "{:?}: {differing} of {area} pixels differ",
            b.text
        );
    }
}

/// Random pixels, every alpha from transparent to opaque.
fn noise(w: u32, h: u32, opaque: bool) -> Rgba8 {
    let mut img = Rgba8::new(w, h);
    let mut x = 0x2545_f491u32;
    for px in img.data_mut().as_chunks_mut::<4>().0 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *px = x.to_le_bytes();
        if opaque {
            px[3] = 255;
        }
    }
    img
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn text_leaves_pixels_it_does_not_cover_bit_identical() {
    // Drawing used to premultiply and demultiply the whole frame at 8 bits:
    // a far-away [200, 100, 50, 7] came back as [182, 109, 36, 7].
    let fonts = fonts();
    let mut renderer = TextRenderer::new();
    let (w, h) = (300, 225);
    let aspect = w as f32 / h as f32;
    let boxes: Vec<_> = incremental_cases()
        .into_iter()
        .take(3)
        .map(|b| {
            let l = renderer.layout(&b, &fonts, aspect).unwrap();
            (b, l)
        })
        .collect();
    let rects: Vec<_> = boxes
        .iter()
        .map(|(b, l)| l.pixel_bounds(b, aspect, h).unwrap())
        .collect();
    let mut base = noise(w, h, false);
    base.set_pixel(w - 1, h - 1, [200, 100, 50, 7]);
    let mut img = base.clone();
    draw_onto(
        &mut img,
        boxes.iter().map(|(b, l)| (b, l)),
        &fonts,
        &mut renderer,
    )
    .unwrap();
    let (mut outside, mut changed) = (0, 0);
    for y in 0..h {
        for x in 0..w {
            let inside = rects
                .iter()
                .any(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height);
            if !inside {
                outside += 1;
                assert_eq!(img.pixel(x, y), base.pixel(x, y), "({x}, {y})");
            }
            changed += usize::from(img.pixel(x, y) != base.pixel(x, y));
        }
    }
    assert!(
        outside > 1000 && changed > 1000,
        "{outside} outside, {changed} changed"
    );
    assert_eq!(img.pixel(w - 1, h - 1), [200, 100, 50, 7]);
}

/// The browser preview: each layer's `rasterize` pixels go through
/// `putImageData` (premultiplied at 8 bits) and the compositor blends them
/// over the opaque base image, in layer order.
fn browser_composite(base: &Rgba8, layers: &[imgpipe::overlay::RasterizedText]) -> Rgba8 {
    let mut out = base.clone();
    for layer in layers {
        let r = layer.rect;
        for y in 0..r.height {
            for x in 0..r.width {
                let i = ((y * r.width + x) * 4) as usize;
                let s = &layer.rgba[i..i + 4];
                let a = s[3] as u32;
                let d = out.pixel(r.x + x, r.y + y);
                let mix = |k: usize| {
                    let premul = (s[k] as u32 * a + 127) / 255;
                    ((premul * 255 + d[k] as u32 * (255 - a) + 127) / 255) as u8
                };
                out.set_pixel(r.x + x, r.y + y, [mix(0), mix(1), mix(2), 255]);
            }
        }
    }
    out
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn export_composites_like_the_browser_preview() {
    // Same layers, same size: export and the browser's compositing of the
    // preview layers must agree exactly, also for a semi-transparent stroke.
    let fonts = fonts();
    let mut renderer = TextRenderer::new();
    let (w, h) = (300, 225);
    let aspect = w as f32 / h as f32;
    let boxes: Vec<_> = incremental_cases()
        .into_iter()
        .map(|b| {
            let l = renderer.layout(&b, &fonts, aspect).unwrap();
            (b, l)
        })
        .collect();
    let layers: Vec<_> = boxes
        .iter()
        .map(|(b, l)| renderer.rasterize(b, l, &fonts, aspect, h).unwrap())
        .collect();
    let base = noise(w, h, true);
    let mut export = base.clone();
    draw_onto(
        &mut export,
        boxes.iter().map(|(b, l)| (b, l)),
        &fonts,
        &mut renderer,
    )
    .unwrap();
    let preview = browser_composite(&base, &layers);
    assert!(
        export == preview,
        "export differs from the composited preview"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn nothing_to_draw_has_no_bounds() {
    let fonts = fonts();
    let mut renderer = TextRenderer::new();
    let b = classic(BOTTOM, "", &["anton"]);
    let layout = renderer.layout(&b, &fonts, 1.0).unwrap();
    assert_eq!(layout.pixel_bounds(&b, 1.0, 100), None);
    assert_eq!(renderer.rasterize(&b, &layout, &fonts, 1.0, 100), None);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_glyph_wider_than_the_box_overflows_without_panicking() {
    let b = TextBox {
        size: FontSize::Fixed(0.5),
        ..classic([0.45, 0.1, 0.1, 0.8], "WM", &["anton"])
    };
    let l = layout(&b);
    assert!(l.overflow);
    assert_eq!(l.break_signature(), [1, 1], "one glyph per line");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn text_steps_parse_from_toml_and_need_a_size() {
    // As in the README.
    let toml = r##"
version = 1

[[steps]]
op = "text"

[[steps.boxes]]
rect = [0.05, 0.7, 0.9, 0.25]
text = "{caption}"
fonts = ["anton"]
size = { fit = { min = 0.04, max = 0.12 } }
stroke = { color = "#000000", width = 0.08 }
valign = "bottom"
uppercase = true
"##;
    let file: imgpipe::config::PipelineFile = toml::from_str(toml).unwrap();
    let Step::Text(step) = &file.pipeline.steps[0] else {
        panic!("{:?}", file.pipeline.steps[0]);
    };
    let b = &step.boxes[0];
    assert_eq!(
        b.size,
        FontSize::Fit {
            min: 0.04,
            max: 0.12
        }
    );
    assert_eq!(
        (b.fill, b.align, b.line_height),
        (Color::WHITE, HAlign::Center, 1.2)
    );

    let fixed: TextBox = toml::from_str(
        "rect = [0, 0, 1, 1]\ntext = \"x\"\nfonts = [\"a\"]\nsize = { fixed = 0.08 }",
    )
    .unwrap();
    assert_eq!(fixed.size, FontSize::Fixed(0.08));
    let err = toml::from_str::<TextBox>("rect = [0, 0, 1, 1]\ntext = \"x\"\nfonts = [\"a\"]")
        .unwrap_err();
    assert!(err.to_string().contains("size"), "{err}");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn text_errors_are_typed() {
    use imgpipe::overlay::{BoxProblem, TextError};
    let fonts = fonts();
    let mut renderer = TextRenderer::new();
    let mut b = classic([0.0, 0.0, 1.0, 0.2], "hi", &["nope"]);
    assert_eq!(
        renderer.layout(&b, &fonts, 1.0),
        Err(TextError::UnknownFont("nope".into()))
    );
    b.rect = [0.5, 0.0, 0.8, 0.2];
    assert_eq!(
        renderer.layout(&b, &fonts, 1.0),
        Err(TextError::InvalidBox(BoxProblem::Rect))
    );
    assert_eq!(
        FontRegistry::new().register("x", vec![1, 2, 3]),
        Err(TextError::UnreadableFont("x".into()))
    );
    // Messages stay readable for people.
    assert_eq!(
        BoxProblem::Rect.to_string(),
        "rect must be within 0..1 with a positive size"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn pipeline_text_errors_keep_their_type() {
    use imgpipe::overlay::TextError;
    use imgpipe::{Error, StepError};
    let fonts = fonts();
    let step = Step::Text(TextStep::new(vec![classic(
        [0.0, 0.0, 1.0, 0.2],
        "hi",
        &["nope"],
    )]));
    let frames = Frames::still(Rgba8::filled(8, 8, [0; 4]));
    let got = Pipeline::new(vec![step.clone()]).run(frames.clone(), &Ctx::new().with_fonts(&fonts));
    match got {
        Err(Error::Step {
            index: 0,
            op: "text",
            source: StepError::Text(TextError::UnknownFont(id)),
        }) => assert_eq!(id, "nope"),
        other => panic!("{other:?}"),
    }
    let got = Pipeline::new(vec![step]).run(frames, &Ctx::new());
    assert!(
        matches!(
            got,
            Err(Error::Step {
                source: StepError::NoFonts,
                ..
            })
        ),
        "{got:?}"
    );
}

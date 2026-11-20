//! `Pipeline::validate`'s peak-memory prediction against what each step
//! really allocates, measured with a counting global allocator.
//!
//! Native only, and the only test in this binary: the allocator counts every
//! thread, so nothing else may run while a step is measured.
#![cfg(not(target_arch = "wasm32"))]

use imgpipe::steps::{
    Adjust, Axis, Crop, Filter, Fit, Flip, Length, Pad, Resize, Rotate, Sharpen, Trim,
};
use imgpipe::{Color, Ctx, Frame, Frames, Limits, Pipeline, Rgba8, Step};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(live, Relaxed);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(live, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, new_size) };
        if !q.is_null() {
            // Counted as alloc-then-free: both blocks may exist at once.
            let live = LIVE.fetch_add(new_size, Relaxed) + new_size;
            PEAK.fetch_max(live, Relaxed);
            LIVE.fetch_sub(layout.size(), Relaxed);
        }
        q
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Allocations that are not pixels (the plan, step bookkeeping, the frame
/// list, a text step's shaping and outline caches): the budget is for
/// pixels, and these stay far below it.
const BOOKKEEPING: u64 = 64 << 10;

/// A noisy, partly transparent image, so no step takes a shortcut.
fn noise(w: u32, h: u32) -> Rgba8 {
    let mut img = Rgba8::new(w, h);
    let mut x = 0x9e37_79b9u32;
    for px in img.data_mut().as_chunks_mut::<4>().0 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *px = x.to_le_bytes();
        px[3] |= 0x80;
    }
    img
}

/// Bordered, for trim: a uniform frame around the noise.
fn bordered(w: u32, h: u32) -> Rgba8 {
    let inner = noise(w - 40, h - 40);
    let mut img = Rgba8::filled(w, h, [255, 255, 255, 255]);
    for y in 0..inner.height() {
        for x in 0..inner.width() {
            img.set_pixel(x + 20, y + 20, inner.pixel(x, y));
        }
    }
    img
}

/// Runs `steps` on `input` and returns (predicted peak, measured peak).
fn measure(steps: Vec<Step>, input: Frames, cx: &Ctx) -> (u64, u64) {
    let pipeline = Pipeline::new(steps);
    let input_bytes = input.info().bytes();
    let plan = pipeline
        .validate(&input.info(), &Limits::default())
        .unwrap();
    let before = LIVE.load(Relaxed);
    PEAK.store(before, Relaxed);
    let out = pipeline.run(input, cx).unwrap();
    let peak = PEAK.load(Relaxed);
    drop(out);
    (plan.peak_bytes, (peak - before) as u64 + input_bytes)
}

fn resize(f: impl FnOnce(&mut Resize)) -> Step {
    let mut r = Resize::default();
    f(&mut r);
    Step::Resize(r)
}

#[test]
fn predicted_peak_memory_covers_every_step() {
    let still = |w, h| Frames::still(noise(w, h));
    let animation = || {
        let frames = (0..3)
            .map(|_| Frame {
                image: noise(400, 300),
                delay_ms: 100,
            })
            .collect();
        Frames::new(frames, None).unwrap()
    };
    let mut cases: Vec<(&str, Vec<Step>, Frames)> = vec![
        (
            "resize linear, 10x down",
            vec![resize(|r| r.max_long_side = Some(120))],
            still(1200, 900),
        ),
        (
            "resize sRGB, 10x down",
            vec![resize(|r| {
                r.max_long_side = Some(120);
                r.linear = false;
            })],
            still(1200, 900),
        ),
        (
            "resize linear, 3x up",
            vec![resize(|r| {
                r.max_long_side = Some(600);
                r.allow_upscale = true;
            })],
            still(200, 150),
        ),
        (
            "resize sRGB, 3x up, Catmull-Rom",
            vec![resize(|r| {
                r.max_long_side = Some(600);
                r.allow_upscale = true;
                r.linear = false;
                r.filter = Filter::CatmullRom;
            })],
            still(200, 150),
        ),
        (
            "resize cover",
            vec![resize(|r| {
                r.width = Some(300);
                r.height = Some(300);
                r.fit = Fit::Cover;
            })],
            still(1200, 600),
        ),
        (
            "resize pad",
            vec![resize(|r| {
                r.width = Some(300);
                r.height = Some(300);
                r.fit = Fit::Pad;
            })],
            still(1200, 600),
        ),
        (
            "resize, animation",
            vec![resize(|r| r.max_long_side = Some(100))],
            animation(),
        ),
        (
            "sharpen",
            vec![Step::Sharpen(Sharpen {
                id: None,
                sigma: 1.5,
                amount: 0.6,
                threshold: 1,
            })],
            still(600, 400),
        ),
        (
            "pad",
            vec![Step::Pad(Pad {
                id: None,
                top: Length::Percent(25.0),
                right: Length::Px(10),
                bottom: Length::Px(10),
                left: Length::Px(10),
                color: Color::WHITE,
            })],
            still(800, 600),
        ),
        (
            "crop",
            vec![Step::Crop(Crop {
                id: None,
                rect: [
                    Length::Px(10),
                    Length::Px(10),
                    Length::Percent(50.0),
                    Length::Percent(50.0),
                ],
            })],
            still(800, 600),
        ),
        (
            "rotate",
            vec![Step::Rotate(Rotate {
                id: None,
                degrees: 90,
            })],
            still(800, 600),
        ),
        (
            "flip",
            vec![Step::Flip(Flip {
                id: None,
                axis: Axis::Vertical,
            })],
            still(800, 600),
        ),
        (
            "trim",
            vec![Step::Trim(Trim {
                id: None,
                threshold: 0,
                padding: 0,
            })],
            Frames::still(bordered(800, 600)),
        ),
        (
            "adjust",
            vec![Step::Adjust(Adjust {
                id: None,
                brightness: 1.1,
                contrast: 0.9,
                saturation: 1.2,
                gamma: 0.8,
            })],
            still(800, 600),
        ),
    ];

    #[cfg(feature = "overlay")]
    let fonts = {
        let mut f = imgpipe::overlay::FontRegistry::new();
        f.register("anton", include_bytes!("fonts/Anton-Regular.ttf").to_vec())
            .unwrap();
        f
    };
    #[cfg(feature = "overlay")]
    cases.push((
        "text",
        vec![Step::Text(imgpipe::overlay::TextStep {
            id: None,
            boxes: vec![imgpipe::overlay::TextBox {
                rect: [0.0, 0.0, 1.0, 1.0],
                text: "a box as large as the frame".into(),
                lang: None,
                fonts: vec!["anton".into()],
                size: imgpipe::overlay::FontSize::Fixed(0.3),
                fill: Color::WHITE,
                stroke: Some(imgpipe::overlay::Stroke {
                    color: Color::BLACK,
                    width: 0.1,
                }),
                align: Default::default(),
                valign: Default::default(),
                uppercase: true,
                line_height: 1.0,
                rotation_deg: 10.0,
            }],
        })],
        still(800, 600),
    ));
    let cx = Ctx::new();
    #[cfg(feature = "overlay")]
    let cx = cx.with_fonts(&fonts);

    let mut report = Vec::new();
    for (name, steps, input) in cases {
        let (predicted, actual) = measure(steps, input, &cx);
        report.push(format!(
            "{name}: predicted {predicted} B, measured {actual} B ({:.0}%)",
            actual as f64 * 100.0 / predicted as f64
        ));
        assert!(
            actual <= predicted + BOOKKEEPING,
            "{name}: measured {actual} B > predicted {predicted} B\n{}",
            report.join("\n")
        );
    }
    println!("{}", report.join("\n"));
}

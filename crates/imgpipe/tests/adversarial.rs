//! Hostile pipelines, parameters and frames: every case must come back as an
//! error (or a bounded result), never a panic, an abort or a write outside
//! the sink's root.

use imgpipe::steps::{Adjust, Axis, Crop, Fit, Flip, Length, Pad, Resize, Rotate, Sharpen, Trim};
use imgpipe::{
    Color, Ctx, EncodeOptions, Encoders, Error, Format, Frame, FrameInfo, Frames, Limits,
    MemoryBudget, Pipeline, Rgba8, Step,
};

fn still(w: u32, h: u32) -> Frames {
    Frames::still(Rgba8::filled(w, h, [10, 20, 30, 255]))
}

fn pad(top: Length, right: Length, bottom: Length, left: Length) -> Step {
    Step::Pad(Pad {
        id: None,
        top,
        right,
        bottom,
        left,
        color: Color::WHITE,
    })
}

fn resize(w: u32, h: u32, fit: Fit, allow_upscale: bool) -> Step {
    Step::Resize(Resize {
        width: Some(w),
        height: Some(h),
        fit,
        allow_upscale,
        ..Default::default()
    })
}

/// Which error a case must produce.
#[derive(Debug)]
enum Want {
    /// Validation fails on this step.
    InvalidStep(usize),
    /// Validation fails on the input itself.
    InvalidInput,
    /// The plan does not fit the budget.
    Budget,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn adversarial_pipelines_fail_cleanly() {
    let gib = MemoryBudget::bytes(1 << 30);
    let px = Length::Px;
    let cases: Vec<(&str, Vec<Step>, Frames, MemoryBudget, Want)> = vec![
        (
            "pad left u32::MAX - 5 (overflows u32)",
            vec![pad(px(0), px(0), px(0), px(u32::MAX - 5))],
            still(10, 10),
            gib,
            Want::InvalidStep(0),
        ),
        (
            "pad top and bottom u32::MAX",
            vec![pad(px(u32::MAX), px(0), px(u32::MAX), px(0))],
            still(10, 10),
            MemoryBudget::UNLIMITED,
            Want::InvalidStep(0),
        ),
        (
            "pad past the maximum side",
            vec![pad(px(0), px(Limits::DEFAULT_MAX_SIDE), px(0), px(0))],
            still(10, 10),
            MemoryBudget::UNLIMITED,
            Want::InvalidStep(0),
        ),
        (
            "second step's pad overflows",
            vec![
                pad(px(0), px(0), px(0), px(100)),
                pad(px(0), px(u32::MAX - 50), px(0), px(0)),
            ],
            still(10, 10),
            gib,
            Want::InvalidStep(1),
        ),
        (
            "resize 2^31 x 2^31 exact (used to predict 12 bytes)",
            vec![resize(1 << 31, 1 << 31, Fit::Exact, false)],
            still(1, 1),
            gib,
            Want::InvalidStep(0),
        ),
        (
            "resize u32::MAX cover",
            vec![resize(u32::MAX, u32::MAX, Fit::Cover, false)],
            still(4, 4),
            MemoryBudget::UNLIMITED,
            Want::InvalidStep(0),
        ),
        (
            "resize 100000 x 100000 exact, default ctx (used to abort)",
            vec![resize(100_000, 100_000, Fit::Exact, false)],
            still(4, 4),
            MemoryBudget::DEFAULT,
            Want::InvalidStep(0),
        ),
        (
            "resize pad onto a huge canvas",
            vec![resize(100_000, 10, Fit::Pad, false)],
            still(4, 4),
            MemoryBudget::DEFAULT,
            Want::InvalidStep(0),
        ),
        (
            "upscale contain to u32::MAX",
            vec![resize(u32::MAX, u32::MAX, Fit::Contain, true)],
            still(4, 4),
            MemoryBudget::UNLIMITED,
            Want::InvalidStep(0),
        ),
        (
            "upscale max_long_side to u32::MAX",
            vec![Step::Resize(Resize {
                max_long_side: Some(u32::MAX),
                allow_upscale: true,
                ..Default::default()
            })],
            still(3, 2),
            MemoryBudget::UNLIMITED,
            Want::InvalidStep(0),
        ),
        (
            "upscale past max_total_pixels within the side limit",
            vec![resize(16_000, 16_000, Fit::Exact, false)],
            still(4, 4),
            MemoryBudget::UNLIMITED,
            Want::InvalidStep(0),
        ),
        (
            "within limits but over the budget",
            vec![resize(8_000, 8_000, Fit::Exact, false)],
            still(4, 4),
            MemoryBudget::bytes(1 << 20),
            Want::Budget,
        ),
        (
            "NaN sharpen",
            vec![Step::Sharpen(Sharpen {
                id: None,
                sigma: f32::NAN,
                amount: 1.0,
                threshold: 0,
            })],
            still(4, 4),
            gib,
            Want::InvalidStep(0),
        ),
        (
            "infinite adjust",
            vec![Step::Adjust(Adjust {
                id: None,
                brightness: f32::INFINITY,
                contrast: 1.0,
                saturation: 1.0,
                gamma: 1.0,
            })],
            still(4, 4),
            gib,
            Want::InvalidStep(0),
        ),
        (
            "NaN pad fraction",
            vec![pad(Length::Percent(f32::NAN), px(0), px(0), px(0))],
            still(4, 4),
            gib,
            Want::InvalidStep(0),
        ),
        (
            "crop outside the image",
            vec![Step::Crop(Crop {
                id: None,
                rect: [px(u32::MAX), px(0), px(u32::MAX), px(u32::MAX)],
            })],
            still(4, 4),
            gib,
            Want::InvalidStep(0),
        ),
    ];

    // Every step on an image with no pixels (used to panic in flip and trim).
    let zero_steps = [
        Step::Flip(Flip {
            id: None,
            axis: Axis::Horizontal,
        }),
        Step::Flip(Flip {
            id: None,
            axis: Axis::Vertical,
        }),
        Step::Trim(Trim {
            id: None,
            threshold: 10,
            padding: 0,
        }),
        Step::Rotate(Rotate {
            id: None,
            degrees: 90,
        }),
        Step::Resize(Resize {
            max_long_side: Some(10),
            ..Default::default()
        }),
        Step::Sharpen(Sharpen {
            id: None,
            sigma: 1.0,
            amount: 1.0,
            threshold: 0,
        }),
        pad(Length::Percent(50.0), px(1), px(0), px(0)),
    ];
    let zero_cases = zero_steps.into_iter().flat_map(|s| {
        [(0, 5), (5, 0), (0, 0)].map(|(w, h)| {
            (
                "step on an empty image",
                vec![s.clone()],
                still(w, h),
                MemoryBudget::DEFAULT,
                Want::InvalidInput,
            )
        })
    });
    let empty_pipeline = (
        "no steps on an empty image",
        vec![],
        still(0, 3),
        MemoryBudget::DEFAULT,
        Want::InvalidInput,
    );

    for (name, steps, frames, budget, want) in
        cases.into_iter().chain(zero_cases).chain([empty_pipeline])
    {
        let cx = Ctx::new().with_budget(budget);
        let input = frames.info();
        let got = Pipeline::new(steps).run(frames, &cx);
        match (&want, &got) {
            (Want::InvalidStep(i), Err(Error::Invalid(d))) if d[0].step == Some(*i) => {}
            (Want::InvalidInput, Err(Error::Invalid(d))) if d[0].step.is_none() => {}
            (Want::Budget, Err(Error::Budget { .. })) => {}
            _ => panic!("{name} on {input:?}: wanted {want:?}, got {got:?}"),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn validate_never_overflows() {
    let mut unlimited = Limits::default();
    unlimited.max_width = u32::MAX;
    unlimited.max_height = u32::MAX;
    unlimited.max_frames = u32::MAX;
    unlimited.max_total_pixels = u64::MAX;
    unlimited.max_alloc = u64::MAX;
    let huge = FrameInfo {
        width: u32::MAX,
        height: u32::MAX,
        frames: u32::MAX,
    };
    assert_eq!(huge.bytes(), u64::MAX, "bytes saturate");

    // With limits lifted, sizes and peaks saturate instead of wrapping, and
    // u32 overflow is still a diagnostic.
    let p = Pipeline::new(vec![
        Step::Sharpen(Sharpen {
            id: None,
            sigma: 1.0,
            amount: 1.0,
            threshold: 0,
        }),
        resize(u32::MAX, u32::MAX, Fit::Exact, false),
    ]);
    let plan = p.validate(&huge, &unlimited).unwrap();
    assert_eq!(plan.peak_bytes, u64::MAX);
    let overflow = Pipeline::new(vec![pad(
        Length::Px(1),
        Length::Px(0),
        Length::Px(0),
        Length::Px(0),
    )]);
    let d = overflow.validate(&huge, &unlimited).unwrap_err();
    assert!(d[0].message.contains("overflows"), "{d:?}");

    // A saturated peak never fits, even an "unlimited" budget.
    let cx = Ctx::new()
        .with_budget(MemoryBudget::UNLIMITED)
        .with_limits(unlimited);
    assert!(MemoryBudget::UNLIMITED.effective() <= isize::MAX as u64);
    let big = Pipeline::new(vec![resize(u32::MAX, u32::MAX, Fit::Exact, false)]);
    assert!(matches!(
        big.run(still(1, 1), &cx),
        Err(Error::Budget { .. })
    ));

    // The input is checked against the limits too.
    let d = Pipeline::default()
        .validate(&huge, &Limits::default())
        .unwrap_err();
    assert_eq!(d[0].step, None);
}

/// On wasm32 the budget is capped at `isize::MAX` (2 GiB): a plan the target
/// cannot address is refused even with an unlimited budget and raised limits.
#[cfg(target_pointer_width = "32")]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn budget_is_capped_by_the_address_space() {
    let mut limits = Limits::default();
    limits.max_width = 40_000;
    limits.max_height = 40_000;
    limits.max_total_pixels = u64::MAX;
    let cx = Ctx::new()
        .with_budget(MemoryBudget::UNLIMITED)
        .with_limits(limits);
    let p = Pipeline::new(vec![resize(30_000, 30_000, Fit::Exact, false)]);
    assert!(matches!(p.run(still(1, 1), &cx), Err(Error::Budget { .. })));
    assert!(Rgba8::try_new(70_000, 70_000).is_none());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rgba8_constructors_check_the_size() {
    // 2^31 * 2^31 * 4 wraps to 0 in 64 bits: used to accept an empty buffer.
    assert!(Rgba8::from_raw(1 << 31, 1 << 31, vec![]).is_none());
    assert!(Rgba8::from_raw(u32::MAX, u32::MAX, vec![]).is_none());
    assert!(Rgba8::try_new(u32::MAX, u32::MAX).is_none());
    assert!(Rgba8::try_filled(u32::MAX, u32::MAX, [0; 4]).is_none());
    assert!(Rgba8::from_raw(2, 2, vec![0; 15]).is_none());
    let img = Rgba8::try_filled(3, 2, [1, 2, 3, 4]).unwrap();
    assert_eq!(img.data().len(), 24);
    assert_eq!(img.pixel(2, 1), [1, 2, 3, 4]);
    assert_eq!(Rgba8::try_new(0, 7).unwrap().data().len(), 0);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn frames_new_enforces_its_invariants() {
    let frame = |w, h| Frame {
        image: Rgba8::filled(w, h, [0, 0, 0, 255]),
        delay_ms: 100,
    };
    assert!(matches!(
        Frames::new(vec![], None),
        Err(Error::InvalidFrames(_))
    ));
    assert!(matches!(
        Frames::new(vec![frame(20, 20), frame(5, 5)], None),
        Err(Error::InvalidFrames(_))
    ));
    assert!(matches!(
        Frames::new(vec![frame(0, 5)], None),
        Err(Error::InvalidFrames(_))
    ));
    let ok = Frames::new(vec![frame(4, 3), frame(4, 3)], Some(2)).unwrap();
    assert_eq!(
        ok.info(),
        FrameInfo {
            width: 4,
            height: 3,
            frames: 2
        }
    );
    assert_eq!(ok.loop_count(), Some(2));
    assert_eq!(ok.frames().len(), 2);
    assert_eq!(ok.into_frames()[1].delay_ms, 100);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn encoders_reject_empty_images() {
    for opts in [
        EncodeOptions::new(Format::Png),
        EncodeOptions::new(Format::Jpeg),
        EncodeOptions::new(Format::Webp).lossless(true),
    ] {
        for (w, h) in [(0, 0), (0, 4), (4, 0)] {
            let got = Encoders::builtin().encode(&still(w, h), &opts);
            assert!(
                matches!(got, Err(Error::Encode(_))),
                "{opts:?} {w}x{h}: {got:?}"
            );
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn fs_sink_stays_inside_its_root() {
    use imgpipe::variants::{FsSink, Sink};

    let base = std::env::temp_dir().join(format!("imgpipe-fs-sink-{}", std::process::id()));
    let root = base.join("out");
    std::fs::create_dir_all(&root).unwrap();
    let mut sink = FsSink { root: root.clone() };

    let mut escapes = vec!["", "../escaped.png", "a/../../escaped.png", "./x.png", ".."];
    let absolute = base.join("abs.png").to_string_lossy().into_owned();
    escapes.push(&absolute);
    #[cfg(windows)]
    escapes.extend(["C:escaped.png", r"\\server\share\x.png", r"..\escaped.png"]);
    for name in escapes {
        let got = sink.write(name, b"data".to_vec());
        assert!(
            matches!(got, Err(Error::OutputName(_))),
            "{name:?}: {got:?}"
        );
    }
    assert!(!base.join("escaped.png").exists());
    assert!(!base.join("abs.png").exists());

    // Plain relative names, nested ones included, still work.
    sink.write("a/b/c.png", b"data".to_vec()).unwrap();
    assert_eq!(std::fs::read(root.join("a/b/c.png")).unwrap(), b"data");

    std::fs::remove_dir_all(&base).unwrap();
}

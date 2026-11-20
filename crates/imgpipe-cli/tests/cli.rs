//! Runs the built `imgpipe` binary on small generated images and pipelines.

use imgpipe::{DecodeOptions, EncodeOptions, Encoders, Format, Frames, Rgba8, decode};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PIPELINE: &str = r#"
version = 1

[[steps]]
op = "resize"
max_long_side = 100

[[outputs]]
name = "{stem}/full.png"
format = "png"

[[outputs]]
name = "{stem}/thumb-{w}x{h}.webp"
format = "webp"
lossless = true
tail = [{ op = "resize", max_long_side = 20 }]
"#;

/// A fresh directory for one test under Cargo's per-target temp dir.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_png(path: &Path, width: u32, height: u32) {
    let img = Rgba8::filled(width, height, [200, 40, 40, 255]);
    let png = Encoders::builtin()
        .encode(&Frames::still(img), &EncodeOptions::new(Format::Png))
        .unwrap();
    std::fs::write(path, png).unwrap();
}

fn size_of(path: &Path) -> (u32, u32) {
    let frames = decode(&std::fs::read(path).unwrap(), &DecodeOptions::default()).unwrap();
    (frames.first().width(), frames.first().height())
}

fn imgpipe(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_imgpipe"))
        .args(args)
        .output()
        .expect("run imgpipe")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn validate_reports_the_predicted_size() {
    let dir = scratch("validate");
    let config = dir.join("pipeline.toml");
    std::fs::write(&config, PIPELINE).unwrap();
    let config = config.to_str().unwrap();

    let out = imgpipe(&["validate", "-c", config, "--input", "400x200"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.starts_with("400x200 -> 100x50 (1 frame), peak memory"),
        "{text}"
    );

    let out = imgpipe(&[
        "validate",
        "-c",
        config,
        "--input",
        "400x200x3",
        "--format",
        "json",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["output"]["width"], 100);
    assert_eq!(report["output"]["frames"], 3);
}

#[test]
fn validate_fails_on_bad_parameters() {
    let dir = scratch("validate-bad");
    let config = dir.join("pipeline.json");
    std::fs::write(
        &config,
        r#"{"version": 1, "steps": [{"op": "rotate", "degrees": 45}]}"#,
    )
    .unwrap();

    let out = imgpipe(&["validate", "-c", config.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(
        stdout(&out).contains("step 0 (rotate): degrees must be a multiple of 90, got 45"),
        "{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("error: 1 problem(s)"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn explain_prints_the_plan_as_json() {
    let dir = scratch("explain");
    let config = dir.join("pipeline.toml");
    std::fs::write(&config, PIPELINE).unwrap();

    let out = imgpipe(&[
        "explain",
        "-c",
        config.to_str().unwrap(),
        "--input",
        "50x200",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let explained: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(explained["imgpipe"], imgpipe::VERSION);
    assert_eq!(explained["pipeline"]["steps"][0]["op"], "resize");
    assert_eq!(explained["plan"]["output"]["width"], 25);
    assert_eq!(explained["plan"]["output"]["height"], 100);
}

#[test]
fn run_writes_one_output() {
    let dir = scratch("run-single");
    let config = dir.join("pipeline.toml");
    std::fs::write(&config, PIPELINE).unwrap();
    let input = dir.join("photo.png");
    write_png(&input, 300, 150);
    let output = dir.join("out.webp");

    let out = imgpipe(&[
        "run",
        "-c",
        config.to_str().unwrap(),
        input.to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    // The `[[outputs]]` entry for WebP supplies its tail.
    assert_eq!(size_of(&output), (20, 10));
}

#[test]
fn run_writes_every_output_entry() {
    let dir = scratch("run-dir");
    let config = dir.join("pipeline.toml");
    std::fs::write(&config, PIPELINE).unwrap();
    for name in ["a.png", "b.png"] {
        write_png(&dir.join(name), 300, 150);
    }
    let pattern = dir.join("*.png");
    let out_dir = dir.join("dist");

    let out = imgpipe(&[
        "run",
        "-c",
        config.to_str().unwrap(),
        pattern.to_str().unwrap(),
        "--out-dir",
        out_dir.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let results: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(results.as_array().unwrap().len(), 2);
    for stem in ["a", "b"] {
        assert_eq!(size_of(&out_dir.join(stem).join("full.png")), (100, 50));
        assert_eq!(
            size_of(&out_dir.join(stem).join("thumb-20x10.webp")),
            (20, 10)
        );
    }
}

#[test]
fn run_reports_failed_inputs() {
    let dir = scratch("run-fail");
    let config = dir.join("pipeline.toml");
    std::fs::write(&config, PIPELINE).unwrap();
    let input = dir.join("not-an-image.png");
    std::fs::write(&input, b"hello").unwrap();

    let out = imgpipe(&[
        "run",
        "-c",
        config.to_str().unwrap(),
        input.to_str().unwrap(),
        "-o",
        dir.join("out.png").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(
        stdout(&out).contains("not-an-image.png: error:"),
        "{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("1 of 1 input(s) failed"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn schema_is_json() {
    let out = imgpipe(&["schema"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let schema: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(schema["properties"]["steps"].is_object(), "{schema}");
}

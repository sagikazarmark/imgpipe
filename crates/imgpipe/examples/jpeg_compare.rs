//! Compares jpeg-encoder with MozJPEG on real images, the way the catalog
//! build makes its JPEGs: the pipeline file's steps, then each JPEG output's
//! tail and quality. Reports size, time and PSNR against the pixels that went
//! into the encoder, plus the MozJPEG quality (and size) that first matches
//! jpeg-encoder's PSNR, so a smaller file isn't just a worse one.
//!
//! `cargo run --release -p imgpipe --features jpeg-mozjpeg --example jpeg_compare -- \
//!     <pipeline.toml> <images...>`, e.g.
//! `... -- pipeline.toml photos/*.jpg`

use imgpipe::config::PipelineFile;
use imgpipe::{Ctx, DecodeOptions, EncodeOptions, Encoders, Format, Frames, Rgba8, decode};
use std::collections::BTreeMap;
use std::time::Instant;

/// PSNR over RGB, with both images flattened onto white (as the encoders do).
fn psnr(reference: &Rgba8, jpeg: &[u8]) -> f64 {
    let back = decode(jpeg, &DecodeOptions::default()).unwrap();
    let back = back.first();
    assert_eq!(
        (back.width(), back.height()),
        (reference.width(), reference.height())
    );
    let mut sum = 0f64;
    for (a, b) in reference
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(back.data().as_chunks::<4>().0)
    {
        let alpha = a[3] as f64 / 255.0;
        for c in 0..3 {
            let flat = a[c] as f64 * alpha + 255.0 * (1.0 - alpha);
            let d = flat - b[c] as f64;
            sum += d * d;
        }
    }
    let mse = sum / (reference.width() as f64 * reference.height() as f64 * 3.0);
    if mse == 0.0 {
        99.0
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

#[derive(Default)]
struct Total {
    n: usize,
    plain: (usize, f64, f64),
    moz: (usize, f64, f64),
    matched: usize,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((pipeline_path, images)) = args.split_first() else {
        eprintln!("usage: jpeg_compare <pipeline.toml> <images...>");
        std::process::exit(2);
    };
    let file: PipelineFile =
        toml::from_str(&std::fs::read_to_string(pipeline_path).unwrap()).unwrap();
    let encoders = Encoders::builtin();
    let cx = Ctx::default();
    let jpeg_outputs: Vec<_> = file
        .outputs
        .iter()
        .filter(|v| v.encode.format == Format::Jpeg)
        .collect();

    println!(
        "{:<28} {:<6} {:>9} {:>3} | {:>8} {:>5} {:>6} | {:>8} {:>5} {:>6} {:>6} | {:>4} {:>8} {:>6}",
        "image",
        "kind",
        "size",
        "q",
        "jpeg-enc",
        "ms",
        "PSNR",
        "mozjpeg",
        "ms",
        "PSNR",
        "diff",
        "q=",
        "bytes",
        "diff"
    );
    let mut totals: BTreeMap<String, Total> = BTreeMap::new();
    for path in images {
        let bytes = std::fs::read(path).unwrap();
        let base = file
            .pipeline
            .run(decode(&bytes, &DecodeOptions::default()).unwrap(), &cx)
            .unwrap();
        let name = std::path::Path::new(path)
            .parent()
            .and_then(|p| p.file_name())
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        for v in &jpeg_outputs {
            let kind = v
                .name
                .rsplit('/')
                .next()
                .unwrap()
                .split('.')
                .next()
                .unwrap()
                .to_string();
            let frames = if v.tail.steps.is_empty() {
                base.clone()
            } else {
                v.tail.run(base.clone(), &cx).unwrap()
            };
            let img = frames.first().clone();
            let q = v.encode.quality.unwrap_or(85);
            let run = |opts: EncodeOptions| {
                let encoder = encoders.find(&opts).unwrap();
                let t = Instant::now();
                let out = encoder.encode(&Frames::still(img.clone()), &opts).unwrap();
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                (encoder.id().name, out, ms)
            };
            let plain_opts = EncodeOptions::new(Format::Jpeg).quality(q);
            let (plain_id, plain, plain_ms) = run(plain_opts);
            let (moz_id, moz, moz_ms) = run(plain_opts.optimize(true));
            assert_eq!((&*plain_id, &*moz_id), ("jpeg-encoder", "mozjpeg"));
            let (plain_psnr, moz_psnr) = (psnr(&img, &plain), psnr(&img, &moz));

            // The lowest MozJPEG quality whose PSNR reaches jpeg-encoder's
            // (PSNR rises with quality; binary search over 30..=100).
            let (mut lo, mut hi) = (30u8, 100u8);
            let mut best: Option<(u8, usize)> = None;
            while lo <= hi {
                let mid = (lo + hi) / 2;
                let (_, out, _) = run(plain_opts.quality(mid).optimize(true));
                if psnr(&img, &out) >= plain_psnr {
                    best = Some((mid, out.len()));
                    hi = mid - 1;
                } else {
                    lo = mid + 1;
                }
            }
            let (match_q, match_bytes) = best.unwrap_or((0, 0));

            let pct = |a: usize, b: usize| (a as f64 / b as f64 - 1.0) * 100.0;
            println!(
                "{:<28} {:<6} {:>9} {:>3} | {:>8} {:>5.0} {:>6.2} | {:>8} {:>5.0} {:>6.2} {:>5.1}% | {:>4} {:>8} {:>5.1}%",
                &name[..name.len().min(28)],
                kind,
                format!("{}x{}", img.width(), img.height()),
                q,
                plain.len(),
                plain_ms,
                plain_psnr,
                moz.len(),
                moz_ms,
                moz_psnr,
                pct(moz.len(), plain.len()),
                match_q,
                match_bytes,
                pct(match_bytes, plain.len()),
            );
            let t = totals.entry(format!("{kind} q{q}")).or_default();
            t.n += 1;
            t.plain.0 += plain.len();
            t.plain.1 += plain_ms;
            t.plain.2 += plain_psnr;
            t.moz.0 += moz.len();
            t.moz.1 += moz_ms;
            t.moz.2 += moz_psnr;
            t.matched += match_bytes;
        }
    }

    println!("\nTotals (bytes, encode time, mean PSNR):");
    for (kind, t) in &totals {
        let n = t.n as f64;
        println!(
            "{:<10} n={:<3} jpeg-encoder {:>9} B {:>7.0} ms {:>6.2} dB | mozjpeg {:>9} B ({:+.1}%) {:>7.0} ms ({:.1}x) {:>6.2} dB | mozjpeg at equal PSNR {:>9} B ({:+.1}%)",
            kind,
            t.n,
            t.plain.0,
            t.plain.1,
            t.plain.2 / n,
            t.moz.0,
            (t.moz.0 as f64 / t.plain.0 as f64 - 1.0) * 100.0,
            t.moz.1,
            t.moz.1 / t.plain.1,
            t.moz.2 / n,
            t.matched,
            (t.matched as f64 / t.plain.0 as f64 - 1.0) * 100.0,
        );
    }
}

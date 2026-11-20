//! Compares the PNG encoders on real images: size and time.
//! `cargo run --release -p imgpipe --features png-optimize --example png_compare -- <images...>`

use imgpipe::{DecodeOptions, decode};
use std::time::Instant;

fn main() {
    println!(
        "{:<40} {:>10} {:>8} {:>10} {:>8} {:>10} {:>8}",
        "image", "png fast", "ms", "png high", "ms", "oxipng 2", "ms"
    );
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).unwrap();
        let img = decode(&bytes, &DecodeOptions::default())
            .unwrap()
            .into_first();
        let mut row = vec![];
        // As imgpipe's PngEncoder does: RGB when the image is opaque.
        let opaque = img.is_opaque();
        let data: Vec<u8> = if opaque {
            img.data()
                .chunks(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect()
        } else {
            img.data().to_vec()
        };
        for level in [png::Compression::Fast, png::Compression::High] {
            let t = Instant::now();
            let mut out = Vec::new();
            let mut enc = png::Encoder::new(&mut out, img.width(), img.height());
            enc.set_color(if opaque {
                png::ColorType::Rgb
            } else {
                png::ColorType::Rgba
            });
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(level);
            enc.write_header().unwrap().write_image_data(&data).unwrap();
            row.push((out.len(), t.elapsed().as_millis()));
        }
        let t = Instant::now();
        let fast = {
            let mut out = Vec::new();
            let mut enc = png::Encoder::new(&mut out, img.width(), img.height());
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            enc.write_header()
                .unwrap()
                .write_image_data(img.data())
                .unwrap();
            out
        };
        let opt = oxipng::optimize_from_memory(&fast, &oxipng::Options::from_preset(2)).unwrap();
        row.push((opt.len(), t.elapsed().as_millis()));
        let name = std::path::Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        println!(
            "{:<40} {:>10} {:>8} {:>10} {:>8} {:>10} {:>8}",
            &name[..name.len().min(40)],
            row[0].0,
            row[0].1,
            row[1].0,
            row[1].1,
            row[2].0,
            row[2].1
        );
    }
}

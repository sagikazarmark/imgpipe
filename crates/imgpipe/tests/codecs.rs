//! Decode and encode: round trips, untrusted-input limits, orientation and
//! colour profiles. Fixtures are generated in code, so nothing binary is
//! checked in.
#![cfg(all(feature = "png", feature = "jpeg", feature = "webp"))]

use imgpipe::{
    DecodeOptions, EncodeOptions, Encoders, Error, Format, Frames, Limits, Rgba8, decode,
};

fn gradient(w: u32, h: u32, alpha: bool) -> Rgba8 {
    let mut img = Rgba8::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let a = if alpha {
                (x * 255 / w.max(1)) as u8
            } else {
                255
            };
            img.set_pixel(x, y, [(x * 255 / w) as u8, (y * 255 / h) as u8, 128, a]);
        }
    }
    img
}

fn encode(img: &Rgba8, opts: EncodeOptions) -> Vec<u8> {
    Encoders::builtin()
        .encode(&Frames::still(img.clone()), &opts)
        .unwrap()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn png_and_lossless_webp_round_trip_exactly() {
    for alpha in [false, true] {
        let img = gradient(37, 23, alpha);
        for opts in [
            EncodeOptions::new(Format::Png),
            EncodeOptions::new(Format::Webp).lossless(true),
        ] {
            let back = decode(&encode(&img, opts), &DecodeOptions::default()).unwrap();
            assert_eq!(back.first(), &img, "{opts:?} alpha={alpha}");
        }
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn jpeg_is_close_and_flattens_alpha_onto_white() {
    let img = gradient(64, 32, false);
    let back = decode(
        &encode(&img, EncodeOptions::new(Format::Jpeg).quality(95)),
        &DecodeOptions::default(),
    )
    .unwrap();
    let back = back.first();
    assert_eq!((back.width(), back.height()), (64, 32));
    let max = img
        .data()
        .iter()
        .zip(back.data())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(max <= 24, "max channel difference {max}");

    let clear = Rgba8::filled(16, 16, [0, 0, 0, 0]);
    let back = decode(
        &encode(&clear, EncodeOptions::new(Format::Jpeg)),
        &DecodeOptions::default(),
    )
    .unwrap();
    assert!(
        back.first().pixel(8, 8)[0] > 250,
        "transparent pixels become white"
    );
}

#[cfg(not(feature = "webp-libwebp"))]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn lossy_webp_needs_a_platform_encoder() {
    let err = Encoders::builtin()
        .encode(
            &Frames::still(gradient(4, 4, false)),
            &EncodeOptions::new(Format::Webp),
        )
        .unwrap_err();
    assert!(err.to_string().contains("libwebp"), "{err}");
}

#[cfg(feature = "webp-libwebp")]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn libwebp_lossy_and_lossless() {
    let img = gradient(128, 64, true);
    let lossy = encode(&img, EncodeOptions::new(Format::Webp).quality(85));
    let lossless = encode(&img, EncodeOptions::new(Format::Webp).lossless(true));
    assert!(lossy.starts_with(b"RIFF") && lossless.starts_with(b"RIFF"));
    for (bytes, tolerance) in [(lossy, 40), (lossless, 0)] {
        let back = decode(&bytes, &DecodeOptions::default()).unwrap();
        let back = back.first();
        assert_eq!((back.width(), back.height()), (128, 64));
        // Fully transparent pixels may change colour; compare the rest.
        let max = img
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(back.data().as_chunks::<4>().0)
            .filter(|(a, _)| a[3] > 0)
            .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)))
            .max()
            .unwrap();
        assert!(
            max <= tolerance,
            "max channel difference {max} (tolerance {tolerance})"
        );
    }
}

#[cfg(feature = "png-optimize")]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn optimize_picks_oxipng_and_stays_lossless() {
    let img = gradient(96, 64, true);
    let plain = encode(&img, EncodeOptions::new(Format::Png));
    let small = encode(&img, EncodeOptions::new(Format::Png).optimize(true));
    let pick = |o: EncodeOptions| {
        Encoders::builtin()
            .find(&o)
            .map(|e| e.id().name.into_owned())
    };
    assert_eq!(pick(EncodeOptions::new(Format::Png)), Some("png".into()));
    assert_eq!(
        pick(EncodeOptions::new(Format::Png).optimize(true)),
        Some("oxipng".into())
    );
    assert!(
        small.len() <= plain.len(),
        "{} > {}",
        small.len(),
        plain.len()
    );
    for bytes in [plain, small] {
        let back = decode(&bytes, &DecodeOptions::default()).unwrap();
        assert_eq!(back.first(), &img);
    }
}

/// MozJPEG is native only; on wasm `optimize` keeps jpeg-encoder.
#[cfg(all(feature = "jpeg-mozjpeg", not(target_arch = "wasm32")))]
#[test]
fn optimize_picks_mozjpeg_for_jpeg() {
    let pick = |o: EncodeOptions| {
        Encoders::builtin()
            .find(&o)
            .map(|e| e.id().name.into_owned())
    };
    assert_eq!(
        pick(EncodeOptions::new(Format::Jpeg)),
        Some("jpeg-encoder".into())
    );
    assert_eq!(
        pick(EncodeOptions::new(Format::Jpeg).optimize(true)),
        Some("mozjpeg".into())
    );

    // Odd sizes (partial MCUs), alpha flattened onto white, both chroma modes.
    let img = gradient(101, 67, true);
    for q in [80, 95] {
        let bytes = encode(
            &img,
            EncodeOptions::new(Format::Jpeg).quality(q).optimize(true),
        );
        assert!(bytes.starts_with(&[0xFF, 0xD8]), "a JPEG");
        // Progressive (SOF2), not baseline.
        let markers = header_markers(&bytes);
        assert!(markers.contains(&0xC2), "progressive (SOF2): {markers:x?}");
        // No metadata: JFIF (APP0) only, no EXIF/ICC (APP1/APP2) or comments.
        assert!(
            markers
                .iter()
                .all(|&m| m != 0xFE && (m & 0xF0 != 0xE0 || m == 0xE0)),
            "{markers:x?}"
        );
        let back = decode(&bytes, &DecodeOptions::default()).unwrap();
        let back = back.first();
        assert_eq!((back.width(), back.height()), (101, 67));
        // The left column is transparent: white after flattening.
        assert!(back.pixel(0, 33)[..3].iter().all(|&c| c > 240), "q{q}");
    }

    let clear = encode(
        &Rgba8::filled(16, 16, [0, 0, 0, 0]),
        EncodeOptions::new(Format::Jpeg).optimize(true),
    );
    let back = decode(&clear, &DecodeOptions::default()).unwrap();
    assert!(back.first().pixel(8, 8)[0] > 250);
}

/// The JPEG marker codes before the first scan.
#[cfg(all(feature = "jpeg-mozjpeg", not(target_arch = "wasm32")))]
fn header_markers(jpeg: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 2; // after SOI
    while i + 4 <= jpeg.len() && jpeg[i] == 0xFF {
        let marker = jpeg[i + 1];
        out.push(marker);
        if marker == 0xDA {
            break;
        }
        i += 2 + u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
    }
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn huge_dimensions_are_rejected_from_the_header() {
    // A PNG that claims 100000 x 100000 pixels but carries no pixel data.
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&100_000u32.to_be_bytes());
    ihdr.extend_from_slice(&100_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    let mut bomb = b"\x89PNG\r\n\x1a\n".to_vec();
    bomb.extend(chunk(b"IHDR", &ihdr));
    bomb.extend(chunk(b"IEND", &[]));
    assert!(matches!(
        decode(&bomb, &DecodeOptions::default()),
        Err(Error::Limit(_))
    ));

    // And a real image over a total-pixel limit.
    let png = encode(&gradient(64, 64, false), EncodeOptions::new(Format::Png));
    let mut limits = Limits::default();
    limits.max_total_pixels = 1000;
    let opts = DecodeOptions::default().with_limits(limits);
    assert!(matches!(decode(&png, &opts), Err(Error::Limit(_))));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn garbage_is_a_decode_error() {
    assert!(matches!(
        decode(b"not an image at all", &DecodeOptions::default()),
        Err(Error::Decode(_))
    ));
}

/// JPEG bytes with an EXIF APP1 segment carrying `orientation`.
fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let mut tiff = b"II*\0\x08\0\0\0".to_vec(); // little-endian TIFF, IFD at offset 8
    tiff.extend_from_slice(&1u16.to_le_bytes()); // one entry
    tiff.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
    tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_le_bytes()); // count
    tiff.extend_from_slice(&orientation.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]); // value padding
    tiff.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(tiff);
    let mut out = jpeg[..2].to_vec(); // SOI
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend(app1);
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn exif_orientation_is_applied_unless_disabled() {
    let jpeg = encode(
        &gradient(40, 20, false),
        EncodeOptions::new(Format::Jpeg).quality(95),
    );
    let rotated = with_exif_orientation(&jpeg, 6); // 90° clockwise
    let back = decode(&rotated, &DecodeOptions::default()).unwrap();
    assert_eq!((back.first().width(), back.first().height()), (20, 40));

    let raw = decode(
        &rotated,
        &DecodeOptions::default().with_apply_orientation(false),
    )
    .unwrap();
    assert_eq!((raw.first().width(), raw.first().height()), (40, 20));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn display_p3_is_converted_to_srgb() {
    // A saturated orange tagged as Display P3: in sRGB its red channel must
    // be higher (P3 red is redder than sRGB red), not passed through as-is.
    let icc = moxcms::ColorProfile::new_display_p3().encode().unwrap();
    let mut info = png::Info::with_size(8, 8);
    info.color_type = png::ColorType::Rgba;
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = Some(icc.into());
    let mut bytes = Vec::new();
    {
        let enc = png::Encoder::with_info(&mut bytes, info).unwrap();
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[200, 100, 50, 255].repeat(64)).unwrap();
        w.finish().unwrap();
    }
    let back = decode(&bytes, &DecodeOptions::default()).unwrap();
    let [r, g, b, a] = back.first().pixel(4, 4);
    assert_eq!(a, 255);
    assert!(
        r > 210 && g < 100 && b < 50,
        "P3 (200,100,50) as sRGB: ({r},{g},{b})"
    );
}

/// An 8-bit PNG of `pixels` (RGBA, or grey when `grey`) tagged with `icc`.
fn png_with_icc(w: u32, h: u32, grey: bool, pixels: &[u8], icc: Vec<u8>) -> Vec<u8> {
    let mut info = png::Info::with_size(w, h);
    info.color_type = if grey {
        png::ColorType::Grayscale
    } else {
        png::ColorType::Rgba
    };
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = Some(icc.into());
    let mut bytes = Vec::new();
    {
        let enc = png::Encoder::with_info(&mut bytes, info).unwrap();
        let mut w = enc.write_header().unwrap();
        w.write_image_data(pixels).unwrap();
        w.finish().unwrap();
    }
    bytes
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn a_profile_without_a_usable_transform_is_treated_as_srgb() {
    // A well-formed 132-byte ICC v4 RGB display profile header with no tags:
    // it parses, but there is no transform to build from it (moxcms:
    // "unsupported profile connection"). That used to make the image
    // undecodable; browsers show such images as sRGB, and so does decode().
    let mut icc = vec![0u8; 132];
    icc[0..4].copy_from_slice(&132u32.to_be_bytes()); // size
    icc[8..12].copy_from_slice(&0x0430_0000u32.to_be_bytes()); // v4.3
    icc[12..16].copy_from_slice(b"mntr");
    icc[16..20].copy_from_slice(b"RGB ");
    icc[20..24].copy_from_slice(b"XYZ ");
    icc[36..40].copy_from_slice(b"acsp");
    for (i, v) in [0x0000_f6d6u32, 0x0001_0000, 0x0000_d32d]
        .iter()
        .enumerate()
    {
        icc[68 + 4 * i..72 + 4 * i].copy_from_slice(&v.to_be_bytes()); // D50
    }
    let pixels = [200, 100, 50, 255].repeat(64);
    let back = decode(
        &png_with_icc(8, 8, false, &pixels, icc),
        &DecodeOptions::default(),
    )
    .unwrap();
    assert_eq!(back.first().data(), &pixels[..]);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn grey_profiles_are_converted_to_srgb() {
    // Grey 128 in a linear (gamma 1.0) grey space is 50% light: sRGB ~188.
    let icc = moxcms::ColorProfile::new_gray_with_gamma(1.0)
        .encode()
        .unwrap();
    let back = decode(
        &png_with_icc(8, 8, true, &[128; 64], icc),
        &DecodeOptions::default(),
    )
    .unwrap();
    let [r, g, b, a] = back.first().pixel(3, 3);
    assert_eq!((r, a), (g, 255));
    assert_eq!(g, b);
    assert!((185..=191).contains(&r), "linear grey 128 as sRGB: {r}");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn limits_apply_to_the_size_after_exif_rotation() {
    // 40 x 20 stored, 20 x 40 shown: a 30 px height limit admits the stored
    // size but not the turned one.
    let jpeg = encode(
        &gradient(40, 20, false),
        EncodeOptions::new(Format::Jpeg).quality(90),
    );
    let rotated = with_exif_orientation(&jpeg, 6);
    let mut limits = Limits::default();
    limits.max_width = 50;
    limits.max_height = 30;
    let opts = DecodeOptions::default().with_limits(limits);
    assert!(matches!(decode(&rotated, &opts), Err(Error::Limit(_))));
    // Without the rotation, the stored size is what counts.
    let raw = opts.with_apply_orientation(false);
    assert!(decode(&rotated, &raw).is_ok());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn sniff_names_imgpipe_formats() {
    let img = gradient(8, 8, false);
    for format in [Format::Png, Format::Jpeg, Format::Webp] {
        let mut opts = EncodeOptions::new(format);
        if format == Format::Webp {
            opts = opts.lossless(true);
        }
        assert_eq!(imgpipe::decode::sniff(&encode(&img, opts)), Some(format));
    }
    assert_eq!(imgpipe::decode::sniff(b"not an image"), None);
}

//! Pixel storage: 8-bit sRGB with straight alpha at rest.

use crate::error::{Error, Result};
use std::fmt;

/// An RGBA image: 8 bits per channel, sRGB-encoded, straight (not
/// premultiplied) alpha, rows top to bottom with no padding.
///
/// The buffer always holds exactly `width * height * 4` bytes; constructors
/// check that product for overflow (`usize` is 32 bits on wasm32).
#[derive(Clone, PartialEq, Eq)]
pub struct Rgba8 {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Default for Rgba8 {
    /// An empty 0×0 image.
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            data: Vec::new(),
        }
    }
}

impl Rgba8 {
    /// A fully transparent image.
    ///
    /// # Panics
    ///
    /// When `width * height * 4` overflows `usize`, like `vec!` with a too
    /// large length; [`try_new`](Self::try_new) returns `None` instead.
    pub fn new(width: u32, height: u32) -> Self {
        Self::try_new(width, height).unwrap_or_else(|| too_large(width, height))
    }

    /// A fully transparent image; `None` when `width * height * 4`
    /// overflows `usize`.
    pub fn try_new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            width,
            height,
            data: vec![0; byte_len(width, height)?],
        })
    }

    /// An image filled with one colour.
    ///
    /// # Panics
    ///
    /// When `width * height * 4` overflows `usize`;
    /// [`try_filled`](Self::try_filled) returns `None` instead.
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        Self::try_filled(width, height, rgba).unwrap_or_else(|| too_large(width, height))
    }

    /// An image filled with one colour; `None` when `width * height * 4`
    /// overflows `usize`.
    pub fn try_filled(width: u32, height: u32, rgba: [u8; 4]) -> Option<Self> {
        let len = byte_len(width, height)?;
        Some(Self {
            width,
            height,
            data: {
                let mut data = Vec::with_capacity(len);
                for _ in 0..len / 4 {
                    data.extend_from_slice(&rgba);
                }
                data
            },
        })
    }

    /// Wraps raw RGBA bytes; `None` if the length is not `width * height * 4`
    /// (or that product overflows).
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (byte_len(width, height) == Some(data.len())).then_some(Self {
            width,
            height,
            data,
        })
    }

    /// Width in px.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in px.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The RGBA bytes, row by row.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The RGBA bytes, mutable.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// The RGBA bytes, by value.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// The pixel at (`x`, `y`); panics when out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Sets the pixel at (`x`, `y`); panics when out of bounds.
    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.data[i..i + 4].copy_from_slice(&rgba);
    }

    /// One row of pixels as bytes.
    pub fn row(&self, y: u32) -> &[u8] {
        let stride = self.width as usize * 4;
        &self.data[y as usize * stride..(y as usize + 1) * stride]
    }

    /// True when every pixel is fully opaque.
    pub fn is_opaque(&self) -> bool {
        self.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255)
    }
}

impl fmt::Debug for Rgba8 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rgba8({}x{})", self.width, self.height)
    }
}

/// `width * height * 4`, or `None` when that overflows `usize`.
pub(crate) fn byte_len(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
}

#[cold]
fn too_large(width: u32, height: u32) -> ! {
    panic!("a {width}x{height} RGBA image does not fit in memory on this target")
}

/// One frame of a still or animated image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The pixels.
    pub image: Rgba8,
    /// Display time; 0 for stills. [`decode()`](crate::decode()) turns GIF
    /// delays of 10 ms or less into 100 ms, as browsers show them.
    pub delay_ms: u32,
}

/// A still image (one frame) or an animation.
///
/// Always holds at least one frame, and all frames have the same size, so
/// [`first`](Self::first) and [`info`](Self::info) never fail. Build one with
/// [`still`](Self::still) or the checked [`new`](Self::new).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frames {
    frames: Vec<Frame>,
    loop_count: Option<u16>,
}

impl Frames {
    /// An animation (or, with one frame, a still). Fails unless there is at
    /// least one frame and every frame has the same, non-zero size.
    /// `loop_count`: `None` loops forever.
    pub fn new(frames: Vec<Frame>, loop_count: Option<u16>) -> Result<Self> {
        let Some(first) = frames.first() else {
            return Err(Error::InvalidFrames("no frames".into()));
        };
        let (w, h) = (first.image.width(), first.image.height());
        if w == 0 || h == 0 {
            return Err(Error::InvalidFrames(format!(
                "{w}x{h} frames have no pixels"
            )));
        }
        if let Some((i, f)) = frames
            .iter()
            .enumerate()
            .find(|(_, f)| (f.image.width(), f.image.height()) != (w, h))
        {
            return Err(Error::InvalidFrames(format!(
                "frame {i} is {}x{}, frame 0 is {w}x{h}",
                f.image.width(),
                f.image.height()
            )));
        }
        Ok(Self { frames, loop_count })
    }

    /// A single still image.
    ///
    /// Unlike [`new`](Self::new) this cannot fail, so it accepts an image
    /// with no pixels (0×N); [`Pipeline::run`](crate::Pipeline::run) and the
    /// encoders reject those.
    pub fn still(image: Rgba8) -> Self {
        Self {
            frames: vec![Frame { image, delay_ms: 0 }],
            loop_count: None,
        }
    }

    /// The frames, in display order (never empty).
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The frames, by value.
    pub fn into_frames(self) -> Vec<Frame> {
        self.frames
    }

    /// How often an animation repeats; `None` loops forever.
    pub fn loop_count(&self) -> Option<u16> {
        self.loop_count
    }

    /// The first (for stills, the only) image.
    pub fn first(&self) -> &Rgba8 {
        &self.frames[0].image
    }

    /// The first image, by value.
    pub fn into_first(self) -> Rgba8 {
        self.frames
            .into_iter()
            .next()
            .expect("at least one frame")
            .image
    }

    /// More than one frame.
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// Size and frame count.
    pub fn info(&self) -> FrameInfo {
        let first = self.first();
        FrameInfo {
            width: first.width(),
            height: first.height(),
            frames: u32::try_from(self.frames.len()).unwrap_or(u32::MAX),
        }
    }

    /// Applies `f` to every frame's image. Every result must have the same
    /// size, which keeps the `Frames` invariant.
    pub(crate) fn map_images<E: From<String>>(
        self,
        mut f: impl FnMut(Rgba8) -> Result<Rgba8, E>,
    ) -> Result<Self, E> {
        let mut size = None;
        let frames = self
            .frames
            .into_iter()
            .map(|fr| {
                let image = f(fr.image)?;
                let s = (image.width(), image.height());
                if *size.get_or_insert(s) != s {
                    return Err(E::from("frames came out with different sizes".to_string()));
                }
                Ok(Frame {
                    image,
                    delay_ms: fr.delay_ms,
                })
            })
            .collect::<Result<_, E>>()?;
        Ok(Self {
            frames,
            loop_count: self.loop_count,
        })
    }
}

/// Size of a `Frames` value without its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FrameInfo {
    /// Width in px.
    pub width: u32,
    /// Height in px.
    pub height: u32,
    /// Number of frames.
    pub frames: u32,
}

impl FrameInfo {
    /// Pixels over all frames (`width * height * frames`), saturating at
    /// `u64::MAX`.
    pub fn pixels(&self) -> u64 {
        (self.width as u64 * self.height as u64).saturating_mul(self.frames as u64)
    }

    /// Bytes the pixels take at rest (4 per pixel), saturating at `u64::MAX`.
    pub fn bytes(&self) -> u64 {
        self.pixels().saturating_mul(4)
    }
}

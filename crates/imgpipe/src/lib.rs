//! Linear image pipeline: image in, image out, no I/O in the core.
//!
//! - Pixels are 8-bit sRGB with straight alpha at rest ([`Rgba8`]); filtering
//!   steps work in 16-bit linear light with premultiplied alpha internally.
//! - A [`Pipeline`] is an ordered list of serializable [`Step`]s, validated
//!   (sizes, memory) before it runs. Running is synchronous:
//!   [`Pipeline::run`] returns when the last step is done.
//! - [`decode()`] and [`Encoders`] sit outside the core; callers combine them,
//!   or use [`variants::run_variants`] to encode several outputs into a
//!   [`variants::Sink`].
//! - Everything builds for native targets and `wasm32-unknown-unknown`.
//!
//! See the README for the feature flags (some pull in C code, and the
//! default `jpeg` feature needs an IJG attribution) and examples.
//!
//! # Stability
//!
//! Types fall into three groups:
//!
//! - **Pipeline data** (the step structs in [`steps`], [`overlay::TextBox`]
//!   and friends, [`Pipeline`], [`config::PipelineFile`],
//!   [`variants::Variant`]) mirrors the pipeline file format and has public
//!   fields, so it can be written as a literal and destructured like the TOML
//!   it comes from. Most have a `Default`; build them with
//!   `..Default::default()` and a later field addition will not break your
//!   code. Adding a field is otherwise a breaking change and waits for the
//!   next 0.x release.
//! - **Options and contexts** ([`EncodeOptions`], [`DecodeOptions`],
//!   [`Limits`], [`Ctx`]) are `#[non_exhaustive]`: start from `new`/`default`
//!   and use their setters (or assign fields). They grow without a breaking
//!   release.
//! - **Results** ([`Plan`], [`Diagnostic`], [`variants::RenderedVariant`],
//!   [`overlay`] layouts, ...) and the enums likely to grow ([`Error`],
//!   [`Step`], [`Format`], ...) are `#[non_exhaustive]` too.

#![warn(missing_docs, missing_debug_implementations)]
// Feature badges on docs.rs (nightly; `doc_cfg` includes the former
// `doc_auto_cfg` since Rust 1.92).
#![cfg_attr(docsrs, feature(doc_cfg))]

/// Compiles the README's examples as doctests (default features; examples
/// that need more are compiled only when those features are on).
#[cfg(all(doctest, feature = "png", feature = "jpeg", feature = "webp"))]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

pub mod color;
pub mod config;
pub mod decode;
pub mod encode;
mod error;
pub mod frame;
pub mod overlay;
mod pipeline;
pub mod steps;
pub mod variants;

#[cfg(any(test, all(target_arch = "wasm32", feature = "wasm-libc-shim")))]
mod c_runtime;

pub use color::Color;
pub use decode::{DecodeOptions, Limits, decode};
pub use encode::{EncodeOptions, Encoder, EncoderId, Encoders, Format};
pub use error::{Diagnostic, Error, Result, StepError};
pub use frame::{Frame, FrameInfo, Frames, Rgba8};
pub use pipeline::{
    CancelToken, Ctx, MemoryBudget, PIPELINE_VERSION, Pipeline, Plan, Progress, ProgressFn,
    StepPlan,
};
pub use steps::Step;

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `Send + Sync` on native targets, nothing on `wasm32`.
///
/// [`Encoder`] and [`overlay::Vars`] are bounded by it, so native callers
/// can share encoders and a [`Ctx`] across threads, while browser code can
/// still implement them over non-`Send` JavaScript handles.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + ?Sized> MaybeSend for T {}

/// `Send + Sync` on native targets, nothing on `wasm32`.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

#[cfg(test)]
mod tests {
    /// Errors cross threads and live in `Box<dyn Error + Send + Sync>`.
    #[test]
    fn error_is_send_sync_static() {
        fn check<T: Send + Sync + 'static + std::error::Error>() {}
        check::<crate::Error>();
        check::<crate::StepError>();
        check::<crate::overlay::TextError>();
    }

    /// A `Ctx` (fonts and vars included) can be shared by worker threads.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn ctx_is_send_sync() {
        fn check<T: Send + Sync>() {}
        check::<crate::Ctx<'static>>();
        check::<crate::Encoders>();
    }
}

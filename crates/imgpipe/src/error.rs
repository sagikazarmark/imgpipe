//! Errors and validation diagnostics.

use crate::overlay::TextError;
use serde::Serialize;
use std::path::PathBuf;

/// `Result` with imgpipe's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A problem found by [`Pipeline::validate`](crate::Pipeline::validate).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Diagnostic {
    /// Step index, when the problem belongs to one step.
    pub step: Option<usize>,
    /// That step's op name (`resize`, `text`, ...).
    pub op: Option<&'static str>,
    /// What is wrong.
    pub message: String,
}
impl Diagnostic {
    pub(crate) fn pipeline(message: impl Into<String>) -> Self {
        Self {
            step: None,
            op: None,
            message: message.into(),
        }
    }

    pub(crate) fn step(index: usize, op: &'static str, message: impl Into<String>) -> Self {
        Self {
            step: Some(index),
            op: Some(op),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.step, self.op) {
            (Some(i), Some(op)) => write!(f, "step {i} ({op}): {}", self.message),
            _ => write!(f, "{}", self.message),
        }
    }
}

/// Everything that can go wrong decoding, running or encoding.
///
/// Errors that wrap another error ([`Error::Step`], [`Error::Io`]) include
/// its message in their own and also return it from
/// [`source`](std::error::Error::source), so it can be inspected or
/// downcast; error reporters that print the whole chain show it twice.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A step failed while running (after validation passed).
    #[error("step {index} ({op}): {source}")]
    Step {
        /// Step index in the pipeline.
        index: usize,
        /// The step's op name.
        op: &'static str,
        /// What went wrong.
        #[source]
        source: StepError,
    },

    /// Validation failed; nothing ran.
    #[error("invalid pipeline: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<Diagnostic>),

    /// [`Frames::new`](crate::Frames::new) got no frames, frames of
    /// different sizes, or frames with no pixels.
    #[error("invalid frames: {0}")]
    InvalidFrames(String),

    /// The input is not a readable image.
    #[error("decode: {0}")]
    Decode(String),

    /// The encoder failed.
    #[error("encode: {0}")]
    Encode(String),

    /// The input exceeds the decode [`Limits`](crate::Limits). (A pipeline
    /// whose steps would exceed them fails validation instead:
    /// [`Error::Invalid`].)
    #[error("limit exceeded: {0}")]
    Limit(String),

    /// The run would need more memory than the [`MemoryBudget`](crate::MemoryBudget)
    /// (or than the target can address).
    #[error("memory budget exceeded: needs {needed} bytes, budget is {budget}")]
    Budget {
        /// Predicted peak bytes.
        needed: u64,
        /// The budget, bytes.
        budget: u64,
    },

    /// The [`CancelToken`](crate::CancelToken) was cancelled.
    #[error("cancelled")]
    Cancelled,

    /// Not available in this build (a missing feature or encoder).
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// A variant's rendered file name is not a plain relative path (see
    /// `variants::FsSink`, native only).
    #[error("output name {0:?} must be a relative path without `..` or `.`")]
    OutputName(String),

    /// Reading or writing a file (sinks). `From<std::io::Error>` gives one
    /// without a path, for [`Sink`](crate::variants::Sink) implementations.
    #[error("{}{source}", .path.as_ref().map(|p| format!("{}: ", p.display())).unwrap_or_default())]
    Io {
        /// The file or directory, when known.
        path: Option<PathBuf>,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Io { path: None, source }
    }
}

/// Why a step failed while running: the source of [`Error::Step`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StepError {
    /// A text box could not be laid out or drawn (an unknown font, ...).
    #[error(transparent)]
    Text(#[from] TextError),
    /// A `text` step ran without fonts in the [`Ctx`](crate::Ctx).
    #[error("the text step needs fonts in the Ctx (Ctx::with_fonts)")]
    NoFonts,
    /// Anything else: an allocation that failed or a resampler error.
    #[error("{0}")]
    Other(String),
}

impl From<String> for StepError {
    fn from(message: String) -> Self {
        StepError::Other(message)
    }
}

impl From<&str> for StepError {
    fn from(message: &str) -> Self {
        StepError::Other(message.to_owned())
    }
}

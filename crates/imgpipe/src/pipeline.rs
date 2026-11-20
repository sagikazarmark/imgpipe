//! The pipeline: an ordered list of steps, validated before it runs.

use crate::decode::Limits;
use crate::error::{Diagnostic, Error, Result};
use crate::frame::{FrameInfo, Frames};
use crate::overlay::Vars;
use crate::steps::Step;
use serde::de::{self, DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Current pipeline file format version. See the [`config`](crate::config)
/// module for what changes it.
pub const PIPELINE_VERSION: u16 = 1;

/// Steps run exactly in the order written; nothing is reordered or merged.
///
/// Deserializing checks `version` first: a file of another version is
/// rejected as "unsupported pipeline version N", whatever its steps say
/// (see [`config`](crate::config)).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Pipeline {
    /// Format version; [`PIPELINE_VERSION`] is the only one this build reads.
    pub version: u16,
    /// The steps, in order.
    #[serde(default)]
    pub steps: Vec<Step>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self {
            version: PIPELINE_VERSION,
            steps: Vec::new(),
        }
    }
}

/// The error for a `version` this build does not read.
pub(crate) fn unsupported_version(version: u16) -> String {
    format!("unsupported pipeline version {version} (this build reads version {PIPELINE_VERSION})")
}

/// Reads the fields of a versioned map (a pipeline or a pipeline file) so
/// that `version` decides before the body does.
///
/// Fields that follow a supported `version` are parsed directly, with the
/// format's own error positions. A field that comes before `version` (JSON
/// with `"steps"` first, or TOML with an inline `steps = [...]` above
/// `version`) is buffered as an untyped value and parsed only once the
/// version has been checked, so a newer file with steps this build doesn't
/// know still reports its version, not an unknown op.
pub(crate) struct VersionedFields {
    version: Option<u16>,
}

/// A field value from [`VersionedFields::value`].
pub(crate) enum FieldValue<T> {
    Parsed(T),
    Buffered(serde_json::Value),
}

impl VersionedFields {
    pub(crate) fn new() -> Self {
        Self { version: None }
    }

    /// Reads the `version` value and rejects an unsupported one at once.
    pub(crate) fn version<'de, A: MapAccess<'de>>(
        &mut self,
        map: &mut A,
    ) -> std::result::Result<(), A::Error> {
        if self.version.is_some() {
            return Err(de::Error::duplicate_field("version"));
        }
        let version: u16 = map.next_value()?;
        if version != PIPELINE_VERSION {
            return Err(de::Error::custom(unsupported_version(version)));
        }
        self.version = Some(version);
        Ok(())
    }

    /// Reads the value of another field: parsed when the version is known,
    /// buffered otherwise. `seen` tracks duplicates.
    pub(crate) fn value<'de, A: MapAccess<'de>, T: DeserializeOwned>(
        &self,
        map: &mut A,
        name: &'static str,
        seen: &mut bool,
    ) -> std::result::Result<FieldValue<T>, A::Error> {
        if std::mem::replace(seen, true) {
            return Err(de::Error::duplicate_field(name));
        }
        Ok(if self.version.is_some() {
            FieldValue::Parsed(map.next_value()?)
        } else {
            FieldValue::Buffered(map.next_value()?)
        })
    }

    /// The checked version; an error when the map had none.
    pub(crate) fn finish<E: de::Error>(&self) -> std::result::Result<u16, E> {
        self.version.ok_or_else(|| E::missing_field("version"))
    }
}

impl<T: DeserializeOwned> FieldValue<T> {
    /// The value, parsing a buffered one now that the version is checked.
    pub(crate) fn get<E: de::Error>(self) -> std::result::Result<T, E> {
        match self {
            FieldValue::Parsed(v) => Ok(v),
            FieldValue::Buffered(v) => T::deserialize(v).map_err(E::custom),
        }
    }
}

impl<'de> Deserialize<'de> for Pipeline {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Version,
            Steps,
        }
        struct PipelineVisitor;
        impl<'de> Visitor<'de> for PipelineVisitor {
            type Value = Pipeline;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a pipeline: `version` and `steps`")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Pipeline, A::Error> {
                let mut fields = VersionedFields::new();
                let (mut steps, mut seen) = (None, false);
                while let Some(key) = map.next_key::<Field>()? {
                    match key {
                        Field::Version => fields.version(&mut map)?,
                        Field::Steps => steps = Some(fields.value(&mut map, "steps", &mut seen)?),
                    }
                }
                Ok(Pipeline {
                    version: fields.finish()?,
                    steps: steps.map(FieldValue::get).transpose()?.unwrap_or_default(),
                })
            }
        }
        d.deserialize_struct("Pipeline", &["version", "steps"], PipelineVisitor)
    }
}

/// One validated step.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct StepPlan {
    /// The step's op name.
    pub op: &'static str,
    /// The step's `id`, if it has one.
    pub id: Option<String>,
    /// Predicted output size (for `trim`, an upper bound).
    pub output: FrameInfo,
    /// Input + output frames + scratch memory while this step runs.
    pub peak_bytes: u64,
}

/// The result of validation: predicted sizes and memory.
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct Plan {
    /// The size validated for.
    pub input: FrameInfo,
    /// Predicted final size.
    pub output: FrameInfo,
    /// Per step, in order.
    pub steps: Vec<StepPlan>,
    /// The largest step peak, bytes.
    pub peak_bytes: u64,
}

/// Bytes a pipeline run may use for pixels: the plan's predicted peak (see
/// [`Plan::peak_bytes`]) must fit. Platforms set their own (a browser tab, a
/// Worker, a server).
///
/// Whatever the value, a run is also refused when the peak exceeds
/// `isize::MAX` bytes, the most one allocation can hold: 2 GiB − 1 on
/// wasm32, where an over-large buffer would otherwise abort the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryBudget {
    /// The limit, bytes.
    pub bytes: u64,
}

impl MemoryBudget {
    /// 2 GiB, the [`Ctx`] default: every step fits for images up to about 67
    /// million pixels (`sharpen`, the hungriest, needs 32 bytes per pixel),
    /// and shrinking fits for anything [`Limits::default`] admits (100
    /// million pixels). Half of wasm32's 4 GiB address space.
    pub const DEFAULT: MemoryBudget = MemoryBudget { bytes: 2 << 30 };

    /// No limit beyond what the target can address. Only for trusted
    /// pipelines and inputs.
    pub const UNLIMITED: MemoryBudget = MemoryBudget { bytes: u64::MAX };

    /// A budget of `bytes`.
    pub fn bytes(bytes: u64) -> Self {
        Self { bytes }
    }

    /// The budget actually enforced: `bytes`, capped at `isize::MAX`.
    pub fn effective(&self) -> u64 {
        self.bytes.min(isize::MAX as u64)
    }
}

impl Default for MemoryBudget {
    /// [`MemoryBudget::DEFAULT`].
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Shared flag for cancelling a run from elsewhere (another thread natively).
/// Checked between steps.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that is not cancelled.
    pub fn new() -> Self {
        Self::default()
    }
    /// Cancels every run holding a clone of this token.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    /// Whether [`cancel`](Self::cancel) was called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Reported before each step starts.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Progress {
    /// Index of the step about to run.
    pub step: usize,
    /// Number of steps.
    pub steps: usize,
    /// Its op name.
    pub op: &'static str,
}

/// Progress callback: `Send + Sync` natively, anything on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub type ProgressFn = dyn Fn(Progress) + Send + Sync;
/// Progress callback: `Send + Sync` natively, anything on wasm.
#[cfg(target_arch = "wasm32")]
pub type ProgressFn = dyn Fn(Progress);

/// Everything a run needs besides the pipeline and the pixels.
///
/// `#[non_exhaustive]` (its fields depend on features): start from
/// [`Ctx::new`] and chain the `with_*` setters.
///
/// ```
/// use imgpipe::{Ctx, Limits, MemoryBudget};
/// let vars = std::collections::HashMap::from([("caption", "hi")]);
/// let cx = Ctx::new()
///     .with_budget(MemoryBudget::bytes(256 << 20))
///     .with_limits(Limits::default())
///     .with_vars(&vars);
/// assert_eq!(cx.budget.bytes, 256 << 20);
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct Ctx<'a> {
    /// Memory limit; checked against the plan before anything runs.
    pub budget: MemoryBudget,
    /// Size limits for the input and every step's output; checked by
    /// validation before anything runs. Use the value you decoded with.
    pub limits: Limits,
    /// Called before each step.
    pub progress: Option<&'a ProgressFn>,
    /// Checked between steps.
    pub cancel: CancelToken,
    /// Fonts for text steps (feature `overlay`).
    #[cfg(feature = "overlay")]
    pub fonts: Option<&'a crate::overlay::FontRegistry>,
    /// Values for `{name}` placeholders in text steps (filled by
    /// [`interpolate`](crate::overlay::interpolate)).
    pub vars: Option<&'a dyn Vars>,
}

impl Default for Ctx<'_> {
    /// [`MemoryBudget::DEFAULT`] (2 GiB), [`Limits::default`] (16 383 px a
    /// side, 100 million pixels), no progress callback, fonts or variables.
    fn default() -> Self {
        Self {
            budget: MemoryBudget::DEFAULT,
            limits: Limits::default(),
            progress: None,
            cancel: CancelToken::new(),
            #[cfg(feature = "overlay")]
            fonts: None,
            vars: None,
        }
    }
}

impl<'a> Ctx<'a> {
    /// The defaults: [`Ctx::default`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets [`budget`](Self::budget).
    pub fn with_budget(mut self, budget: MemoryBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Sets [`limits`](Self::limits).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Sets the [`progress`](Self::progress) callback.
    pub fn with_progress(mut self, progress: &'a ProgressFn) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Sets the [`cancel`](Self::cancel) token (keep a clone to cancel).
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Sets the fonts for text steps (feature `overlay`).
    #[cfg(feature = "overlay")]
    pub fn with_fonts(mut self, fonts: &'a crate::overlay::FontRegistry) -> Self {
        self.fonts = Some(fonts);
        self
    }

    /// Sets the values for `{name}` placeholders: a `HashMap` or `BTreeMap`
    /// of strings, or anything else that implements [`Vars`].
    pub fn with_vars(mut self, vars: &'a dyn Vars) -> Self {
        self.vars = Some(vars);
        self
    }
}

impl std::fmt::Debug for Ctx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Ctx");
        s.field("budget", &self.budget)
            .field("limits", &self.limits)
            .field("progress", &self.progress.map(|_| ..))
            .field("cancel", &self.cancel);
        #[cfg(feature = "overlay")]
        s.field("fonts", &self.fonts);
        s.field("vars", &self.vars.map(|_| ..)).finish()
    }
}

/// Why `info` is outside `limits`, if it is.
fn check_size(info: &FrameInfo, limits: &Limits) -> std::result::Result<(), String> {
    let FrameInfo {
        width,
        height,
        frames,
    } = *info;
    if width == 0 || height == 0 || frames == 0 {
        return Err(format!("{width}x{height} ({frames} frames) has no pixels"));
    }
    if width > limits.max_width || height > limits.max_height {
        return Err(format!(
            "{width}x{height} exceeds the maximum of {}x{}",
            limits.max_width, limits.max_height
        ));
    }
    if frames > limits.max_frames {
        return Err(format!(
            "{frames} frames exceed the maximum of {}",
            limits.max_frames
        ));
    }
    if info.pixels() > limits.max_total_pixels {
        return Err(format!(
            "{width}x{height} ({frames} frames) exceeds the maximum of {} pixels",
            limits.max_total_pixels
        ));
    }
    Ok(())
}

impl Pipeline {
    /// A pipeline of the current version.
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            version: PIPELINE_VERSION,
            steps,
        }
    }

    /// Checks every step and predicts sizes and peak memory for `input`.
    ///
    /// The input and every step's predicted output must be within `limits`
    /// (non-empty, at most `max_width` × `max_height`, `max_frames`,
    /// `max_total_pixels`); size arithmetic that would overflow is a problem
    /// too. Never panics, whatever the parameters.
    pub fn validate(
        &self,
        input: &FrameInfo,
        limits: &Limits,
    ) -> std::result::Result<Plan, Vec<Diagnostic>> {
        let mut problems = Vec::new();
        if self.version != PIPELINE_VERSION {
            problems.push(Diagnostic::pipeline(unsupported_version(self.version)));
        }
        if let Err(m) = check_size(input, limits) {
            // Step predictions for an input that cannot run would only add noise.
            problems.push(Diagnostic::pipeline(format!("input {m}")));
            return Err(problems);
        }
        let mut info = *input;
        let mut peak = input.bytes();
        let mut steps = Vec::new();
        for (i, step) in self.steps.iter().enumerate() {
            let effect = step.effect(i, info).and_then(|effect| {
                check_size(&effect.output, limits)
                    .map(|()| effect)
                    .map_err(|m| Diagnostic::step(i, step.op(), format!("output {m}")))
            });
            match effect {
                Ok(effect) => {
                    let step_peak = info
                        .bytes()
                        .saturating_add(effect.output.bytes())
                        .saturating_add(effect.scratch_bytes);
                    peak = peak.max(step_peak);
                    info = effect.output;
                    steps.push(StepPlan {
                        op: step.op(),
                        id: step.id().map(Into::into),
                        output: info,
                        peak_bytes: step_peak,
                    });
                }
                Err(d) => problems.push(d),
            }
        }
        if problems.is_empty() {
            Ok(Plan {
                input: *input,
                output: info,
                steps,
                peak_bytes: peak,
            })
        } else {
            Err(problems)
        }
    }

    /// Runs the pipeline on `frames`, step by step, on the calling thread.
    ///
    /// Validates first against `cx.limits` ([`Error::Invalid`], also for an
    /// input with no pixels) and `cx.budget` ([`Error::Budget`]); then a
    /// step can still fail ([`Error::Step`]) or the run be cancelled
    /// ([`Error::Cancelled`]) between steps. Work that should not block (a
    /// browser's main thread) belongs on a worker or a blocking-task pool.
    pub fn run(&self, mut frames: Frames, cx: &Ctx<'_>) -> Result<Frames> {
        let plan = self
            .validate(&frames.info(), &cx.limits)
            .map_err(Error::Invalid)?;
        let budget = cx.budget.effective();
        if plan.peak_bytes > budget {
            return Err(Error::Budget {
                needed: plan.peak_bytes,
                budget,
            });
        }
        let steps = self.steps.len();
        for (index, step) in self.steps.iter().enumerate() {
            if cx.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if let Some(progress) = cx.progress {
                progress(Progress {
                    step: index,
                    steps,
                    op: step.op(),
                });
            }
            frames = step.apply(frames, cx).map_err(|source| Error::Step {
                index,
                op: step.op(),
                source,
            })?;
        }
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Rgba8;
    use crate::steps::{Axis, Flip, Resize, Rotate};

    fn checker(w: u32, h: u32) -> Frames {
        let mut img = Rgba8::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [(x * 255 / w) as u8, (y * 255 / h) as u8, 0, 255]);
            }
        }
        Frames::still(img)
    }

    #[test]
    fn toml_round_trip_and_unknown_fields() {
        let toml = r##"
            version = 1
            [[steps]]
            op = "resize"
            max_long_side = 800
            [[steps]]
            op = "pad"
            top = "20%"
            color = "#ffffff"
            [[steps]]
            op = "crop"
            rect = [10, 10, "50%", "50%"]
        "##;
        let p: Pipeline = toml::from_str(toml).unwrap();
        assert_eq!(p.steps.len(), 3);
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Pipeline>(&json).unwrap(), p);

        // A typo is an error, not a silent default.
        let typo = "version = 1\n[[steps]]\nop = \"resize\"\nmax_long_sde = 800\n";
        let err = toml::from_str::<Pipeline>(typo).unwrap_err().to_string();
        assert!(err.contains("max_long_sde"), "{err}");
        let unknown_op = "version = 1\n[[steps]]\nop = \"blur\"\n";
        assert!(toml::from_str::<Pipeline>(unknown_op).is_err());
    }

    #[test]
    fn validate_predicts_sizes_and_reports_bad_steps() {
        let input = FrameInfo {
            width: 1600,
            height: 900,
            frames: 1,
        };
        let p = Pipeline::new(vec![
            Step::Resize(Resize {
                max_long_side: Some(800),
                linear: true,
                ..Default::default()
            }),
            Step::Rotate(Rotate {
                id: None,
                degrees: 90,
            }),
        ]);
        let plan = p.validate(&input, &Limits::default()).unwrap();
        assert_eq!((plan.output.width, plan.output.height), (450, 800));
        assert!(plan.peak_bytes > input.bytes());

        let bad = Pipeline::new(vec![Step::Rotate(Rotate {
            id: None,
            degrees: 45,
        })]);
        let problems = bad.validate(&input, &Limits::default()).unwrap_err();
        assert_eq!(problems[0].step, Some(0));
        assert!(problems[0].to_string().starts_with("step 0 (rotate)"));
    }

    #[test]
    fn run_applies_steps_in_order() {
        let p = Pipeline::new(vec![
            Step::Rotate(Rotate {
                id: None,
                degrees: 90,
            }),
            Step::Flip(Flip {
                id: None,
                axis: Axis::Horizontal,
            }),
        ]);
        let src = checker(4, 2);
        let out = p.run(src.clone(), &Ctx::default()).unwrap();
        assert_eq!(
            out.info(),
            FrameInfo {
                width: 2,
                height: 4,
                frames: 1
            }
        );
        // Rotate 90° clockwise then mirror = transpose: (x, y) -> (y, x).
        for y in 0..2 {
            for x in 0..4 {
                assert_eq!(out.first().pixel(y, x), src.first().pixel(x, y));
            }
        }
    }

    #[test]
    fn budget_and_cancel_are_enforced() {
        let p = Pipeline::new(vec![Step::Resize(Resize {
            max_long_side: Some(8),
            linear: true,
            ..Default::default()
        })]);
        let cx = Ctx::new().with_budget(MemoryBudget::bytes(100));
        assert!(matches!(
            p.run(checker(64, 64), &cx),
            Err(Error::Budget { .. })
        ));

        let cx = Ctx::default();
        cx.cancel.cancel();
        assert!(matches!(p.run(checker(64, 64), &cx), Err(Error::Cancelled)));
    }
}

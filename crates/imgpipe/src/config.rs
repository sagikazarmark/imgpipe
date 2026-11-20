//! Pipeline files: `version` and `steps` (the library's part) plus
//! `[[outputs]]` (encoded variants), as the `imgpipe` CLI reads them.
//!
//! # Versioning
//!
//! Every file starts with `version`, and this build reads exactly
//! [`PIPELINE_VERSION`](crate::PIPELINE_VERSION). The version is checked
//! before anything else in the file: a file of another version fails with
//! "unsupported pipeline version N", never with an error about the steps
//! inside it, wherever `version` appears in the file.
//!
//! [`PIPELINE_VERSION`](crate::PIPELINE_VERSION) goes up when an existing
//! file would stop meaning the same thing: an op or field removed or
//! renamed, a default or the meaning of a parameter changed, or a field
//! made required. A file of the old version is then rejected rather than
//! run differently.
//!
//! Additions do not bump it: a new op, or a new optional field with a
//! default that keeps the old behaviour. Every struct here denies unknown
//! fields, so typos are errors; the flip side is that an older build
//! rejects a file that uses such an addition ("unknown field" or "unknown
//! variant"), even though it has the same version. Files that only use
//! what a build knows run unchanged on every later build of that version.

//!
//! # Example
//!
//! ```toml
//! version = 1
//!
//! [[steps]]
//! op = "resize"
//! max_long_side = 1600
//!
//! [[outputs]]
//! name = "{stem}.{hash8}.jpg"
//! format = "jpeg"
//! quality = 85
//!
//! [[outputs]]
//! name = "{stem}/thumb.webp"
//! format = "webp"
//! lossless = true
//! tail = [{ op = "resize", max_long_side = 320 }]
//! ```

use crate::pipeline::{FieldValue, Pipeline, VersionedFields};
use crate::steps::Step;
use crate::variants::Variant;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A pipeline file as written in TOML or JSON: `version` and `steps` (the
/// [`Pipeline`]), then one `[[outputs]]` entry per encoded output (a
/// [`Variant`]). Deserializing checks `version` first (see the
/// [module docs](self)).
///
/// Run it with `file.pipeline.run(...)`, then
/// [`run_variants`](crate::variants::run_variants) with `&file.outputs`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PipelineFile {
    /// The version and the steps run on every input, in order.
    pub pipeline: Pipeline,
    /// Encoded outputs made from the result (`[[outputs]]`).
    pub outputs: Vec<Variant>,
}

// The file as written, for the JSON Schema; the doc comment is its description.
/// A pipeline file: `version`, the `steps` run on every input, and one
/// `[[outputs]]` entry per encoded output.
#[cfg(feature = "schemars")]
#[derive(schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename = "PipelineFile")]
#[allow(dead_code)]
struct FileFields {
    /// File format version; see `PIPELINE_VERSION`.
    version: u16,
    /// Steps run on every input, in order.
    #[serde(default)]
    steps: Vec<Step>,
    /// Encoded outputs made from the result.
    #[serde(default)]
    outputs: Vec<Variant>,
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for PipelineFile {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PipelineFile".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        FileFields::json_schema(g)
    }
}

impl Serialize for PipelineFile {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Fields<'a> {
            version: u16,
            steps: &'a [Step],
            outputs: &'a [Variant],
        }
        Fields {
            version: self.pipeline.version,
            steps: &self.pipeline.steps,
            outputs: &self.outputs,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for PipelineFile {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Version,
            Steps,
            Outputs,
        }
        struct FileVisitor;
        impl<'de> Visitor<'de> for FileVisitor {
            type Value = PipelineFile;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a pipeline file: `version`, `steps` and `outputs`")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<PipelineFile, A::Error> {
                let mut fields = VersionedFields::new();
                let (mut steps, mut outputs) = (None, None);
                let (mut seen_steps, mut seen_outputs) = (false, false);
                while let Some(key) = map.next_key::<Field>()? {
                    match key {
                        Field::Version => fields.version(&mut map)?,
                        Field::Steps => {
                            steps = Some(fields.value(&mut map, "steps", &mut seen_steps)?)
                        }
                        Field::Outputs => {
                            outputs = Some(fields.value(&mut map, "outputs", &mut seen_outputs)?)
                        }
                    }
                }
                Ok(PipelineFile {
                    pipeline: Pipeline {
                        version: fields.finish()?,
                        steps: steps.map(FieldValue::get).transpose()?.unwrap_or_default(),
                    },
                    outputs: outputs
                        .map(FieldValue::get)
                        .transpose()?
                        .unwrap_or_default(),
                })
            }
        }
        d.deserialize_struct(
            "PipelineFile",
            &["version", "steps", "outputs"],
            FileVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::PipelineFile;
    use crate::Pipeline;

    const NEWER: &str = "unsupported pipeline version 2 (this build reads version 1)";

    #[test]
    fn version_is_checked_before_the_steps() {
        // A newer file with an op this build doesn't know: the version is
        // the problem to report, not the op.
        let tomls = [
            "version = 2\n[[steps]]\nop = \"blur\"\n",
            // An inline array can come before `version` in TOML.
            "steps = [{ op = \"blur\" }]\nversion = 2\n",
            "version = 2\n[[outputs]]\nname = \"a\"\nformat = \"avif2\"\n",
        ];
        for text in tomls {
            let err = toml::from_str::<PipelineFile>(text)
                .unwrap_err()
                .to_string();
            assert!(err.contains(NEWER), "{text:?}: {err}");
            if !text.contains("outputs") {
                let err = toml::from_str::<Pipeline>(text).unwrap_err().to_string();
                assert!(err.contains(NEWER), "{text:?}: {err}");
            }
        }
        for json in [
            r#"{"version": 2, "steps": [{"op": "blur"}]}"#,
            r#"{"steps": [{"op": "blur"}], "version": 2}"#,
            r#"{"outputs": [{"name": "a", "format": "png", "tail": [{"op": "blur"}]}], "version": 2}"#,
        ] {
            let err = serde_json::from_str::<PipelineFile>(json)
                .unwrap_err()
                .to_string();
            assert!(err.contains(NEWER), "{json}: {err}");
        }
        let err = serde_json::from_str::<Pipeline>(r#"{"steps": [{"op": "blur"}], "version": 2}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains(NEWER), "{err}");
    }

    #[test]
    fn current_files_parse_in_any_field_order() {
        let a: PipelineFile = serde_json::from_str(
            r#"{"steps": [{"op": "resize", "max_long_side": 10}],
                "outputs": [{"name": "{stem}.png", "format": "png"}], "version": 1}"#,
        )
        .unwrap();
        let b: PipelineFile = toml::from_str(
            "version = 1\n[[steps]]\nop = \"resize\"\nmax_long_side = 10\n\
             [[outputs]]\nname = \"{stem}.png\"\nformat = \"png\"\n",
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.pipeline.steps.len(), 1);
        // Round trip through our own serialization.
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<PipelineFile>(&json).unwrap(), a);
        // `steps` and `outputs` are optional; `version` is not.
        let empty: PipelineFile = toml::from_str("version = 1").unwrap();
        assert!(empty.pipeline.steps.is_empty() && empty.outputs.is_empty());
        let err = toml::from_str::<PipelineFile>("steps = []").unwrap_err();
        assert!(err.to_string().contains("missing field `version`"), "{err}");
    }

    #[test]
    fn errors_still_name_the_problem() {
        let cases = [
            ("version = 1\nverison = 1\n", "unknown field `verison`"),
            ("version = 1\nversion = 1\n", "duplicate"),
            (
                "version = 1\n[[steps]]\nop = \"blur\"\n",
                "unknown variant `blur`",
            ),
            // Buffered (steps before version), then parsed: still precise.
            (
                "steps = [{ op = \"resize\", max_long_sde = 3 }]\nversion = 1\n",
                "max_long_sde",
            ),
        ];
        for (text, expected) in cases {
            let err = toml::from_str::<PipelineFile>(text)
                .unwrap_err()
                .to_string();
            assert!(err.contains(expected), "{text:?}: {err}");
        }
    }
}

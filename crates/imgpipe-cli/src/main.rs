//! `imgpipe`: run, validate and explain pipeline files.

use clap::{Parser, Subcommand, ValueEnum};
use imgpipe::variants::{FsSink, RenderedVariant, run_variants};
use imgpipe::{Ctx, DecodeOptions, EncodeOptions, Encoders, Format, FrameInfo, Limits, decode};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A pipeline file (`version`, `steps`, `[[outputs]]`), read from TOML or JSON.
type Config = imgpipe::config::PipelineFile;

fn load_config(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "json" => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        _ => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
    }
}

#[derive(Parser)]
#[command(name = "imgpipe", version, about = "Run image pipeline files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Run a pipeline over input images.
    Run {
        /// Pipeline file (TOML, or JSON by extension).
        #[arg(short, long)]
        config: PathBuf,
        /// Input files or quoted glob patterns.
        #[arg(required = true)]
        inputs: Vec<String>,
        /// Write one output here (format from the extension); needs one input.
        /// The first `[[outputs]]` entry of that format, if any, supplies its
        /// extra steps (tail) and encoder settings.
        #[arg(short, long, conflicts_with = "out_dir")]
        output: Option<PathBuf>,
        /// Write every `[[outputs]]` entry under this directory.
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Parallel jobs (default: available cores).
        #[arg(short, long)]
        jobs: Option<usize>,
        /// Font for text steps: ID=PATH, or ID=PATH@AXIS=VALUE,... to pin a
        /// variable font (e.g. oswald-bold=Oswald.ttf@wght=700). Repeatable.
        #[arg(long = "font", value_name = "ID=PATH")]
        fonts: Vec<String>,
        /// Value for a {name} placeholder in text steps: NAME=VALUE. Repeatable.
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
        /// Report format: one line per output, or JSON.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },
    /// Check a pipeline file: schema, parameters, encoders, predicted memory.
    Validate {
        /// Pipeline file (TOML, or JSON by extension).
        #[arg(short, long)]
        config: PathBuf,
        /// Input size to predict for, as WxH or WxHxFRAMES.
        #[arg(long, default_value = "1600x1600")]
        input: String,
        /// Report format: problems and the predicted size, or JSON.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },
    /// Print the resolved steps and predicted sizes as JSON.
    Explain {
        /// Pipeline file (TOML, or JSON by extension).
        #[arg(short, long)]
        config: PathBuf,
        /// Input size to predict for, as WxH or WxHxFRAMES.
        #[arg(long, default_value = "1600x1600")]
        input: String,
    },
    /// Print the JSON Schema of pipeline files.
    Schema,
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Run {
            config,
            inputs,
            output,
            out_dir,
            jobs,
            fonts,
            vars,
            format,
        } => load_env(&fonts, &vars)
            .and_then(|env| run(&config, &inputs, output, out_dir, jobs, &env, format)),
        Command::Validate {
            config,
            input,
            format,
        } => validate(&config, &input, format),
        Command::Explain { config, input } => explain(&config, &input),
        Command::Schema => {
            println!(
                "{}",
                serde_json::to_string_pretty(&schemars::schema_for!(Config)).unwrap()
            );
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_size(s: &str) -> Result<FrameInfo, String> {
    let parts: Vec<u32> = s
        .split('x')
        .map(|p| p.parse().map_err(|_| format!("invalid size {s:?}")))
        .collect::<Result<_, _>>()?;
    match parts[..] {
        [w, h] => Ok(FrameInfo {
            width: w,
            height: h,
            frames: 1,
        }),
        [w, h, f] => Ok(FrameInfo {
            width: w,
            height: h,
            frames: f,
        }),
        _ => Err(format!("invalid size {s:?}; use WxH or WxHxFRAMES")),
    }
}

#[derive(Serialize)]
struct Report {
    ok: bool,
    problems: Vec<String>,
    output: Option<FrameInfo>,
    peak_bytes: Option<u64>,
}

fn check(config: &Config, input: FrameInfo) -> Report {
    let mut problems = Vec::new();
    let plan = match config.pipeline.validate(&input, &Limits::default()) {
        Ok(p) => Some(p),
        Err(d) => {
            problems.extend(d.iter().map(ToString::to_string));
            None
        }
    };
    let encoders = Encoders::builtin();
    for (i, v) in config.outputs.iter().enumerate() {
        if encoders.find(&v.encode).is_none() {
            let kind = if v.encode.lossless {
                "lossless"
            } else {
                "lossy"
            };
            problems.push(format!(
                "output {i} ({}): no {kind} {} encoder in this build",
                v.name, v.encode.format
            ));
        }
        if let Some(p) = &plan
            && let Err(d) = v.tail.validate(&p.output, &Limits::default())
        {
            problems.extend(d.iter().map(|d| format!("output {i} tail: {d}")));
        }
    }
    Report {
        ok: problems.is_empty(),
        output: plan.as_ref().map(|p| p.output),
        peak_bytes: plan.map(|p| p.peak_bytes),
        problems,
    }
}

/// "1 frame", "3 frames".
fn frames(n: u32) -> String {
    format!("{n} frame{}", if n == 1 { "" } else { "s" })
}

fn validate(path: &Path, input: &str, format: OutputFormat) -> Result<(), String> {
    let config = load_config(path)?;
    let report = check(&config, parse_size(input)?);
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report).unwrap()),
        OutputFormat::Text => {
            for p in &report.problems {
                println!("{p}");
            }
            if let (Some(out), Some(peak)) = (report.output, report.peak_bytes) {
                println!(
                    "{input} -> {}x{} ({}), peak memory {:.1} MB",
                    out.width,
                    out.height,
                    frames(out.frames),
                    peak as f64 / 1e6
                );
            }
        }
    }
    if report.ok {
        Ok(())
    } else {
        Err(format!("{} problem(s)", report.problems.len()))
    }
}

fn explain(path: &Path, input: &str) -> Result<(), String> {
    let config = load_config(path)?;
    let plan = config
        .pipeline
        .validate(&parse_size(input)?, &Limits::default())
        .map_err(|d| {
            d.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        })?;
    let explained = serde_json::json!({
        "imgpipe": imgpipe::VERSION,
        "pipeline": config.pipeline,
        "plan": plan,
        "encoders": Encoders::builtin().ids(),
    });
    println!("{}", serde_json::to_string_pretty(&explained).unwrap());
    Ok(())
}

fn expand_inputs(patterns: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for p in patterns {
        if p.contains(['*', '?', '[']) {
            let matches: Vec<_> = glob::glob(p)
                .map_err(|e| format!("{p}: {e}"))?
                .filter_map(Result::ok)
                .collect();
            if matches.is_empty() {
                return Err(format!("{p}: no files match"));
            }
            files.extend(matches);
        } else {
            files.push(PathBuf::from(p));
        }
    }
    Ok(files)
}

#[derive(Serialize)]
struct InputResult {
    input: String,
    outputs: Vec<RenderedVariant>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn process(
    path: &Path,
    config: &Config,
    target: &Target,
    encoders: &Encoders,
    env: &RunEnv,
) -> Result<Vec<RenderedVariant>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let frames = decode(&bytes, &DecodeOptions::default()).map_err(|e| e.to_string())?;
    let cx = Ctx::new().with_fonts(&env.fonts).with_vars(&env.vars);
    let result = config
        .pipeline
        .run(frames, &cx)
        .map_err(|e| e.to_string())?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    match target {
        Target::Single(out) => {
            let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("");
            let format = Format::from_extension(ext)
                .ok_or_else(|| format!("cannot infer an output format from {ext:?}"))?;
            // Use the first [[outputs]] entry of that format when there is
            // one: its extra steps (`tail`) and its encoder settings.
            let (result, encode) = match config.outputs.iter().find(|v| v.encode.format == format) {
                Some(v) => {
                    let tailed = if v.tail.steps.is_empty() {
                        result
                    } else {
                        v.tail.run(result, &cx).map_err(|e| e.to_string())?
                    };
                    (tailed, v.encode)
                }
                None => (
                    result,
                    EncodeOptions::new(format).lossless(format == Format::Webp),
                ),
            };
            let encoded = encoders
                .encode(&result, &encode)
                .map_err(|e| e.to_string())?;
            std::fs::write(out, &encoded).map_err(|e| format!("{}: {e}", out.display()))?;
            Ok(vec![RenderedVariant::new(
                out.display().to_string(),
                &result,
                encoded.len(),
            )])
        }
        Target::Dir(dir) => {
            let mut sink = FsSink::new(dir);
            run_variants(&result, stem, &config.outputs, &cx, encoders, &mut sink)
                .map_err(|e| e.to_string())
        }
    }
}

enum Target {
    Single(PathBuf),
    Dir(PathBuf),
}

fn run(
    config_path: &Path,
    inputs: &[String],
    output: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    jobs: Option<usize>,
    env: &RunEnv,
    format: OutputFormat,
) -> Result<(), String> {
    let config = load_config(config_path)?;
    let files = expand_inputs(inputs)?;
    let target = match (output, out_dir) {
        (Some(o), _) if files.len() == 1 => Target::Single(o),
        (Some(_), _) => return Err("-o needs exactly one input; use --out-dir for several".into()),
        (None, Some(d)) if !config.outputs.is_empty() => Target::Dir(d),
        (None, Some(_)) => return Err("--out-dir needs [[outputs]] in the pipeline file".into()),
        (None, None) => return Err("give -o FILE or --out-dir DIR".into()),
    };
    let jobs = jobs
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
        .clamp(1, files.len().max(1));
    let encoders = Encoders::builtin();
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let r = process(path, &config, &target, &encoders, env);
                    let entry = match r {
                        Ok(outputs) => InputResult {
                            input: path.display().to_string(),
                            outputs,
                            error: None,
                        },
                        Err(e) => InputResult {
                            input: path.display().to_string(),
                            outputs: vec![],
                            error: Some(e),
                        },
                    };
                    results.lock().unwrap().push((i, entry));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(i, _)| *i);
    let results: Vec<_> = results.into_iter().map(|(_, r)| r).collect();
    let failed = results.iter().filter(|r| r.error.is_some()).count();
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&results).unwrap()),
        OutputFormat::Text => {
            for r in &results {
                match &r.error {
                    Some(e) => println!("{}: error: {e}", r.input),
                    None => {
                        for o in &r.outputs {
                            println!(
                                "{} -> {} ({}x{}, {} bytes)",
                                r.input, o.name, o.width, o.height, o.bytes
                            );
                        }
                    }
                }
            }
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} of {} input(s) failed", results.len()))
    }
}

/// Fonts and placeholder values for text steps.
struct RunEnv {
    fonts: imgpipe::overlay::FontRegistry,
    vars: std::collections::HashMap<String, String>,
}

fn load_env(fonts: &[String], vars: &[String]) -> Result<RunEnv, String> {
    let mut registry = imgpipe::overlay::FontRegistry::new();
    for spec in fonts {
        let (id, rest) = spec
            .split_once('=')
            .ok_or_else(|| format!("--font {spec:?}: use ID=PATH"))?;
        let (path, axes) = rest.split_once('@').unwrap_or((rest, ""));
        let axes: Vec<(&str, f32)> = axes
            .split(',')
            .filter(|a| !a.is_empty())
            .map(|a| {
                let (tag, value) = a
                    .split_once('=')
                    .ok_or_else(|| format!("--font {spec:?}: axes are AXIS=VALUE"))?;
                let value = value
                    .parse()
                    .map_err(|_| format!("--font {spec:?}: {value:?} is not a number"))?;
                Ok((tag, value))
            })
            .collect::<Result<_, String>>()?;
        let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        registry
            .register_instance(id, data, &axes)
            .map_err(|e| e.to_string())?;
    }
    let vars = vars
        .iter()
        .map(|v| {
            v.split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .ok_or_else(|| format!("--var {v:?}: use NAME=VALUE"))
        })
        .collect::<Result<_, String>>()?;
    Ok(RunEnv {
        fonts: registry,
        vars,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_output_runs_the_matching_outputs_tail() {
        let dir = std::env::temp_dir().join(format!("imgpipe-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.png");
        let img = imgpipe::Rgba8::filled(180, 120, [10, 20, 30, 255]);
        let png = Encoders::builtin()
            .encode(
                &imgpipe::Frames::still(img),
                &EncodeOptions::new(Format::Png),
            )
            .unwrap();
        std::fs::write(&input, png).unwrap();
        let config: Config = toml::from_str(
            r#"
            version = 1
            [[outputs]]
            name = "{stem}.webp"
            format = "webp"
            lossless = true
            [[outputs]]
            name = "{stem}.png"
            format = "png"
            tail = [{ op = "resize", max_long_side = 10 }]
            "#,
        )
        .unwrap();
        let env = RunEnv {
            fonts: imgpipe::overlay::FontRegistry::new(),
            vars: Default::default(),
        };
        let out = dir.join("single.png");
        let outputs = process(
            &input,
            &config,
            &Target::Single(out.clone()),
            &Encoders::builtin(),
            &env,
        )
        .unwrap();
        assert_eq!((outputs[0].width, outputs[0].height), (10, 7));
        let back = decode(&std::fs::read(&out).unwrap(), &DecodeOptions::default()).unwrap();
        assert_eq!((back.first().width(), back.first().height()), (10, 7));

        // No entry of that format: the base result, default settings.
        let out = dir.join("single.jpg");
        let outputs = process(
            &input,
            &config,
            &Target::Single(out),
            &Encoders::builtin(),
            &env,
        )
        .unwrap();
        assert_eq!((outputs[0].width, outputs[0].height), (180, 120));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

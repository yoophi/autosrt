use autosrt::{
    adapters::{
        config::{ConfigFile, legacy_model_env, normalize_language},
        storage::FileStore,
        tools::{Ffmpeg, Whisper, YtDlp, find_tool, parse_input},
        ui,
    },
    application::{Context, Error, Pipeline, Request, Result},
    domain::Input,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::{
    io::IsTerminal,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Parser)]
#[command(
    version,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    about = "Turn YouTube videos or local media into SRT subtitles using local speech recognition"
)]
struct Cli {
    /// YouTube URL or local media file
    #[arg(required = true)]
    input: Option<String>,
    #[command(subcommand)]
    command: Option<Commands>,
    /// whisper.cpp multilingual model file
    #[arg(long, env = "AUTOSRT_MODEL")]
    model: Option<PathBuf>,
    /// Spoken language (overrides saved lang; otherwise auto)
    #[arg(short = 'l', long, visible_alias = "lang", value_parser = normalize_language)]
    language: Option<String>,
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Replace an existing subtitle file
    #[arg(long)]
    force: bool,
    /// Disable the interactive terminal UI
    #[arg(long)]
    no_tui: bool,
    /// Preserve downloaded media, extracted audio, and raw transcript
    #[arg(long)]
    keep_work: bool,
}
#[derive(Subcommand)]
enum Commands {
    /// Diagnose dependencies, versions, configuration and model (offline)
    Doctor {
        /// Check only dependencies needed for local media
        #[arg(long)]
        local: bool,
        /// Override the configured model for this check
        #[arg(long, env = "AUTOSRT_MODEL")]
        model: Option<PathBuf>,
    },
    /// Manage persistent user settings (no speech engine required)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}
#[derive(Subcommand)]
enum ConfigAction {
    /// Copy legacy ystt settings and its model into the AutoSRT directory
    Migrate,
    /// Save a default setting
    Set { key: ConfigKey, value: String },
    /// Print the effective default value
    Get { key: ConfigKey },
    /// List effective default settings
    List,
    /// Print the configuration file path
    Path,
}
#[derive(Clone, ValueEnum)]
enum ConfigKey {
    Lang,
    Model,
}

fn main() {
    let cli = Cli::parse();
    match execute(cli) {
        Ok(()) => {}
        Err(Error::Cancelled) => {
            eprintln!("autosrt: cancelled");
            std::process::exit(130);
        }
        Err(e) => {
            eprintln!("autosrt: {e}");
            std::process::exit(1);
        }
    }
}
fn execute(cli: Cli) -> Result<()> {
    if let Some(Commands::Doctor { local, model }) = cli.command {
        let report = autosrt::adapters::doctor::diagnose(local, model.or_else(legacy_model_env));
        report.print();
        return if report.passed() {
            Ok(())
        } else {
            Err(Error::Message(
                "Dependency checks failed; follow the FAIL entries above.".into(),
            ))
        };
    }
    if let Some(Commands::Config { action }) = cli.command {
        if matches!(action, ConfigAction::Migrate) {
            let copied = ConfigFile::migrate_legacy()?;
            println!(
                "{}",
                if copied {
                    "Migrated settings and model to AutoSRT; legacy files preserved."
                } else {
                    "AutoSRT config already exists; no files overwritten."
                }
            );
            return Ok(());
        }
        let file = ConfigFile::discover()?;
        match action {
            ConfigAction::Migrate => unreachable!(),
            ConfigAction::Set {
                key: ConfigKey::Lang,
                value,
            } => {
                file.set_language(&value)?;
                println!("lang={}", file.language()?);
            }
            ConfigAction::Get {
                key: ConfigKey::Lang,
            } => println!("{}", file.language()?),
            ConfigAction::Set {
                key: ConfigKey::Model,
                value,
            } => {
                file.set_model(&value)?;
                println!("model={}", file.model()?.unwrap().display());
            }
            ConfigAction::Get {
                key: ConfigKey::Model,
            } => {
                let model = file.model()?.ok_or_else(|| {
                    Error::Message(
                        "No default model; use autosrt config set model /path/to/ggml-small.bin"
                            .into(),
                    )
                })?;
                println!("{}", model.display());
            }
            ConfigAction::List => {
                println!("lang={}", file.language()?);
                if let Some(model) = file.model()? {
                    println!("model={}", model.display());
                }
            }
            ConfigAction::Path => println!("{}", file.path.display()),
        }
        return Ok(());
    }
    let raw_input = cli.input.expect("clap requires input or subcommand");
    let input = parse_input(&raw_input)?;
    let mut required = vec!["ffmpeg", "ffprobe", "whisper-cli"];
    if matches!(input, Input::Youtube(_)) {
        required.insert(0, "yt-dlp");
    }
    let missing: Vec<_> = required
        .into_iter()
        .filter(|tool| find_tool(tool).is_err())
        .collect();
    if !missing.is_empty() {
        return Err(Error::Message(format!(
            "Missing tool: {}. Install the required commands and add them to PATH (see README.md).",
            missing.join(", ")
        )));
    }
    let language = match cli.language {
        Some(language) => language,
        None => ConfigFile::discover()?.language()?,
    };
    let model = match cli.model.or_else(legacy_model_env) {
        Some(model) => Some(model),
        None => ConfigFile::discover()?.model()?,
    }.ok_or_else(|| Error::Message("No speech model configured. Run: autosrt config set model /path/to/ggml-small.bin (or use --model / AUTOSRT_MODEL). See README.md.".into()))?;
    let model = std::fs::canonicalize(model)?;
    if !model.is_file() {
        return Err(Error::Message("Model must be a file".into()));
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
        .map_err(|e| Error::Message(e.to_string()))?;
    let (progress, events) = ui::channel();
    let ctx = Context {
        cancel: cancelled,
        events: progress,
    };
    let worker_ctx = ctx.clone();
    let tui = !cli.no_tui && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let pipeline = Pipeline {
        downloader: Box::new(YtDlp),
        extractor: Box::new(Ffmpeg),
        transcriber: Box::new(Whisper { model, language }),
        store: Box::new(FileStore),
    };
    let work = tempfile::Builder::new().prefix("autosrt-").tempdir()?;
    let work_path = work.path().to_owned();
    let req = Request {
        input,
        output: cli.output,
        force: cli.force,
    };
    if cli.keep_work {
        let path = work.keep();
        eprintln!("Working files: {}", path.display());
    } else {
        // The owner lives through monitor so cleanup happens after child exit.
        let result = ui::monitor(tui, raw_input, ctx, events, move || {
            pipeline.run(req, &work_path, &worker_ctx)
        });
        drop(work);
        return result.map(ui::output_path);
    }
    ui::monitor(tui, raw_input, ctx, events, move || {
        pipeline.run(req, &work_path, &worker_ctx)
    })
    .map(ui::output_path)
}

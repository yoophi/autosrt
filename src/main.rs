mod help;

use autosrt::{
    adapters::{
        config::{ConfigFile, legacy_model_env, normalize_language},
        storage::FileStore,
        tools::{Ffmpeg, Whisper, YtDlp, parse_input},
        ui,
    },
    application::{Context, Error, Pipeline, Request, Result},
    domain::SubtitleMode,
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
    about = "Generate UTF-8 SRT subtitles from one YouTube URL or local media file",
    before_long_help = help::OVERVIEW,
    after_long_help = help::ROOT,
    after_help = "AI agents and scripts: autosrt --no-tui <URL_OR_FILE> --output <SRT_FILE>. Use --help for the full contract."
)]
struct Cli {
    /// Exactly one YouTube video URL or existing local media file
    #[arg(required = true, value_name = "URL_OR_FILE")]
    input: Option<String>,
    #[command(subcommand)]
    command: Option<Commands>,
    /// whisper.cpp multilingual model file (needed only for transcription)
    #[arg(
        long,
        env = "AUTOSRT_MODEL",
        hide_env_values = true,
        value_name = "GGML_FILE"
    )]
    model: Option<PathBuf>,
    /// Subtitle track / spoken language (saved lang or auto)
    #[arg(short = 'l', long, visible_alias = "lang", value_parser = normalize_language, long_help = help::language_help())]
    language: Option<String>,
    /// Output SRT file; default: ./<input-stem-or-video-id>.srt. Parent must exist
    #[arg(short, long, value_name = "SRT_FILE")]
    output: Option<PathBuf>,
    /// Allow replacing the output file; never overwrite input media
    #[arg(long)]
    force: bool,
    /// Non-interactive agent mode: SRT paths on stdout, progress/errors on stderr
    #[arg(long)]
    no_tui: bool,
    /// Strategy: CLI > saved setting > auto-first; both emits up to 2 files (1 if captions absent)
    #[arg(long, value_name = "MODE")]
    subtitle_mode: Option<SubtitleMode>,
    /// Keep temporary media/WAV/JSON on success, failure or cancellation; path on stderr
    #[arg(long)]
    keep_work: bool,
}
#[derive(Subcommand)]
enum Commands {
    /// Diagnose dependencies, versions, configuration and model (offline)
    #[command(after_long_help = help::DOCTOR)]
    Doctor {
        /// Check only dependencies needed for local media
        #[arg(long)]
        local: bool,
        /// Override the configured model for this check
        #[arg(
            long,
            env = "AUTOSRT_MODEL",
            hide_env_values = true,
            value_name = "GGML_FILE"
        )]
        model: Option<PathBuf>,
    },
    /// Manage persistent user settings (no speech engine required)
    #[command(after_long_help = help::config_help())]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}
#[derive(Subcommand)]
enum ConfigAction {
    /// Copy legacy ystt settings and its model into the AutoSRT directory
    #[command(after_long_help = help::MIGRATE)]
    Migrate,
    /// Save a default setting
    #[command(after_long_help = help::set_help())]
    Set { key: ConfigKey, value: String },
    /// Print one saved value (lang defaults to auto; unset model is an error)
    Get { key: ConfigKey },
    /// Print saved defaults as key=value lines (no JSON; unset model omitted)
    List,
    /// Print the active config file path without creating the file
    Path,
}
#[derive(Clone, ValueEnum)]
enum ConfigKey {
    Lang,
    Model,
    SubtitleMode,
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
            ConfigAction::Set {
                key: ConfigKey::SubtitleMode,
                value,
            } => {
                file.set_subtitle_mode(&value)?;
                println!("subtitle-mode={}", file.subtitle_mode()?.as_str());
            }
            ConfigAction::Get {
                key: ConfigKey::SubtitleMode,
            } => println!("{}", file.subtitle_mode()?.as_str()),
            ConfigAction::List => {
                println!("subtitle-mode={}", file.subtitle_mode()?.as_str());
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
    let mode = match cli.subtitle_mode {
        Some(mode) => mode,
        None => ConfigFile::discover()?.subtitle_mode()?,
    };
    let language = match cli.language {
        Some(language) => language,
        None => ConfigFile::discover()?.language()?,
    };
    let model = match cli.model.or_else(legacy_model_env) {
        Some(model) => Some(model),
        None => ConfigFile::discover()?.model()?,
    };
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
        captions: Box::new(YtDlp),
        extractor: Box::new(Ffmpeg),
        transcriber: Box::new(Whisper {
            model,
            language: language.clone(),
        }),
        store: Box::new(FileStore),
    };
    let work = tempfile::Builder::new().prefix("autosrt-").tempdir()?;
    let work_path = work.path().to_owned();
    let req = Request {
        input,
        output: cli.output,
        force: cli.force,
        mode,
        language,
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
        return result.map(|paths| paths.into_iter().for_each(ui::output_path));
    }
    ui::monitor(tui, raw_input, ctx, events, move || {
        pipeline.run(req, &work_path, &worker_ctx)
    })
    .map(|paths| paths.into_iter().for_each(ui::output_path))
}

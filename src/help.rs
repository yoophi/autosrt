//! CLI contracts kept next to the driving adapter, independent of transcription logic.

pub const ROOT: &str = r#"AGENT QUICK START:
  autosrt doctor --local
  autosrt config set model /absolute/path/to/ggml-small.bin
  autosrt --no-tui ./audio.mp3 --language ko --output ./audio.srt
  autosrt --no-tui 'https://youtu.be/VIDEO_ID' --output ./video.srt

  VIDEO_ID is a placeholder. Replace it with the real video ID.
  Options may precede or follow INPUT. Quote URLs and paths containing spaces.
  Use an absolute --output path when another process will consume the result.

INPUT CONTRACT:
  Exactly one YouTube video URL OR one existing local media file (MP3, WAV,
  MP4, etc.; any format supported by FFmpeg with an audio stream).
  Local files bypass yt-dlp. No stdin input, playlist, live stream, batch input,
  translation, JSON output, or streaming transcription is supported.
  Use ./doctor or ./config for files with reserved command names.
  For names beginning with a dash: autosrt --no-tui -- ./-recording.mp3

OUTPUT AND AUTOMATION:
  --no-tui never opens an interactive screen or prompts for input.
  Without --no-tui, redirected/non-terminal execution also disables the TUI.
  Transcription success: stdout contains only the SRT path followed by a newline.
  Progress/errors go to stderr. Exit status is authoritative; do not parse
  progress text as a stable protocol. SRT contents are written to a UTF-8 file.
  Default output: current directory + local filename stem or YouTube video ID
  + .srt. --output is a file path, not a directory or stdout destination.
  The parent directory must exist. Existing output fails unless --force is set.
  Input media is never overwritten. Temporary files are cleaned up unless
  --keep-work is set (its directory is reported on stderr).

DEPENDENCIES:
  All transcription: ffmpeg, ffprobe, whisper-cli on PATH and a local GGML model.
  YouTube also needs yt-dlp with its EJS component and an enabled JS runtime
  (Deno recommended). No tools or models are installed/downloaded automatically.
  Run autosrt doctor for all checks, or doctor --local for local-media checks.
  Missing tools are reported before model checks and before starting work.

CONFIGURATION AND PRECEDENCE:
  Model: --model > AUTOSRT_MODEL > legacy YSTT_MODEL > saved config model.
  Language: --language / --lang / -l > saved config lang > auto.
  Language selects source speech, not the desired subtitle/translation language.
  A mismatched language can produce repeated or invented captions. auto detects
  the initial language; it does not switch languages within a mixed-language file.
  For a Korean intro followed by English speech, use --language en for the body,
  or split at the language boundary and transcribe each part separately.
  Config directory: AUTOSRT_CONFIG_DIR > legacy YSTT_CONFIG_DIR > OS default.
  macOS: ~/Library/Application Support/autosrt/config.json
  Linux: $XDG_CONFIG_HOME/autosrt/config.json (default ~/.config/autosrt/).
  If the new default file is absent, legacy ystt config is read when present.
  Use autosrt config path to discover the active path; config migrate copies
  legacy settings/model safely. Environment overrides do not modify saved values.

EXIT STATUS:
  0    Transcription completed; doctor passed (warnings allowed); config succeeded;
       or help/version was displayed.
  1    Runtime/input-file/URL/config/model/dependency/processing/output error;
       or doctor reported FAIL. Inspect stderr and doctor output for next steps.
  2    CLI argument parsing error (missing argument, extra input, invalid option).
  130  Cancelled by SIGINT / Ctrl+C. No final SRT is published for incomplete work.

RETRY GUIDANCE:
  Existing output: choose another --output or deliberately use --force.
  Missing model: autosrt config set model /absolute/path/to/ggml-small.bin
  YouTube HTTP 403: update yt-dlp using its original package manager and run
  autosrt doctor. Access restrictions may remain; do not retry indefinitely.
  GPU initialization failure: AutoSRT reports it and retries once on CPU.
  Other processing failures are returned without an automatic retry loop.

COMMAND HELP:
  autosrt doctor --help
  autosrt config --help
  autosrt config set --help"#;

pub const DOCTOR: &str = r#"EXAMPLES:
  autosrt doctor
  autosrt doctor --local
  autosrt doctor --local --model /absolute/path/to/ggml-small.bin

Checks executable paths, version-command execution, yt-dlp release age, enabled
YouTube JS/EJS diagnostics, configuration and the selected model's GGML header.
Each external probe has a 5-second timeout. Checks continue after a failed item.
--local skips yt-dlp and YouTube JS/EJS checks.

OUTPUT: human-readable OK/WARN/FAIL report and summary on stdout; errors on stderr.
EXIT: 0 when no FAIL entries exist (WARN is allowed), otherwise 1.
No network request, full model loading, transcription or GPU compatibility test
is performed. No installation, model download or configuration write occurs.
Model precedence: --model > AUTOSRT_MODEL > YSTT_MODEL > saved config model.
Options belong after the subcommand: autosrt doctor --model PATH."#;

pub const CONFIG: &str = r#"EXAMPLES:
  autosrt config set lang ko
  autosrt config set model /absolute/path/to/ggml-small.bin
  autosrt config get lang
  autosrt config get model
  autosrt config list
  autosrt config path
  autosrt config migrate

Keys: lang (Whisper language code or auto), model (existing local GGML model path).
No speech engine or model loading is needed for configuration commands.
set writes persistent defaults; a model must be an existing regular file and is
stored as an absolute path. lang is normalized to lowercase.
get/list return stored defaults, not invocation/environment model overrides.
Missing lang defaults to auto; get model fails if no model has been saved.
list prints one key=value per line and omits an unset model; it is not JSON.
get prints only the value; path prints the active configuration file path.
Reading absent configuration does not create a file. Invalid JSON is an error.
Config directory: AUTOSRT_CONFIG_DIR > YSTT_CONFIG_DIR > OS default.
Exit 0 on success; 1 on config/I/O failure; 2 on invalid command arguments."#;

pub const SET: &str = r#"EXAMPLES:
  autosrt config set lang ko
  autosrt config set lang auto
  autosrt config set model '/absolute/path with spaces/ggml-small.bin'

lang: Whisper language codes such as ko, en, ja, zh, or auto. Names like
'Korean' and locale tags like 'ko-KR' are not accepted. This does not translate.
model: existing file path; stored as an absolute path. Quoted ~/ is supported.
The command prints key=value after an atomic save. Unrelated keys are preserved.
It does not download a model or validate the entire model; use doctor next."#;

pub const MIGRATE: &str = r#"Copies the standard legacy ystt config and its referenced internal model to
the standard autosrt directory, then updates the copied model's absolute path.
External model paths are preserved. Original files are never removed.
An existing autosrt config is left unchanged (success, no overwrite).
Conflicting model contents or an unreadable legacy model cause failure.
Unset AUTOSRT_CONFIG_DIR and YSTT_CONFIG_DIR before running this command.
Run autosrt config path and autosrt doctor after migration."#;

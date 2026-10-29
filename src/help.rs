//! CLI contracts kept next to the driving adapter, independent of transcription logic.

pub const OVERVIEW: &str = "YouTube: CLI --subtitle-mode > saved mode > auto-first.
Local files: transcription only (auto-only rejects them).
Inspect saved defaults with autosrt config list; CLI overrides are not saved.";

pub const ROOT: &str = r#"QUICK START:
  With no saved/explicit mode, YouTube prefers automatic captions. The examples
  below specify a mode explicitly so saved defaults cannot change their policy.

  # Download automatic captions only; no Whisper/model required
  autosrt --no-tui 'https://youtu.be/VIDEO_ID' --language ko --subtitle-mode auto-only --output video.srt
  Only automatic captions are considered, not uploader-provided manual captions.
  Downloaded text/timing, repeated cues and overlaps are preserved, not cleaned up.

  # Prefer automatic captions; transcribe if absent or retrieval fails
  autosrt --no-tui 'https://youtu.be/VIDEO_ID' --language ko --subtitle-mode auto-first

  # Compare sources: result.auto.srt and result.transcribed.srt (at most two)
  autosrt --no-tui 'https://youtu.be/VIDEO_ID' --language ko --subtitle-mode both --output result.srt

  # Local speech transcription: configure a model first
  autosrt config set model /absolute/path/to/ggml-small.bin
  autosrt --no-tui ./audio.mp3 --language ko --output ./audio.srt

  VIDEO_ID is a placeholder. Quote URLs and paths containing spaces.
  Options may precede or follow the input. Exactly one URL/file is supported.

MODE REFERENCE (YouTube):
  auto-only
    Order: automatic-caption lookup and download only; no media download.
    Output: one <video-id>.srt (or the exact --output path).
    Failure: captions absent, retrieval/conversion failure, or invalid SRT.

  auto-first
    Order: automatic captions; on absence/failure warn and try transcription.
    Output: one <video-id>.srt from the first successful source.
    Failure: captions unavailable/failed AND transcription fails.

  transcribe-first
    Order: transcription; on a transcription-path failure warn and try captions.
    Output: one <video-id>.srt from the first successful source.
    Failure: transcription fails AND captions are unavailable/failed.
    Example: autosrt URL --subtitle-mode transcribe-first

  transcribe-only
    Order: transcription only; no automatic-caption lookup.
    Output: one <video-id>.srt.
    Failure: transcription-path failure; no source fallback.
    Example: autosrt URL --subtitle-mode transcribe-only

  both
    Order: automatic captions, then transcription. At most two outputs.
    Output: <video-id>.auto.srt and <video-id>.transcribed.srt.
    With --output /tmp/result.srt: /tmp/result.auto.srt and
    /tmp/result.transcribed.srt; /tmp/result.srt itself is not created.
    Captions absent: warn, produce only .transcribed.srt, succeed (exit 0).
    Caption retrieval/conversion or transcription failure: exit 1; neither new
    result is published, even if one source had succeeded. Existing files remain.
    Both results are prepared before writing, but publication is per file, not
    a two-file transaction. A late write error/cancellation can leave one file.
    Identify source by .auto.srt/.transcribed.srt, not by stdout line count:
      /tmp/result.transcribed.srt alone means only transcription was available.

FALLBACK BOUNDARIES:
  transcribe-first DOES try captions after missing ffmpeg/ffprobe/whisper-cli,
  unset/missing/unreadable model, media-download error, audio-extraction error,
  Whisper error or invalid transcription result. Captions still require yt-dlp
  and may require ffmpeg for conversion, so the fallback can also fail.
  CLI parsing errors, input validation errors and configuration-loading errors
  occur before source selection and do NOT trigger fallback.
  Cancellation and output conflicts/write errors never trigger source fallback.
  Each source is tried at most once; GPU initialization failure permits one CPU
  retry within transcription. yt-dlp may perform its own internal retries.

SETTINGS AND PRECEDENCE:
  autosrt config set subtitle-mode auto-first
  autosrt config get subtitle-mode
  autosrt config set lang ko
  autosrt config get lang
  autosrt config list
  autosrt config path

  Change the mode value to both, transcribe-first, auto-only or transcribe-only
  as needed. A CLI override applies only to that invocation and is not saved.
  Mode: --subtitle-mode > saved subtitle-mode > auto-first.
  Language: --language / --lang / -l > saved lang > auto.
  Model: --model > AUTOSRT_MODEL > legacy YSTT_MODEL > saved model.
  Config directory: AUTOSRT_CONFIG_DIR > legacy YSTT_CONFIG_DIR > OS default.
  macOS: ~/Library/Application Support/autosrt/config.json
  Linux: $XDG_CONFIG_HOME/autosrt/config.json (default ~/.config/autosrt/).
  Config JSON stores subtitle_mode; CLI uses subtitle-mode. If the new default
  config is absent, legacy ystt config is read when present. config migrate copies
  legacy settings/model. Reading absent config does not create it.

OUTPUT AND AUTOMATION:
  --no-tui does not open an interactive screen or prompt for input.
  Redirected/non-terminal execution also disables the TUI automatically.
  On success stdout contains generated SRT paths, one per line; in both mode,
  automatic first when present, then transcription. No JSON output is provided.
  Progress/warnings/errors go to stderr. Check exit status, not progress text.
  SRT files are UTF-8. Default directory is the current working directory;
  local filenames use <input-stem>.srt, including when the saved mode is both.
  --output names a file, not a directory or stdout destination. Its parent must
  exist. Use absolute paths in scripts. Existing output requires --force;
  original local media is never overwritten. --keep-work preserves temporary
  files on success/failure/cancellation and reports their directory on stderr.
  After a late write error, inspect target files before retrying (see both above).

DEPENDENCIES AND DIAGNOSTICS:
  Automatic captions: yt-dlp; ffmpeg only when subtitle conversion is needed.
  Transcription: ffmpeg, ffprobe, whisper-cli on PATH and a local GGML model.
  YouTube extraction may also require an enabled JS runtime and yt-dlp EJS.
  Tools and models are not installed/downloaded automatically.
  autosrt doctor          Checks the FULL transcription environment, including
                          model/Whisper even if you only intend auto-only.
  autosrt doctor --local  Skips YouTube dependency checks, not speech-engine checks.
  Therefore doctor can fail while auto-only still works. There is no dedicated
  automatic-caption-only doctor option. doctor is offline and does not test
  GPU compatibility, actual downloads or transcription quality.

INPUT LIMITS:
  One YouTube video URL or existing local media file with an audio stream.
  No stdin input, playlists, live streams, batches or streaming transcription.
  Whisper does not translate. See --language above for track selection and
  mixed-language limits. Use ./doctor or ./config for reserved filenames.
  For a dash-prefixed filename: autosrt --no-tui -- ./-recording.mp3

EXIT STATUS AND RECOVERY:
  0    Requested generation policy completed (warnings may be present), or
       help/version/config succeeded, or doctor reported no FAIL entries.
  1    Runtime/config/dependency/processing/output failure, or doctor FAIL.
  2    CLI parsing error: missing argument, extra input or invalid option/value.
       config set validates setting values during execution (exit 1); e.g.
       config set lang ko-KR exits 1, while --language ko-KR exits 2.
  130  Cancelled by SIGINT / Ctrl+C; no fallback.
  Existing output: choose another path or deliberately use --force.
  Missing model: autosrt config set model /absolute/path/to/ggml-small.bin
  YouTube HTTP 403: update yt-dlp using its package manager; access restrictions
  may remain, so do not retry indefinitely.
  Damaged UTF-8 in Whisper JSON is replaced with U+FFFD with a warning. Review
  affected captions against the audio; replacement does not restore lost text.

MORE HELP:
  autosrt config --help
  autosrt config set --help
  autosrt doctor --help"#;

pub const DOCTOR: &str = r#"EXAMPLES:
  autosrt doctor
  autosrt doctor --local
  autosrt doctor --local --model /absolute/path/to/ggml-small.bin

Checks executable paths, version-command execution, yt-dlp release age, enabled
YouTube JS/EJS diagnostics, configuration and the selected model's GGML header.
Each external probe has a 5-second timeout. Checks continue after a failed item.
--local skips yt-dlp and YouTube JS/EJS checks.
This command still checks the full speech engine/model setup in auto-only mode.
It can fail even when automatic-caption download works. There is no
automatic-caption-only diagnostic option.

OUTPUT: human-readable OK/WARN/FAIL report and summary on stdout; errors on stderr.
EXIT: 0 when no FAIL entries exist (WARN is allowed), otherwise 1.
No network request, full model loading, transcription or GPU compatibility test
is performed. No installation, model download or configuration write occurs.
Model precedence: --model > AUTOSRT_MODEL > YSTT_MODEL > saved config model.
Options belong after the subcommand: autosrt doctor --model PATH."#;

pub const CONFIG: &str = r#"EXAMPLES:
  autosrt config set lang ko
  autosrt config set subtitle-mode auto-first
  autosrt config set model /absolute/path/to/ggml-small.bin
  autosrt config get lang
  autosrt config get model
  autosrt config get subtitle-mode
  autosrt config list
  autosrt config path
  autosrt config migrate

Keys: lang (automatic-caption track / source speech language),
model (existing local GGML model path),
subtitle-mode (auto-first, transcribe-first, both, transcribe-only, auto-only).
No speech engine or model loading is needed for configuration commands.
set writes persistent defaults; a model must be an existing regular file and is
stored as an absolute path. lang is normalized to lowercase.
get/list return stored defaults, not invocation/environment overrides.
Missing subtitle-mode defaults to auto-first.
Missing lang defaults to auto; get model fails if no model has been saved.
list prints one key=value per line and omits an unset model; it is not JSON.
get prints only the value; path prints the active configuration file path.
Reading absent configuration does not create a file. Invalid JSON is an error.
Config directory: AUTOSRT_CONFIG_DIR > YSTT_CONFIG_DIR > OS default.
Exit 0 on success. Exit 1 on invalid setting values (e.g. set lang ko-KR or
set subtitle-mode invalid), model-path validation, or config/I/O failure.
Exit 2 on CLI parsing errors: unknown keys/options or missing arguments."#;

pub const SET: &str = r#"EXAMPLES:
  autosrt config set lang ko
  autosrt config set subtitle-mode auto-first
  autosrt config set lang auto
  autosrt config set model '/absolute/path with spaces/ggml-small.bin'

subtitle-mode: auto-first, transcribe-first, both, transcribe-only, auto-only.
model: existing file path; stored as an absolute path. Quoted ~/ is supported.
Exit 1 for an unsupported setting value or invalid model path; exit 2 for
unknown keys/options or missing arguments. For example, config set lang ko-KR
exits 1, while the top-level --language ko-KR is a CLI parsing error (exit 2).
The command prints key=value after an atomic save. Unrelated keys are preserved.
It does not download a model or validate the entire model; use doctor next."#;

pub const MIGRATE: &str = r#"Copies the standard legacy ystt config and its referenced internal model to
the standard autosrt directory, then updates the copied model's absolute path.
External model paths are preserved. Original files are never removed.
An existing autosrt config is left unchanged (success, no overwrite).
Conflicting model contents or an unreadable legacy model cause failure.
Unset AUTOSRT_CONFIG_DIR and YSTT_CONFIG_DIR before running this command.
Run autosrt config path and autosrt doctor after migration."#;

const LANGUAGE: &str =
    "Automatic-caption track / source speech language; e.g. ko, en, ja, zh, auto.

CLI --language and saved lang accept the same fixed language-code list below,
including auto. Values are trimmed and lowercased. Names (Korean), locale tags
(ko-KR), arbitrary YouTube track IDs (ko-orig) and regex patterns are not accepted.

Automatic captions: prefer <code>-orig, then <code>. An explicitly requested
code may select a YouTube-provided translated automatic track. auto uses the
metadata original language or a marked -orig track; if neither matches, the
source is unavailable. It does not choose an arbitrary translated language.

Transcription: the code is the source speech language, not a translation target.
auto detects the initial speech language; it does not switch languages within
a mixed-language file. A wrong code can cause repeated or invented captions.

both uses ONE language value for both sources. It cannot request Korean
translated captions plus English speech transcription in one invocation. Use
two commands with separate outputs, for example:
  autosrt URL --subtitle-mode auto-only --language ko --output translated-ko.srt
  autosrt URL --subtitle-mode transcribe-only --language en --output spoken-en.srt";

pub fn language_help() -> String {
    format!(
        "{LANGUAGE}\n\nAccepted codes: {}",
        autosrt::adapters::config::LANGUAGE_CODES
    )
}
pub fn config_help() -> String {
    format!("{CONFIG}\n\nLANGUAGE / lang:\n{}", language_help())
}
pub fn set_help() -> String {
    format!("{SET}\n\nLANGUAGE / lang:\n{}", language_help())
}

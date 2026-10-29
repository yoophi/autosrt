use crate::{
    application::{
        AudioExtractor, CaptionDocument, CaptionSource, Context, Downloader, Error, Event, Result,
        Transcriber,
    },
    domain::{Input, Segment},
};
use serde::Deserialize;
use std::{
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub fn parse_input(raw: &str) -> Result<Input> {
    if raw.starts_with("https://") || raw.starts_with("http://") {
        let u = url::Url::parse(raw).map_err(|e| Error::Message(e.to_string()))?;
        if !matches!(
            u.host_str(),
            Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be")
        ) || !u.username().is_empty()
            || u.password().is_some()
        {
            return Err(Error::Message("Only YouTube URLs are supported".into()));
        }
        if u.path() == "/playlist" || u.path().starts_with("/live/") {
            return Err(Error::Message(
                "Playlists and live URLs are not supported".into(),
            ));
        }
        let valid_video = if u.host_str() == Some("youtu.be") {
            u.path().trim_matches('/').split('/').count() == 1 && u.path() != "/"
        } else {
            (u.path() == "/watch" && u.query_pairs().any(|(k, v)| k == "v" && !v.is_empty()))
                || ["/shorts/", "/embed/"].iter().any(|prefix| {
                    u.path()
                        .strip_prefix(prefix)
                        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
                })
        };
        if !valid_video {
            return Err(Error::Message("Provide a single YouTube video URL".into()));
        }
        Ok(Input::Youtube(raw.into()))
    } else {
        let path = std::fs::canonicalize(raw)
            .map_err(|e| Error::Message(format!("Cannot read input {raw}: {e}")))?;
        if !path.is_file() {
            return Err(Error::Message("Input must be a file".into()));
        }
        Ok(Input::Local(path))
    }
}

pub fn find_tool(name: &str) -> Result<PathBuf> {
    let found = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .map(|p| p.join(name))
        .find(|p| {
            if !p.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                p.metadata()
                    .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                true
            }
        });
    found.ok_or_else(|| {
        Error::Message(format!(
            "Missing tool: {name}; install it and add it to PATH (see README.md)"
        ))
    })
}

// Both output streams are continuously drained; UI progress never blocks a child.
fn run(
    program: &Path,
    args: &[OsString],
    ctx: &Context,
    mut on_line: impl FnMut(&str),
) -> Result<()> {
    ctx.check()?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let (tx, rx) = mpsc::sync_channel(128);
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let drain = |stream: Box<dyn std::io::Read + Send>, tx: mpsc::SyncSender<String>| {
        thread::spawn(move || {
            // Engine logs can contain incomplete UTF-8 tokens. Decode only after
            // reading bytes so a malformed caption cannot close the pipe and
            // terminate a still-running child with SIGPIPE.
            let mut reader = BufReader::new(stream);
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                match reader.read_until(b'\n', &mut bytes) {
                    Ok(0) => break,
                    Ok(_) => {
                        if bytes.last() == Some(&b'\n') {
                            bytes.pop();
                            if bytes.last() == Some(&b'\r') {
                                bytes.pop();
                            }
                        }
                        if tx
                            .send(String::from_utf8_lossy(&bytes).into_owned())
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
    };
    let a = drain(Box::new(stdout), tx.clone());
    let b = drain(Box::new(stderr), tx.clone());
    drop(tx);
    let mut cancelled = None;
    let mut tail = std::collections::VecDeque::new();
    let status = loop {
        if ctx.check().is_err() && cancelled.is_none() {
            cancelled = Some(Instant::now());
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGTERM);
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
            }
        }
        if cancelled.is_some_and(|t: Instant| t.elapsed() > Duration::from_secs(2)) {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
        }
        match rx.recv_timeout(Duration::from_millis(30)) {
            Ok(line) => {
                on_line(&line);
                if tail.len() == 6 {
                    tail.pop_front();
                }
                tail.push_back(line.chars().take(500).collect::<String>());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(s) = child.try_wait()? {
                    break s;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let _ = a.join();
    let _ = b.join();
    if cancelled.is_some() {
        return Err(Error::Cancelled);
    }
    if !status.success() {
        return Err(Error::Message(format!(
            "{} failed ({status}): {}",
            program.display(),
            tail.into_iter().collect::<Vec<_>>().join(" | ")
        )));
    }
    Ok(())
}
fn args(values: &[&OsStr]) -> Vec<OsString> {
    values.iter().map(|s| s.to_os_string()).collect()
}
fn percent(line: &str) -> Option<f64> {
    let end = line.find('%')?;
    let s = line[..end]
        .rsplit(|c: char| !c.is_ascii_digit() && c != '.')
        .next()?;
    s.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .map(|n| n.clamp(0.0, 100.0))
}

pub struct YtDlp;
impl Downloader for YtDlp {
    fn download(&self, url: &str, work: &Path, ctx: &Context) -> Result<PathBuf> {
        let tool = find_tool("yt-dlp")?;
        // Inspect metadata before starting an unbounded live download.
        let mut metadata = String::new();
        run(
            &tool,
            &args(&[
                "--ignore-config".as_ref(),
                "--no-playlist".as_ref(),
                "--dump-single-json".as_ref(),
                "--".as_ref(),
                url.as_ref(),
            ]),
            ctx,
            |line| {
                if line.starts_with('{') {
                    metadata.push_str(line);
                }
            },
        )?;
        let info: serde_json::Value = serde_json::from_str(&metadata)
            .map_err(|e| Error::Message(format!("Invalid yt-dlp metadata: {e}")))?;
        if info["is_live"].as_bool() == Some(true)
            || info["live_status"].as_str() == Some("is_upcoming")
        {
            return Err(Error::Message(
                "Live or upcoming video is not supported".into(),
            ));
        }
        let template = work.join("%(id)s.%(ext)s");
        let mut file = None;
        run(
            &tool,
            &args(&[
                "--ignore-config".as_ref(),
                "--no-playlist".as_ref(),
                "--newline".as_ref(),
                "--no-simulate".as_ref(),
                "--progress".as_ref(),
                "--progress-template".as_ref(),
                "download:AUTOSRT_PROGRESS %(progress._percent_str)s".as_ref(),
                "-f".as_ref(),
                "bv*+ba/b".as_ref(),
                "--merge-output-format".as_ref(),
                "mkv".as_ref(),
                "-o".as_ref(),
                template.as_os_str(),
                "--print".as_ref(),
                "after_move:AUTOSRT_FILE %(filepath)s".as_ref(),
                "--".as_ref(),
                url.as_ref(),
            ]),
            ctx,
            |line| {
                if let Some(path) = line.strip_prefix("AUTOSRT_FILE ") {
                    file = Some(PathBuf::from(path));
                } else if line.starts_with("AUTOSRT_PROGRESS ") {
                    if let Some(p) = percent(line) {
                        ctx.events.emit(Event::Progress(p));
                    }
                } else {
                    ctx.events
                        .emit(Event::Log(line.chars().take(300).collect()));
                }
            },
        )?;
        let path =
            file.ok_or_else(|| Error::Message("yt-dlp returned no completed file".into()))?;
        let path = std::fs::canonicalize(path)?;
        if !path.starts_with(std::fs::canonicalize(work)?) || !path.is_file() {
            return Err(Error::Message("Invalid downloaded path".into()));
        }
        Ok(path)
    }
}

pub struct Ffmpeg;
impl AudioExtractor for Ffmpeg {
    fn extract(&self, input: &Path, work: &Path, ctx: &Context) -> Result<PathBuf> {
        let probe = find_tool("ffprobe")?;
        let mut json = String::new();
        run(
            &probe,
            &args(&[
                "-v".as_ref(),
                "error".as_ref(),
                "-show_entries".as_ref(),
                "format=duration,start_time:stream=codec_type,start_time".as_ref(),
                "-of".as_ref(),
                "json".as_ref(),
                input.as_os_str(),
            ]),
            ctx,
            |line| {
                json.push_str(line);
            },
        )?;
        let v: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| Error::Message(format!("Invalid ffprobe result: {e}")))?;
        let audio = v["streams"]
            .as_array()
            .and_then(|s| s.iter().find(|s| s["codec_type"] == "audio"))
            .ok_or_else(|| Error::Message("Media has no audio stream".into()))?;
        let parse_time = |v: &serde_json::Value| v.as_str().and_then(|s| s.parse::<f64>().ok());
        let origin = parse_time(&v["format"]["start_time"]).unwrap_or(0.0);
        let start = parse_time(&audio["start_time"]).unwrap_or(origin);
        if (start - origin).abs() > 0.05 {
            return Err(Error::Message("Audio starts at a different media timestamp; offset alignment is not yet supported".into()));
        }
        let duration = parse_time(&v["format"]["duration"]).filter(|n| n.is_finite() && *n > 0.0);
        let output = work.join("audio.wav");
        run(
            &find_tool("ffmpeg")?,
            &args(&[
                "-nostdin".as_ref(),
                "-hide_banner".as_ref(),
                "-y".as_ref(),
                "-i".as_ref(),
                input.as_os_str(),
                "-map".as_ref(),
                "0:a:0".as_ref(),
                "-vn".as_ref(),
                "-ac".as_ref(),
                "1".as_ref(),
                "-ar".as_ref(),
                "16000".as_ref(),
                "-c:a".as_ref(),
                "pcm_s16le".as_ref(),
                "-progress".as_ref(),
                "pipe:1".as_ref(),
                "-nostats".as_ref(),
                output.as_os_str(),
            ]),
            ctx,
            |line| {
                if let Some(t) = line
                    .strip_prefix("out_time_us=")
                    .and_then(|s| s.parse::<f64>().ok())
                    && let Some(d) = duration
                {
                    ctx.events.emit(Event::Progress(
                        (t / 1_000_000.0 / d * 100.0).clamp(0.0, 100.0),
                    ));
                }
            },
        )?;
        Ok(output)
    }
}

pub struct Whisper {
    pub model: Option<PathBuf>,
    pub language: String,
}
#[derive(Deserialize)]
struct WhisperResult {
    transcription: Vec<WhisperSegment>,
}
#[derive(Deserialize)]
struct WhisperSegment {
    offsets: Offsets,
    text: String,
}
#[derive(Deserialize)]
struct Offsets {
    from: u64,
    to: u64,
}
pub fn parse_transcript(json: &str) -> Result<Vec<Segment>> {
    let result: WhisperResult = serde_json::from_str(json)
        .map_err(|e| Error::Message(format!("Invalid whisper JSON: {e}")))?;
    Ok(result
        .transcription
        .into_iter()
        .map(|s| Segment {
            start_ms: s.offsets.from,
            end_ms: s.offsets.to,
            text: s.text,
        })
        .collect())
}
impl Transcriber for Whisper {
    fn check(&self, youtube: bool) -> Result<()> {
        let mut required = vec!["ffmpeg", "ffprobe", "whisper-cli"];
        if youtube {
            required.insert(0, "yt-dlp");
        }
        let missing: Vec<_> = required
            .into_iter()
            .filter(|t| find_tool(t).is_err())
            .collect();
        if !missing.is_empty() {
            return Err(Error::Message(format!(
                "Missing tool: {}. Install the required commands and add them to PATH (see README.md).",
                missing.join(", ")
            )));
        }
        let model = self.model.as_ref().ok_or_else(|| Error::Message("No speech model configured. Run: autosrt config set model /path/to/ggml-small.bin (or use --model / AUTOSRT_MODEL). See README.md.".into()))?;
        if !std::fs::canonicalize(model)?.is_file() {
            return Err(Error::Message("Model must be a file".into()));
        }
        Ok(())
    }
    fn transcribe(&self, audio: &Path, work: &Path, ctx: &Context) -> Result<Vec<Segment>> {
        let prefix = work.join("transcript");
        let tool = find_tool("whisper-cli")?;
        let mut arguments = args(&[
            "-m".as_ref(),
            self.model
                .as_ref()
                .ok_or_else(|| Error::Message("No speech model configured".into()))?
                .as_os_str(),
            "-f".as_ref(),
            audio.as_os_str(),
            "-l".as_ref(),
            self.language.as_ref(),
            "-oj".as_ref(),
            "-of".as_ref(),
            prefix.as_os_str(),
            "-pp".as_ref(),
        ]);
        for attempt in 0..2 {
            let mut gpu_failed = false;
            let result = run(&tool, &arguments, ctx, |line| {
                gpu_failed |= line.contains("whisper_backend_init_gpu: failed");
                if line.starts_with("whisper_print_progress_callback:") {
                    if let Some(p) = percent(line) {
                        ctx.events.emit(Event::Progress(p));
                    }
                } else {
                    ctx.events.emit(Event::Log(line.to_owned()));
                }
            });
            match result {
                Ok(()) => break,
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(_) if attempt == 0 && gpu_failed => {
                    ctx.check()?;
                    ctx.stage("GPU initialization failed; retrying on CPU")?;
                    arguments.push("-ng".into());
                }
                Err(e) => return Err(e),
            }
        }
        let bytes = std::fs::read(prefix.with_extension("json"))?;
        let json = String::from_utf8_lossy(&bytes);
        if matches!(json, std::borrow::Cow::Owned(_)) {
            ctx.events.emit(Event::Warning(
                "Whisper transcript contains invalid UTF-8; damaged bytes were replaced with � (U+FFFD). Review those captions.".into(),
            ));
        }
        parse_transcript(&json)
    }
}

impl CaptionSource for YtDlp {
    fn fetch(
        &self,
        url: &str,
        language: &str,
        work: &Path,
        ctx: &Context,
    ) -> Result<Option<CaptionDocument>> {
        let tool = find_tool("yt-dlp")?;
        let mut metadata = String::new();
        run(
            &tool,
            &args(&[
                "--ignore-config".as_ref(),
                "--no-playlist".as_ref(),
                "--skip-download".as_ref(),
                "--dump-single-json".as_ref(),
                "--".as_ref(),
                url.as_ref(),
            ]),
            ctx,
            |line| {
                if line.starts_with('{') {
                    metadata.push_str(line);
                }
            },
        )?;
        let info: serde_json::Value = serde_json::from_str(&metadata)
            .map_err(|e| Error::Message(format!("Invalid yt-dlp metadata: {e}")))?;
        if info["is_live"].as_bool() == Some(true)
            || matches!(
                info["live_status"].as_str(),
                Some("is_live" | "is_upcoming")
            )
        {
            return Err(Error::Message(
                "Live or upcoming video is not supported".into(),
            ));
        }
        let Some(tracks) = info["automatic_captions"].as_object() else {
            return Ok(None);
        };
        let available = |key: &str| {
            tracks
                .get(key)
                .and_then(|v| v.as_array())
                .is_some_and(|v| !v.is_empty())
        };
        let selected = if language == "auto" {
            let original = info["language"].as_str().unwrap_or("");
            [format!("{original}-orig"), original.to_owned()]
                .into_iter()
                .find(|k| available(k))
                .or_else(|| {
                    tracks
                        .keys()
                        .find(|k| k.ends_with("-orig") && available(k))
                        .cloned()
                })
        } else {
            [format!("{language}-orig"), language.to_owned()]
                .into_iter()
                .find(|k| available(k))
        };
        let Some(selected) = selected else {
            return Ok(None);
        };
        if !selected
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(Error::Message(
                "Invalid automatic subtitle language identifier".into(),
            ));
        }
        let id = info["id"]
            .as_str()
            .filter(|id| {
                !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            })
            .ok_or_else(|| Error::Message("Invalid YouTube video ID in metadata".into()))?;
        let has_srt = tracks[&selected]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["ext"] == "srt");
        if !has_srt {
            find_tool("ffmpeg")?;
        }
        let captions_dir = work.join("automatic");
        std::fs::create_dir_all(&captions_dir)?;
        let template = captions_dir.join("caption.%(ext)s");
        let pattern = format!("^{selected}$");
        ctx.stage("Downloading automatic subtitles")?;
        run(
            &tool,
            &args(&[
                "--ignore-config".as_ref(),
                "--no-playlist".as_ref(),
                "--skip-download".as_ref(),
                "--no-simulate".as_ref(),
                "--no-write-subs".as_ref(),
                "--write-auto-subs".as_ref(),
                "--sub-langs".as_ref(),
                pattern.as_ref(),
                "--sub-format".as_ref(),
                "srt/vtt/best".as_ref(),
                "--convert-subs".as_ref(),
                "srt".as_ref(),
                "-o".as_ref(),
                template.as_os_str(),
                "--".as_ref(),
                url.as_ref(),
            ]),
            ctx,
            |line| {
                ctx.events.emit(Event::Log(line.to_owned()));
            },
        )?;
        let path = captions_dir.join(format!("caption.{selected}.srt"));
        let canonical = std::fs::canonicalize(&path)?;
        if !canonical.starts_with(std::fs::canonicalize(&captions_dir)?) || !canonical.is_file() {
            return Err(Error::Message("Invalid downloaded subtitle path".into()));
        }
        let srt = std::fs::read_to_string(canonical)?;
        validate_downloaded_srt(&srt)?;
        Ok(Some(CaptionDocument {
            stem: id.into(),
            srt,
        }))
    }
}

fn validate_downloaded_srt(srt: &str) -> Result<()> {
    let normalized = srt.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut count = 0;
    for block in normalized
        .trim()
        .split("\n\n")
        .filter(|b| !b.trim().is_empty())
    {
        let mut lines = block.lines();
        let index = lines.next().unwrap_or("").trim().parse::<u64>();
        let timing = lines.next().unwrap_or("");
        let stamps: Vec<_> = timing.split(" --> ").collect();
        let parse = |s: &str| -> Option<u64> {
            let values: Vec<u64> = s
                .trim()
                .split([':', ','])
                .map(str::parse)
                .collect::<std::result::Result<_, _>>()
                .ok()?;
            if values.len() != 4 || values[1] >= 60 || values[2] >= 60 || values[3] >= 1000 {
                return None;
            }
            values[0]
                .checked_mul(3_600_000)?
                .checked_add(values[1] * 60_000 + values[2] * 1000 + values[3])
        };
        let valid_time = stamps.len() == 2
            && parse(stamps[0])
                .zip(parse(stamps[1]))
                .is_some_and(|(a, b)| a < b);
        if index.is_err() || !valid_time || !lines.any(|l| !l.trim().is_empty()) {
            return Err(Error::Message(
                "Downloaded automatic subtitles are not valid SRT".into(),
            ));
        }
        count += 1;
    }
    if count == 0 {
        return Err(Error::Message(
            "Downloaded automatic subtitles are empty".into(),
        ));
    }
    Ok(())
}

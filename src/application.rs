use crate::domain::{Input, Segment, SubtitleMode, render_srt};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error("Cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub enum Event {
    Stage(&'static str),
    Progress(f64),
    Log(String),
    Warning(String),
}
pub trait Progress: Send + Sync {
    fn emit(&self, event: Event);
}
#[derive(Clone)]
pub struct Context {
    pub cancel: Arc<AtomicBool>,
    pub events: Arc<dyn Progress>,
}
impl Context {
    pub fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
    pub fn stage(&self, stage: &'static str) -> Result<()> {
        self.check()?;
        self.events.emit(Event::Stage(stage));
        Ok(())
    }
}

pub trait Downloader: Send + Sync {
    fn download(&self, url: &str, work: &Path, ctx: &Context) -> Result<PathBuf>;
}
pub trait AudioExtractor: Send + Sync {
    fn extract(&self, input: &Path, work: &Path, ctx: &Context) -> Result<PathBuf>;
}
pub trait Transcriber: Send + Sync {
    fn check(&self, youtube: bool) -> Result<()>;
    fn transcribe(&self, audio: &Path, work: &Path, ctx: &Context) -> Result<Vec<Segment>>;
}
pub trait SubtitleStore: Send + Sync {
    fn check_target(&self, output: &Path, media: &Path, force: bool) -> Result<()>;
    fn save(&self, path: &Path, content: &str, force: bool) -> Result<()>;
}

pub struct CaptionDocument {
    pub stem: String,
    pub srt: String,
}
pub trait CaptionSource: Send + Sync {
    fn fetch(
        &self,
        url: &str,
        language: &str,
        work: &Path,
        ctx: &Context,
    ) -> Result<Option<CaptionDocument>>;
}
pub struct Pipeline {
    pub downloader: Box<dyn Downloader>,
    pub captions: Box<dyn CaptionSource>,
    pub extractor: Box<dyn AudioExtractor>,
    pub transcriber: Box<dyn Transcriber>,
    pub store: Box<dyn SubtitleStore>,
}
pub struct Request {
    pub input: Input,
    pub output: Option<PathBuf>,
    pub force: bool,
    pub mode: SubtitleMode,
    pub language: String,
}
impl Pipeline {
    fn transcribe(&self, req: &Request, work: &Path, ctx: &Context) -> Result<CaptionDocument> {
        self.transcriber
            .check(matches!(req.input, Input::Youtube(_)))?;
        let media = match &req.input {
            Input::Youtube(url) => {
                ctx.stage("Downloading")?;
                self.downloader.download(url, work, ctx)?
            }
            Input::Local(path) => path.clone(),
        };
        ctx.stage("Extracting audio")?;
        let audio = self.extractor.extract(&media, work, ctx)?;
        ctx.stage("Transcribing")?;
        let transcript = self.transcriber.transcribe(&audio, work, ctx)?;
        let srt = render_srt(&transcript).map_err(Error::Message)?;
        Ok(CaptionDocument {
            stem: media
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            srt,
        })
    }
    fn auto(&self, req: &Request, work: &Path, ctx: &Context) -> Result<Option<CaptionDocument>> {
        if let Input::Youtube(url) = &req.input {
            ctx.stage("Checking YouTube automatic subtitles")?;
            self.captions.fetch(url, &req.language, work, ctx)
        } else {
            Ok(None)
        }
    }
    pub fn run(&self, req: Request, work: &Path, ctx: &Context) -> Result<Vec<PathBuf>> {
        ctx.stage("Preparing")?;
        let local = matches!(req.input, Input::Local(_));
        if local && req.mode == SubtitleMode::AutoOnly {
            return Err(Error::Message("auto-only requires a YouTube URL".into()));
        }
        let mode = if local {
            SubtitleMode::TranscribeOnly
        } else {
            req.mode
        };
        let media = match &req.input {
            Input::Local(p) => p.as_path(),
            _ => work,
        };
        if let Some(output) = &req.output {
            if mode == SubtitleMode::Both {
                for suffix in ["auto", "transcribed"] {
                    self.store
                        .check_target(&with_source(output, suffix), media, req.force)?;
                }
            } else {
                self.store.check_target(output, media, req.force)?;
            }
        }
        let mut docs = Vec::new();
        match mode {
            SubtitleMode::TranscribeOnly => {
                docs.push(("transcribed", self.transcribe(&req, work, ctx)?))
            }
            SubtitleMode::AutoOnly => {
                let doc = self.auto(&req, work, ctx)?.ok_or_else(|| {
                    Error::Message("No automatic subtitles for the selected language".into())
                })?;
                docs.push(("auto", doc));
            }
            SubtitleMode::AutoFirst => match self.auto(&req, work, ctx) {
                Ok(Some(doc)) => docs.push(("auto", doc)),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                other => {
                    let reason = match other {
                        Err(e) => e.to_string(),
                        _ => "No matching automatic subtitles".into(),
                    };
                    ctx.events.emit(Event::Warning(format!(
                        "{reason}; falling back to transcription"
                    )));
                    docs.push(("transcribed", self.transcribe(&req, work, ctx)?));
                }
            },
            SubtitleMode::TranscribeFirst => match self.transcribe(&req, work, ctx) {
                Ok(doc) => docs.push(("transcribed", doc)),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => {
                    ctx.events.emit(Event::Warning(format!(
                        "Transcription failed: {e}; trying automatic subtitles"
                    )));
                    let doc = self.auto(&req, work, ctx)?.ok_or_else(|| {
                        Error::Message(format!(
                            "Transcription failed: {e}; no matching automatic subtitles"
                        ))
                    })?;
                    docs.push(("auto", doc));
                }
            },
            SubtitleMode::Both => {
                match self.auto(&req, work, ctx)? {
                    Some(doc) => docs.push(("auto", doc)),
                    None => ctx.events.emit(Event::Warning(
                        "No matching automatic subtitles; generating transcription only".into(),
                    )),
                }
                docs.push(("transcribed", self.transcribe(&req, work, ctx)?));
            }
        }
        let outputs: Vec<_> = docs
            .iter()
            .map(|(source, doc)| {
                let base = req
                    .output
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(format!("{}.srt", doc.stem)));
                if mode == SubtitleMode::Both {
                    with_source(&base, source)
                } else {
                    base
                }
            })
            .collect();
        for output in &outputs {
            self.store.check_target(output, media, req.force)?;
        }
        ctx.stage("Writing subtitles")?;
        for ((_, doc), output) in docs.iter().zip(&outputs) {
            ctx.check()?;
            self.store.save(output, &doc.srt, req.force)?;
        }
        Ok(outputs)
    }
}
fn with_source(path: &Path, source: &str) -> PathBuf {
    let mut stem = path.file_stem().unwrap_or_default().to_os_string();
    stem.push(format!(".{source}.srt"));
    path.with_file_name(stem)
}

use crate::domain::{Input, Segment, render_srt};
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
    fn transcribe(&self, audio: &Path, work: &Path, ctx: &Context) -> Result<Vec<Segment>>;
}
pub trait SubtitleStore: Send + Sync {
    fn check_target(&self, output: &Path, media: &Path, force: bool) -> Result<()>;
    fn save(&self, path: &Path, content: &str, force: bool) -> Result<()>;
}

pub struct Pipeline {
    pub downloader: Box<dyn Downloader>,
    pub extractor: Box<dyn AudioExtractor>,
    pub transcriber: Box<dyn Transcriber>,
    pub store: Box<dyn SubtitleStore>,
}
pub struct Request {
    pub input: Input,
    pub output: Option<PathBuf>,
    pub force: bool,
}
impl Pipeline {
    pub fn run(&self, req: Request, work: &Path, ctx: &Context) -> Result<PathBuf> {
        ctx.stage("Preparing")?;
        let media = match &req.input {
            Input::Youtube(url) => {
                ctx.stage("Downloading")?;
                self.downloader.download(url, work, ctx)?
            }
            Input::Local(path) => path.clone(),
        };
        let output = req.output.unwrap_or_else(|| {
            let mut name = media.file_stem().unwrap_or_default().to_os_string();
            name.push(".srt");
            PathBuf::from(name)
        });
        self.store.check_target(&output, &media, req.force)?;
        ctx.stage("Extracting audio")?;
        let audio = self.extractor.extract(&media, work, ctx)?;
        ctx.stage("Transcribing")?;
        let transcript = self.transcriber.transcribe(&audio, work, ctx)?;
        ctx.stage("Writing subtitles")?;
        let srt = render_srt(&transcript).map_err(Error::Message)?;
        ctx.check()?;
        self.store.save(&output, &srt, req.force)?;
        Ok(output)
    }
}

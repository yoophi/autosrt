use crate::application::{Error, Result, SubtitleStore};
use std::{io::Write, path::Path};
pub struct FileStore;
impl SubtitleStore for FileStore {
    fn check_target(&self, output: &Path, media: &Path, force: bool) -> Result<()> {
        if output.exists() && !force {
            return Err(Error::Message(format!(
                "Output already exists: {} (use --force)",
                output.display()
            )));
        }
        if output.exists() && std::fs::canonicalize(output)? == std::fs::canonicalize(media)? {
            return Err(Error::Message(
                "Output must not overwrite input media".into(),
            ));
        }
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if !parent.is_dir() {
            return Err(Error::Message(format!(
                "Output directory does not exist: {}",
                parent.display()
            )));
        }
        Ok(())
    }

    fn save(&self, path: &Path, content: &str, force: bool) -> Result<()> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(content.as_bytes())?;
        temp.as_file().sync_all()?;
        if force {
            temp.persist(path).map_err(|e| e.error)?;
        } else {
            temp.persist_noclobber(path).map_err(|e| e.error)?;
        }
        Ok(())
    }
}

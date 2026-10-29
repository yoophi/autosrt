use crate::{
    application::{Error, Result},
    domain::SubtitleMode,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write, path::PathBuf};

pub const LANGUAGE_CODES: &str = "auto en zh de es ru ko fr ja pt tr pl ca nl ar sv it id hi fi vi he uk el ms cs ro da hu ta no th ur hr bg lt la mi ml cy sk te fa lv bn sr az sl kn et mk br eu is hy ne mn bs kk sq sw gl mr pa si km sn yo so af oc ka be tg sd gu am yi lo uz fo ht ps tk nn mt sa lb my bo tl mg as tt haw ln ha ba jw su yue";

// Language codes accepted by whisper.cpp's g_lang table, plus auto-detection.
pub fn normalize_language(raw: &str) -> std::result::Result<String, String> {
    let value = raw.trim().to_ascii_lowercase();

    if LANGUAGE_CODES.split_whitespace().any(|c| c == value) {
        Ok(value)
    } else {
        Err(format!(
            "Unsupported language {raw:?}; use a supported automatic-caption / source-speech language code such as ko, en, ja, or auto"
        ))
    }
}

#[derive(Default, Deserialize, Serialize)]
struct Settings {
    #[serde(default)]
    lang: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subtitle_mode: Option<String>,
    // Preserve unrelated settings when updating lang.
    #[serde(flatten)]
    other: BTreeMap<String, serde_json::Value>,
}

pub struct ConfigFile {
    pub path: PathBuf,
}
impl ConfigFile {
    pub fn discover() -> Result<Self> {
        let explicit =
            std::env::var_os("AUTOSRT_CONFIG_DIR").or_else(|| std::env::var_os("YSTT_CONFIG_DIR"));
        let directory = match explicit {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            Some(_) => {
                return Err(Error::Message(
                    "AUTOSRT_CONFIG_DIR / YSTT_CONFIG_DIR must not be empty".into(),
                ));
            }
            None => {
                let root = config_root()?;
                let current = root.join("autosrt");
                if !current.join("config.json").exists() && root.join("ystt/config.json").exists() {
                    root.join("ystt")
                } else {
                    current
                }
            }
        };
        Ok(Self {
            path: directory.join("config.json"),
        })
    }
    pub fn migrate_legacy() -> Result<bool> {
        if std::env::var_os("AUTOSRT_CONFIG_DIR").is_some()
            || std::env::var_os("YSTT_CONFIG_DIR").is_some()
        {
            return Err(Error::Message("Unset AUTOSRT_CONFIG_DIR and YSTT_CONFIG_DIR before migrating the standard configuration".into()));
        }
        let root = config_root()?;
        migrate(&root.join("ystt"), &root.join("autosrt"))
    }
    fn read(&self) -> Result<Settings> {
        match std::fs::read_to_string(&self.path) {
            Ok(data) => serde_json::from_str(&data).map_err(|e| {
                Error::Message(format!("Invalid config {}: {e}", self.path.display()))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(Error::Message(format!(
                "Cannot read config {}: {e}",
                self.path.display()
            ))),
        }
    }
    pub fn language(&self) -> Result<String> {
        let settings = self.read()?;
        normalize_language(settings.lang.as_deref().unwrap_or("auto"))
            .map_err(|e| Error::Message(format!("Invalid config {}: {e}", self.path.display())))
    }
    pub fn set_language(&self, value: &str) -> Result<()> {
        let value = normalize_language(value).map_err(Error::Message)?;
        let mut settings = self.read()?;
        settings.lang = Some(value);
        self.save(&settings)
    }
    pub fn subtitle_mode(&self) -> Result<SubtitleMode> {
        self.read()?
            .subtitle_mode
            .as_deref()
            .unwrap_or("auto-first")
            .parse()
            .map_err(Error::Message)
    }
    pub fn set_subtitle_mode(&self, value: &str) -> Result<()> {
        let mode: SubtitleMode = value.parse().map_err(Error::Message)?;
        let mut settings = self.read()?;
        settings.subtitle_mode = Some(mode.as_str().into());
        self.save(&settings)
    }
    pub fn model(&self) -> Result<Option<PathBuf>> {
        Ok(self.read()?.model)
    }
    pub fn set_model(&self, value: &str) -> Result<()> {
        let path = if let Some(relative) = value.strip_prefix("~/") {
            dirs::home_dir()
                .ok_or_else(|| Error::Message("Cannot locate home directory".into()))?
                .join(relative)
        } else {
            PathBuf::from(value)
        };
        let path = std::fs::canonicalize(&path)
            .map_err(|e| Error::Message(format!("Cannot read model {}: {e}", path.display())))?;
        if !path.is_file() {
            return Err(Error::Message("Model must be a file".into()));
        }
        let mut settings = self.read()?;
        settings.model = Some(path);
        self.save(&settings)
    }
    fn save(&self, settings: &Settings) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Message("Config path has no parent".into()))?;
        std::fs::create_dir_all(parent)?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        let json =
            serde_json::to_string_pretty(settings).map_err(|e| Error::Message(e.to_string()))?;
        writeln!(tmp, "{json}")?;
        tmp.as_file().sync_all()?;
        tmp.persist(&self.path).map_err(|e| e.error)?;
        Ok(())
    }
}

fn config_root() -> Result<PathBuf> {
    dirs::config_dir()
        .ok_or_else(|| Error::Message("Cannot locate the user configuration directory".into()))
}

pub fn legacy_model_env() -> Option<PathBuf> {
    std::env::var_os("YSTT_MODEL").map(PathBuf::from)
}

fn migrate(legacy: &std::path::Path, current: &std::path::Path) -> Result<bool> {
    let target = current.join("config.json");
    if target.exists() {
        return Ok(false);
    }
    let source = ConfigFile {
        path: legacy.join("config.json"),
    };
    if !source.path.is_file() {
        return Err(Error::Message("No legacy ystt configuration found".into()));
    }
    let mut settings = source.read()?;
    if let Some(lang) = &settings.lang {
        normalize_language(lang).map_err(Error::Message)?;
    }
    if let Some(model) = &settings.model {
        let original = std::fs::canonicalize(model).map_err(|e| {
            Error::Message(format!("Cannot migrate model {}: {e}", model.display()))
        })?;
        if !original.is_file() {
            return Err(Error::Message("Legacy model must be a file".into()));
        }
        let old_root = std::fs::canonicalize(legacy)?;
        if let Ok(relative) = original.strip_prefix(&old_root) {
            let destination = current.join(relative);
            if destination == target {
                return Err(Error::Message(
                    "Model path conflicts with config.json".into(),
                ));
            }
            let parent = destination
                .parent()
                .ok_or_else(|| Error::Message("Model path has no parent".into()))?;
            std::fs::create_dir_all(parent)?;
            if !std::fs::canonicalize(parent)?.starts_with(std::fs::canonicalize(current)?) {
                return Err(Error::Message(
                    "Model destination escapes the configuration directory".into(),
                ));
            }
            if destination.exists() {
                if !same_file_bytes(&original, &destination)? {
                    return Err(Error::Message(format!(
                        "Migration conflict: {} already exists with different contents",
                        destination.display()
                    )));
                }
            } else {
                let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
                std::io::copy(&mut std::fs::File::open(&original)?, &mut tmp)?;
                tmp.as_file().sync_all()?;
                tmp.persist_noclobber(&destination).map_err(|e| e.error)?;
            }
            settings.model = Some(std::fs::canonicalize(destination)?);
        }
    }
    std::fs::create_dir_all(current)?;
    let mut tmp = tempfile::NamedTempFile::new_in(current)?;
    writeln!(
        tmp,
        "{}",
        serde_json::to_string_pretty(&settings).map_err(|e| Error::Message(e.to_string()))?
    )?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(&target).map_err(|e| e.error)?;
    Ok(true)
}
fn same_file_bytes(a: &std::path::Path, b: &std::path::Path) -> Result<bool> {
    use std::io::Read;
    let mut a = std::io::BufReader::new(std::fs::File::open(a)?);
    let mut b = std::io::BufReader::new(std::fs::File::open(b)?);
    if a.get_ref().metadata()?.len() != b.get_ref().metadata()?.len() {
        return Ok(false);
    }
    let mut left = [0; 65536];
    let mut right = [0; 65536];
    loop {
        let n = a.read(&mut left)?;
        if n == 0 {
            return Ok(true);
        }
        b.read_exact(&mut right[..n])?;
        if left[..n] != right[..n] {
            return Ok(false);
        }
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    fn fixture(root: &std::path::Path) -> (PathBuf, PathBuf) {
        let old = root.join("ystt");
        let new = root.join("autosrt");
        std::fs::create_dir_all(old.join("models")).unwrap();
        let model = old.join("models/small.bin");
        std::fs::write(&model, b"lmggmodel").unwrap();
        std::fs::write(
            old.join("config.json"),
            serde_json::json!({"lang":"ko","model":model,"future":42}).to_string(),
        )
        .unwrap();
        (old, new)
    }
    #[test]
    fn migration_rewrites_path_preserves_original_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let (old, new) = fixture(temp.path());
        assert!(migrate(&old, &new).unwrap());
        let settings = ConfigFile {
            path: new.join("config.json"),
        }
        .read()
        .unwrap();
        assert_eq!(
            settings.model.unwrap(),
            std::fs::canonicalize(new.join("models/small.bin")).unwrap()
        );
        assert_eq!(settings.lang.as_deref(), Some("ko"));
        assert_eq!(settings.other["future"], 42);
        assert!(old.join("models/small.bin").exists());
        assert!(!migrate(&old, &new).unwrap());
    }
    #[test]
    fn migration_conflict_and_broken_model_do_not_publish_config() {
        let temp = tempfile::tempdir().unwrap();
        let (old, new) = fixture(temp.path());
        std::fs::create_dir_all(new.join("models")).unwrap();
        std::fs::write(new.join("models/small.bin"), b"different").unwrap();
        assert!(migrate(&old, &new).is_err());
        assert!(!new.join("config.json").exists());
        std::fs::remove_file(old.join("models/small.bin")).unwrap();
        assert!(migrate(&old, &new).is_err());
        assert!(!new.join("config.json").exists());
    }
    #[test]
    fn external_model_path_is_not_moved() {
        let temp = tempfile::tempdir().unwrap();
        let (old, new) = fixture(temp.path());
        let external = temp.path().join("external.bin");
        std::fs::write(&external, b"model").unwrap();
        std::fs::write(
            old.join("config.json"),
            serde_json::json!({"model":external}).to_string(),
        )
        .unwrap();
        migrate(&old, &new).unwrap();
        assert_eq!(
            ConfigFile {
                path: new.join("config.json")
            }
            .model()
            .unwrap(),
            Some(external)
        );
    }
}

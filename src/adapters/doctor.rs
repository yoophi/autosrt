use super::{config::ConfigFile, tools::find_tool};
use crate::application::{Error, Result};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Ok,
    Warn,
    Fail,
}
struct Check {
    status: Status,
    name: &'static str,
    detail: String,
}
#[derive(Default)]
pub struct Report {
    checks: Vec<Check>,
}
impl Report {
    fn add(&mut self, status: Status, name: &'static str, detail: impl Into<String>) {
        self.checks.push(Check {
            status,
            name,
            detail: detail.into(),
        });
    }
    pub fn passed(&self) -> bool {
        !self.checks.iter().any(|c| c.status == Status::Fail)
    }
    pub fn print(&self) {
        println!("autosrt dependency diagnostics (offline)");
        for c in &self.checks {
            let label = match c.status {
                Status::Ok => "OK",
                Status::Warn => "WARN",
                Status::Fail => "FAIL",
            };
            println!("[{label}] {}: {}", c.name, c.detail);
        }
        let failed = self
            .checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .count();
        let warnings = self
            .checks
            .iter()
            .filter(|c| c.status == Status::Warn)
            .count();
        println!("Summary: {failed} failure(s), {warnings} warning(s)");
        println!(
            "Checks do not download media or load the model; network access, transcription quality and GPU compatibility are not verified."
        );
    }
}

// A file capture avoids pipe deadlocks and caps the bytes read into memory.
fn probe(path: &Path, arguments: &[&str], timeout: Duration) -> Result<(ExitStatus, String)> {
    let mut output = tempfile::tempfile()?;
    let mut command = Command::new(path);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Message(format!(
                "timed out after {} seconds",
                timeout.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(20));
    };
    output.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    output.take(64 * 1024).read_to_end(&mut bytes)?;
    Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
}

fn check_tool(
    report: &mut Report,
    name: &'static str,
    flag: &str,
    hint: &str,
) -> Option<(PathBuf, String)> {
    let path = match find_tool(name) {
        Ok(path) => path,
        Err(_) => {
            report.add(Status::Fail, name, format!("not found on PATH. {hint}"));
            return None;
        }
    };
    match probe(&path, &[flag], Duration::from_secs(5)) {
        Ok((status, output)) if status.success() && !output.trim().is_empty() => {
            let version = output
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or_default()
                .trim()
                .to_string();
            report.add(Status::Ok, name, format!("{version} | {}", path.display()));
            Some((path, version))
        }
        Ok((status, _)) => {
            report.add(
                Status::Fail,
                name,
                format!(
                    "{}: version probe failed ({status}) or returned no version. {hint}",
                    path.display()
                ),
            );
            None
        }
        Err(e) => {
            report.add(
                Status::Fail,
                name,
                format!("{}: {e}. {hint}", path.display()),
            );
            None
        }
    }
}

pub fn diagnose(local: bool, model_override: Option<PathBuf>) -> Report {
    let mut report = Report::default();
    check_tool(
        &mut report,
        "ffmpeg",
        "-version",
        "Install FFmpeg and add ffmpeg to PATH.",
    );
    check_tool(
        &mut report,
        "ffprobe",
        "-version",
        "Install FFmpeg and add ffprobe to PATH.",
    );
    check_tool(
        &mut report,
        "whisper-cli",
        "--version",
        "Install/update whisper.cpp and add whisper-cli to PATH.",
    );
    if !local {
        if let Some((path, version)) = check_tool(
            &mut report,
            "yt-dlp",
            "--version",
            "Install/update yt-dlp using its original package manager (for example brew upgrade yt-dlp).",
        ) {
            let today = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                / 86400;
            if let Some(age) = release_age(&version, today)
                && age > 90
            {
                report.add(Status::Warn,"yt-dlp age",format!("release is {age} days old; update yt-dlp before investigating HTTP 403 errors."));
            }
            // Without a URL, yt-dlp prints its actual runtime/EJS diagnostics then exits 2.
            // No network request is made. Never echo debug logs (they may include proxy settings).
            match probe(&path,&["--ignore-config","--verbose","--simulate"],Duration::from_secs(5)) {
                Ok((status,output)) if status.code()==Some(2) || status.success()=>{
                    match output.lines().find_map(|l| l.strip_prefix("[debug] JS runtimes: ")) {
                        Some(runtime) if !runtime.trim().is_empty() && runtime.trim()!="none"=>report.add(Status::Ok,"YouTube JS runtime",runtime),
                        _=>report.add(Status::Fail,"YouTube JS runtime","No enabled runtime detected by yt-dlp. Install/update Deno and add it to PATH; autosrt ignores user yt-dlp config."),
                    }
                    match output.lines().find(|l| l.starts_with("[debug] Optional libraries:")).and_then(|l| l.split(", ").find(|s| s.starts_with("yt_dlp_ejs-"))) {
                        Some(ejs)=>report.add(Status::Ok,"YouTube EJS",ejs),
                        None=>report.add(Status::Warn,"YouTube EJS","Could not confirm yt-dlp-ejs from yt-dlp diagnostics. Update the yt-dlp package including its default dependencies; bundled builds may omit this diagnostic."),
                    }
                }
                _=>report.add(Status::Warn,"YouTube extraction","Could not inspect yt-dlp's runtime/EJS diagnostics. Update yt-dlp and check its --verbose output manually."),
            }
        }
    } else {
        report.add(
            Status::Ok,
            "scope",
            "local media only; YouTube dependencies skipped",
        );
    }
    let config = ConfigFile::discover();
    let mut configured_model = None;
    match config {
        Ok(file) => {
            match file
                .language()
                .and_then(|lang| file.subtitle_mode().map(|mode| (lang, mode)))
            {
                Ok((lang, mode)) => report.add(
                    Status::Ok,
                    "config",
                    format!(
                        "{} | lang={lang} | subtitle-mode={}{}",
                        file.path.display(),
                        mode.as_str(),
                        if file.path.exists() {
                            ""
                        } else {
                            " (not created; defaults in use)"
                        }
                    ),
                ),
                Err(e) => report.add(Status::Fail, "config", e.to_string()),
            }
            if model_override.is_none() {
                match file.model() {
                    Ok(model) => configured_model = model,
                    Err(e) => report.add(Status::Fail, "model config", e.to_string()),
                }
            }
        }
        Err(e) => report.add(Status::Fail, "config", e.to_string()),
    }
    match model_override.or(configured_model) {
        Some(path)=> {
            let valid=(||->std::io::Result<u64>{
                let mut file=File::open(&path)?;
                let metadata=file.metadata()?;
                if !metadata.is_file() {return Err(std::io::Error::other("not a regular file"));}
                let mut magic=[0;4];file.read_exact(&mut magic)?;
                if &magic!=b"lmgg" {return Err(std::io::Error::other("not a whisper.cpp GGML model (invalid header)"));}
                Ok(metadata.len())
            })();
            match valid {
                Ok(size)=>report.add(Status::Ok,"model",format!("{} | {size} bytes | GGML header readable (full model not loaded)",path.display())),
                Err(e)=>report.add(Status::Fail,"model",format!("{}: {e}. Set a valid file with: autosrt config set model /path/to/ggml-small.bin",path.display())),
            }
        }
        None=>report.add(Status::Fail,"model","Not configured. Run: autosrt config set model /path/to/ggml-small.bin (or use --model / AUTOSRT_MODEL)."),
    }
    report
}

fn release_age(version: &str, today: u64) -> Option<u64> {
    let parts: Vec<u64> = version
        .trim()
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    if parts.len() != 3 {
        return None;
    }
    let (year, month, day) = (parts[0], parts[1], parts[2]);
    if !(1970..=2200).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let leap = |y: u64| y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > months[(month - 1) as usize] {
        return None;
    }
    let days = (1970..year)
        .map(|y| if leap(y) { 366 } else { 365 })
        .sum::<u64>()
        + months[..(month - 1) as usize].iter().sum::<u64>()
        + day
        - 1;
    today.checked_sub(days)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_age_handles_dates_and_non_release_versions() {
        assert_eq!(release_age("1970.01.01", 100), Some(100));
        assert_eq!(release_age("2024.02.29", 19782), Some(0));
        assert_eq!(release_age("2025.02.29", 25000), None);
        assert_eq!(release_age("nightly@abc", 25000), None);
    }
    #[test]
    #[cfg(unix)]
    fn probe_has_a_deadline() {
        let start = Instant::now();
        let r = probe(
            Path::new("/bin/sh"),
            &["-c", "sleep 10"],
            Duration::from_millis(100),
        );
        assert!(r.unwrap_err().to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}

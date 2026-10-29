#![cfg(unix)]
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
};

fn setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for name in ["yt-dlp", "ffprobe", "ffmpeg", "whisper-cli"] {
        let path = dir.path().join(name);
        fs::write(&path,r#"#!/usr/bin/env python3
import sys, os, json, pathlib, time
name=pathlib.Path(sys.argv[0]).name
a=sys.argv[1:]
with open(os.environ['CALLS'], 'a') as f: f.write(json.dumps([name]+a)+'\n')
if os.environ.get('FAIL_GPU') and name=='whisper-cli' and '-ng' not in a:
    print('whisper_backend_init_gpu: failed to initialize MTL0 backend',file=sys.stderr);sys.exit(7)
if os.environ.get('FAIL_TOOL')==name:
    print('deliberate tool failure',file=sys.stderr);sys.exit(7)
if os.environ.get('WAIT_TOOL')==name:
    print('waiting',flush=True);time.sleep(30)
if name=='yt-dlp':
    if '--dump-single-json' in a: print(json.dumps({'id':'test-video','is_live':False}))
    else:
        p=pathlib.Path(a[a.index('-o')+1].replace('%(id)s','test-video').replace('%(ext)s','mkv'))
        p.write_bytes(b'video')
        print('AUTOSRT_PROGRESS 100.0%')
        print('AUTOSRT_FILE '+str(p))
elif name=='ffprobe': print(json.dumps({'format':{'duration':'2','start_time':'0'},'streams':[{'codec_type':'audio','start_time':'0'}]}))
elif name=='ffmpeg':
    pathlib.Path(a[-1]).write_bytes(b'wav')
    print('out_time_us=2000000')
elif name=='whisper-cli':
    p=pathlib.Path(a[a.index('-of')+1]+'.json')
    p.write_text(json.dumps({'transcription':[{'offsets':{'from':100,'to':1500},'text':' 안녕하세요 '}]}))
    print('whisper_print_progress_callback: progress = 100%')
"#).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(dir.path().join("model.bin"), b"fake-model").unwrap();
    fs::write(dir.path().join("한글 sample.part.mp3"), b"fake-media").unwrap();
    dir
}
fn command(dir: &Path, input: &str) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_autosrt"));
    c.current_dir(dir)
        .env(
            "PATH",
            format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()),
        )
        .env("AUTOSRT_CONFIG_DIR", dir.join("config"))
        .env("CALLS", dir.join("calls.jsonl"))
        .env("AUTOSRT_MODEL", dir.join("model.bin"))
        .args([input, "--no-tui"]);
    c
}
fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn local_pipeline_and_no_overwrite() {
    let d = setup();
    let result = command(d.path(), "한글 sample.part.mp3").output().unwrap();
    assert_ok(&result);
    let output = d.path().join("한글 sample.part.srt");
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        "1\n00:00:00,100 --> 00:00:01,500\n안녕하세요\n\n"
    );
    assert!(
        !fs::read_to_string(d.path().join("calls.jsonl"))
            .unwrap()
            .contains("yt-dlp")
    );
    fs::write(&output, "existing").unwrap();
    assert!(
        !command(d.path(), "한글 sample.part.mp3")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read_to_string(&output).unwrap(), "existing");
    assert_ok(
        &command(d.path(), "한글 sample.part.mp3")
            .arg("--force")
            .output()
            .unwrap(),
    );
}
#[test]
fn youtube_pipeline_uses_final_path_and_expected_flags() {
    let d = setup();
    assert_ok(&command(d.path(), "https://youtu.be/xxx").output().unwrap());
    assert!(d.path().join("test-video.srt").is_file());
    let calls = fs::read_to_string(d.path().join("calls.jsonl")).unwrap();
    for flag in [
        "--no-playlist",
        "--ignore-config",
        "after_move:AUTOSRT_FILE",
        "bv*+ba/b",
    ] {
        assert!(calls.contains(flag));
    }
}
#[test]
fn errors_do_not_publish_output_or_change_input() {
    let d = setup();
    let r = command(d.path(), "한글 sample.part.mp3")
        .env("FAIL_TOOL", "whisper-cli")
        .output()
        .unwrap();
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("deliberate tool failure"));
    assert!(!d.path().join("한글 sample.part.srt").exists());
    let r = command(d.path(), "한글 sample.part.mp3")
        .args(["--output", "한글 sample.part.mp3", "--force"])
        .output()
        .unwrap();
    assert!(!r.status.success());
    assert_eq!(
        fs::read(d.path().join("한글 sample.part.mp3")).unwrap(),
        b"fake-media"
    );
}
#[test]
fn cancellation_terminates_worker_and_publishes_nothing() {
    let d = setup();
    let mut child = command(d.path(), "한글 sample.part.mp3")
        .env("WAIT_TOOL", "whisper-cli")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if fs::read_to_string(d.path().join("calls.jsonl"))
            .unwrap_or_default()
            .contains("whisper-cli")
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(130));
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("cancellation timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!d.path().join("한글 sample.part.srt").exists());
}

#[test]
fn missing_engine_is_actionable() {
    let d = setup();
    fs::remove_file(d.path().join("whisper-cli")).unwrap();
    let result = command(d.path(), "한글 sample.part.mp3")
        .env("PATH", d.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Missing tool: whisper-cli"));
}

#[test]
fn tui_cancel_restores_terminal() {
    let d = setup();
    let result = Command::new("python3")
        .args([
            "-c",
            r#"
import os, sys, pty, termios, fcntl, struct, subprocess, select, time, pathlib
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
before = termios.tcgetattr(slave)
p = subprocess.Popen([sys.argv[1], '한글 sample.part.mp3'], stdin=slave, stdout=slave, stderr=slave)
data = b''
deadline = time.monotonic()+12
sent = False
scroll_state = 0
follow_offset = 0
try:
    while time.monotonic()<deadline:
        if select.select([master],[],[],0.1)[0]:
            data += os.read(master,65536)
        if pathlib.Path(os.environ['CALLS']).exists() and 'whisper-cli' in pathlib.Path(os.environ['CALLS']).read_text() and not sent:
            if scroll_state == 0:
                os.write(master,b'\x1b[H');scroll_state=1
            elif scroll_state == 1 and b'SCROLL' in data:
                follow_offset=len(data)
                os.write(master,b'\x1b[F');scroll_state=2
            elif scroll_state == 2 and b'FOLLOW' in data[follow_offset:]:
                os.write(master,b'q');sent=True
        if p.poll() is not None: break
    assert p.poll()==130, (p.poll(),data[-2000:])
    assert sent, 'TUI did not handle Home/End and return to follow mode'
    assert termios.tcgetattr(slave)==before, 'terminal settings not restored'
    assert b'\x1b[?1049h' in data, 'alternate screen not entered'
    assert b'\x1b[?1049l' in data, 'alternate screen not left'
    assert b'\x1b[?1000h' in data, 'mouse capture not enabled'
    assert b'\x1b[?1000l' in data, 'mouse capture not disabled'
finally:
    if p.poll() is None: p.kill();p.wait()
    os.close(master);os.close(slave)
"#,
            env!("CARGO_BIN_EXE_autosrt"),
        ])
        .current_dir(d.path())
        .env(
            "PATH",
            format!("{}:{}", d.path().display(), std::env::var("PATH").unwrap()),
        )
        .env("AUTOSRT_CONFIG_DIR", d.path().join("config"))
        .env("CALLS", d.path().join("calls.jsonl"))
        .env("AUTOSRT_MODEL", d.path().join("model.bin"))
        .env("WAIT_TOOL", "whisper-cli")
        .output()
        .unwrap();
    assert_ok(&result);
}

fn config_command(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_autosrt"))
        .env("AUTOSRT_CONFIG_DIR", dir.join("config"))
        .env_remove("AUTOSRT_MODEL")
        .env("PATH", "")
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn config_set_get_list_and_language_precedence() {
    let d = setup();
    let get = config_command(d.path(), &["config", "get", "lang"]);
    assert_ok(&get);
    assert_eq!(String::from_utf8_lossy(&get.stdout), "auto\n");
    assert!(!d.path().join("config").exists());
    assert_ok(&config_command(d.path(), &["config", "set", "lang", "KO"]));
    let list = config_command(d.path(), &["config", "list"]);
    assert_ok(&list);
    assert_eq!(String::from_utf8_lossy(&list.stdout), "lang=ko\n");
    assert_eq!(
        config_command(d.path(), &["config", "get", "lang"]).stdout,
        b"ko\n"
    );
    assert_ok(&command(d.path(), "한글 sample.part.mp3").output().unwrap());
    let calls = fs::read_to_string(d.path().join("calls.jsonl")).unwrap();
    let last: serde_json::Value = serde_json::from_str(calls.lines().last().unwrap()).unwrap();
    let a = last.as_array().unwrap();
    let i = a.iter().position(|v| v == "-l").unwrap();
    assert_eq!(a[i + 1], "ko");
    assert_ok(
        &command(d.path(), "한글 sample.part.mp3")
            .args(["--lang", "ja", "--force"])
            .output()
            .unwrap(),
    );
    let calls = fs::read_to_string(d.path().join("calls.jsonl")).unwrap();
    let last: serde_json::Value = serde_json::from_str(calls.lines().last().unwrap()).unwrap();
    assert_eq!(last[i + 1], "ja");
    assert_eq!(
        config_command(d.path(), &["config", "get", "lang"]).stdout,
        b"ko\n"
    );
    assert_ok(&config_command(
        d.path(),
        &["config", "set", "lang", "auto"],
    ));
    assert_eq!(
        config_command(d.path(), &["config", "get", "lang"]).stdout,
        b"auto\n"
    );
}
#[test]
fn config_rejects_invalid_input_without_clobbering() {
    let d = setup();
    assert_ok(&config_command(d.path(), &["config", "set", "lang", "en"]));
    let p = d.path().join("config/config.json");
    let before = fs::read(&p).unwrap();
    for args in [
        vec!["config", "set", "lang", "zz"],
        vec!["config", "set", "typo", "ko"],
        vec!["config", "get", "typo"],
    ] {
        assert!(!config_command(d.path(), &args).status.success());
        assert_eq!(fs::read(&p).unwrap(), before);
    }
    fs::write(&p, "{broken").unwrap();
    for args in [
        vec!["config", "list"],
        vec!["config", "get", "lang"],
        vec!["config", "set", "lang", "ko"],
    ] {
        assert!(!config_command(d.path(), &args).status.success());
        assert_eq!(fs::read_to_string(&p).unwrap(), "{broken");
    }
    fs::write(&p, r#"{"lang":"en","future_key":42}"#).unwrap();
    assert_ok(&config_command(d.path(), &["config", "set", "lang", "ko"]));
    let v: serde_json::Value = serde_json::from_slice(&fs::read(p).unwrap()).unwrap();
    assert_eq!(v["future_key"], 42);
}

#[test]
fn saved_model_is_used_without_environment_and_can_be_overridden() {
    let d = setup();
    let model = fs::canonicalize(d.path().join("model.bin")).unwrap();
    assert_ok(&config_command(
        d.path(),
        &["config", "set", "model", model.to_str().unwrap()],
    ));
    assert_eq!(
        String::from_utf8_lossy(&config_command(d.path(), &["config", "get", "model"]).stdout)
            .trim(),
        model.to_str().unwrap()
    );
    assert_ok(
        &command(d.path(), "한글 sample.part.mp3")
            .env_remove("AUTOSRT_MODEL")
            .output()
            .unwrap(),
    );
    let override_model = fs::canonicalize(d.path()).unwrap().join("override.bin");
    fs::write(&override_model, b"override").unwrap();
    assert_ok(
        &command(d.path(), "한글 sample.part.mp3")
            .env("AUTOSRT_MODEL", &override_model)
            .arg("--force")
            .output()
            .unwrap(),
    );
    let calls = fs::read_to_string(d.path().join("calls.jsonl")).unwrap();
    let last: serde_json::Value = serde_json::from_str(calls.lines().last().unwrap()).unwrap();
    let a = last.as_array().unwrap();
    let i = a.iter().position(|v| v == "-m").unwrap();
    assert_eq!(a[i + 1], override_model.to_str().unwrap());
    assert_ok(
        &command(d.path(), "한글 sample.part.mp3")
            .env("AUTOSRT_MODEL", &override_model)
            .args(["--model", model.to_str().unwrap(), "--force"])
            .output()
            .unwrap(),
    );
    let calls = fs::read_to_string(d.path().join("calls.jsonl")).unwrap();
    let last: serde_json::Value = serde_json::from_str(calls.lines().last().unwrap()).unwrap();
    assert_eq!(last[i + 1], model.to_str().unwrap());
}

#[test]
fn required_commands_are_checked_before_model_and_no_job_starts() {
    let d = setup();
    let empty = d.path().join("empty-path");
    fs::create_dir(&empty).unwrap();
    for (input, expected) in [
        (
            "한글 sample.part.mp3",
            vec!["ffmpeg", "ffprobe", "whisper-cli"],
        ),
        (
            "https://youtu.be/xxx",
            vec!["yt-dlp", "ffmpeg", "ffprobe", "whisper-cli"],
        ),
    ] {
        let result = command(d.path(), input)
            .env("PATH", &empty)
            .env_remove("AUTOSRT_MODEL")
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("Missing tool:"));
        for tool in expected {
            assert!(error.contains(tool), "{error}");
        }
        if input.ends_with("mp3") {
            assert!(!error.contains("yt-dlp"));
        }
        assert!(!error.contains("No speech model configured"));
        assert!(!d.path().join("calls.jsonl").exists());
    }
}

#[test]
fn gpu_initialization_failure_retries_on_cpu_once() {
    let d = setup();
    assert_ok(
        &command(d.path(), "한글 sample.part.mp3")
            .env("FAIL_GPU", "1")
            .output()
            .unwrap(),
    );
    let calls = fs::read_to_string(d.path().join("calls.jsonl")).unwrap();
    let whisper: Vec<_> = calls
        .lines()
        .filter(|l| l.contains("whisper-cli"))
        .collect();
    assert_eq!(whisper.len(), 2);
    assert!(!whisper[0].contains("-ng"));
    assert!(whisper[1].contains("-ng"));
}

fn doctor_setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for name in ["yt-dlp", "ffmpeg", "ffprobe", "whisper-cli"] {
        let path = dir.path().join(name);
        fs::write(
            &path,
            r#"#!/bin/sh
name=${0##*/}
if [ "$DOCTOR_BROKEN" = "$name" ]; then exit 7; fi
if [ "$name" = yt-dlp ]; then
  if [ "$1" = --version ]; then
    echo 2000.01.01
  else
    echo '[debug] Optional libraries: test-1.0, yt_dlp_ejs-0.8.0'
    if [ -n "$DOCTOR_NO_JS" ]; then
      echo '[debug] JS runtimes: none'
    else
      echo '[debug] JS runtimes: deno-2.9.8'
    fi
    echo '[debug] Proxy map: secret-must-not-appear'
    exit 2
  fi
else
  echo "$name version 1.0"
fi
"#,
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(dir.path().join("model.bin"), b"lmggfixture-model").unwrap();
    dir
}
fn doctor_command(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_autosrt"));
    c.env("PATH", dir)
        .env("AUTOSRT_CONFIG_DIR", dir.join("config"))
        .env("AUTOSRT_MODEL", dir.join("model.bin"))
        .arg("doctor");
    c
}
#[test]
fn doctor_reports_versions_age_runtime_and_model_without_transcribing() {
    let d = doctor_setup();
    let r = doctor_command(d.path()).output().unwrap();
    assert_ok(&r);
    let text = String::from_utf8_lossy(&r.stdout);
    for expected in [
        "[OK] ffmpeg:",
        "[OK] ffprobe:",
        "[OK] whisper-cli:",
        "[OK] yt-dlp:",
        "[WARN] yt-dlp age:",
        "deno-2.9.8",
        "yt_dlp_ejs-0.8.0",
        "GGML header readable",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert!(!text.contains("secret-must-not-appear"));
    assert!(!d.path().join("config").exists());
}
#[test]
fn doctor_collects_failures_and_local_scope_skips_youtube() {
    let d = doctor_setup();
    fs::remove_file(d.path().join("yt-dlp")).unwrap();
    let r = doctor_command(d.path()).output().unwrap();
    assert_eq!(r.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&r.stdout).contains("[FAIL] yt-dlp"));
    assert_ok(&doctor_command(d.path()).arg("--local").output().unwrap());
    let r = doctor_command(d.path())
        .arg("--local")
        .env("DOCTOR_BROKEN", "ffmpeg")
        .env_remove("AUTOSRT_MODEL")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&r.stdout);
    assert_eq!(r.status.code(), Some(1));
    assert!(text.contains("[FAIL] ffmpeg"));
    assert!(text.contains("[FAIL] model"));
    assert!(text.contains("[OK] whisper-cli"));
}
#[test]
fn doctor_checks_enabled_js_and_invalid_model_and_config() {
    let d = doctor_setup();
    fs::write(d.path().join("model.bin"), b"bad model").unwrap();
    fs::create_dir(d.path().join("config")).unwrap();
    fs::write(d.path().join("config/config.json"), "broken json").unwrap();
    let r = doctor_command(d.path())
        .env("DOCTOR_NO_JS", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&r.stdout);
    assert_eq!(r.status.code(), Some(1));
    for expected in ["[FAIL] YouTube JS runtime", "[FAIL] config", "[FAIL] model"] {
        assert!(text.contains(expected), "{text}");
    }
}

#[test]
fn legacy_environment_names_still_work_and_new_names_win() {
    let d = setup();
    assert_ok(
        &command(d.path(), "한글 sample.part.mp3")
            .env_remove("AUTOSRT_MODEL")
            .env_remove("AUTOSRT_CONFIG_DIR")
            .env("YSTT_MODEL", d.path().join("model.bin"))
            .env("YSTT_CONFIG_DIR", d.path().join("legacy-config"))
            .output()
            .unwrap(),
    );
    assert_ok(
        &command(d.path(), "한글 sample.part.mp3")
            .env("YSTT_MODEL", d.path().join("missing.bin"))
            .env("YSTT_CONFIG_DIR", d.path().join("missing-config"))
            .arg("--force")
            .output()
            .unwrap(),
    );
}

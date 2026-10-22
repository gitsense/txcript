#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Hermetic CLI discovery using handcrafted sessions, never personal history.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const PI_ID: &str = "11111111-1111-4111-8111-111111111111";
const CODEX_ID: &str = "22222222-2222-4222-8222-222222222222";

struct Sessions {
    home: tempfile::TempDir,
    pi_root: PathBuf,
    codex_home: PathBuf,
    pi: PathBuf,
    codex: PathBuf,
}

impl Sessions {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let pi_root = home.path().join("custom-pi-sessions");
        let codex_home = home.path().join("custom-codex");
        let pi = pi_root.join("project/session-with-different-filename.jsonl");
        let codex = codex_home.join("sessions/2026/01/01/rollout-different-filename.jsonl");
        write_records(
            &pi,
            &[
                json!({"type":"session","version":3,"id":PI_ID,"timestamp":"2026-01-01T00:00:00Z","cwd":"/synthetic/pi"}),
                json!({"type":"message","id":"pi-message","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"Synthetic Pi answer."}],"stopReason":"stop"}}),
            ],
        );
        write_records(
            &codex,
            &[
                json!({"type":"session_meta","timestamp":"2026-01-01T00:00:00Z","payload":{"id":CODEX_ID,"timestamp":"2026-01-01T00:00:00Z","cwd":"/synthetic/codex"}}),
                json!({"type":"response_item","timestamp":"2026-01-01T00:00:01Z","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Synthetic Codex answer."}]}}),
            ],
        );
        Self {
            home,
            pi_root,
            codex_home,
            pi,
            codex,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_txcript"))
            .args(args)
            // In particular, do not inherit real session-root overrides.
            .env_clear()
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("PI_CODING_AGENT_SESSION_DIR", &self.pi_root)
            .env("CODEX_HOME", &self.codex_home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                let output = child.wait_with_output().unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                return output;
            }
            if started.elapsed() > Duration::from_secs(10) {
                child.kill().unwrap();
                let _ = child.wait_with_output();
                panic!("CLI timed out: it may have opened an out-of-scope store");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn list(&self, harness: &str) -> Vec<Value> {
        serde_json::from_slice(
            &self
                .run(&["list", "--from", harness, "--format", "json"])
                .stdout,
        )
        .unwrap()
    }
}

fn write_records(path: &Path, records: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(path, text).unwrap();
}

#[test]
fn scoped_discovery_returns_header_ids_paths_and_tail_readable_sessions() {
    let sessions = Sessions::new();
    for (harness, id, path, text) in [
        ("pi", PI_ID, &sessions.pi, "Synthetic Pi answer."),
        (
            "codex",
            CODEX_ID,
            &sessions.codex,
            "Synthetic Codex answer.",
        ),
    ] {
        let rows = sessions.list(harness);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["harness"], harness);
        assert_eq!(rows[0]["id"], id);
        let resolved = rows[0]["path"].as_str().unwrap();
        assert_eq!(Path::new(resolved), path);
        assert!(rows[0]["started_at"].is_string());
        assert!(rows[0]["updated_at"].is_string());
        let update: Value =
            serde_json::from_slice(&sessions.run(&["tail", resolved, "--from", harness]).stdout)
                .unwrap();
        assert_eq!(update["status"], "message");
        assert_eq!(update["message"]["text"], text);
    }
}

#[test]
fn missing_selected_store_does_not_fall_back_to_other_harnesses() {
    let sessions = Sessions::new();
    std::fs::remove_dir_all(&sessions.pi_root).unwrap();
    assert!(sessions.list("pi").is_empty());
    assert_eq!(sessions.list("codex").len(), 1);
}

#[test]
fn version_handshake_matches_short_version_and_package_metadata() {
    let sessions = Sessions::new();
    let short = sessions.run(&["--version"]);
    assert_eq!(
        String::from_utf8(short.stdout).unwrap(),
        format!("txcript {}\n", env!("CARGO_PKG_VERSION"))
    );
    let info: Value =
        serde_json::from_slice(&sessions.run(&["version", "--format", "json"]).stdout).unwrap();
    assert_eq!(info["schema_version"], 1);
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["distribution"], "gitsense");
    assert_eq!(info["contracts"], json!(["gitsense.sessions.v1"]));
    assert!(info.as_object().unwrap().contains_key("git_commit"));
    let npm: Value = serde_json::from_str(include_str!("../../package.json")).unwrap();
    assert_eq!(info["version"], npm["version"]);
    for manifest in [
        include_str!("../../Cargo.toml"),
        include_str!("../Cargo.toml"),
    ] {
        let package_version = manifest
            .lines()
            .find_map(|line| line.strip_prefix("version = \""))
            .unwrap()
            .trim_end_matches('"');
        assert_eq!(info["version"], package_version);
    }
}

#[cfg(unix)]
#[test]
fn handshake_and_registry_do_not_open_session_stores() {
    let sessions = Sessions::new();
    blocking_log(&sessions.pi);
    blocking_log(&sessions.codex);
    let info: Value =
        serde_json::from_slice(&sessions.run(&["version", "--format", "json"]).stdout).unwrap();
    assert_eq!(info["schema_version"], 1);
    let harnesses: Vec<String> =
        serde_json::from_slice(&sessions.run(&["harnesses", "--format", "json"]).stdout).unwrap();
    assert!(harnesses.iter().any(|name| name == "pi"));
    assert!(harnesses.iter().any(|name| name == "codex"));
}

// A FIFO would block if discovery even opened the unrelated log. The parent
// deadline kills/reaps a regressed child rather than hanging the test suite.
#[cfg(unix)]
fn blocking_log(path: &Path) {
    std::fs::remove_file(path).unwrap();
    assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
}

#[cfg(unix)]
#[test]
fn codex_scope_never_opens_pi_logs() {
    let sessions = Sessions::new();
    blocking_log(&sessions.pi);
    assert_eq!(sessions.list("codex")[0]["id"], CODEX_ID);
}

#[cfg(unix)]
#[test]
fn pi_scope_never_opens_codex_logs() {
    let sessions = Sessions::new();
    blocking_log(&sessions.codex);
    assert_eq!(sessions.list("pi")[0]["id"], PI_ID);
}

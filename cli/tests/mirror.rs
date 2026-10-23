#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Generic mirror acceptance tests: native stores in an isolated home, no
//! personal sessions, app processes, gateway calls, or cloud authentication.
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{Value, json};
use txcript::harness::{
    amp, antigravity, campfire, claude_code, codex, cowork, cursor, cursor_desktop, fx, grok,
    grok_bot, pi, simple,
};
use txcript::{Codec, Common, Store, TextCodec, Transcript};

struct Sandbox(tempfile::TempDir);
impl Sandbox {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn root(&self) -> &Path {
        self.0.path()
    }
    fn path(&self, path: &str) -> PathBuf {
        self.root().join(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_txcript"));
        command
            .env_clear()
            .env("HOME", self.root())
            .env("USERPROFILE", self.root())
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("COWORK_SESSIONS_DIR", self.path("cowork"))
            .env("CURSOR_DESKTOP_USER_DIR", self.path("cursor-user"))
            .env("OPENCODE_DB", self.path("opencode.db"))
            .env("HERMES_HOME", self.path("hermes"))
            .env("AMP_THREADS_DIR", self.path("amp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn output(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "args={args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn pi(&self) -> PathBuf {
        let store = pi::PiStore::new(self.path(".pi/agent/sessions"));
        store
            .save(&pi::Pi::from_common(&sample()).unwrap())
            .unwrap()
            .reference
    }
    fn follow(&self, id: &str, harness: &str) -> Follower {
        let mut child = self
            .command()
            .args([
                "observe",
                id,
                "--from",
                harness,
                "--format",
                "json",
                "--follow",
                "--interval-ms",
                "100",
            ])
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, recv) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Follower { child, recv }
    }
}
struct Follower {
    child: Child,
    recv: Receiver<std::io::Result<String>>,
}
impl Follower {
    fn frame(&self) -> Value {
        serde_json::from_str(
            &self
                .recv
                .recv_timeout(Duration::from_secs(10))
                .expect("follower must emit promptly")
                .unwrap(),
        )
        .unwrap()
    }
}
impl Drop for Follower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn sample() -> Transcript<Common> {
    simple::Simple::to_common(
        &simple::Simple::from_text(
            r#"{
        "id":"11111111-1111-4111-8111-111111111111",
        "timestamp":"2026-01-01T00:00:00Z", "cwd":"/synthetic", "title":"Mirror test",
        "messages":[{"role":"user","content":"question"},{"role":"assistant","content":"answer"}]
    }"#,
        )
        .unwrap(),
    )
    .unwrap()
}

fn check(frame: &Value, harness: &str, id: &str) {
    assert_eq!(frame["schema_version"], 1);
    assert_eq!(frame["session"]["harness"], harness);
    assert_eq!(frame["session"]["id"], id);
    assert!(frame["session"]["started_at"].is_string());
    assert_eq!(frame["projection"], "common_text");
    assert_eq!(frame["order"], "normalized");
    assert_eq!(frame["read_mode"], "full_snapshot");
    assert_eq!(frame["status"], "message");
    assert_eq!(frame["message"]["content"], "answer");
    assert_eq!(frame["message"]["role"], "assistant");
    assert!(frame["message"]["id"].is_null());
    assert!(frame["message"]["timestamp"].is_null());
    assert_eq!(frame["message_count"], 2);
    assert!(
        frame["change_token"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn generic_mirror_reads_all_local_store_adapters() {
    let sandbox = Sandbox::new();
    let common = sample();
    macro_rules! save {
        ($store:expr, $codec:ty) => {
            $store
                .save(&<$codec>::from_common(&common).unwrap())
                .unwrap()
                .id
        };
    }
    let mut ids = vec![
        (
            "claude_code",
            save!(
                claude_code::ClaudeStore::new(sandbox.path(".claude/projects")),
                claude_code::ClaudeCode
            ),
        ),
        (
            "pi",
            save!(pi::PiStore::new(sandbox.path(".pi/agent/sessions")), pi::Pi),
        ),
        (
            "codex",
            save!(
                codex::CodexStore::new(sandbox.path(".codex/sessions")),
                codex::Codex
            ),
        ),
        (
            "campfire",
            save!(
                campfire::CampfireStore::new(sandbox.path(".campfire/agent/sessions")),
                campfire::Campfire
            ),
        ),
        (
            "amp",
            save!(amp::AmpStore::new(sandbox.path("amp")), amp::Amp),
        ),
        (
            "fx",
            save!(fx::FxStore::new(sandbox.path(".fx/sessions")), fx::Fx),
        ),
        (
            "grok",
            save!(
                grok::GrokStore::new(sandbox.path(".grok/sessions")),
                grok::Grok
            ),
        ),
        (
            "grok_bot",
            save!(
                grok_bot::GrokBotStore::new(sandbox.path("agent-data/agent-transcripts")),
                grok_bot::GrokBot
            ),
        ),
        (
            "cursor",
            save!(
                cursor::CursorStore::new(sandbox.path(".cursor/chats")),
                cursor::Cursor
            ),
        ),
        (
            "cursor_desktop",
            save!(
                cursor_desktop::CursorDesktopStore::new(sandbox.path("cursor-user")),
                cursor_desktop::CursorDesktop
            ),
        ),
        (
            "antigravity",
            save!(
                antigravity::AntigravityStore::new(sandbox.path(".gemini/antigravity-cli")),
                antigravity::Antigravity
            ),
        ),
    ];
    std::fs::create_dir_all(
        sandbox.path(
            "cowork/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa/bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        ),
    )
    .unwrap();
    ids.push((
        "cowork",
        save!(
            cowork::CoworkStore::new(sandbox.path("cowork")),
            cowork::Cowork
        ),
    ));
    let db = make_opencode(&sandbox);
    drop(db);
    ids.push(("opencode", "ses_test".into()));
    make_hermes(&sandbox);
    ids.push(("hermes", "hermes-test".into()));
    for (harness, id) in &ids {
        let show = sandbox.json(&["show", id, "--from", harness, "--format", "json"]);
        check(&show, harness, id);
        let observation = sandbox.json(&["observe", id, "--from", harness, "--format", "json"]);
        check(&observation, harness, id);
        assert_eq!(show["change_token"], observation["change_token"]);
        let history = sandbox.json(&["messages", id, "--from", harness, "--format", "json"]);
        assert_eq!(history["session"]["id"], *id);
        assert_eq!(history["projection"], "common_text");
        assert_eq!(history["order"], "normalized");
        assert_eq!(history["read_mode"], "full_snapshot");
        assert_eq!(history["messages"].as_array().unwrap().len(), 2);
        assert_eq!(history["messages"][0]["role"], "user");
        assert_eq!(history["messages"][0]["content"], "question");
        assert_eq!(history["messages"][1]["role"], "assistant");
        assert_eq!(history["messages"][1]["content"], "answer");
        assert_eq!(history["has_earlier"], false);
        let filtered = sandbox.json(&[
            "messages",
            id,
            "--from",
            harness,
            "--role",
            "assistant",
            "--limit",
            "1",
            "--format",
            "json",
        ]);
        assert_eq!(filtered["messages"].as_array().unwrap().len(), 1);
        assert_eq!(filtered["messages"][0]["content"], "answer");
        assert_eq!(filtered["has_earlier"], false);
        let limited = sandbox.json(&[
            "messages", id, "--from", harness, "--limit", "1", "--format", "json",
        ]);
        assert_eq!(limited["messages"][0]["content"], "answer");
        assert_eq!(limited["has_earlier"], true);
    }
    // The same full native ID in multiple harnesses must never be silently
    // resolved to whichever store happened to be scanned first.
    let output = sandbox.output(&["show", "11111111", "--format", "json"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ambiguous"));
}

fn make_opencode(sandbox: &Sandbox) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(sandbox.path("opencode.db")).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, title TEXT, version TEXT, time_created INTEGER, time_archived INTEGER, model TEXT, slug TEXT, time_updated INTEGER, parent_id TEXT);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, data TEXT);
         INSERT INTO session VALUES ('ses_test','/synthetic','Mirror test','1',1767225600000,NULL,NULL,NULL,1767225600000,NULL);
         INSERT INTO message VALUES ('u','ses_test',1767225600001,'{\"role\":\"user\"}');
         INSERT INTO message VALUES ('a','ses_test',1767225600002,'{\"role\":\"assistant\"}');
         INSERT INTO part VALUES ('pu','u','ses_test','{\"type\":\"text\",\"text\":\"question\"}');
         INSERT INTO part VALUES ('pa','a','ses_test','{\"type\":\"text\",\"text\":\"answer\"}');"
    ).unwrap();
    conn
}

fn make_hermes(sandbox: &Sandbox) {
    std::fs::create_dir_all(sandbox.path("hermes")).unwrap();
    let conn = rusqlite::Connection::open(sandbox.path("hermes/state.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT, model TEXT, started_at REAL, cwd TEXT, git_branch TEXT, title TEXT, archived INTEGER);
         CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL, active INTEGER, compacted INTEGER);
         INSERT INTO sessions VALUES ('hermes-test','cli','test',1767225600.0,'/synthetic',NULL,'Mirror test',0);
         INSERT INTO messages VALUES (1,'hermes-test','user','question',NULL,NULL,NULL,1767225601.0,1,0);
         INSERT INTO messages VALUES (2,'hermes-test','assistant','answer',NULL,NULL,NULL,1767225602.0,1,0);"
    ).unwrap();
}

#[test]
fn observe_detects_sqlite_wal_edits_to_an_existing_message_part() {
    let sandbox = Sandbox::new();
    let conn = make_opencode(&sandbox);
    let follower = sandbox.follow("ses_test", "opencode");
    let initial = follower.frame();
    assert_eq!(initial["message"]["content"], "answer");
    conn.execute(
        "UPDATE part SET data = ?1 WHERE id = 'pa'",
        [r#"{"type":"text","text":"edited"}"#],
    )
    .unwrap();
    let edited = follower.frame();
    assert_eq!(edited["message"]["content"], "edited");
    assert_ne!(initial["change_token"], edited["change_token"]);
    assert_eq!(initial["message_count"], edited["message_count"]);
    assert!(
        follower
            .recv
            .recv_timeout(Duration::from_millis(350))
            .is_err()
    );
}

#[test]
fn follow_keeps_binding_ignores_nontext_and_handles_partial_appends() {
    let sandbox = Sandbox::new();
    let path = sandbox.pi();
    let follower = sandbox.follow("11111111", "pi");
    let initial = follower.frame();
    check(&initial, "pi", "11111111-1111-4111-8111-111111111111");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "{}", json!({"type":"message","id":"user2","message":{"role":"user","content":[{"type":"text","text":"next question"}]}})).unwrap();
    assert!(
        follower
            .recv
            .recv_timeout(Duration::from_millis(350))
            .is_err()
    );
    let next = json!({"type":"message","id":"a2","message":{"role":"assistant","content":[{"type":"text","text":"updated"}]}}).to_string();
    // A parser may omit an unfinished trailing record. It must not invent a
    // new assistant text value while that record is incomplete.
    file.write_all(&next.as_bytes()[..20]).unwrap();
    assert!(
        follower
            .recv
            .recv_timeout(Duration::from_millis(350))
            .is_err()
    );
    file.write_all(&next.as_bytes()[20..]).unwrap();
    file.write_all(b"\n").unwrap();
    let changed = follower.frame();
    assert_eq!(changed["message"]["content"], "updated");
    assert_eq!(changed["message_count"], 4);
    assert_ne!(initial["change_token"], changed["change_token"]);
}

#[test]
fn follower_stops_on_source_identity_change_without_publishing_replacement() {
    let sandbox = Sandbox::new();
    let path = sandbox.pi();
    let mut follower = sandbox.follow("11111111", "pi");
    assert_eq!(follower.frame()["message"]["content"], "answer");
    let replacement = std::fs::read_to_string(&path)
        .unwrap()
        .replace(
            "11111111-1111-4111-8111-111111111111",
            "99999999-9999-4999-8999-999999999999",
        )
        .replace("answer", "must not publish");
    std::fs::write(path, replacement).unwrap();
    assert!(follower.recv.recv_timeout(Duration::from_secs(5)).is_err());
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = follower.child.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut error = String::new();
    std::io::Read::read_to_string(follower.child.stderr.as_mut().unwrap(), &mut error).unwrap();
    assert!(error.contains("identity changed"), "{error}");
}

#[test]
fn compressed_archived_codex_sessions_use_the_generic_reader() {
    let sandbox = Sandbox::new();
    let store = codex::CodexStore::new(sandbox.path(".codex/sessions"));
    let saved = store
        .save(&codex::Codex::from_common(&sample()).unwrap())
        .unwrap();
    let bytes = std::fs::read(&saved.reference).unwrap();
    let archived = sandbox.path(".codex/archived_sessions");
    std::fs::create_dir_all(&archived).unwrap();
    let compressed = archived.join(format!(
        "{}.zst",
        saved.reference.file_name().unwrap().to_string_lossy()
    ));
    std::fs::write(
        compressed,
        zstd::stream::encode_all(bytes.as_slice(), 1).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(&saved.reference).unwrap();
    check(
        &sandbox.json(&["show", &saved.id, "--from", "codex", "--format", "json"]),
        "codex",
        &saved.id,
    );
}

#[test]
fn mirror_rejects_title_and_duplicate_file_resolution() {
    let sandbox = Sandbox::new();
    let path = sandbox.pi();
    let title = sandbox.output(&["show", "Mirror test", "--from", "pi", "--format", "json"]);
    assert!(!title.status.success());
    let copy = path.with_file_name("duplicate.jsonl");
    std::fs::copy(path, copy).unwrap();
    let duplicate = sandbox.output(&["show", "11111111", "--from", "pi", "--format", "json"]);
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("ambiguous"));
}

#[test]
fn generic_commands_reject_cloud_scopes_before_credentials_or_network_access() {
    let sandbox = Sandbox::new();
    for command in ["show", "observe", "messages"] {
        for harness in ["claude_chat", "cowork_remote", "chatgpt"] {
            // No credentials exist. The local-only error must win before any
            // auth/store/network operation, independent of login availability.
            let output =
                sandbox.output(&[command, "11111111", "--from", harness, "--format", "json"]);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("local-disk sessions only"));
        }
    }
}

#[test]
fn no_message_is_distinct_from_missing_source_and_simple_document() {
    let sandbox = Sandbox::new();
    let path = sandbox.pi();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.lines().next().unwrap().to_owned() + "\n").unwrap();
    let frame = sandbox.json(&["show", "11111111", "--from", "pi", "--format", "json"]);
    assert_eq!(frame["status"], "no_message");
    assert!(frame["message"].is_null());
    assert_eq!(frame["message_count"], 0);
    let history = sandbox.json(&["messages", "11111111", "--from", "pi", "--format", "json"]);
    assert_eq!(history["messages"], json!([]));
    assert_eq!(history["has_earlier"], false);
    std::fs::remove_file(path).unwrap();
    let missing = sandbox.output(&["show", "11111111", "--from", "pi", "--format", "json"]);
    assert!(!missing.status.success());
    let simple = sandbox.output(&["show", "11111111", "--from", "simple", "--format", "json"]);
    assert!(!simple.status.success());
    assert!(String::from_utf8_lossy(&simple.stderr).contains("interchange"));
}

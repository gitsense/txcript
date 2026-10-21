//! Bounded, read-only JSONL observation. No discovery, conversion, or imports.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
struct Message {
    id: Option<String>,
    timestamp: Option<String>,
    text: String,
    /// Native metadata, not a guarantee that the entire agent turn is finished.
    stop_reason: Option<String>,
    phase: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
struct Update {
    version: u8,
    harness: String,
    offset: u64,
    status: &'static str,
    message: Option<Message>,
}

// Path metadata is cheap to inspect without opening/decoding the log. Include
// Unix ctime as well as mtime so restored mtimes do not hide same-size edits.
#[derive(Debug, PartialEq)]
struct FileStamp {
    identity: (u64, u64),
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl FileStamp {
    fn from_metadata(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            #[cfg(unix)]
            identity: (meta.dev(), meta.ino()),
            #[cfg(not(unix))]
            identity: (0, 0),
            len: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            changed: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}

struct Reader {
    offset: u64,
    pending: Vec<u8>,
    dropping: bool,
    identity: Option<(u64, u64)>,
    stamp: Option<FileStamp>,
    latest: Option<Message>,
    #[cfg(test)]
    file_opens: usize,
    #[cfg(test)]
    bytes_read: usize,
}

impl Reader {
    fn new() -> Self {
        Self {
            offset: 0,
            pending: Vec::new(),
            dropping: false,
            identity: None,
            stamp: None,
            latest: None,
            #[cfg(test)]
            file_opens: 0,
            #[cfg(test)]
            bytes_read: 0,
        }
    }

    fn has_changed(&self, path: &Path) -> Result<bool, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
        let stamp = FileStamp::from_metadata(&meta);
        // If the platform cannot supply mtime, never silently suppress reads.
        Ok(stamp.modified.is_none() || self.stamp.as_ref() != Some(&stamp))
    }

    // Called only after has_changed: no open, message clone, or decode on the
    // idle path. Re-stat the opened descriptor to handle replacement races.
    fn poll(&mut self, path: &Path, harness: &str, budget: usize) -> Result<Update, String> {
        let mut file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        #[cfg(test)]
        {
            self.file_opens += 1;
        }
        let meta = file.metadata().map_err(|e| e.to_string())?;
        let stamp = FileStamp::from_metadata(&meta);
        let identity = stamp.identity;
        let len = stamp.len;
        let same_size_rewrite = len == self.offset
            && self
                .stamp
                .as_ref()
                .is_some_and(|previous| previous != &stamp);
        let reset = self.identity != Some(identity) || len < self.offset || same_size_rewrite;
        if reset {
            self.offset = 0;
            self.pending.clear();
            self.latest = None;
            self.dropping = false;
        }
        self.identity = Some(identity);
        // A fast-growing log must not make a single poll unbounded either.
        let start = self.offset.max(len.saturating_sub(budget as u64));
        if start > self.offset {
            self.pending.clear();
            file.seek(SeekFrom::Start(start - 1))
                .map_err(|e| e.to_string())?;
            let mut previous = [0];
            file.read_exact(&mut previous).map_err(|e| e.to_string())?;
            #[cfg(test)]
            {
                self.bytes_read += 1;
            }
            self.dropping = previous[0] != b'\n';
        }
        file.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(len - start)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        self.offset = start + bytes.len() as u64;
        #[cfg(test)]
        {
            self.bytes_read += bytes.len();
        }
        // Save the pre-read descriptor metadata, not a later stat: writes
        // concurrent with this read must remain detectable on the next tick.
        self.stamp = Some(stamp);
        for byte in bytes {
            if byte == b'\n' {
                if !self.dropping
                    && let Some(message) = extract(&self.pending, harness)
                {
                    self.latest = Some(message);
                }
                self.pending.clear();
                self.dropping = false;
            } else if !self.dropping {
                self.pending.push(byte);
                if self.pending.len() > budget {
                    self.pending.clear();
                    self.dropping = true;
                }
            }
        }
        Ok(Update {
            version: 1,
            harness: harness.into(),
            offset: self.offset,
            status: if self.latest.is_some() {
                "message"
            } else {
                "not_found_in_window"
            },
            message: self.latest.clone(),
        })
    }
}

fn extract(line: &[u8], harness: &str) -> Option<Message> {
    let row: Value = serde_json::from_slice(line).ok()?;
    let payload = match harness {
        "pi" if row["type"] == "message" => &row["message"],
        "codex" if row["type"] == "response_item" && row["payload"]["type"] == "message" => {
            &row["payload"]
        }
        _ => return None,
    };
    if payload["role"] != "assistant" {
        return None;
    }
    let text = payload["content"]
        .as_array()?
        .iter()
        .filter_map(|block| match block["type"].as_str()? {
            "text" | "output_text" => block["text"].as_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.trim().is_empty() {
        return None;
    }
    Some(Message {
        id: row["id"]
            .as_str()
            .or_else(|| payload["id"].as_str())
            .map(str::to_owned),
        timestamp: row["timestamp"].as_str().map(str::to_owned),
        text,
        stop_reason: payload["stopReason"].as_str().map(str::to_owned),
        phase: payload["phase"].as_str().map(str::to_owned),
    })
}

pub(crate) fn run(
    path: &Path,
    harness: &str,
    bytes: usize,
    follow: bool,
    interval_ms: u64,
) -> Result<ExitCode, String> {
    if !matches!(harness, "pi" | "codex") {
        return Err("tail currently supports --from pi or codex only".into());
    }
    if bytes == 0 || bytes > 64 * 1024 * 1024 {
        return Err("--bytes must be between 1 and 67108864".into());
    }
    if interval_ms < 100 {
        return Err("--interval-ms must be at least 100".into());
    }
    let mut reader = Reader::new();
    let mut previous = None;
    loop {
        if !reader.has_changed(path)? {
            if !follow {
                return Ok(ExitCode::SUCCESS);
            }
            std::thread::sleep(Duration::from_millis(interval_ms));
            continue;
        }
        let update = reader.poll(path, harness, bytes)?;
        // Offset changes alone (e.g. a tool result) should not republish text.
        let signature = (update.status, update.message.clone());
        if previous.as_ref() != Some(&signature) {
            let mut out = std::io::stdout().lock();
            let line = serde_json::to_string(&update).map_err(|e| e.to_string())?;
            if let Err(e) = writeln!(out, "{line}").and_then(|()| out.flush()) {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    return Ok(ExitCode::SUCCESS);
                }
                return Err(e.to_string());
            }
            previous = Some(signature);
        }
        if !follow {
            return Ok(ExitCode::SUCCESS);
        }
        std::thread::sleep(Duration::from_millis(interval_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn pi(text: &str) -> String {
        serde_json::json!({"type":"message","id":"a","message":{"role":"assistant","content":[{"type":"text","text":text}]}}).to_string() + "\n"
    }
    #[test]
    fn follows_partial_records_and_ignores_tool_results() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let mut reader = Reader::new();
        let line = pi("hello\nworld");
        file.write_all(&line.as_bytes()[..20]).unwrap();
        assert!(
            reader
                .poll(file.path(), "pi", 4096)
                .unwrap()
                .message
                .is_none()
        );
        file.write_all(&line.as_bytes()[20..]).unwrap();
        assert_eq!(
            reader
                .poll(file.path(), "pi", 4096)
                .unwrap()
                .message
                .unwrap()
                .text,
            "hello\nworld"
        );
        writeln!(
            file,
            "{{\"type\":\"message\",\"message\":{{\"role\":\"toolResult\"}}}}"
        )
        .unwrap();
        assert_eq!(
            reader
                .poll(file.path(), "pi", 4096)
                .unwrap()
                .message
                .unwrap()
                .text,
            "hello\nworld"
        );
    }
    #[test]
    fn bounded_suffix_and_exact_boundary() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let line = pi("latest");
        write!(file, "{}\n{}", "x".repeat(10000), line).unwrap();
        for budget in [line.len(), line.len() + 10] {
            assert_eq!(
                Reader::new()
                    .poll(file.path(), "pi", budget)
                    .unwrap()
                    .message
                    .unwrap()
                    .text,
                "latest"
            );
        }
        assert!(
            Reader::new()
                .poll(file.path(), "pi", 10)
                .unwrap()
                .message
                .is_none()
        );
    }
    #[test]
    fn truncation_clears_old_message() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(pi("old").as_bytes()).unwrap();
        let mut reader = Reader::new();
        assert!(
            reader
                .poll(file.path(), "pi", 4096)
                .unwrap()
                .message
                .is_some()
        );
        file.as_file().set_len(0).unwrap();
        assert!(
            reader
                .poll(file.path(), "pi", 4096)
                .unwrap()
                .message
                .is_none()
        );
    }
    #[test]
    fn oversized_record_is_skipped_and_next_message_recovers() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let mut reader = Reader::new();
        let long = pi(&"x".repeat(1000));
        for chunk in long.as_bytes().chunks(100) {
            file.write_all(chunk).unwrap();
            reader.poll(file.path(), "pi", 256).unwrap();
            assert!(reader.pending.len() <= 256);
        }
        assert!(reader.latest.is_none());
        file.write_all(pi("recovered").as_bytes()).unwrap();
        assert_eq!(
            reader
                .poll(file.path(), "pi", 256)
                .unwrap()
                .message
                .unwrap()
                .text,
            "recovered"
        );
    }
    #[test]
    fn replacement_file_resets_old_message() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("log.jsonl");
        std::fs::write(&path, pi("old")).unwrap();
        let mut reader = Reader::new();
        reader.poll(&path, "pi", 4096).unwrap();
        let replacement = directory.path().join("next.jsonl");
        std::fs::write(&replacement, pi("replacement")).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert_eq!(
            reader
                .poll(&path, "pi", 4096)
                .unwrap()
                .message
                .unwrap()
                .text,
            "replacement"
        );
    }
    #[test]
    fn utf8_fragment_is_not_decoded_until_record_completes() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let mut reader = Reader::new();
        let line = pi("hello 🦀");
        let split = line.find('🦀').unwrap() + 1;
        file.write_all(&line.as_bytes()[..split]).unwrap();
        assert!(
            reader
                .poll(file.path(), "pi", 4096)
                .unwrap()
                .message
                .is_none()
        );
        file.write_all(&line.as_bytes()[split..]).unwrap();
        assert_eq!(
            reader
                .poll(file.path(), "pi", 4096)
                .unwrap()
                .message
                .unwrap()
                .text,
            "hello 🦀"
        );
    }
    #[test]
    fn dozens_of_idle_mirrors_only_stat_and_append_reads_only_new_bytes() {
        let mut mirrors = Vec::new();
        let initial = pi(&"x".repeat(32 * 1024));
        for _ in 0..48 {
            let mut file = tempfile::NamedTempFile::new().unwrap();
            file.write_all(initial.as_bytes()).unwrap();
            let mut reader = Reader::new();
            assert!(reader.has_changed(file.path()).unwrap());
            reader.poll(file.path(), "pi", 65536).unwrap();
            mirrors.push((file, reader));
        }
        for _ in 0..64 {
            for (file, reader) in &mirrors {
                assert!(!reader.has_changed(file.path()).unwrap());
                assert_eq!(reader.file_opens, 1);
                assert_eq!(reader.bytes_read, initial.len());
            }
        }
        let appended = pi("new response");
        let (file, reader) = &mut mirrors[0];
        file.write_all(appended.as_bytes()).unwrap();
        assert!(reader.has_changed(file.path()).unwrap());
        assert_eq!(
            reader
                .poll(file.path(), "pi", 65536)
                .unwrap()
                .message
                .unwrap()
                .text,
            "new response"
        );
        assert_eq!(reader.file_opens, 2);
        assert_eq!(reader.bytes_read, initial.len() + appended.len());
        assert!(!reader.has_changed(file.path()).unwrap());
    }
    #[test]
    fn same_size_rewrite_rescans_and_removes_previous_message() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let old = pi("old");
        file.write_all(old.as_bytes()).unwrap();
        let mut reader = Reader::new();
        reader.poll(file.path(), "pi", 4096).unwrap();
        // Rewrite in place at exactly the same length, not an append.
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(pi("new").as_bytes()).unwrap();
        file.as_file()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1000)),
            )
            .unwrap();
        assert!(reader.has_changed(file.path()).unwrap());
        assert_eq!(
            reader
                .poll(file.path(), "pi", 4096)
                .unwrap()
                .message
                .unwrap()
                .text,
            "new"
        );
        assert!(!reader.has_changed(file.path()).unwrap());
        // A rewrite without any assistant record must not leave old text.
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(" ".repeat(old.len() - 1).as_bytes())
            .unwrap();
        file.write_all(b"\n").unwrap();
        file.as_file()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(2000)),
            )
            .unwrap();
        assert!(reader.has_changed(file.path()).unwrap());
        assert!(
            reader
                .poll(file.path(), "pi", 4096)
                .unwrap()
                .message
                .is_none()
        );
    }
    #[test]
    fn codex_uses_protocol_messages_not_duplicate_display_events() {
        assert!(
            extract(
                br#"{"type":"event_msg","payload":{"type":"agent_message","message":"duplicate"}}"#,
                "codex"
            )
            .is_none()
        );
        let msg = extract(br#"{"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"hello"}]}}"#, "codex").unwrap();
        assert_eq!(msg.text, "hello");
        assert_eq!(msg.phase.as_deref(), Some("final_answer"));
    }
}

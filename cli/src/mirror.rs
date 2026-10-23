//! Cross-harness mirror projection. This is deliberately separate from bounded
//! native JSONL tail: every poll loads the selected adapter's canonical snapshot.
use std::collections::HashSet;
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use sha2::{Digest, Sha256};
use txcript::common::{Block, Role};
use txcript::{Common, HarnessId, Transcript, local};

const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, PartialEq, Serialize)]
struct Message {
    // Common has no general native ID or trustworthy native timestamp. Never
    // mistake a regenerated index or fallback timestamp for source provenance.
    id: Option<String>,
    timestamp: Option<String>,
    role: &'static str,
    content: String,
}

#[derive(Serialize)]
struct Frame {
    schema_version: u8,
    session: super::ListedSession,
    projection: &'static str,
    order: &'static str,
    read_mode: &'static str,
    observed_at: String,
    status: &'static str,
    message: Option<Message>,
    change_token: String,
    message_count: usize,
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.starts_with('-')
        || id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '#'))
    {
        return Err(
            "provide a literal session ID or ID prefix, not a title, path, or range".into(),
        );
    }
    Ok(())
}

// Unlike view/continue, mirror resolution never accepts titles or collapses
// copies with the same native ID across different sources.
fn resolve(sessions: Vec<local::Session>, id: &str) -> Result<local::Session, String> {
    validate_id(id)?;
    let exact = sessions.iter().any(|s| s.meta.id == id);
    let mut seen = HashSet::new();
    let mut matches: Vec<_> = sessions
        .into_iter()
        .filter(|s| {
            (if exact {
                s.meta.id == id
            } else {
                s.meta.id.starts_with(id)
            }) && seen.insert(s.binding_key())
        })
        .collect();
    match matches.len() {
        0 => Err(format!(
            "no discovered session matches ID `{id}`; check harness scope, configured roots, and read permissions"
        )),
        1 => {
            let mut session = matches.remove(0);
            session
                .pin_path()
                .map_err(|error| format!("pin session source: {error}"))?;
            Ok(session)
        }
        _ => {
            let choices = matches
                .iter()
                .take(10)
                .map(|s| {
                    format!(
                        "{} {} {}",
                        s.harness,
                        s.meta.id,
                        super::style::scrub(&s.location())
                    )
                })
                .collect::<Vec<_>>()
                .join("\n  ");
            Err(format!(
                "ambiguous session ID `{id}`; add characters or --from (duplicate files remain ambiguous):\n  {choices}"
            ))
        }
    }
}

fn project(common: &Transcript<Common>) -> Result<(Option<Message>, usize, Option<usize>), String> {
    let mut latest = None;
    let mut count = 0;
    let mut ordinal = None;
    for message in &common.body {
        let blocks: Vec<_> = message
            .content
            .iter()
            .filter_map(|block| {
                if let Block::Text { text } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        if blocks.iter().all(|text| text.trim().is_empty()) {
            continue;
        }
        count += 1;
        if message.role == Role::Assistant {
            // Do not reject oversized old messages which are not the final
            // selection. Decode cost is explicitly not bounded by this cap.
            latest = Some(blocks);
            ordinal = Some(count);
        }
    }
    let latest = latest
        .map(|blocks| {
            let bytes = blocks
                .iter()
                .try_fold(0usize, |sum, block| sum.checked_add(block.len()))
                .and_then(|sum| sum.checked_add(blocks.len().saturating_sub(1).saturating_mul(2)))
                .ok_or_else(|| "latest assistant text size overflow".to_string())?;
            if bytes > MAX_TEXT_BYTES {
                return Err(format!(
                    "latest assistant text exceeds {MAX_TEXT_BYTES} bytes"
                ));
            }
            Ok(Message {
                id: None,
                timestamp: None,
                role: "assistant",
                content: blocks.join("\n\n"),
            })
        })
        .transpose()?;
    Ok((latest, count, ordinal))
}

fn change_token(
    binding: &str,
    message: Option<&Message>,
    ordinal: Option<usize>,
) -> Result<String, String> {
    // Serialize with explicit boundaries; unrelated transcript metadata,
    // generated timestamps, counts, and observed_at do not affect deduplication.
    let bytes =
        serde_json::to_vec(&(binding, message, ordinal)).map_err(|error| error.to_string())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn session_metadata(session: &local::Session, common: &Transcript<Common>) -> super::ListedSession {
    let path = session.source_path();
    let updated_at = path
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .map(chrono::DateTime::<Utc>::from);
    super::ListedSession {
        harness: session.harness.to_string(),
        id: session.meta.id.clone(),
        started_at: session.meta.timestamp,
        updated_at: updated_at.or(session.updated_at),
        cwd: common.meta.cwd.clone(),
        title: common.meta.title.clone(),
        model: common.meta.model.clone(),
        path: path.map(|path| path.display().to_string()),
    }
}

fn snapshot(session: &local::Session, binding: &str) -> Result<Frame, String> {
    let common = session
        .read_for_mirror()
        .map_err(|error| format!("read {} {}: {error}", session.harness, session.meta.id))?;
    let (message, message_count, ordinal) = project(&common)?;
    let change_token = change_token(binding, message.as_ref(), ordinal)?;
    Ok(Frame {
        schema_version: 1,
        session: session_metadata(session, &common),
        projection: "common_text",
        order: "normalized",
        read_mode: "full_snapshot",
        observed_at: Utc::now().to_rfc3339(),
        status: if message.is_some() {
            "message"
        } else {
            "no_message"
        },
        message,
        change_token,
        message_count,
    })
}

fn check_scope(from: Option<HarnessId>) -> Result<(), String> {
    if from.is_some_and(is_remote) {
        return Err(
            "show/observe/messages support local-disk sessions only; cloud sources are unsupported"
                .into(),
        );
    }
    if from == Some(HarnessId::Simple) {
        return Err(
            "simple is an interchange document format, not a discoverable native session store"
                .into(),
        );
    }
    Ok(())
}

fn is_remote(harness: HarnessId) -> bool {
    matches!(
        harness,
        HarnessId::ClaudeChat | HarnessId::CoworkRemote | HarnessId::ChatGpt
    )
}

fn escape_text(text: &str) -> String {
    // Captured text is inert even when stdout is a terminal.
    text.chars()
        .flat_map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

fn write_frame(frame: &Frame, format: &str, out: &mut impl Write) -> Result<(), String> {
    let output = match format {
        "json" => serde_json::to_string(frame).map_err(|error| error.to_string())?,
        "human" => {
            let text = frame
                .message
                .as_ref()
                .map_or("No persisted assistant text.", |message| {
                    message.content.as_str()
                });
            let text = escape_text(text);
            format!(
                "{} {}\n{text}",
                frame.session.harness,
                super::style::scrub(&frame.session.id)
            )
        }
        _ => return Err("--format must be human or json".into()),
    };
    if output.len() > MAX_FRAME_BYTES {
        return Err(format!("mirror output exceeds {MAX_FRAME_BYTES} bytes"));
    }
    writeln!(out, "{output}")
        .and_then(|()| out.flush())
        .map_err(|error| error.to_string())
}

pub(crate) fn run(
    id: &str,
    from: Option<HarnessId>,
    format: &str,
    follow: bool,
    interval_ms: u64,
) -> Result<ExitCode, String> {
    validate_id(id)?;
    if !matches!(format, "human" | "json") {
        return Err("--format must be human or json".into());
    }
    if interval_ms < 100 {
        return Err("--interval-ms must be at least 100".into());
    }
    check_scope(from)?;
    let session = resolve(super::discover_with_spinner(from)?, id)?;
    let binding = session.binding_key();
    let mut previous = None;
    loop {
        let frame = snapshot(&session, &binding)?;
        if previous.as_ref() != Some(&frame.change_token) {
            // Preserve I/O error kind here so broken pipes are quiet.
            let mut bytes = Vec::new();
            write_frame(&frame, format, &mut bytes)?;
            let mut out = std::io::stdout().lock();
            if let Err(error) = out.write_all(&bytes).and_then(|()| out.flush()) {
                if error.kind() == std::io::ErrorKind::BrokenPipe {
                    return Ok(ExitCode::SUCCESS);
                }
                return Err(error.to_string());
            }
            previous = Some(frame.change_token);
        }
        if !follow {
            return Ok(ExitCode::SUCCESS);
        }
        std::thread::sleep(Duration::from_millis(interval_ms));
    }
}

#[derive(Serialize)]
struct HistoryFrame {
    schema_version: u8,
    session: super::ListedSession,
    projection: &'static str,
    order: &'static str,
    read_mode: &'static str,
    observed_at: String,
    messages: Vec<Message>,
    has_earlier: bool,
}

fn project_history(
    common: &Transcript<Common>,
    role: Option<&str>,
    limit: usize,
) -> Result<(Vec<Message>, bool), String> {
    // Common is already loaded. Select borrowed blocks from the END before
    // allocating/capping text, so large evicted history cannot abort latest-N.
    let mut selected: Vec<_> = common
        .body
        .iter()
        .rev()
        .filter_map(|message| {
            let name = match message.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            if role.is_some_and(|role| role != name) {
                return None;
            }
            let blocks: Vec<_> = message
                .content
                .iter()
                .filter_map(|block| {
                    if let Block::Text { text } = block {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            if blocks.iter().all(|text| text.trim().is_empty()) {
                None
            } else {
                Some((name, blocks))
            }
        })
        .take(limit + 1)
        .collect();
    let has_earlier = selected.len() > limit;
    selected.truncate(limit);
    selected.reverse();
    let total = selected
        .iter()
        .try_fold(0usize, |sum, (_, blocks)| {
            blocks
                .iter()
                .try_fold(sum, |sum, block| sum.checked_add(block.len()))
                .and_then(|sum| sum.checked_add(blocks.len().saturating_sub(1).saturating_mul(2)))
        })
        .ok_or_else(|| "selected history text size overflow".to_string())?;
    if total > MAX_TEXT_BYTES {
        return Err(format!(
            "selected history text exceeds {MAX_TEXT_BYTES} bytes; use a smaller --limit"
        ));
    }
    let messages = selected
        .into_iter()
        .map(|(role, blocks)| Message {
            id: None,
            timestamp: None,
            role,
            content: blocks.join("\n\n"),
        })
        .collect();
    Ok((messages, has_earlier))
}

pub(crate) fn run_history(
    id: &str,
    from: Option<HarnessId>,
    role: Option<&str>,
    limit: usize,
    format: &str,
) -> Result<ExitCode, String> {
    validate_id(id)?;
    if !(1..=10_000).contains(&limit) {
        return Err("--limit must be between 1 and 10000".into());
    }
    if role.is_some_and(|role| !matches!(role, "user" | "assistant")) {
        return Err("--role must be user or assistant".into());
    }
    if !matches!(format, "human" | "json") {
        return Err("--format must be human or json".into());
    }
    check_scope(from)?;
    let session = resolve(super::discover_with_spinner(from)?, id)?;
    let common = session
        .read_for_mirror()
        .map_err(|error| format!("read {} {}: {error}", session.harness, session.meta.id))?;
    let (messages, has_earlier) = project_history(&common, role, limit)?;
    let frame = HistoryFrame {
        schema_version: 1,
        session: session_metadata(&session, &common),
        projection: "common_text",
        order: "normalized",
        read_mode: "full_snapshot",
        observed_at: Utc::now().to_rfc3339(),
        messages,
        has_earlier,
    };
    let output = if format == "json" {
        serde_json::to_string(&frame).map_err(|error| error.to_string())?
    } else {
        frame
            .messages
            .iter()
            .map(|message| format!("[{}]\n{}", message.role, escape_text(&message.content)))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    if output.len() > MAX_FRAME_BYTES {
        return Err(format!("history output exceeds {MAX_FRAME_BYTES} bytes"));
    }
    let mut out = std::io::stdout().lock();
    if let Err(error) = writeln!(out, "{output}").and_then(|()| out.flush())
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error.to_string());
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use txcript::harness::simple::Simple;
    use txcript::{Codec, TextCodec};

    fn conversation() -> Transcript<Common> {
        Simple::to_common(&Simple::from_text(r#"{"messages":[{"role":"user","content":"question"},{"role":"assistant","content":"answer"}]}"#).unwrap()).unwrap()
    }

    #[test]
    fn projection_counts_text_and_ignores_tools_reasoning_and_unrelated_timestamps() {
        let mut common = conversation();
        let (message, count, ordinal) = project(&common).unwrap();
        assert_eq!(count, 2);
        assert_eq!(message.as_ref().unwrap().content, "answer");
        assert!(message.as_ref().unwrap().id.is_none());
        let token = change_token("source", message.as_ref(), ordinal).unwrap();
        common.body[1].timestamp = Utc::now();
        common.body[1].content.push(Block::Thinking {
            text: "private".into(),
            signature: None,
            encrypted: None,
        });
        assert_eq!(
            token,
            change_token("source", project(&common).unwrap().0.as_ref(), ordinal).unwrap()
        );
        common.body[1].content.push(Block::Text {
            text: "changed".into(),
        });
        assert_ne!(
            token,
            change_token("source", project(&common).unwrap().0.as_ref(), ordinal).unwrap()
        );
        assert_ne!(
            token,
            change_token("other-source", message.as_ref(), ordinal).unwrap()
        );
    }

    #[test]
    fn identical_new_assistant_messages_have_distinct_observation_tokens() {
        let mut common = conversation();
        let (message, _, ordinal) = project(&common).unwrap();
        let token = change_token("source", message.as_ref(), ordinal).unwrap();
        common.body.push(common.body[0].clone());
        let (message, _, ordinal) = project(&common).unwrap();
        assert_eq!(
            token,
            change_token("source", message.as_ref(), ordinal).unwrap()
        );
        common.body.push(common.body[1].clone());
        let (message, _, ordinal) = project(&common).unwrap();
        assert_ne!(
            token,
            change_token("source", message.as_ref(), ordinal).unwrap()
        );
    }

    #[test]
    fn only_final_selected_text_is_capped() {
        let mut common = conversation();
        let latest = common.body[1].clone();
        common.body[1].content = vec![Block::Text {
            text: "x".repeat(MAX_TEXT_BYTES + 1),
        }];
        assert!(project(&common).is_err());
        common.body.push(latest);
        assert_eq!(project(&common).unwrap().0.unwrap().content, "answer");
    }

    #[test]
    fn history_filters_before_limit_and_preserves_normalized_order() {
        let mut common = conversation();
        let mut next_user = common.body[0].clone();
        next_user.content = vec![Block::Text {
            text: "next question".into(),
        }];
        common.body.push(next_user);
        let (history, earlier) = project_history(&common, Some("assistant"), 1).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "answer");
        assert!(!earlier);
        let (history, earlier) = project_history(&common, None, 2).unwrap();
        assert!(earlier);
        assert_eq!(history[0].role, "assistant");
        assert_eq!(history[1].content, "next question");
        for message in history {
            assert!(message.id.is_none() && message.timestamp.is_none());
        }
        common.body.clear();
        let (history, earlier) = project_history(&common, None, 2).unwrap();
        assert!(history.is_empty() && !earlier);
    }

    #[test]
    fn history_caps_only_final_selected_text() {
        let mut common = conversation();
        let answer = common.body[1].clone();
        common.body[1].content = vec![Block::Text {
            text: "x".repeat(MAX_TEXT_BYTES + 1),
        }];
        assert!(project_history(&common, Some("assistant"), 1).is_err());
        common.body.push(answer);
        let (history, earlier) = project_history(&common, Some("assistant"), 1).unwrap();
        assert!(earlier);
        assert_eq!(history[0].content, "answer");
        assert!(project_history(&common, Some("assistant"), 2).is_err());
    }

    #[test]
    fn human_output_escapes_terminal_controls_but_json_preserves_content() {
        let frame = Frame {
            schema_version: 1,
            session: super::super::ListedSession {
                harness: "pi".into(),
                id: "id".into(),
                started_at: Utc::now(),
                updated_at: None,
                cwd: None,
                title: None,
                model: None,
                path: None,
            },
            projection: "common_text",
            order: "normalized",
            read_mode: "full_snapshot",
            observed_at: "now".into(),
            status: "message",
            message: Some(Message {
                id: None,
                timestamp: None,
                role: "assistant",
                content: "a\n\t\x1b]52;clipboard\x07".into(),
            }),
            change_token: "token".into(),
            message_count: 1,
        };
        let mut bytes = Vec::new();
        write_frame(&frame, "human", &mut bytes).unwrap();
        assert!(!bytes.contains(&0x1b));
        assert!(!bytes.contains(&7));
        assert!(bytes.contains(&b'\n'));
        bytes.clear();
        write_frame(&frame, "json", &mut bytes).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["message"]["content"], frame.message.unwrap().content);
    }

    #[test]
    fn remote_codecs_use_the_same_projection_without_network_access() {
        use txcript::harness::{
            chatgpt::ChatGpt, claude_chat::ClaudeChat, cowork_remote::CoworkRemote,
        };
        let claude = serde_json::json!({"uuid":"11111111-1111-4111-8111-111111111111", "created_at":"2026-01-01T00:00:00Z",
            "chat_messages":[{"uuid":"a","sender":"assistant","content":[{"type":"text","text":"answer"}]}]});
        let chatgpt = serde_json::json!({"conversation_id":"11111111-1111-4111-8111-111111111111","create_time":1,"current_node":"n",
            "mapping":{"n":{"id":"n","parent":null,"children":[],"message":{"id":"a","author":{"role":"assistant"},"create_time":1,"content":{"content_type":"text","parts":["answer"]}}}}});
        let cowork = serde_json::json!({"session":{"session":{"id":"cse_test01","created_at":"2026-01-01T00:00:00Z","tags":["cowork-remote"]}},
            "events":[{"event_id":"e1","payload":{"type":"assistant","uuid":"a","message":{"role":"assistant","content":[{"type":"text","text":"answer"}]}}}]});
        for common in [
            ClaudeChat::to_common(&ClaudeChat::from_text(&claude.to_string()).unwrap()).unwrap(),
            ChatGpt::to_common(&ChatGpt::from_text(&chatgpt.to_string()).unwrap()).unwrap(),
            CoworkRemote::to_common(&CoworkRemote::from_text(&cowork.to_string()).unwrap())
                .unwrap(),
        ] {
            let (history, earlier) = project_history(&common, None, 50).unwrap();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].content, "answer");
            assert!(!earlier);
            let (message, count, _) = project(&common).unwrap();
            assert_eq!(count, 1);
            assert_eq!(message.unwrap().content, "answer");
        }
        for remote in [
            HarnessId::ClaudeChat,
            HarnessId::ChatGpt,
            HarnessId::CoworkRemote,
        ] {
            assert!(is_remote(remote));
        }
        assert!(!is_remote(HarnessId::Pi));
    }

    #[test]
    fn id_validation_is_literal_only() {
        for invalid in ["", "-id", "a b", "abc#1", "a/b", "a\\b", "a\x1b"] {
            assert!(validate_id(invalid).is_err());
        }
        assert!(validate_id("abc-123").is_ok());
    }
}

# Bounded latest-message observation

`tail` is a read-only JSONL observer for Pi and Codex. It does not import,
convert, resume, or modify a session. Pass the exact source path so discovery
is not repeated on each poll.

```sh
txcript tail /absolute/session.jsonl --from pi --bytes 65536
txcript tail /absolute/rollout.jsonl --from codex --bytes 65536 --follow
```

Output is always JSON (one object for a snapshot, changed objects as JSONL for
follow). `--interval-ms` defaults to 1000 and must be at least 100.

```json
{"version":1,"harness":"pi","offset":1234,"status":"message","message":{"id":"entry-id","timestamp":"2026-10-21T15:00:00Z","text":"Latest persisted assistant text","stop_reason":"stop","phase":null}}
```

If no suitable complete record is available, `status` is `not_found_in_window`
and `message` is null. This does not mean the session has no assistant messages.
The `offset` belongs to the poll that emitted the update; it is not continuously
republished just because unrelated records were appended.

## Bounds and following

- Initial observation seeks to the final `--bytes` bytes (default 64 KiB).
- If the window starts inside a record, its leading fragment is discarded.
  A window starting exactly on a line boundary keeps that complete record.
- Only newline-terminated JSON records are parsed. A trailing partial record
  is buffered until the next poll completes it, including split UTF-8 bytes.
- Idle polls compare path metadata (file identity, length, and modification
  timestamp; Unix also checks ctime). If unchanged, the log is not opened,
  read, decoded, or cloned into a new message. Only a metadata stat is needed.
- Following reads only appended bytes. Each poll and buffered record is bounded
  by `--bytes`; a large append can skip intervening records.
- A record larger than the bound is skipped until its newline. Increase the
  window explicitly if needed; no automatic full-file fallback occurs.
- Bounds are 1 byte through 64 MiB. Memory is O(bound), including decoding and
  retained latest text; it is not proportional to session history.
- Observed inode replacement, size shrink, or same-size metadata change resets
  the cursor and previous message and rescans a bounded suffix. Same-inode
  truncate-and-regrow past the old offset between polls cannot always be
  distinguished from an append. Changes preserving every checked metadata
  field may be missed; this is not a filesystem journal.
- Source-open/read failure ends the command with an error. A supervising
  producer should show stale/error state rather than silently restart.
- Broken stdout pipes end quietly.

## Semantics

Pi reads assistant `message.content` text blocks. Codex reads assistant
`response_item` message content, ignoring duplicate display events. Reasoning,
tool calls/results, and user messages are not emitted. Available native message
ID, timestamp, stop reason and phase are retained; absent fields are null.

The output is **latest observed persisted assistant text**, not an authoritative
active-branch reconstruction or a promise that the agent's entire turn is
complete. Commentary can precede tool use. Native phase/stop reason are hints,
not synthesized completion guarantees. Log persistence timing controls whether
updates appear during generation or only after a message completes.

The initial bounded window need not include session headers, model metadata,
turn starts, or branch ancestry. This deliberately bypasses full-transcript
aggregation. No prior history is replayed: each poll retains only the most
recent matching message. Subsequent unchanged polls emit nothing.

Consumers displaying untrusted text should keep it inert; do not automatically
execute Markdown actions or follow instructions contained in the captured text.

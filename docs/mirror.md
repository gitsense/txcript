# Generic session details and mirrors

The `gitsense.sessions.mirror.v1` contract (first available in the GitSense
`0.15.0-gitsense.2` fork) projects the latest persisted assistant text from
**every readable local-disk session adapter enabled in the build**. It is separate
from the optimized, Pi/Codex-only [bounded tail](tail.md).

```sh
txcript show <session-id-or-prefix> --format json
txcript observe <session-id-or-prefix> --format json --follow
# Optional scope makes discovery cheaper and disambiguates different harnesses:
txcript show <session-id-or-prefix> --from claude_code --format json
txcript observe <session-id-or-prefix> --from opencode --follow --format json
```

`show` and a non-following `observe` return the same snapshot schema. Human
output defaults to the harness/session ID and latest text (terminal controls
escaped). JSON preserves the original text. Follow emits an initial snapshot,
then changed observations as flushed JSONL. There is no history replay or token
streaming. The gsc wrapper can expose these operations as `sessions show` and
`sessions tail`; consumers need not implement per-format codecs.

## Binding and access

Resolution matches literal full IDs first, then unambiguous ID prefixes. Titles,
paths, and `#ranges` are not accepted. Identical native IDs in different files,
or harnesses are ambiguous, not first-copy wins.
Canonical aliases of the same backing file are deduplicated. More characters
cannot resolve two copies of the same ID: narrow the scope or remove ambiguity
in the source store. Discovery's existing tolerant omission of unreadable local
stores still applies; no match is not proof of a complete filesystem search.

An observer resolves once and holds that session's native locator throughout
its lifetime. File locators are canonicalized at startup. It does not re-run
prefix discovery or pick a newer session on each tick. Every loaded snapshot's
native session ID must still match. A missing source or mismatched identity is
an error and exits nonzero. This is **not atomic filesystem authentication**:
read consistency, symlink handling inside bundles, database snapshots, and
native identity availability follow each adapter's existing guarantees. Store
configuration and credentials belong to the observing process environment;
there is no externally reusable source-binding token or authorization grant.

These APIs support **local disk only**: JSONL/JSON files, directory bundles,
compressed local rollouts, and local SQLite databases. Claude Chat,
cloud Cowork (`cowork_remote`), and ChatGPT scopes are rejected **before any
store construction, credential access, or network discovery**. Other txcript
operations retain their existing explicit cloud support; it is not part of the
mirror/history contracts. Grok Bot mirrors read persisted JSONL or SQLite
locally and **never invoke gateway `openAgent`**; a gateway-only agent without
local persisted conversation returns an error.

Simple is an interchange document format, not a discoverable native agent
session, and is explicitly rejected by these ID-based commands.

## Frame schema

```json
{
  "schema_version": 1,
  "session": {
    "harness": "claude_code",
    "id": "11111111-1111-4111-8111-111111111111",
    "started_at": "2026-01-01T00:00:00Z",
    "updated_at": "2026-01-01T00:00:02Z",
    "path": "/absolute/session.jsonl"
  },
  "projection": "common_text",
  "order": "normalized",
  "read_mode": "full_snapshot",
  "observed_at": "2026-01-01T00:00:03Z",
  "status": "message",
  "message": {
    "id": null,
    "timestamp": null,
    "role": "assistant",
    "content": "Latest persisted assistant text"
  },
  "change_token": "sha256:...",
  "message_count": 2
}
```

All top-level fields shown are required. `session` follows the discovery JSON
metadata schema: required canonical `harness`, full `id`, and `started_at`;
optional `updated_at`, `cwd`, `title`, `model`, and `path`. A path can denote a
file, database, record, or directory bundle; database-ID sessions can
omit it. Start/update times come from adapter metadata, not an assertion of
native message timestamp provenance. Some adapters backfill missing start
times, and some stores expose no current per-session modification time.

`schema_version` is integer `1`; the three semantic labels have the exact
values shown. `observed_at` is an RFC3339 local observation time. `status` is:

- `message`: `message` is an object with all four keys shown. `role` is
  `assistant`, `content` is nonempty text, and `id` and `timestamp` are nullable
  strings. This first implementation sets both native provenance fields to
  null because Common does not retain them reliably across adapters.
- `no_message`: `message` is null. There is no nonempty assistant text in the
  loaded normalized transcript. This is distinct from a read failure.

`message_count` counts normalized user/assistant messages containing nonempty
text, **not raw log records, tools, reasoning, or complete agent turns**. Block
text is joined with blank lines; tool results and reasoning are not promoted to
assistant text. Adapter-specific message splitting/merging and branch rules
apply. Pi includes file-order branches; Claude Chat and ChatGPT normalize a
selected branch. No universal active-branch or raw-file-order guarantee is
implied. Commentary is eligible; this is not a final-answer/completed-turn
filter.

`change_token` is an opaque nonempty comparison value, not a native message ID,
security signature, byte offset, or resume cursor. It includes source binding,
latest content, and its normalized text-message ordinal, so another assistant
message with identical text still changes the token. Unrelated user/tool
appends after the latest assistant do not change it. Generated fallback
message timestamps, `observed_at`, counts, and other metadata do not affect it.
Do not parse the current SHA-256 representation. An observer emits only when
this token changes, so message counts and metadata are snapshots, not separately
streamed updates. A changed projection after rewrite may produce a new token
without representing a newly generated agent message.

Consumers require the recognized handshake contract and frame schema, validate
required fields, and tolerate additional fields. Never treat read failure as
successful `no_message`. The supervising consumer owns authorization, source
ownership/lifetime, stale/error presentation, cancellation, and restart/backoff.
Stdout carries frames only; errors go to stderr and terminate the process.
Broken stdout pipes exit quietly when detected during a write.

## Polling cost and limits

This deliberately favors coverage and correctness over incremental speed.
Every poll reloads the fixed session using its native adapter, converts it to
Common, then selects assistant text. This catches existing SQLite part edits,
including WAL-only writes; it does not rely on incomplete mtime or
new-message-only fingerprints. There is no implicit fallback to a different
session or network discovery on every tick. Native reads may omit incomplete
trailing JSONL records according to their existing parser behavior; this is
not a strict corruption validator or a transactional snapshot across multiple
files. A backend parse/access error is fatal, not silently retried as empty.

- Polling interval defaults to 1000 milliseconds; minimum 100.
- Cloud reads and remote polling are not supported by these commands.
- The selected latest assistant text is capped at 16 MiB and serialized frame
  output at 32 MiB. Oversized output fails explicitly; old unselected text does
  not itself trip the selected-text cap.
- These are **output bounds, not input/CPU/memory bounds**. Full transcript
  reads, decompression, database queries, normalization and attachment loading
  can cost much more than the selected text. Very large or malformed sources
  need process-level resource/time supervision. Polling latency cannot be
  promised uniformly across file, SQLite and directory stores.

Use `tail` for explicitly bounded native Pi/Codex suffix observation when its
file-order/window semantics are desired. `observe` never silently changes to
that different projection based on harness. Future optimizations must preserve
this normalized contract or advertise a distinct mode/contract.

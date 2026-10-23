# Normalized text history

`gitsense.sessions.history.v1` is an additive contract first available in the
GitSense `0.15.0-gitsense.3` fork. Check it in the existing one-shot version
handshake, independently of the mirror contract.

```sh
txcript messages <literal-session-id-or-prefix> --format json
txcript messages <id-or-prefix> --from claude_code --role assistant --limit 20 --format json
```

This uses the same strict ID-only resolver, fixed native source, read policy,
local-disk backend coverage, no-network policy, and normalized text projection
as [show/observe](mirror.md). It never uses the title/range/export resolver or
implements an independent set of native codecs. It reads the source once,
through the existing adapter, and returns the latest matching text messages.
Simple is an interchange document format, not a discoverable native session.
Grok Bot never invokes its gateway. Cloud scopes are rejected before any store
construction, credential access, or network discovery, even when `--from` is
explicit. Files, directory bundles, compressed rollouts, and local databases
are supported.

## Selection and framing

- `--role` is optional; accepted values are `user` and `assistant`.
- The role filter is applied **before** selecting the latest `--limit` matches.
- `--limit` (short `-n`) defaults to 50 and accepts 1 through 10000.
- The selected messages are returned oldest-to-newest in the adapter's
  **normalized conversation order**, not guaranteed native record order.
- Only Common text blocks are included, joined by blank lines. Messages with
  no non-whitespace text are excluded. Tool calls/results, reasoning, images,
  and artifacts are not promoted into user/assistant text.
- Native branch-selection and message splitting/merging rules apply. This is
  not a full-fidelity export, replay log, or universally active-branch view.

One JSON object followed by a newline is written to stdout and flushed:

```json
{
  "schema_version": 1,
  "session": {
    "harness": "pi",
    "id": "11111111-1111-4111-8111-111111111111",
    "started_at": "2026-01-01T00:00:00Z"
  },
  "projection": "common_text",
  "order": "normalized",
  "read_mode": "full_snapshot",
  "observed_at": "2026-01-01T00:00:03Z",
  "messages": [
    {"id": null, "timestamp": null, "role": "user", "content": "Question"},
    {"id": null, "timestamp": null, "role": "assistant", "content": "Answer"}
  ],
  "has_earlier": false
}
```

All top-level fields shown are required. `schema_version` is integer 1;
`projection`, `order`, and `read_mode` have the exact string values shown.
`observed_at` is an RFC3339 local observation time. `session` uses the same
required/optional discovery metadata as the mirror frame: required harness,
full ID and start time; optional updated time, working directory, title,
model, and source path. Paths can be absent for database-ID backends.

Every message has all four required keys. Role is `user` or `assistant`, and
content is nonempty text. Native `id` and `timestamp` are nullable strings;
they are currently always null because Common does not preserve trustworthy
native provenance across all adapters. Indices are not fabricated message IDs.
`has_earlier` is true if additional **matching normalized text messages** were
omitted by the limit, not simply if earlier native records exist.

No matching messages is a successful `messages: []`, `has_earlier: false`.
There is deliberately no separate history `status` field. Missing/ambiguous
source, access, parse, identity mismatch, and size failures exit nonzero with
stderr diagnostics, never an empty successful result. Consumers must validate
the required frame shape while allowing additive fields. There is no cursor
or follow option on history. Human output defaults to role-labelled text with
terminal controls escaped; JSON preserves text. Broken stdout pipes exit
quietly when detected.

## Limits and cost

The **final selected text** has a combined 16 MiB budget. Earlier evicted or
role-excluded text does not trigger that output budget. The complete serialized
frame is capped at 32 MiB. Over-budget selections fail explicitly; reduce the
limit rather than assuming silent truncation. Common normalization can alter
message boundaries, so count limits apply to normalized text messages.

These are output bounds only. Loading and normalizing the complete source can
require much more disk, decompression, memory, SQLite, and local attachment
loading than the output suggests. Input/read-consistency and
partial-record behavior follow the adapter, as documented for mirrors. Use
process-level deadlines/resource supervision for large or untrusted sources;
this API does not promise bounded input work. Prefer `observe --follow` for
last-assistant mirrors rather than repeatedly retrieving full history.

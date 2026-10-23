# GitSense sessions CLI contract

`gitsense.sessions.v1` identifies the interoperable behavior below. It is not
an assertion that every known harness supports every operation, a guarantee
of binary authenticity, or a package-version range. A compatible release may
advertise this same contract even when its package version changes. Breaking
these guarantees requires a new contract identifier; additive fields and
additional independent contracts do not.

## Compatibility handshake

```sh
txcript version --format json
```

Exactly one JSON object followed by a newline is written to stdout, with no
session discovery, remote access, or installed-tool probes:

```json
{
  "schema_version": 1,
  "version": "0.15.0-gitsense.3",
  "distribution": "gitsense",
  "git_commit": null,
  "contracts": ["gitsense.sessions.v1", "gitsense.sessions.mirror.v1", "gitsense.sessions.history.v1"]
}
```

All five fields are required. `schema_version` is the integer `1`; `version`
and `distribution` are nonempty strings; `git_commit` is a full hexadecimal
Git object ID or null; `contracts` is an array of strings. Consumers must
require a recognized schema version and exact contract membership, while
allowing unknown additional fields and contracts. Package version and
source revision are diagnostics, not the compatibility gate. `--version`
continues to print `txcript <package-version>`; `version` defaults to a
human-readable diagnostic summary.

For verified clean-source builds, the builder can set `TXCRIPT_GIT_COMMIT`
at compile time to the full source commit ID. Without this build metadata
(or if it is not a full hexadecimal object ID), `git_commit` is null. The
binary never shells out to Git at runtime. Do not label uncommitted source
changes with their parent revision. This field does not replace artifact
checksums or signatures.

Check this handshake once per selected executable/process, not once per
feature or polling tick. Invoke the same resolved executable afterward.
Long-lived consumers should invalidate their cached check when the executable
identity or metadata changes. Missing command, invalid JSON, unsupported
schema/contract, timeout, and nonzero exit should produce an actionable
compatibility error rather than an implicit fallback. Bound subprocess output
and diagnostics. Continue validating actual output frames: that is protocol
validation, not feature probing.

## Additive generic mirror contract

`gitsense.sessions.mirror.v1` additionally guarantees strict ID-based `show`
and `observe` across the readable local-disk native adapters in the build. It does not
change the bounded Pi/Codex tail-v1 meanings below. See the [generic mirror
contract](mirror.md) for frames, native metadata nullability, normalized order,
full-snapshot polling cost, cloud rejection, and source-binding limits.
Consumers check this additional contract in the same handshake, not by probing
individual flags or assuming a package version range.

`gitsense.sessions.history.v1` additionally guarantees normalized text history
through `messages`, using the same strict resolver and read policy. See the
[history contract](history.md) for role-before-limit selection, chronological
normalized order, nullable native metadata, framing, and final-output budgets.
It is independent of the mirror contract and does not add history guarantees
to `gitsense.sessions.v1`.

## Harness identifiers

```sh
txcript harnesses --format json
```

Stdout is a JSON array of unique canonical identifier strings followed by a
newline, generated from `HarnessId::ALL`. For example, `pi` and `codex` appear.
New identifiers may be added; do not depend on array order or length. Human
output defaults to one identifier per line.

This is a registry, not detection of installed tools, enabled Cargo features,
local session availability, or tail support. In this contract, tail supports
only `pi` and `codex`.

## Session discovery

```sh
txcript list --format json
txcript list --from pi --format json
txcript list --from codex --format json
```

Stdout is one JSON array followed by a newline. Each entry has:

| Field | Type | Meaning |
|---|---|---|
| `harness` | string, required | Canonical harness identifier |
| `id` | string, required | Discovered full native session ID |
| `started_at` | timestamp string, required | Session start time |
| `updated_at` | timestamp string, optional | Backing-store modification time |
| `cwd` | string, optional | Recorded working directory |
| `title` | string, optional | Available title |
| `model` | string, optional | Available model |
| `path` | string, optional generally | Backing path for file-backed sessions |

Discovered Pi/Codex entries include `path`. Optional unavailable fields are
omitted, not serialized as null. Consumers must tolerate extra fields and
unknown harnesses. Entries are ordered newest session start first, not newest
file modification first. A selected local `--from` scans only that harness's
store. With no `--from`, supported local stores are scanned; live web sources
are never implicitly contacted. Build features affect available backends.

Pi roots honor `PI_CODING_AGENT_SESSION_DIR`, then
`PI_CODING_AGENT_DIR/sessions`, then `~/.pi/agent/sessions`. Codex uses
`CODEX_HOME/sessions` or `~/.codex/sessions`. Relative root overrides can yield
relative paths; consumers should normalize paths deliberately. Missing or
unreadable local stores/files may be skipped. Discovery is not an exhaustive
filesystem journal and does not authenticate a session against a trusted
external identity. Pi discovery still reads complete candidate logs for
metadata; there is no bounded-discovery latency guarantee.

There is no list ID-prefix argument or dedicated resolve command. Wrappers
must match IDs and reject ambiguous harness/path bindings themselves. Do not
truncate the candidate list with `--limit` or unrelated filters before checking
ambiguity. Do not infer native identity solely from a filename. A returned
path may change before a later open; stronger identity/authorization guarantees
belong to the caller or a future explicit identity-binding protocol.

## Latest assistant observation

```sh
txcript tail <exact-path> --from pi --bytes 65536
txcript tail <exact-path> --from codex --bytes 65536 --follow --interval-ms 1000
```

Snapshot mode emits one JSON object followed by a newline. Follow emits an
initial snapshot and then newline-framed changed-message objects. Each update
is flushed; stdout contains no human status messages. Idle polls and changes
to offset alone do not emit frames. The version-1 frame is:

```json
{"version":1,"harness":"codex","offset":1234,"status":"message","message":{"id":null,"timestamp":"2026-01-01T00:00:01Z","text":"Answer","stop_reason":null,"phase":"final_answer"}}
```

All shown keys are required. `version` is integer `1`; `harness` matches the
selected `pi` or `codex`; `offset` is a nonnegative byte position; `status` is
`message` or `not_found_in_window`. With `message`, the message object contains
string `text` and nullable-string `id`, `timestamp`, `stop_reason`, and `phase`.
With `not_found_in_window`, `message` is null. That status does not prove there
are no assistant messages elsewhere in history. Consumers should accept
additional fields while validating required fields and known frame versions.

The output is latest **observed persisted assistant text** in file order:
Pi assistant text blocks, or Codex assistant response-item text blocks, not
duplicate display events, tool results, or reasoning. It is not every message,
token streaming, active-branch reconstruction, or a completed-turn guarantee.
The offset is observational, **not a resumable cursor**; no history/replay API
is implied. Fast appends may skip intermediate records, and a follower retains
its last matching message across unrelated appends.

`--bytes` defaults to 65536 and accepts 1 through 67108864. Initial reads seek
to a bounded suffix; follow reads new bytes with the same per-poll bound and
bounded record buffering. Leading partial and oversized records are skipped;
trailing partial records wait for a terminating newline. `--interval-ms`
defaults to 1000, with a minimum of 100. Observed file replacement, shrink, or
same-size metadata rewrite resets the bounded reader, not a session identity
binding. It follows the configured path, not a newly discovered session.

Source/open/read failures exit nonzero with diagnostics on stderr; restart,
backoff, ownership, and stale/error presentation belong to the consumer.
Broken stdout pipes terminate quietly. See [tail semantics and limits](tail.md)
for concurrency, metadata-detection, and memory-bound details.

# animus-subject-requirements

A requirements subject backend plugin for [Animus](https://github.com/launchapp-dev/animus-cli).

> **Status:** Under construction — landing in Animus v0.4.0.

## What this is

Animus v0.4.0 makes subjects (units of dispatchable work) pluggable. This repository ships `animus-subject-requirements`, a standalone stdio plugin that stores requirements as structured `.md` files on disk and surfaces them as Animus subjects of `kind = "requirement"`.

Requirements have a slightly different lifecycle than tasks:

- They go through a **refinement loop** (drafted → refined → approved) before any task is spawned.
- They typically **outlive the tasks they unlock** — the requirement stays as the durable record long after `TASK-0042` is closed.
- They cross-reference the tasks and workflows they spawn, so a workflow can dispatch on "what requirement does this task implement?"

Storing requirements as files keeps them **git-native**: every refinement is a diff, every approval is a commit, every deprecation is reviewable in a PR.

## Layout

```
<project_root>/.animus/requirements/
├── REQ-0001.md
├── REQ-0002.md
├── archived/
│   └── REQ-0003.md
└── _index.json
```

Each `REQ-NNNN.md` file pairs structured YAML frontmatter with a freeform markdown body:

```markdown
---
id: requirement:REQ-0001
kind: requirement
title: "Users must be able to log in with OAuth"
status: refined
priority: high
labels: [auth, p1]
linked_tasks: [task:TASK-0042, task:TASK-0099]
linked_workflows: [delivery]
acceptance_criteria:
  - "Google + GitHub OAuth providers supported"
  - "Session tokens stored server-side, not in localStorage"
created_at: 2026-05-18T12:00:00Z
updated_at: 2026-05-18T13:30:00Z
refined_at: 2026-05-18T13:30:00Z
refined_by: alice@example.com
custom_fields:
  origin: stakeholder-interview-2026-q2
---

# REQ-0001: Users must be able to log in with OAuth

## Context
[Multi-paragraph description...]

## Acceptance criteria
- [x] Google OAuth provider integrated
- [ ] GitHub OAuth provider integrated
```

## Status mapping

Requirements carry a four-state native lifecycle that maps onto Animus's normalized `SubjectStatus` taxonomy:

| Native (`status:`) | Normalized        | Meaning                                        |
|--------------------|-------------------|------------------------------------------------|
| `drafted`          | `ready`           | Newly captured; awaiting refinement            |
| `refined`          | `in-progress`     | Iteratively being clarified                    |
| `approved`         | `done`            | Approved for downstream dispatch               |
| `deprecated`       | `cancelled`       | Abandoned without implementation               |

The native value is surfaced in `Subject.native_status` so workflows can dispatch on the rich vocabulary — e.g. *"when a requirement moves to `drafted`, run the refinement workflow"*.

## Why dedicated to requirements?

This plugin sits beside `animus-subject-markdown` (general task storage in markdown) and `animus-subject-sqlite` (general task storage in SQLite). The split exists because requirements:

- Have an **iterative refine verb** that doesn't exist for tasks.
- Carry **cross-references to tasks and workflows** as first-class fields.
- Have **stable IDs that outlive the work** — `REQ-0001` may spawn `TASK-0042`, which closes, but `REQ-0001` stays.
- Benefit from **git-reviewable mutations** — refining a requirement is exactly the kind of thing you want a PR for.

## Configuration

| Env var                                | Default                                          | Description                                    |
|----------------------------------------|--------------------------------------------------|------------------------------------------------|
| `ANIMUS_REQUIREMENTS_ROOT`             | `<project_root>/.animus/requirements`            | Where requirement files live                   |
| `ANIMUS_REQUIREMENTS_ID_PREFIX`        | `REQ`                                            | Prefix for new ids (`REQ-0001`)                |
| `ANIMUS_REQUIREMENTS_INDEX_TTL_SECS`   | `60`                                             | How stale `_index.json` may go before rebuild  |
| `ANIMUS_REQUIREMENTS_LEGACY_JSON`      | _(unset)_                                        | Path to the in-tree `core-state.json` for legacy read+migrate compat |
| `ANIMUS_SCOPED_ROOT`                   | _(unset)_                                        | Fallback for legacy path: `<scoped>/core-state.json` is probed when `ANIMUS_REQUIREMENTS_LEGACY_JSON` is unset |
| `ANIMUS_REQUIREMENTS_MIGRATE_LEGACY`   | `false`                                          | When truthy, runs one-shot legacy → Markdown migration on startup |

## Legacy in-tree JSON compatibility

Before v0.4.0, requirements lived inside the in-tree `core-state.json` file under `~/.animus/<repo-scope>/`. This plugin can read that legacy file alongside its own Markdown store so existing projects keep working unmodified during the migration:

- **Read+union:** On every `list()` call, legacy entries are merged into the Markdown set. Markdown wins on id collision; legacy entries surface with their rich fields (acceptance criteria, comments, linked tasks, links, legacy id, category, source) preserved under `custom_fields.*` and `custom_fields.origin_store = "legacy_json"`.
- **Read fallback for `get()`:** If a `REQ-NNNN.md` file does not exist, the plugin falls back to the legacy JSON before returning `NotFound`.
- **Status fold:** The in-tree eleven-state lifecycle (`draft / refined / planned / in-progress / done / po-review / em-review / needs-rework / approved / implemented / deprecated`) collapses to the four-state native model (`drafted / refined / approved / deprecated`). The original string is preserved under `custom_fields.legacy_status`.
- **One-shot migration:** Set `ANIMUS_REQUIREMENTS_MIGRATE_LEGACY=1` to convert every legacy entry into a `REQ-NNNN.md` file at startup. The legacy `requirements` map is cleared after every entry is successfully written; other top-level fields in `core-state.json` are preserved verbatim. Idempotent — re-running after a successful migration is a no-op.

The legacy read path is intentionally **append-only on the standalone side**: new requirements always land in Markdown, never back into the legacy JSON. `delete()` on a legacy-only id is a no-op (returns `Ok(false)`) — operators should migrate first, then delete.

### What this plugin does not own (yet)

The in-tree `BuiltinRequirementsProvider` also exposed three orchestration verbs on the CLI:

- `draft_requirements` — LLM-driven generation of an initial requirement set from project context.
- `refine_requirements` — LLM-driven iterative clarification of one or more requirements.
- `execute_requirements` — Materializes approved requirements into tasks via the planning state machine.

These are **planning-pipeline operations**, not data ops — they require the agent runtime, model registry, and codebase scanner. They do not fit the `SubjectBackend` trait surface (which is data: `list / get / update / watch / health`). The `delete_requirement` CLI verb is implemented locally as [`RequirementsBackend::delete`](src/backend.rs) but is not yet wired through the protocol (the wire `SubjectBackend` trait has no `delete` method as of `animus-subject-protocol` v0.1.6 — adding it is a protocol-version change).

Deleting the in-tree `InTreeRequirementsSubjectBackend` for data ops is unblocked by this release. Removing the three orchestration verbs requires either:

1. Adding a small `subject_planning/*` JSON-RPC namespace to the protocol that orchestrates LLM-driven verbs, or
2. Keeping those three verbs in the orchestrator-core service layer as a separate (non-subject-backend) facade.

## Index cache

Listing requirements walks the entire directory, which is fine for tens but bad for thousands. The plugin maintains `_index.json` — a flat cache of every requirement's frontmatter that is rebuilt when:

1. The cache file is older than `ANIMUS_REQUIREMENTS_INDEX_TTL_SECS` (default 60s).
2. Any `*.md` file's `mtime` is newer than the cache file's `mtime` (catches external edits like `git pull`).

Rebuilds are guarded by an in-process lock so concurrent calls don't all scan.

## Subject schema

```yaml
kinds: [requirement]
status_values: [ready, in-progress, done, cancelled]
supports_create: true
supports_watch: true
supports_pagination: true
native_status_values: [drafted, refined, approved, deprecated]
```

Watch is implemented via [`notify`](https://crates.io/crates/notify) — every `*.md` change under the requirements root emits a `subject/changed` notification.

## Development

```bash
cargo build --release
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
./target/release/animus-subject-requirements --manifest
```

## License

MIT

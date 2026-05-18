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

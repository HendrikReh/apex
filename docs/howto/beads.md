# Beads Guide

This repository uses `bd` (Beads) for issue tracking.

Use Beads for all task tracking. Do not use markdown TODO files or ad hoc task lists.

## Core CRUD

### Create

Create a new issue with a title, description, type, and priority:

```bash
bd create \
  --title "Add chat service" \
  --description "Implement the Phase 6 chat orchestration flow." \
  --type feature \
  --priority 2
```

Common create flags:

- `--type bug|feature|task|epic|chore|decision`
- `--priority 0-4` or `P0-P4`
- `--labels backend,chat`
- `--assignee <name>`
- `--acceptance "..."` for acceptance criteria
- `--design "..."` for design notes
- `--deps <id>` to add dependencies during creation
- `--external-ref gh-123` to link external systems

Quick capture:

```bash
bd q "Fix retrieval timeout handling"
```

### Read

Inspect one issue:

```bash
bd show apex-123
bd show apex-123 --long
```

List and search issues:

```bash
bd ready
bd list --status=open
bd list --status=in_progress
bd search "chat service"
```

Useful read commands:

- `bd ready`: open and unblocked issues
- `bd blocked`: blocked issues
- `bd show --current`: current or last-touched issue
- `bd list --assignee <name>`: assigned work

### Update

Claim an issue:

```bash
bd update apex-123 --claim
```

Change fields:

```bash
bd update apex-123 \
  --title "Refine chat service orchestration" \
  --description "Updated scope after design review." \
  --priority 1 \
  --assignee HendrikReh
```

Append context:

```bash
bd update apex-123 --append-notes "Need to share Stores across ChatService and RetrievalService."
bd update apex-123 --design "Use a single Stores handle and clone it."
```

Set status:

```bash
bd update apex-123 --status in_progress
bd update apex-123 --status blocked
bd update apex-123 --status open
```

Manage labels and metadata:

```bash
bd update apex-123 --add-label rust --add-label api
bd update apex-123 --remove-label api
bd update apex-123 --set-metadata team=platform
```

Add a dependency:

```bash
bd dep add apex-200 apex-123
```

Meaning: `apex-200` depends on `apex-123`.

### Delete vs Close

Normal completion uses `close`, not `delete`.

Close a completed issue:

```bash
bd close apex-123
bd close apex-123 --reason "Implemented and merged"
bd close apex-123 --suggest-next
```

Delete is destructive and should be used only for mistakes, accidental duplicates, or bad test data.

Preview deletion:

```bash
bd delete apex-123 --dry-run
```

Delete permanently:

```bash
bd delete apex-123 --force
bd delete apex-123 --cascade --force
```

`bd delete` removes dependency links, rewrites references, and permanently deletes the issue from the database.

## Typical Workflow

Start work:

```bash
bd ready
bd show apex-123
bd update apex-123 --claim
```

During work:

```bash
bd update apex-123 --append-notes "Found config validation gap for empty env vars."
```

Finish work:

```bash
bd close apex-123
bd dolt push
git push
```

## Repository-Specific Rules

- Use Beads for all task tracking in this repo.
- Run `bd prime` after session recovery, compaction, or when starting fresh.
- Prefer `bd create`, `bd show`, `bd update`, and `bd close`.
- Do not use `bd edit` from an agent; it opens `$EDITOR`.
- Use `bd close` for completed work. Do not use `bd delete` unless you really want permanent removal.

## Handy Commands

```bash
bd prime
bd ready
bd show <id>
bd update <id> --claim
bd close <id>
bd search <query>
bd blocked
bd doctor
bd preflight
bd dolt push
```

## Cheat Sheet

```bash
# recover context
bd prime

# find work
bd ready
bd list --status=open
bd blocked
bd search "query"

# inspect
bd show <id>
bd show <id> --long
bd show --current

# create
bd create --title "Title" --description "What and why" --type task --priority 2
bd q "Quick issue"

# claim / update
bd update <id> --claim
bd update <id> --status in_progress
bd update <id> --append-notes "New finding"
bd update <id> --design "Decision made"
bd update <id> --add-label backend

# dependencies
bd dep add <issue> <depends-on>

# complete
bd close <id>
bd close <id> --reason "Implemented"

# destructive delete, use rarely
bd delete <id> --dry-run
bd delete <id> --force

# sync / health
bd dolt push
bd doctor
bd preflight
```

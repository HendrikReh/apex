# Beads Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce that a beads issue is created before any code file can be edited, via a PreToolUse hook.

**Architecture:** A PreToolUse hook on Edit|Write checks for a `/tmp/apex-beads-gate` marker file before allowing edits to `.rs/.toml/.sql/.yaml/.yml` files. A PostToolUse hook on Bash detects `bd create` and creates the marker. CLAUDE.md is updated to document the gate as the first instruction.

**Tech Stack:** Claude Code hooks (settings.json), shell scripts, markdown

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `.claude/settings.json` | Modify | Add PreToolUse gate hook, add PostToolUse Bash hook |
| `CLAUDE.md` | Modify | Add "Mandatory: Beads Issue Gate" as first section, replace old beads section |
| `memory/feedback_beads_tracking.md` | Modify | Update to reference hook enforcement |

---

### Task 1: Add hooks to settings.json

**Files:**
- Modify: `.claude/settings.json` (hooks section, lines 11-24)

- [ ] **Step 1: Add the PreToolUse gate hook**

In `.claude/settings.json`, replace the existing `hooks` object with:

```json
"hooks": {
  "PreToolUse": [
    {
      "matcher": "Edit|Write",
      "hooks": [
        {
          "type": "command",
          "command": "FILE_PATH=$(echo \"$TOOL_INPUT\" | jq -r '.file_path // empty'); case \"$FILE_PATH\" in *.rs|*.toml|*.sql|*.yaml|*.yml) if [ ! -f /tmp/apex-beads-gate ]; then echo 'BLOCKED: No beads issue created this session. Run bd create first, or user can say skip beads to override.'; exit 1; fi;; esac",
          "timeout": 5000
        }
      ]
    }
  ],
  "PostToolUse": [
    {
      "matcher": "Edit|Write",
      "hooks": [
        {
          "type": "command",
          "command": "if echo \"$TOOL_INPUT\" | grep -q '\\.rs\"'; then cargo check --message-format=short 2>&1 | head -20; fi",
          "timeout": 30000
        }
      ]
    },
    {
      "matcher": "Bash",
      "hooks": [
        {
          "type": "command",
          "command": "if echo \"$TOOL_INPUT\" | grep -qE 'bd create|bd update.*--claim'; then touch /tmp/apex-beads-gate; fi",
          "timeout": 5000
        }
      ]
    }
  ]
}
```

This preserves the existing `cargo check` PostToolUse hook and adds two new hooks:
- PreToolUse on Edit|Write: blocks code file edits without the gate marker
- PostToolUse on Bash: creates the gate marker when `bd create` or `bd update --claim` runs

- [ ] **Step 2: Verify JSON is valid**

Run: `jq . .claude/settings.json`
Expected: valid JSON output (no parse errors)

- [ ] **Step 3: Commit**

```bash
git add .claude/settings.json
git commit -m "feat(hooks): add beads gate PreToolUse hook

Blocks Edit/Write on .rs/.toml/.sql/.yaml/.yml files unless
a beads issue has been created via bd create."
```

---

### Task 2: Elevate beads requirement in CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (add new section after line 5, replace section at ~line 126-128)

- [ ] **Step 1: Add the gate section as the first content section**

Insert after the `[GitHub Repository]` link (line 5) and before `## Project Overview` (line 7):

```markdown

## Mandatory: Beads Issue Gate

**HARD GATE — Do NOT write any code until a beads issue exists for the work.**

Before editing any `.rs`, `.toml`, `.sql`, `.yaml`, or `.yml` file:
1. Run `bd create --title="..." --type=<type> --priority=<N>` to create an issue
2. Run `bd update <id> --claim` to mark it in-progress
3. Only then begin implementation

If the user says "skip beads", run `touch /tmp/apex-beads-gate` and proceed without an issue.

This is enforced by a PreToolUse hook — edits to code files will be **blocked** without an active gate.
```

- [ ] **Step 2: Replace the old Task Tracking section**

Replace the existing "Task Tracking (Beads)" section (~line 126-128):

```markdown
## Task Tracking (Beads)

See `AGENTS.md` for full beads workflow. Key rule: create a `bd` issue **before** writing code. Never use `bd edit` (blocks agents) — use `bd update` with inline flags.
```

With:

```markdown
## Task Tracking (Beads)

See **Mandatory: Beads Issue Gate** above. Full beads workflow in `AGENTS.md`.
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(CLAUDE.md): elevate beads gate to first section

Moves the beads-before-code requirement to the top of CLAUDE.md
so it's the first instruction Claude reads."
```

---

### Task 3: Update memory entry

**Files:**
- Modify: `/Users/hendrik/.claude/projects/-Users-hendrik-Developer-apex/memory/feedback_beads_tracking.md`

- [ ] **Step 1: Update the memory content**

Replace the full content of `feedback_beads_tracking.md` with:

```markdown
---
name: Beads gate is hook-enforced
description: PreToolUse hook blocks code file edits unless bd create has been run — CLAUDE.md documents this as the first instruction
type: feedback
---

A PreToolUse hook in `.claude/settings.json` blocks Edit/Write on `.rs/.toml/.sql/.yaml/.yml` files unless `/tmp/apex-beads-gate` exists. The marker is created automatically when `bd create` or `bd update --claim` runs.

**Why:** Claude repeatedly forgot to create beads issues before coding, despite documentation in CLAUDE.md, AGENTS.md, and memory. The hook provides technical enforcement.

**How to apply:** Do not attempt to bypass the hook. If it blocks an edit, run `bd create` first. If the user says "skip beads", run `touch /tmp/apex-beads-gate`. The gate resets on system reboot.
```

- [ ] **Step 2: Commit**

```bash
git add -f /Users/hendrik/.claude/projects/-Users-hendrik-Developer-apex/memory/feedback_beads_tracking.md
git commit -m "chore(memory): update beads tracking to reference hook enforcement"
```

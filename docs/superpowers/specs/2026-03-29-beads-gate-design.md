# Beads Issue Gate — Design Spec

## Problem

Claude forgets to run `bd create` before starting implementation work. The beads requirement is documented in CLAUDE.md, AGENTS.md, and memory, but Claude jumps straight into coding — especially when the superpowers brainstorming/planning flow takes over.

## Solution

A two-layer enforcement system:

1. **PreToolUse hook** on `Edit|Write` that blocks code file edits unless a gate marker exists
2. **Elevated CLAUDE.md section** that places the beads requirement before all other instructions

## Scope

### Gated file extensions

All edits to `.rs`, `.toml`, `.sql`, `.yaml`, `.yml` files require an active beads issue.

### Exempt files

`.md`, `.json`, `.claude/*`, and any other non-code files pass through ungated. This allows meta-work (editing CLAUDE.md, skills, docs) without a beads issue.

### Override

User can say "skip beads" (or similar). Claude runs `touch /tmp/apex-beads-gate` and proceeds.

## Implementation

### 1. PreToolUse hook (the gate)

Added to `.claude/settings.json`. Fires before every `Edit` or `Write` call. Parses `file_path` from `$TOOL_INPUT`, checks extension, blocks if no marker:

```bash
FILE_PATH=$(echo "$TOOL_INPUT" | jq -r '.file_path // empty')
case "$FILE_PATH" in
  *.rs|*.toml|*.sql|*.yaml|*.yml)
    if [ ! -f /tmp/apex-beads-gate ]; then
      echo "BLOCKED: No beads issue created this session. Run bd create first, or user can say skip beads to override."
      exit 1
    fi
    ;;
esac
```

- Non-zero exit blocks the tool use
- Timeout: 5000ms
- Matcher: `Edit|Write`

### 2. PostToolUse hook (the unlock)

Fires after every `Bash` call. Detects `bd create` or `bd update --claim` and creates the marker:

```bash
if echo "$TOOL_INPUT" | grep -qE 'bd create|bd update.*--claim'; then
  touch /tmp/apex-beads-gate
fi
```

- Timeout: 5000ms
- Matcher: `Bash`

### 3. CLAUDE.md changes

Move beads requirement to the first section after the title (before Project Overview):

```markdown
## Mandatory: Beads Issue Gate

**HARD GATE — Do NOT write any code until a beads issue exists for the work.**

Before editing any `.rs`, `.toml`, `.sql`, `.yaml`, or `.yml` file:
1. Run `bd create --title="..." --type=<type> --priority=<N>` to create an issue
2. Run `bd update <id> --claim` to mark it in-progress
3. Only then begin implementation

If the user says "skip beads", run `touch /tmp/apex-beads-gate` and proceed.

This is enforced by a PreToolUse hook — edits to code files will be blocked without an active gate.
```

Replace the old "Task Tracking (Beads)" section (line ~126) with:
```markdown
## Task Tracking (Beads)

See **Mandatory: Beads Issue Gate** above. Full beads workflow in `AGENTS.md`.
```

### 4. Memory update

Update `feedback_beads_tracking.md` to reference the hook enforcement mechanism.

## Known limitations

- **Stale gate marker**: The `/tmp/apex-beads-gate` file persists across sessions until reboot. A previous session's `bd create` leaves the gate open for the next session. Mitigation: the CLAUDE.md instruction still serves as the primary reminder. The hook is a safety net.
- **jq dependency**: The PreToolUse hook uses `jq` to parse JSON. Available on macOS via Homebrew (likely already installed). Falls through gracefully if `jq` is missing (empty FILE_PATH matches no case branch, so the edit proceeds ungated).

## Files changed

| File | Change |
|------|--------|
| `CLAUDE.md` | New first section, old section replaced with pointer |
| `.claude/settings.json` | PreToolUse hook + PostToolUse hook for Bash |
| Memory: `feedback_beads_tracking.md` | Updated to reference hook enforcement |

# Agent Instructions

This project uses **bd** (beads) for issue tracking. Run `bd onboard` to get started.

## Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

**Use these forms instead:**
```bash
# Force overwrite without prompting
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file

# For recursive operations
rm -rf directory            # NOT: rm -r directory
cp -rf source dest          # NOT: cp -r source dest
```

**Other commands that may prompt:**
- `scp` - use `-o BatchMode=yes` for non-interactive
- `ssh` - use `-o BatchMode=yes` to fail instead of prompting
- `apt-get` - use `-y` flag
- `brew` - use `HOMEBREW_NO_AUTO_UPDATE=1` env var

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol

### Worktrees

**All worktrees must be created under `.worktrees/`.** This is enforced by a PreToolUse hook.

Git worktrees get a copy of `.beads/` but NOT the running Dolt server state. Use `bd worktree create` for proper beads redirect:

```bash
bd worktree create .worktrees/<name>                    # Creates worktree with beads redirect
bd worktree create .worktrees/<name> --branch <branch>  # With specific branch
```

If a worktree was created without `bd worktree create` (e.g., via `git worktree add`), fix beads manually:

```bash
# In the worktree directory:
rm -rf .beads/dolt .beads/dolt-server.* .beads/interactions.jsonl .beads/last-touched .beads/push-state.json .beads/backup .beads/.local_version
echo "../../.beads" > .beads/redirect    # Adjust relative path to main repo's .beads/
bd doctor                                # Verify: should show 0 errors
```

## Session Completion

**When ending a work session**, complete these steps:

1. **File issues for remaining work** — `bd create` for anything needing follow-up
2. **Run quality gates** (if code changed) — `cargo check` at minimum
3. **Update issue status** — close finished work, update in-progress items
4. **Push to remote** — commit, then:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   ```
5. **Hand off** — provide context for next session

If push fails, diagnose and resolve before ending the session.
<!-- END BEADS INTEGRATION -->

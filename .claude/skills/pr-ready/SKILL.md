---
name: pr-ready
description: Use when creating a pull request, pushing code for review, or finishing a feature branch — enforces hygiene checklist (labels, reviewers, beads) and produces a policy-compliant PR body with evidence. Also use when rewriting a weak PR description, or when the user says "prepare the PR", "write the PR body", or "format this as a PR".
---

# PR Readiness Workflow

**Every step is mandatory. Do NOT skip any. Do NOT reorder.**

## Steps

### 1. Classify the change

- **Type**: `bug`, `enhancement`, `refactor`, `documentation`, `performance`, etc.
- **Scope**: crates touched (`rag-core`, `rag-server`, `rag-chunking`, `agent-core`, etc.)
- **Risk level**:
  - **Low**: config-only, docs, formatting, test-only changes
  - **Medium**: single-crate behavior change with existing tests covering the area
  - **High**: cross-crate behavior change, auth modifications, migration changes, or anything affecting data integrity

### 2. Verify beads tracking

Check that a `bd` issue exists for this work. If not, create one before proceeding:
```bash
bd list  # Check for existing issue
bd create --title "<description>" --type <type>  # Create if missing
```

Every PR should reference a beads issue.

### 3. Collect evidence

- Always include the commands you ran and the key result lines
- For build/perf work, provide before/after timings and % change
- If something was not run, state it explicitly (e.g., "Integration tests not run — no store changes")
- Per project policy: do NOT run `just test` autonomously — only include results if the user ran it

| Change Type | Required Validation | Optional |
|-------------|-------------------|----------|
| Any code change | `cargo check -p <crate>` | -- |
| User ran full suite | `just test` results | -- |
| Stores/API/auth flows | `just integration-tests` | -- |
| Route changes | OpenAPI spec consistency | -- |

### 4. Push branch and create PR

Title follows conventional commit pattern with scope:

```
fix(rag-server): prevent panic on empty tenant header
feat(rag-core): add semantic chunking strategy
refactor(agent-core): extract checkpoint logic into trait
perf(build): reduce incremental build time by 40%
docs(contributing): clarify label policy
chore(deps): update tokio to 1.38
```

PR body template:

```bash
git push -u origin HEAD
gh pr create --title "<type>(<scope>): <short description>" --body "$(cat <<'EOF'
## Summary

<1-3 bullets describing what changed and why>

## Related Issue

Closes #<issue>

## Scope

### Changed files
- `<path>` - <purpose>

### Out of scope
- <what was intentionally not changed>

## Validation

| Check | Command | Result |
|---|---|---|
| Compilation | `cargo check -p <crate>` | pass |
| Format/lint/tests | `just test` | <pass/fail or "not run -- user decision"> |
| Integration (if relevant) | `just integration-tests` | <pass/fail or n/a> |

## Benchmark Evidence (include for perf/build changes)

| Scenario | Before | After | Delta |
|---|---:|---:|---:|
| <case 1> | <time> | <time> | <percent> |

## Risk and Rollback

- Risk: <low/medium/high> -- <concrete reason>
- Rollback: <exact files/flags to revert>

## Reviewer Focus

1. <highest-risk design or policy point>
2. <benchmark methodology or interpretation>

Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

**Required body sections:** Summary, Related Issue (if applicable), Scope, Validation, Risk and Rollback.
**Conditional sections:** Benchmark Evidence (perf/build changes only), Reviewer Focus (medium/high risk).

### 5. Apply label policy (MANDATORY)

Apply ALL required categories:
- **Priority**: exactly one (`priority:critical|high|medium|low|nice-to-have`)
- **Type**: at least one (`bug`, `enhancement`, `refactor`, `documentation`, etc.)
- **Component**: 1-3 scope labels (`rag-core`, `rag-server`, `rag-chunking`, `agent-core`, etc.)

```bash
gh pr edit <number> --add-label "priority:medium,enhancement,component-name"
```

If unsure which labels -> ask the user. Do NOT skip. Do NOT "do it later."

### 6. Tag reviewers (MANDATORY)

Both comments are required. Do NOT skip either.

```bash
gh pr comment <number> --body "@codex review"
gh pr comment <number> --body "@claude review this PR"
```

### 7. Update beads

```bash
bd update <issue-id> --status in-review --pr <pr-number>
```

### 8. Report

Print: PR URL, labels applied, reviewers tagged, beads issue updated, any skipped steps with reason.

## Fast Checklist

- [ ] Title follows conventional commit pattern with scope
- [ ] Beads issue exists and is linked
- [ ] Summary explains both change and intent
- [ ] Validation includes command + result evidence
- [ ] Perf/build PR includes before/after table with % delta
- [ ] Risks and rollback are concrete with specific files/steps
- [ ] Issue/label suggestions match project matrix
- [ ] Out-of-scope section is explicit
- [ ] Reviewers tagged with both comments

## Red Flags

| Thought | Reality |
|---------|---------|
| "Just push and create the PR quickly" | Follow the full checklist. Every step. |
| "Labels aren't important right now" | Labels are always required. Apply them. |
| "I'll tag reviewers later" | You won't. Do it now. |
| "The PR body is fine without a test plan" | Validation section is mandatory. |
| "Reviewers will find it" | Tag them explicitly with both comments. |
| "Let me run the test suite first" | **NEVER run `just test` or any test commands.** Tests are the user's decision. |
| Claims without numbers for perf PRs | Always include before/after table with % delta. |
| Risk described as just "low" | Include a reason: "low -- docs-only change, no behavior impact". |
| "I'll create the beads issue later" | Create it before writing code. Update it now. |

## Cross-references

- `/review` — pre-PR code and security review
- `/resolve-review-comments` — address review feedback after PR creation

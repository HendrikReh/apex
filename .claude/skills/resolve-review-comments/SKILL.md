---
name: resolve-review-comments
description: Use when a PR has review comments from Codex or Claude that need to be addressed — categorizes, resolves, and posts a summary. Also use when the user says "fix the review comments", "address PR feedback", or mentions review comments that need attention on a pull request.
---

# Resolve PR Review Comments

Address all open review comments on a PR from automated reviewers (Codex, Claude), then post a structured summary and trigger a new review cycle. This skill ensures review comments don't get lost or silently ignored — every comment gets an explicit resolution or documented rationale for deferral.

## Step 1: Fetch All Comments

```bash
# PR review comments (inline code comments)
gh api repos/{owner}/{repo}/pulls/{number}/comments --paginate

# Issue-level comments (general PR discussion)
gh api repos/{owner}/{repo}/issues/{number}/comments --paginate
```

Filter for comments from these bot accounts:
- **chatgpt-codex-connector** — Codex AI review
- **claude** or **github-actions[bot]** with `@claude` mentions — Claude AI review

Ignore resolved/outdated comments. Focus on unresolved ones.

## Step 2: Categorize Each Comment

| Category | What It Looks Like | Action |
|----------|-------------------|--------|
| **Committable suggestion** | A concrete code diff or "Apply this suggestion" block | Apply the code change directly |
| **Enhancement** | "Consider using...", "This could be improved by..." | Implement the improvement if it's scoped and reasonable |
| **Clarification** | "What does this do?", "This is unclear" | Add a code comment or doc explaining the intent |
| **False positive** | Suggestion conflicts with project conventions (e.g., suggesting `.expect()` when ADR-001 forbids it) | Document why it's intentionally unchanged |
| **Out of scope** | Valid suggestion but unrelated to the PR's purpose | Acknowledge and create a follow-up issue if warranted |

## Step 3: Resolve Each Comment

For each comment, apply the appropriate action. Rules:

- **Keep changes minimal** — scoped to the review feedback, no unrelated refactoring
- **If two reviewers conflict** — prefer the suggestion that aligns with project conventions in CLAUDE.md. If genuinely ambiguous, ask the user before proceeding
- **If a fix breaks compilation** — run `cargo check -p <crate>` after each change. If it fails, investigate rather than reverting blindly
- **If a fix might break tests** — note it in the summary but don't run the full test suite (that's the user's decision per project policy)

## Step 4: Post Summary Comment

After all changes are applied, post a single summary comment on the PR:

```bash
gh pr comment {number} --body "$(cat <<'EOF'
## Review Comments Resolved

### Codex

| # | Finding | Action |
|---|---------|--------|
| 1 | Handler missing tenant validation | **Fixed** — added tenant check via `RequestContext` |

### Claude

| # | Finding | Action |
|---|---------|--------|
| 1 | Test doesn't cover error path | **Deferred** — created follow-up issue #123 |

All changes compile clean (`cargo check -p rag-server`).
EOF
)"
```

The summary enables a reviewer to validate every fix without re-reading the full diff. Every comment must appear in the table — nothing silently dropped.

## Step 5: Trigger New Review Cycle

```bash
gh pr comment {number} --body "@codex review"
gh pr comment {number} --body "@claude review this PR"
```

This triggers fresh reviews against the updated code.

## Acceptance Criteria

- [ ] Every unresolved comment has an entry in the summary table
- [ ] Each entry has a clear action: **Fixed**, **Intentionally unchanged** (with rationale), or **Deferred** (with issue link)
- [ ] All changes compile clean (`cargo check`)
- [ ] New review cycle triggered for both Codex and Claude

## Cross-references

- `/review` — dispatches the initial code and security review
- `/pr-ready` — full PR workflow including reviewer tagging

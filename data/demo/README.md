# Demo Data

`data/demo/` is reserved for the curated demo corpus used to stand up an MVP demo system after the implementation phase is complete.

This directory is intentionally documentation-only in git right now. The current local PDF corpus is not committed. That keeps the repository small and avoids accidentally versioning large files or content with unclear redistribution terms.

## Intended Use

Populate `data/demo/` with a small, high-signal document set that is suitable for a product demo:

- Organize documents into topic folders such as `ai_governance/`, `security/`, `research/`, or other collections that map cleanly to demo scenarios.
- Keep sidecar metadata files next to source documents when needed, for example `<name>.metadata.json`.
- Prefer stable, reviewable source files over generated artifacts.

## What Should Go Here

- Demo source documents that we are explicitly allowed to redistribute.
- Small metadata sidecars required to ingest or label the demo corpus.
- Documentation describing how the demo corpus is assembled.

## What Should Not Go Here

- Local-only or unlicensed PDFs that should not be redistributed.
- Generated database state, vector indexes, or other runtime artifacts.
- Temporary downloads, caches, or OS files such as `.DS_Store`.

## How To Proceed

When we are ready to ship the MVP demo setup:

1. Decide which demo documents are safe to commit.
2. Remove or replace any local-only files that cannot live in the repository.
3. Add the approved corpus under the relevant subdirectories in `data/demo/`.
4. Keep runtime state out of git; only commit the source corpus and metadata.

If the final demo corpus should stay out of git, keep only a manifest here and add a fetch/setup script elsewhere in the repo.

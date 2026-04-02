# Routed Agentic Search Foundation — Design Spec

**Beads issue:** apex-fp4

## Goal

Define Apex milestone 1 for agentic search without changing `/chat`. The milestone should prove that Apex can route queries between the current single-pass RAG path and a new search-first agent path, expose richer evidence to the runtime, and evaluate that routing offline before any production cutover.

This milestone is successful if:

- `/chat` remains behaviorally unchanged
- Apex exposes a synchronous `agentic_search_v1` path through the existing agent API
- the agent path uses explicit retrieval tools instead of one opaque hybrid search call
- run output carries enough routing and evidence detail to debug and evaluate decisions
- an offline benchmark set shows no material regression on simple queries and a clear gain on the agentic subset

## Context

The current repository already has the right substrate for this work:

- `rag-core` provides dense, sparse, and hybrid retrieval plus grounded chat
- `agent-core` provides a spec-driven runtime and graph execution loop
- `rag-server` already exposes `/agents/:id/execute` and run inspection endpoints
- sidecar and document metadata already contain richer provenance than the retrieval path exposes today

The gap is that the executable agent path is still narrower than the spec model:

- `RetrievalPort` only exposes `search_hybrid`
- the runtime only wires `classify -> hybrid_search -> summarize -> approval_checkpoint -> final_answer`
- retrieval-time result payloads omit most of the metadata an agent would need to critique and refine evidence
- query classification is too coarse to support routing

Milestone 1 closes that gap just far enough to test the thesis: agentic search should be an explicitly routed, search-first path, not a replacement for all chat requests.

## Scope

### In scope

- extract a reusable single-shot baseline answer path from `ChatService`
- add a deterministic structured router for `single_pass_rag` vs `agentic_search`
- expand retrieval contracts into explicit agent-visible tools
- enrich retrieval-time evidence payloads and run output
- add a new synchronous, auto-completing `agentic_search_v1` agent spec
- add an offline benchmark and scoring harness that compares baseline `/chat` with `agentic_search_v1`

### Out of scope

- changing `/chat` behavior or API shape
- introducing a new `/chat/agentic` endpoint
- human approval checkpoints for the new path
- durable run/session persistence
- query memory or past-query retrieval
- rerank or late-interaction retrieval stages
- live traffic shadow mode

## Architecture

Milestone 1 introduces a second execution path but keeps the public production baseline unchanged.

### Public surfaces

- `/chat`
  - remains the production single-pass RAG interface
  - continues to manage conversations and citations as it does today
- `/agents/:id/execute`
  - becomes the public interface for milestone-1 agentic search evaluation
  - hosts a new `agentic_search_v1` spec

### High-level flow

```text
baseline callers
  -> /chat
  -> ChatService::chat
  -> single-pass RAG path

evaluation callers
  -> /agents/agentic_search_v1/execute
  -> route_query
  -> baseline_answer OR retrieve_evidence
  -> compose_answer (agentic branch only)
  -> final_answer
```

The router belongs to the agent path, not to `/chat`. This preserves the existing contract while allowing direct, apples-to-apples evaluation between:

- baseline `/chat`
- routed agent path under `agentic_search_v1`

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Public entry point | Reuse `/agents/:id/execute` | Existing run lifecycle and inspection APIs are already in place. |
| `/chat` during milestone 1 | Unchanged | Evaluation should not perturb the stable production path. |
| Routing implementation | Deterministic heuristics | Reproducible evals are more important than classifier sophistication in milestone 1. |
| Agent runtime shape | Fixed retrieval profiles, not open-ended ReAct | Smallest slice that tests the routing thesis without pretending the runtime is more capable than it is. |
| New agent spec | `agentic_search_v1` | Keeps milestone-1 behavior separate from the existing `rag_spike` artifact. |
| Checkpoints | None | The milestone is synchronous and auto-completing. |
| Eval mode | Offline benchmark first | Establishes measurable proof before any shadow traffic or `/chat` routing. |
| Retrieval enrichment strategy | Duplicate hot metadata into retrieval payloads while keeping Postgres canonical | Avoids per-hit joins on the hot path while preserving richer source-of-truth metadata. |

## Section 1: Reusable Baseline Answer Path

Today, `ChatService::chat` in `crates/rag-core/src/chat.rs` combines:

- conversation resolution
- message history loading
- hybrid retrieval
- context assembly
- prompt rendering
- LLM answer generation
- persistence of the new turn

Milestone 1 should split out the single-shot RAG core into a reusable path that accepts:

- `query`
- `collection`
- `tenant`
- optional language instruction

That reusable path should return a grounded answer artifact containing:

- answer text
- retrieved evidence chunks used to build context
- citations or citation-ready metadata
- model and usage details where available

`/chat` should continue to call this path after conversation resolution and history loading. The new agent runtime should call the same baseline path when the router chooses `single_pass_rag`. This prevents baseline drift and keeps the evaluation fair.

### Required boundary

`agent-core` must not depend on `rag-core`. The runtime should therefore reach the baseline path through a port, not by calling `ChatService` directly. Extending the current `ChatPort` is acceptable if it remains conceptually coherent; adding a dedicated answer-generation port is also acceptable. The important constraint is that `agent-core` continues to own only traits and domain types.

## Section 2: Structured Routing

The current `QueryType` enum in `crates/agent-core/src/types.rs` is too coarse for milestone-1 routing. Milestone 1 should introduce a richer `RouteDecision` domain type and keep `QueryType` only for compatibility with existing code.

`RouteDecision` should include:

- `selected_path`: `single_pass_rag` or `agentic_search`
- `query_class`: one of `simple_fact`, `ambiguity_disambiguation`, `exploratory_search`, `procedural`, `multi_hop_research`
- `retrieval_profile`: one of the fixed milestone-1 profiles
- `ambiguity`: boolean
- `needs_multi_hop`: boolean
- `needs_high_evidence`: boolean
- `time_sensitive`: boolean
- `normalized_filters`: extracted constraints such as document IDs, tags, language, or date-like tokens when present
- `reasons`: machine-readable reason codes explaining why the route was chosen

### Routing behavior

The router should stay deterministic in milestone 1. It should not call an LLM.

The intended default policy is:

- route to `single_pass_rag` for simple fact lookup and straightforward summarization
- route to `agentic_search` for ambiguous, exploratory, procedural, and multi-hop queries

### Failure behavior

- empty query remains a request validation error
- if routing logic encounters an internal error, the run fails explicitly rather than silently choosing a different path
- if routing produces an unknown retrieval profile, spec loading or runtime validation should fail before execution

## Section 3: Retrieval Tool Contract

Milestone 1 should replace the single opaque retrieval capability in `crates/agent-core/src/ports.rs` with explicit tool methods the agent runtime can call directly.

The milestone-1 retrieval surface should include:

- `search_dense`
- `search_sparse`
- `search_hybrid`
- `search_fts`
- `expand_chunk_neighbors`
- `fetch_document`

The following should remain out of scope until milestone 2:

- `rerank`
- `search_similar_queries`

### Tool semantics

- `search_dense`, `search_sparse`, `search_hybrid`, and `search_fts`
  - accept `collection`, `query`, `tenant`, and tool-specific limits or overrides
  - return ranked evidence chunks with score provenance
- `expand_chunk_neighbors`
  - accepts one or more anchor chunk IDs plus a neighbor window
  - returns adjacent chunks from the same document to recover local context and headings
- `fetch_document`
  - accepts `document_id` and `tenant`
  - returns document-level metadata needed to support reasoning or response rendering when chunk payloads are insufficient

### Server implementation

`rag-server` should implement this contract by composing:

- `RetrievalService` in `crates/rag-core/src/retrieval.rs`
- `Stores` accessors for metadata and neighbor lookup
- a new FTS path backed by Postgres or an equivalent lexical search implementation already consistent with Apex’s storage model

The important design point is that the agent runtime chooses retrieval tools explicitly rather than asking one “do search” method to decide internally.

## Section 4: Evidence Payloads

Milestone 1 should enrich the retrieval result type that flows through:

- `crates/rag-core/src/fusion.rs`
- `crates/agent-core/src/types.rs`
- `crates/rag-server/src/routes/search.rs`
- `crates/rag-server/src/routes/agents.rs`

The current payload is effectively:

- `chunk_id`
- `document_id`
- `chunk_index`
- `text`
- `score`

That is too thin for routed search. The milestone-1 evidence payload should include:

- `chunk_id`
- `document_id`
- `chunk_index`
- `text`
- `title`
- `source_url`
- `source_domain`
- `language`
- `tags`
- `section_heading`
- `collection`
- `score`
- `score_type`
- `sources`
- `source_scores`

### Provenance rules

- fused results should preserve both the fused score and the per-source component scores
- non-fused results should still identify which tool produced the score
- retrieval output returned by the agent API should preserve enough metadata to explain why a result was considered relevant

### Storage strategy

Hot retrieval metadata should be copied into the Qdrant payload during ingest in `crates/rag-core/src/ingest.rs`. The canonical and richer metadata should remain in Postgres documents and sidecars:

- `crates/rag-core/src/stores/documents.rs`
- `crates/rag-core/src/sidecar.rs`

This duplication is intentional. It avoids a document-table lookup for every retrieved chunk while keeping Postgres as the canonical source for complete metadata.

## Section 5: Milestone-1 Runtime

Milestone 1 should not implement a full generic ReAct loop. It should make the current runtime just expressive enough to execute a routed search path.

### New task set

Add the following runtime tasks in `crates/agent-core/src/runtime/graph_flow/tasks.rs`:

- `route_query`
- `baseline_answer`
- `retrieve_evidence`
- `compose_answer`
- `final_answer`

The existing spike tasks can remain for `rag_spike`.

### Branching model

`route_query` writes the `RouteDecision` plus a boolean context key used for graph branching.

The new graph shape should be:

```text
route_query
  -> retrieve_evidence   if route_to_agentic_search = true
  -> baseline_answer     otherwise

retrieve_evidence -> compose_answer -> final_answer
baseline_answer  -> final_answer
```

This shape fits the current graph runtime model because it uses one conditional edge and one unconditional fallback edge from the same source task.

### Retrieval profiles

Milestone 1 should support a small fixed set of deterministic retrieval profiles:

- `simple_hybrid`
  - hybrid search only
- `lexical_first`
  - FTS or sparse search first, then hybrid search
- `broad_then_expand`
  - hybrid search, then neighbor expansion around the best evidence

These are runtime-interpreted profiles, not free-form plans. The goal is to prove routed search, not to expose an unrestricted planner yet.

### New spec

Add a new spec under `config/agents/`:

- `agentic_search_v1.yaml`

Properties:

- synchronous
- no checkpoints
- no human approval
- uses the new task graph
- declares the retrieval tools it needs

The existing `rag_spike.yaml` remains unchanged and continues to represent the earlier vertical slice.

## Section 6: Agent API Response Shape

`crates/rag-server/src/routes/agents.rs` should return more than the current `answer`, `summary`, `query_type`, and `search_results`.

Milestone-1 agent run responses should also include:

- `route_decision`
- enriched `search_results`
- the executed retrieval profile when applicable

This response shape is not just for clients. It is the main debugging and evaluation artifact for milestone 1, so it should surface the route chosen and the evidence actually used.

## Section 7: Testing And Evaluation

Milestone 1 needs three layers of verification.

### Unit tests

- router classification and route-decision serialization
- retrieval payload mapping and score-provenance mapping
- runtime branch selection from `route_query`
- retrieval profile expansion logic

### Integration tests

- agent execution through `/agents/:id/execute`
- baseline branch executes and completes synchronously
- agentic branch executes and completes synchronously
- enriched search results are present in run output
- `rag_spike` remains unaffected

### Offline benchmark harness

Add a checked-in benchmark set under a dedicated eval fixture path such as:

- `data/evals/agentic_search_v1/`

The benchmark should include at least 25 queries, distributed across:

- simple fact lookup
- ambiguity/disambiguation
- exploratory search
- procedural questions
- multi-hop research

Each example should label:

- expected route
- query class
- expected evidence hints such as document IDs, titles, or source domains

The offline harness should compare:

- baseline `/chat`
- `agentic_search_v1`

### Evaluation metrics

Milestone 1 promotion should depend primarily on routing and evidence quality, not free-form answer-text grading.

The harness should score:

- route agreement with the labeled expected path
- evidence recall: whether expected evidence appears in the returned result set
- simple-query non-regression
- reproducibility of route decisions across repeated runs on the same corpus

### Promotion gate

The router is not eligible to move into `/chat` until the offline benchmark shows:

- no material regression on the simple-query subset
- a clear win on the agentic subset
- stable deterministic routing outputs on repeated runs

Answer-text quality can be reviewed manually during milestone 1, but it is not the gate for promoting routing into `/chat`.

## Section 8: Error Handling

- invalid request validation should remain client-actionable and continue to return `400`
- collection-not-found behavior should remain client-actionable for both search and agent execution paths
- empty evidence on the agentic path should produce an explicit “insufficient evidence” style answer, not a silent fallback to baseline
- spec/tool mismatches should fail during agent load or startup, not at arbitrary mid-run points

## Section 9: Non-Goals And Deferred Work

Milestone 1 explicitly defers:

- durable session persistence and run storage
- memory of prior queries or tool traces
- user-behavior feedback signals
- late-interaction or rerank stages
- live traffic shadow comparisons
- routing decisions inside `/chat`

This deferred work should land only after milestone 1 proves the routed search foundation.

## Section 10: Roadmap After Milestone 1

### Milestone 2: Search-first runtime

- move from fixed retrieval profiles to a real search loop
- add query rewrite, critique, expand, and conclude tasks
- make more of `crates/agent-core/src/spec.rs` executable
- add rerank or late-interaction after broad recall

### Milestone 3: Memory and durable runs

- replace in-memory run/session state with durable storage
- add a query-interaction memory store
- expose memory-backed retrieval helpers such as past useful queries
- absorb `apex-dn3` into this milestone rather than treating it as milestone-1 work

### Milestone 4: Feedback and rollout

- add shadow runs on real traffic
- capture user-behavior feedback signals
- define operational promotion criteria for moving routing into `/chat`

## Acceptance Criteria

Milestone 1 is complete when all of the following are true:

1. `/chat` remains unchanged in behavior and contract.
2. A new `agentic_search_v1` spec executes synchronously through `/agents/:id/execute`.
3. The runtime emits a structured `RouteDecision` and branches between baseline and agentic paths.
4. The retrieval contract exposes explicit milestone-1 tools instead of only `search_hybrid`.
5. Retrieved evidence carries enriched metadata and score provenance through server responses.
6. The baseline branch reuses the same extracted single-shot answer path as `/chat`.
7. The repository includes an offline benchmark harness and checked-in benchmark fixtures.
8. The benchmark shows no material regression on simple queries and a clear gain on the agentic subset.

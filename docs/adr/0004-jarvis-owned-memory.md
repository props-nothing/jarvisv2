# ADR-0004: JARVIS Owns Canonical Memory

- Status: Accepted
- Date: 2026-09-20

## Context

Models and agent runtimes expose convenient chat history, checkpoints, or memory stores, but each has different retention, provenance, retrieval, deletion, and portability semantics. Voice providers may also retain transcripts. Treating one as canonical would couple user identity and continuity to a replaceable vendor.

## Decision

JARVIS stores canonical typed memory with workspace scope, provenance, confidence, validity, sensitivity, correction/supersession, retention, and deletion. Runtime/provider memory is an execution cache or opaque binding only.

JARVIS selects and supplies eligible context to runtimes and accepts normalized memory candidates back through its own admission lifecycle.

## Consequences

- Users can inspect, correct, export, and delete memory consistently across runtimes.
- Runtime switching does not erase personal continuity.
- JARVIS must implement admission, hybrid retrieval, entity resolution, context budgets, and derived-index cleanup.
- Provider-side retention remains a separate disclosed concern.
- More integration work is required than delegating memory to a framework.

## Alternatives

- OpenClaw/LangGraph/OpenAI/provider memory as truth: locks JARVIS to one runtime and weakens cross-provider deletion/provenance.
- Chat transcript only: cannot represent durable preferences, relationships, procedures, validity, or corrections safely.
- Vector store as truth: embeddings are lossy indexes without adequate provenance or lifecycle.

## Revisit When

Canonical ownership does not change. Internal storage/retrieval implementations may change through storage ADRs while preserving the memory contract.
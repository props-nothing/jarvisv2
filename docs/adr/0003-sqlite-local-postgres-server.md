# ADR-0003: SQLite Local, PostgreSQL Server

- Status: Accepted
- Date: 2026-09-20

## Context

An installable personal assistant should work without database administration. Server and multi-device deployments need stronger concurrent writers, operations, and vector indexing. Requiring PostgreSQL locally harms onboarding; using only SQLite constrains server growth.

## Decision

Use SQLite as canonical storage in local/personal mode. Use PostgreSQL plus pgvector in server/multi-device mode. Put behavioral repository and unit-of-work ports above backend implementations while allowing backend-native SQL, migrations, locking, and indexing.

Large artifacts use local files in personal mode and object storage in server mode. Secrets remain in keychain/secret-manager providers, not either database.

## Consequences

- Default install has no external infrastructure.
- Repository contracts and migrations require cross-backend parity tests.
- SQLite uses a coordinated writer and local process ownership; PostgreSQL supports multiple workers later.
- Vector retrieval may have different physical implementations while preserving query semantics and explanations.
- SQL cannot be hidden behind a lowest-common-denominator generic CRUD layer.

## Alternatives

- PostgreSQL everywhere: operationally heavy for personal install.
- SQLite everywhere: insufficiently flexible for planned server concurrency and pgvector.
- Separate database per subsystem: premature operational and consistency complexity.
- Dedicated vector database in v1: unnecessary until measured scale requires it.

## Revisit When

Measured workloads exceed pgvector/SQLite search needs or a backend cannot preserve required durability/isolation. New storage engines require an ADR and migration/exit plan.
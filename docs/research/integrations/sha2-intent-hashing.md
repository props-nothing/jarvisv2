---
integration: sha2-intent-hashing
status: decided
last_verified: 2026-09-22
owners: []
selected_spec_version: SHA-256 (FIPS 180-4)
selected_sdk: sha2 0.10.9
---

# SHA-256 For Approval Intent Hashing And Nonce Digests

## Scope

In scope: choosing a digest implementation for two uses in durable approvals (`P3-004`).

- the **canonical intent hash** a decision binds to, required by
  `docs/architecture/security.md` ("exact canonical intent hash and human-readable preview") and by
  `docs/architecture/events-and-workflows.md` ("an approval stores the exact normalized intent hash");
- the **nonce digest** stored in place of the one-time decision nonce, so that a leaked row proves a
  decision happened without yielding the ability to make one.

Out of scope: password hashing (no passwords are involved), signing (an approval is not signed), and
any content-addressing of memory or documents (`P4`).

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| crates.io API | `https://crates.io/api/v1/crates/sha2` | 2026-09-22 | version, license, MSRV, features |
| crate source | `~/.cargo/registry/src/index.crates.io-*/sha2-0.10.9/` | 2026-09-22 | the actual resolved version, read from the vendored source |
| locked graph | `Cargo.lock` | 2026-09-22 | confirms the version is already resolved |
| FIPS 180-4 | the SHA-2 standard | 2026-09-22 | the algorithm's definition |

The crate was read from the **vendored source in `Cargo.lock`'s resolved version** rather than from a
documentation site. That is the method `docs/development/external-research.md` ranks as "official SDK
source", and this project has already been bitten once by reading a version that was not the one
resolved (`P3-002`: `referencing` 0.33.0 and 0.57.0 have different constructors).

## Verified Contract

### Versions And Features

- **0.10.9** is already in the locked graph as a `sqlx` transitive dependency, so declaring it adds no
  new package. That is a reuse argument, the same one `P2-007a` used to select `axum`.
- **MIT OR Apache-2.0**, both already in `deny.toml`'s allow list.
- Declared here with `default-features = false`. The default set is `std` plus `asm`; this project
  needs neither the assembly intrinsics nor `oid`, `zeroize`, or `sha2-asm`.
- The API used is `Digest::new`, `Digest::update`, and `Digest::finalize`, all in the crate's stable
  surface.

### Properties Relied On

- **Deterministic**: the same bytes always produce the same digest, which is what lets a stored intent
  hash be compared against a recomputed one.
- **Collision resistance** is not relied on as a proof of identity but as a bound on the ability to
  find a second action with the same digest. The threat is a modified action reusing an approval, and
  that requires a second preimage of a specific digest.
- **No key, no salt.** Both digests are of values that are already high-entropy (the nonce) or not
  secret (the intent), so the usual reasons to salt or to use a slow KDF do not apply. See "Rejected
  alternatives".

## JARVIS Mapping

- intent hash → `jarvis_core::CanonicalIntentHash`, computed from the tool, its version, and the
  canonicalized argument object, rendered as 64 lowercase hexadecimal characters and stored in
  `approvals.intent_hash`.
- nonce digest → the same function applied to the nonce, stored in `approvals.nonce_hash`. The nonce
  itself is never stored.
- The canonical form is an explicit **sorted-key** JSON rendering rather than `serde_json`'s default
  serialization. `serde_json::Map` is a `BTreeMap` unless the `preserve_order` feature is enabled, so
  the default is already sorted — but that is a property of a feature flag, and a future feature added
  for an unrelated reason would silently make two equal argument objects hash differently.

## Decisions

1. **Select `sha2` at the version already resolved**, with `default-features = false`.
2. **Domain-separate the parts with `\u{1f}`** (unit separator) when building the canonical text.
   Without a separator, `("a", "bc")` and `("ab", "c")` produce the same digest, so one approval would
   authorize a different tool. This is asserted by a test named for it.
3. **Bound the canonical text before hashing** (`MAX_CANONICAL_INTENT_CHARS`), so an enormous argument
   object cannot make the hash computation itself a denial of service.
4. **Require the arguments to be a JSON object.** A validated tool call's input schema is an object, so
   non-object arguments did not come from a validated call and cannot be canonicalized meaningfully.
5. **Store the digest, never the nonce.** A digest in a leaked row is inert; the nonce would be a
   bearer token, which `events-and-workflows.md` explicitly forbids.

## Rejected Alternatives

- **A password-hashing function (Argon2, bcrypt, scrypt) for the nonce.** Rejected: the nonce is 32
  bytes of platform randomness, so there is no dictionary and no guessable input; a work factor would
  add latency to every approval decision without adding strength. A slow hash is the right answer for a
  human-chosen secret, and this is not one.
- **A keyed hash (HMAC) for the intent.** Rejected: it would need a key, and the key would become the
  thing to protect. The intent is not a secret — it is the action's description, already stored in the
  preview — so keying it buys nothing. The binding comes from the digest being compared against a
  recomputation, not from the digest being unforgeable in isolation.
- **Storing the intent itself and comparing it.** Rejected: the arguments are tool payloads, which the
  logging rules exclude from durable records by default, and an exact-match comparison of a stored
  payload would put them back. The hash binds the decision without storing the content.
- **`blake3`.** Rejected on the reuse argument rather than on strength: it is not already in the graph,
  so it adds a package to a project that prefers not to. `sha2` is resolved, well-understood, and the
  choice was not otherwise close.
- **A hand-written digest.** Rejected without consideration: no part of this project needs one, and a
  hand-rolled hash is a category of mistake rather than a trade-off.
- **`md5` or `sha1` for the nonce digest.** Rejected: collision resistance is the property that is
  weakened, and there is no reason to accept a weaker primitive when a stronger one is already
  resolved.

## Unresolved

- **Length-extension.** SHA-256 is not resistant to it, which does not matter here: neither digest is
  used as a MAC or over a secret-prefix construction, and both are compared for equality rather than
  extended. Recorded because a future use of the same function for a MAC would need HMAC, and the
  reason it is safe today should not be lost.
- **Which of two `sha2` entries `Cargo.lock` resolves for this project.** The lock file has two
  versions in the graph (0.10.9 via `sqlx` and 0.11.0 another way). The workspace pins `=0.10.9`
  exactly, so this project's dependency is unambiguous, but the second entry remains in the lock file
  as some other crate's dependency. Not a problem; noted so a future reader does not treat the two
  entries as a mistake.

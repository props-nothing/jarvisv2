# TODO archive: P5 connectors, engineering notes

Moved out of `TODO.md` on 2026-10-10, **word for word and unedited**. It was about 3,060 lines, mostly 45 "continued" entries under `P5-005`, and made the backlog hard to read. `TODO.md` keeps the status and what is open; this file keeps the reasoning, the findings and the limits recorded at the time, including the research and the mutation checks behind each change.

The text below is historical: it describes the state of the code when each entry was written (for example "has no caller" was true then and, for the push and watch stack, still is: see the P5 section of `TODO.md`). Search it with the entry titles, the `ADR-xxxx` numbers, or the module names (`google::watch`, `google::channel`, `google::teardown`, ...).

---

## P5: Connectors

- [x] `P5-001` Define connector manifest, account, auth flow, health, sync cursor, webhook, rate-limit, scope, and diagnostics contracts.
      **New crate `crates/jarvis-connectors`** (8 modules, **80 tests**), an adapter depending only on
      `jarvis-core` + `jarvis-tools`. `ADR-0054`.
      The manifest is a *security* document: `docs/architecture/tools-and-connectors.md`'s "parseable without
      loading provider code" is a **trust** requirement, not a packaging one, because the manifest is the only
      artifact an operator reads before granting access and the provider code it describes is what has *not*
      run. So the module's job is not to list fields but to **check its own claims against each other**:
      - **Effects are per operation and the classification is cross-checked.** A connector whose operations
        reach outward may not declare `Public` or `Internal` — those are exactly the levels
        `Classification::may_reach_a_remote_model` permits, so the combination mislabels content leaving the
        machine. The rule is a **floor, not an equality**: a mail connector legitimately declares
        `Confidential` because what it *handles* is confidential, and requiring equality would push every
        outward connector to the top of the ladder, making the classification meaningless. Tested in all three
        directions (two low levels refused, three high accepted, a read-only connector allowed `Internal`).
      - **A secret is a *field name*, never a value.** No `String` in the crate holds token material; that
        absence is the mechanical form of `security.md`. Names are environment-variable-shaped because they
        become config keys, and a name that could not be one would fail at deployment.
      - **Documentation links must discharge the research requirement.** `LinkKind`'s variants *are*
        `external-research.md`'s ordered source list (`LlmsTxt`, `Documentation`, `Specification`, `Sdk`, …), so
        a manifest with only a homepage is **incomplete rather than terse** — `satisfies_research_requirement()`
        must be true for at least one link. Plus a `ResearchRecord` path that is refused if absolute or holding
        `..`/`:`.
      - **The risk floor is checked here**, the first place an operation's risk is stated. Deliberately *not*
        checked: `P3-001`'s blind-retry rule — a non-idempotent outward operation is legitimate and common, and
        **this type carries no retry policy to disagree with**. That pairing becomes expressible in `P5-009`;
        the boundary is written down so a later reader does not "fix" it by refusing every write connector.
      - **A cursor's *kind* names what may be concluded from it.** `Start` is a variant with **no token**, not
        "no cursor yet", and a `Start` carrying a token is refused because it makes *never synced* and *synced
        to here* indistinguishable — the one distinction the type exists to keep. Bound to the account **and
        connector version** (`applies_to`), since a 1.0.0 cursor applied after a 2.0.0 upgrade is a silent
        misread.
      - **Health carries its probe, and staleness takes a *supplied* instant.** A state without its probe is an
        assertion with no evidence: "connected" by an identity call and by a subscription are different claims.
        `unix_nanos` comparison, not RFC 3339 text (`P3-004`'s trap: a zero fraction makes the text not sort).
      - **`Unknown` never permits a call, and `RetryClass::Unknown` never retries whatever the idempotency
        says.** Two rules, one shape: *the unanswered question refuses*. `Transient`/`Throttled`/`ProviderFault`
        delegate to the provider's idempotency claim; `Unknown` returns false in **every** column because the
        question is unanswered rather than because the provider said no.
      - **A webhook body is raw bytes, and an ambiguous security header is refused rather than picked.**
        `single_header` returns `None` for zero **and** for more than one: a repeated security header is either
        an accident or an attack, and taking the first turns *two signatures disagree* into *one was valid*.
        Only `Replayed` is acknowledged to the provider, so a genuine retry stops and a forgery gets no signal.
      - **The diagnostics field set is closed so `is_loggable()` is true for all twenty.** The constancy is the
        point — a field needing an exception would mean a value in the type was not safe to log.
        `may_reach_a_model()` is false only for `ProviderRequestId`, and **not because it is a secret** but
        because `security.md` minimizes what reaches a model rather than deciding per call site. A cursor's
        token is never a field, only its kind and observation time.
      - **PKCE against RFC 7636's own numbers**, with the S256 challenge asserted on **Appendix B's published
        vector** rather than a second implementation of the same mistake. `is_loopback_redirect` matches whole
        hosts over `http://` only, since a substring check accepts `127.0.0.1.evil.example`.
      **Two defects this slice found in itself, both from writing the tests, both an implementation disagreeing
      with its own doc comment:** `VerifiedAccount::new` checked `is_empty()` rather than `trim().is_empty()`, so
      `"  "` — an identifier whose whole purpose is to be usable — was accepted; and `ConnectorHealth::is_fresh_at`
      rejected only `checked_sub`'s overflow, so **a future observation was reported as fresh**, letting a record
      whose clock moved backwards look current.
      **All 12 guards falsified in an A-B-A design** (guard intact *passes* → neutered *fails* → restored
      *passes*). A′ is load-bearing: restoring with `Copy-Item` sets the mtime **backwards**, older than the
      mutant build, so cargo can run the stale mutant again — which happened here and produced a false
      "SURVIVED" for a guard whose test does catch it. The first harness also produced two false verdicts
      because the *mutant* was wrong, not the guard (`elapsed < -1000000` is still true one second in the
      future; one probe mutated `needs_full_resync_when_lost` while the test exercised `requires_full_resync`).
      **Limits:** **nothing consumes this crate** — no `Connector` trait, no HTTP, no RNG (so no code path
      *generates* a verifier), no loopback listener, no `SecretStore`, no persistence, no daemon wiring
      (`P5-002`+). No signature is verified (`P5-010`), no pagination is followed, no rate limit is enforced,
      and the manifest checks **self-consistency, not truth**: a connector that lies consistently is caught by
      review and the recorded research, not by a type. The ceilings are chosen, not measured.
- [x] `P5-002` Implement shared OAuth 2.0 Authorization Code plus PKCE flow, state/nonce validation, loopback callback, refresh rotation, revocation, and secret references.
      **New modules `authorization` + `token`** (36 tests, so **116 in `jarvis-connectors`**). `ADR-0055`.
      Research record `docs/research/integrations/oauth2-pkce-native-apps.md` written **before** the code, from
      the live RFC texts (RFC 7636, RFC 8252 BCP 212, RFC 9700 BCP 240, with RFC 6749 + RFC 9207 as
      referenced). `getrandom` added — already in the workspace and the lock file, so **no new package**.
      - **The transaction is consumable once, by construction.** `consume(self, ..)` takes `self` by value, so
        "answer the same setup twice" has no expression — RFC 9700 §4.2.4's "SHOULD be invalidated after its
        first use" as a property of the type rather than a caller's discipline. It carries the verifier, method,
        state, nonce, **the redirect URI**, the optional issuer and the instant in ONE value, because RFC 8252
        §8.10 requires exactly that grouping ("MUST store the redirect URI […] along with 'state'").
      - **A loopback redirect is a two-variant type, so nothing else is representable.** `LoopbackHost` is
        `127.0.0.1` or `[::1]`, so `http://evil.example/cb` and **`http://localhost/cb`** cannot be constructed
        rather than being checked. `localhost` is excluded deliberately: RFC 8252 §8.3 says NOT RECOMMENDED
        because it "avoids inadvertently listening on network interfaces other than the loopback interface" and
        is "less susceptible to […] misconfigured host name resolution" — a name whose meaning depends on a
        resolver cannot ground "only reachable from this machine".
      - **The port varies and nothing else does, in the direction the RFCs specify.** `matches_except_port`
        (RFC 9700 §2.1's "exact string matching except for port numbers in localhost redirection URIs"), plus
        `matches_exactly` for the portless *registration* versus the ported *listening* URI. Tested both ways
        (a different port accepted, a different **path** refused), since a one-sided test passes on a check that
        refuses everything.
      - **`state` is always sent even though PKCE can substitute.** RFC 9700 §2.1 permits PKCE-as-CSRF only
        after confirming PKCE support and §4.7.1 makes that confirmation a **MUST** — so rather than make safety
        depend on a discovery step, the state is unconditional and `parameters()` has no branch without it.
        `StateMissing` and `StateMismatch` are distinct refusals; the value comparison is constant-time.
      - **`nonce` is carried and handed back, unvalidated.** RFC 9700 §4.5.3.2 makes it meaningful only with an
        ID token, and verifying one needs OIDC (signature/`iss`/`aud`/`at_hash`). A `validate_nonce` that could
        not validate anything would be the declared-but-unconstructed pattern this repo has removed four times.
      - **The mix-up defence has three states and only one is a refusal.** `NotNeeded` / `IssuerConfirmed` /
        `NotSatisfied` / `IssuerMismatch`. RFC 9207's `iss` is **optional**, so its absence is not evidence of an
        attack — collapsing `NotSatisfied` into a refusal breaks every conforming server that omits it, and
        collapsing it into `IssuerConfirmed` reports a defence that never ran. Only a mismatch aborts.
      - **The listener is a trait whose capabilities are reported, security separated from compatibility.**
        RFC 8252 §8.3 + B.3 (`SO_EXCLUSIVEADDRUSE`) + B.5 ("SHOULD NOT set `SO_REUSEPORT`/`SO_REUSEADDR`") are
        one rule for two OSes: **no second binder on the port**. Both-stacks (§7.3) is a *compatibility* concern,
        so `UnmetListenerRequirement::is_security` is false for it — conflating a single-stack listener with an
        off-machine-reachable one makes a real gap dismissible as cosmetic.
      - **`TokenSet` has no access-token field, and that absence is the design.** Extends `P5-001`'s absence to
        the exchange: the lifetime, the granted scopes, the scope change and a `SecretRef` **locating** the
        refresh material, plus `has_refresh_token: bool` rather than the text. So no token material can reach a
        model context, log, diagnostic, or column even by mistake. A test asserts the `Debug` rendering contains
        neither `access_token` nor `refresh_token`, which is what catches a field added later.
      - **Retry-safety is a variant, because the two cases are indistinguishable and opposite.** `NeverSent`
        (safe) vs `SentAnswerUnknown` (**not** safe): RFC 9700 §4.2.4 makes a retry of a lost-answer request
        able to get `invalid_grant` **and destroy a working grant** the first attempt issued. Same shape as
        `P3-001`'s `RefusedBeforeReaching`/`AmbiguousAfterReaching`, one protocol over.
      - **Rotation is detected, because noticing is what makes replay visible.** RFC 9700 §4.14.2's detection
        property needs the client to see which shape came back; a client that kept its old token discards the
        defence. `Rotated` is reported **even when the caller supplied no reference**, since a caller's storage
        defect must not hide the half of the exchange that makes replay detectable.
      - **The transient check outranks the error code — and that ordering is the falsification run's finding.**
        A 503 carrying `invalid_grant` is a real shape (a proxy picks the code), and checking the grant first
        would send the user to a consent screen **during a provider outage**. The first mutation of this
        SURVIVED, and diagnosing why found the **fixture** was weak: it used `server_error`, for which both
        orders agree. A fixture with a transient `invalid_grant` is the only shape where they differ, and the
        guard then falsified.
      - **`invalid_grant` needs the user; a client misconfiguration does not.** RFC 6749 §5.2's `invalid_grant`
        covers "invalid, expired, revoked, does not match the redirection URI […] or issued to another client",
        all needing a new authorization. `invalid_client`/`unauthorized_client` mean the *client* is broken, so
        a consent screen lands on the same failure — they map to `Transient`, whose `needs_user` is false.
      - **A revocation's failure is not the same claim as its success.** RFC 7009 §2.2 makes "already invalid" a
        **success** (200 "if the token has been revoked successfully **or** if the client submitted an invalid
        token"), so `AlreadyInvalid::is_withdrawn` is true while `Unsupported`/`Refused`/`Unreachable` are not.
        Local material is discarded on every outcome, and an always-`true` predicate written here first was
        **removed** — a predicate with one answer is not a predicate.
      - **The generated verifier is RFC 7636 §7.1's own recipe** (32 octets → 43 unpadded base64url chars, the
        RFC's floor) and goes **through `new()`** anyway, because the generator and the validator are two
        implementations of one rule. `plain` stays representable (a server MAY ignore PKCE entirely, §5) but is
        not reachable by accident: `code_challenge_method` is always sent, removing the `plain` default's effect.
      **TWO DEFECTS THIS SLICE FOUND IN ITSELF, both a claim disagreeing with the code.** A doc comment said a
      pathless redirect URI would be refused; the code normalises it to `/`, which is what **RFC 3986 §6.2.3**
      says it is — the test written from the claim is what surfaced it. And a scope-loss fixture asserted a loss
      from a grant that had lost nothing (`previous=[mail.read, calendar.read]`, `current=[mail.read,
      calendar.read, new.scope]` is a pure *gain*), so the precedence rule was never exercised; the fixture now
      loses one scope **and** gains another.
      **All 25 guards falsified, A-B-A** (intact passes → neutered fails → restored passes). Two did not prove
      first time: one mutant was invalid (it left its `match` unbalanced — the second attempt kept the arm and
      made it unreachable), and one SURVIVED, which is what produced the ordering finding above. The harness
      restores with `WriteAllText` + a forward mtime bump, because `Copy-Item` restores the **older** timestamp
      and cargo then re-runs the stale mutant — the trap `P5-001` recorded.
      **Limits:** **nothing consumes this crate** — no `Connector` trait, HTTP client, socket, `SecretStore`,
      persistence, or daemon wiring. **No live verification**, deliberately: a live test needs a vendor,
      registered credentials and a browser, i.e. the per-connector smoke test (`P5-005`+). `nonce` carried but
      **not validated** (OIDC not implemented, so injection defence rests on PKCE alone). The mix-up defence
      depends on a vendor sending `iss`. `Expired` vs `Revoked` is the caller's classification (no code
      distinguishes them). **Sender-constraining is not implemented**, so a provider that does **not** rotate is
      a configuration this client cannot make RFC 9700 §2.2.2-compliant on its own — the strongest reason DPoP
      is on the roadmap. No automatic token-request retry, by design. No `localhost`, and no flag to enable it.
      `AuthError::RandomUnavailable`'s branch is **untestable here** (the platform source does not fail on
      demand), so it is verified by reading.
- [x] `P5-003` Create connector quality checklist and scaffold generator modeled on manifest-driven integration projects.
      **New modules `readiness` + `scaffold`** in `jarvis-connectors` (24 tests, so **140 in the crate**), and a
      **`jarvis connector` verb group** in `jarvis-cli` (`new`, `check`, `items`). `ADR-0056`.
      **49 suites, 1393 tests, 0 failed, 0 ignored** (was 49/1368). The lock delta is one line — `jarvis-cli`
      depending on a crate already in the workspace — so **no package joined the tree**.
      - **The checklist is a program, not a Markdown file, and the reason is `AGENTS.md`'s hardest-won rule:** a
        recorded claim is read downstream as *verified evidence* with nothing distinguishing "checked" from
        "assumed". A prose gate is read once and then trusted. `docs/architecture/tools-and-connectors.md`'s
        "Connector Completion Gate" is the checklist; this evaluates it.
      - **Every item is one of four evidence kinds, and the kind is reported.** 3 `Derived` (inside the manifest,
        so it cannot be asserted falsely), 3 `DeclaredArtifact` (a path, shape-checked), 5 `DeclaredCount` (a
        number), 1 `Conditional` (refusable, with a reason). A gate that printed a uniform "ok" would hide that
        **nine of twelve rest on the author's word and three on the manifest** — so the counts are asserted in a
        test and `jarvis connector items` prints the kind beside each item.
      - **⭐ THE SLICE'S CENTRAL DEFECT: the two webhook items were classified as `Derived` from the manifest.**
        That conflates **applicability** with **evidence**: *does this connector need webhook tests* is a
        manifest fact, but *does it have them* is not, and a manifest declaring `hmac_sha256` proves nothing was
        tested. The gate would have reported a push connector **complete** with no signature or replay test —
        exactly the condition `WebhookRejection` exists to make visible. The test asserting a push connector
        without those suites still reports them as gaps is what caught it (`got []` for the two expected).
        `applies_to` and `evidence` are now separate questions with separate answers.
      - **⭐ A SECOND DEFECT ONLY RUNNING THE COMMAND COULD FIND: the sidecar path.** The first version used
        `Path::with_extension("readiness.json")`, which replaces only the **last** extension, so
        `vendor.manifest.json` gave `vendor.manifest.readiness.json` while `connector new` writes
        `vendor.readiness.json`. The documented workflow produced a connector whose every declared item silently
        became a gap. **No unit test could have caught it** — both sides were "correct" against the same wrong
        assumption and disagreed only when the two verbs ran in sequence. Now one `sidecar_path` function with
        a regression test, so the writer and the reader share a rule.
      - **⭐ A THIRD: an unreachable refusal inside `EvidencePath`.** A separate `chars().nth(1) == Some(':')`
        drive-prefix branch, commented "a colon is legal later in a path on unix", **could never fire** because
        the next check refuses a colon *anywhere*. An unreachable refusal reads as protection while enforcing
        nothing — the defect `P5-001` recorded for an unreachable bound, found here **by mutation** rather than
        by reading. Removed.
      - **Applicability is derived and a wrong attestation is REFUSED, not dropped.** `applies_to(&WebhookSupport)`
        omits a non-applying item from the review *and* refuses an attestation naming it
        (`NotApplicable`), because a silent drop hides that the author is working from a template rather than
        from their own connector.
      - **A `ReadinessReview` exists only when the items are satisfied**, so "is this complete" is answered by
        whether the value exists (`ADR-0037`'s and `ADR-0055`'s move). `gaps` is the complement and names
        **every** outstanding item at once rather than refusing at the first.
      - **Each declared kind has a shape, and the wrong shape is an error.** Artifact ⇒ path, no count; suite ⇒
        count ≥1, no path (`None` and `Some(0)` are different claims — "not reported" vs "there are none");
        every item ⇒ a purpose; `covers_failure` ⇒ meaningful for **one** item and refused elsewhere, because a
        caller that set it believed it said something; onboarding **must** report failure coverage, since the
        document says "successful **and** failed".
      - **`EvidencePath` checks shape, not existence, and its doc says so.** It refuses empty/whitespace-padded,
        absolute, home-relative, `..`, a colon, an unknown extension, and >512 chars — and **cannot** prove a
        file exists, because this crate has no filesystem. The honest claim is "a path that *could* name a
        repository artifact", and that limitation is in the type's own docs.
      - **The conditionally-refused item carries its reason in the type.** `LiveSmokeTest` is
        `Present { path, gate }` or `Absent { reason }` and **both** constructors refuse a blank string, so the
        rule lives where the value is built. `ReadinessAssessment` carries the **strength explicitly** rather
        than letting a reader infer it from `declared_by.is_some()` — the conditional item's `declared_by` is
        `None`, so the first version made a stated *reason* indistinguishable from a manifest fact.
      - **The scaffold is generated from the manifest's own constructor**, so it **cannot be stale** — a
        committed template drifts the moment `ConnectorManifest` gains a field, and the failure surfaces far
        from the template. It refuses to invent three things: operations (empty, because `ConnectorManifest::new`
        refuses an empty list, so the skeleton is **deliberately invalid**), auth methods/secret fields/links
        (vendor facts), and the research date (**`--research-date` is required**, because defaulting it would put
        a real date in `last_verified` on a record whose every section is blank — a stub that looks verified).
      - **The skeleton is provably not a manifest**: it carries `_comment` keys and
        `deny_unknown_fields` refuses it. The test asserts the refusal, every field name, the emptiness of the
        four collections, the classification's starting level (`confidential`, since `ADR-0054` refuses below it
        for an outward operation), and that **every non-field key starts with `_comment` and is non-trivial**.
      - **The skeleton is built with `serde_json::json!`, and the first version was a `format!` template that
        produced invalid JSON** — its comments quote JSON examples, so their quotes needed escaping and were not.
        A hand-written template that must quote a document inside itself is a defect waiting for an edit.
      - **The two verbs are local, and that is a property of the subject**: a manifest and a research record are
        repository artifacts reviewed in a pull request, so putting them behind the daemon would mean starting a
        service to write a file the service must read back. `check` exits `DoctorWarnings` (6) rather than a
        failure code, because an unfinished connector is normal work in progress.
      - The attestations live in a **sidecar** (`<connector>.readiness.json`), not in the manifest, because
        `deny_unknown_fields` would make a readiness section part of the document an operator reads to decide
        whether to grant access — and evidence about tests is not a fact about authority.
      **All 21 guards falsified A-B-A.** Three findings beyond the defects above: **two tests were insensitive to
      their mutation and the fix was the assertion, not the mutant** (the skeleton test accepted *any*
      unknown-field refusal, so renaming a `_comment` key to `note` still failed to deserialize and the test
      passed); **two probes were written from a guess** rather than read from the file (a four-line formatted
      refusal; the JSON `"_comment"` string); and **a harness bug produced a uniform false negative** —
      `cargo test -p a -p b <filter>` treats `<filter>` as another `-p` pattern, so the first run reported
      "vacuous" for all 21 cases because **no test ran**. A harness that runs nothing reports success-shaped
      output, which is precisely what the A-B-A design exposed.
      **A `--force` flag the usage string promised but the program did not have was found while writing the ADR
      and fixed rather than recorded** — a one-line correction is cheaper than a documented inconsistency.
      **Limits:** **nothing consumes the review** — no connector exists, no daemon reads a sidecar, and no CI job
      runs `connector check`. `EvidencePath` **cannot prove a file exists**, so a connector can pass while
      naming three artifacts that do not. **Nine of twelve items rest on the author's word**, and the counts are
      **not compared against anything** — nothing runs a suite and checks the number. The scaffold writes two
      documents and nothing else (no crate, no test scaffolding, no registry entry). `check` reads one manifest
      at a time and has no notion of a connector repository.
- [x] `P5-004` Research Google identity, Gmail, Calendar, push notifications, quotas, and restricted scopes; record findings.
  - `docs/research/integrations/google.md`, fifteen live official sources with access dates, indexed in
    `docs/research/integrations/README.md`. **No Google API was called, no credential was used, and no Cloud
    project was created**; the record says so and claims no live verification.
  - **`llms.txt` is `not found`**, not assumed absent: `https://developers.google.com/llms.txt` and
    `https://developers.google.com/gmail/api/llms.txt` both return **HTTP 404**. Recorded with the substitution
    (the official guide and reference pages), because `external-research.md` requires a missing index to be
    recorded as missing rather than silently worked around.
  - **The research found a defect in shipped code, and the defect is the substantive outcome of this slice.**
    Writing down Google's two push mechanisms — an OIDC bearer JWT for Pub/Sub, an echoed channel token for
    Calendar, with a **zero-length body** so there is nothing to MAC — required explaining why neither fits
    `WebhookSupport::Push`. Reading the type to explain that showed that `SignatureAlgorithm::None`'s doc claimed
    the value "is refused by `WebhookBinding::new`", that `WebhookBinding::new` validates the *binding* and never
    sees the algorithm, and that `SignatureScheme::authenticates()` was reachable **only from tests**.
    `ConnectorManifest::new` never validated the webhook at all, so a manifest could declare `push` with an
    authenticator of `none` and be accepted — an endpoint applying unauthenticated writes, declared in a
    document a reviewer would read as having provided a control. **Fixed**, not merely recorded:
    `validate_webhook` + `ConnectorError::Webhook`, with a refusal message that names the honest alternative
    (`polling`). Two tests: the refusal, and an **over-refusal guard** proving all three real algorithms and
    `polling` are still accepted — because a rule that refused every push declaration would satisfy the first
    test while making the capability unusable.
  - **Guard falsified with the three-run A-B-A design**, not asserted. A (intact) `1 passed`; B (branch
    replaced with `false`) the refusal test **panicked at `manifest_tests.rs:1069`**; A′ (restored,
    `mutant=0 guard=1` verified by regex count) `2 passed`. The first attempt used the filter `verify_nothing`
    and reported `0 passed; 142 filtered out` — **a vacuous run that reads as a pass**, the trap `P5-003`
    recorded; the correct filter is `verifies`. Recorded because the near-miss is the reusable part.
  - **A second false doc claim was corrected in the same type**: `is_keyed_mac`'s comment said "a keyed MAC is
    the only mechanism that authenticates the **bytes**", which `Ed25519` contradicts — it authenticates the
    bytes with a public key, so the property the method actually answers is "does verifying require a secret".
    The `Ed25519` and `None` variant docs were rewritten to match. Both were claims that read as protections.
  - **Two findings reach past this integration.** (1) **Stale cursors arrive as ordinary status codes on a
    read**: Gmail returns **HTTP 404** for a `startHistoryId` outside the retained window, which is the same
    status as an absent account, while the required remedy is a full resync; Calendar returns **410 Gone**. So
    `SyncCursorKind::can_be_detected_as_stale()` is answered "only from the read's response, not the cursor's
    shape", and a Gmail connector must classify a 404 on `history.list` specifically. (2) **A 200 from a Gmail
    send does not mean the mail was sent** — the page says so verbatim, the quota is shared with the user's web
    client and IMAP, and 429s can lag "several minutes" — so `P5-009`'s send must be non-idempotent, never
    auto-retried, and reported as *submitted* rather than *delivered*.
  - **A third finding is an alternative to `P5-005` itself:** Google publishes a first-party **Gmail MCP
    server** (`https://gmailmcp.googleapis.com/mcp/v1`) in **Developer Preview**, which `jarvis-mcp-transport`'s
    existing `StreamableHttpClientTransport` could reach. Recorded as a **second surface, not a substitute** —
    ten tools, no `watch`, no `history.list`, no send, no sync, and its tools would arrive as an unclassified
    third-party source held for approval.
  - **Limits:** the record **cannot state a Calendar scope's category** — the Gmail page gives categories per
    scope, the Calendar page lists scopes without them, and the consent page gives the category *table* but not
    the mapping, so it is an Unresolved Question rather than a guess. **Whether a self-hosted single-user JARVIS
    qualifies for Google's internal-app exemption is unverified** and is the difference between days and months
    of lead time before a Gmail connector is usable by its own author; **`quotaUser`/`userIp` behaviour was not
    checked**, and it decides whether a multi-mailbox connector pays the 6,000-units/minute ceiling once or per
    mailbox. Gmail read scopes are **all restricted** — including `gmail.metadata`, the least-privileged way to
    read a mailbox — meaning a security assessment if the data is stored or transmitted. The quota model changed
    **2026-05-01** and charges are announced as pending later in 2026, so quota must be **configuration, not a
    constant**. **Six unresolved questions** are listed with impact and blocked capability; **no test for this
    research exists** and the record's Verification Plan names them as `P5-005`'s work.
- [ ] `P5-005` Implement Google connection setup and Gmail/Calendar read tools with recorded wire fixtures.
  - **PARTIALLY DONE, so the box stays unchecked.** Done: the connector's declared contract and its **auth
    flow** — `crates/jarvis-connectors/src/google.rs` (`GoogleConnector::manifest()`, `auth_flow()`, the three
    endpoints, the registered redirect) plus `google_tests.rs`. NOT done: the provider client, **any**
    operation implementation, and **no wire fixture** — which is the other half of the task.
    `CompatibilityVerdict::Unverified` says so in the manifest itself, and `is_installable()` is false.
  - **The declared contract** (`ADR-0057`, previous commit): three read operations at risk 0, `oauth_pkce`, an
    empty secret list, `Confidential` classification, `Polling { interval: Unknown }`, and the documented Gmail
    quota as a `PerClient` limit. The one link per `LinkKind` constraint is real for a multi-API connector, so
    Gmail's contract is the link of record and Calendar's pages live in the research record.
  - **This round settled the auth flow, which the previous round deliberately refused to guess.**
    `AuthFlow::new` requires an `https://` authorization endpoint and a redirect URI, and `P5-004`'s record did
    not establish either — so the previous commit exposed a bare endpoint constant and **constructed no flow**.
    Two authoritative sources fixed that: **`https://accounts.google.com/.well-known/openid-configuration`**,
    which is the one source here that is **machine-readable** (the server's own published configuration rather
    than a page's example), and the OAuth 2.0 for native apps guide.
  - **Verified endpoint facts**, now in the record with the truncation of
    `token_endpoint_auth_methods_supported` **recorded rather than glossed**: `authorization_endpoint`
    `https://accounts.google.com/o/oauth2/v2/auth`, `token_endpoint` `https://oauth2.googleapis.com/token`
    (**a different host from the consent screen**, which a reader might "fix"), `revocation_endpoint`
    `https://oauth2.googleapis.com/revoke`, `userinfo_endpoint`, `jwks_uri`, `code_challenge_methods_supported`
    = `plain`+`S256`, and **`authorization_response_iss_parameter_supported: true`** — which is what gives
    `AuthorizationTransaction::with_issuer` something to check instead of a permanent `NotSatisfied`.
  - **The loopback method is the only non-embedded option**: the page calls it the recommended desktop
    mechanism and states that **custom URI schemes are no longer supported** "due to the risk of app
    impersonation", with the OOB copy/paste method deprecated. Google's own advice also discourages `localhost`
    because of client firewalls — a second, independent reason for the refusal `P5-002` already makes.
  - **Three more facts that change how the connector must behave**, all recorded:
    **`client_secret` is `Optional`** on both the code exchange and the refresh ("not applicable to requests
    from clients registered as Android, iOS, or Chrome applications"), which is what the manifest's **empty
    `secret_fields`** rests on; **refresh-token issuance is limited** so "older refresh tokens will stop
    working" — a deployment that re-authorizes repeatedly can invalidate the token it was relying on; and
    **revocation removes the project's grants**, not one account's, taking "some time" to take effect, so a
    disconnect must not assume its effect is scoped to the account it named.
  - **`id_token` is expected on the exchange** because the manifest requests `openid`, and `P5-002` records that
    `nonce` is carried but **not validated**. The discovery document supplies `jwks_uri`, so the verification is
    now unblocked but still unbuilt; recorded as a limit rather than a capability.
  - **DPoP is recorded as a decision, not a flag.** Google supports it, recommends it, and (for a code exchange)
    requires `jti = BASE64URL(SHA256(AUTHORIZATION_CODE))` with a cacheable `DPoP-Nonce`. It binds the **refresh
    token** to a private key Google advises storing so it "cannot be copied off-device, for example by using
    TPMs, Secure Enclaves, or other hardware-backed keystores" — which is exactly the sender-constraining
    `P5-002` records as **not implemented** and whose own ADR calls the strongest argument for the work.
    Bypassing it is safe because it is optional; adopting it is a slice.
  - **One fact is still not established, and it is a question rather than a guess.** Google's page shows the
    *exchange request* using a ported URI (`redirect_uri=http://127.0.0.1:9004`) and requires `redirect_uri` to
    match an authorized URI **exactly**, but does **not** state which string the console accepts as the
    **registered** value for a Desktop-app client. Recorded as Unresolved Question 7. The connector uses the
    **portless** form because that is what `P5-002`'s own rules produce and `matches_except_port` compares, and
    `the_registered_redirect_is_the_portless_loopback_form` asserts the comparison works **while explicitly not
    claiming** the console accepts it.
  - **Falsified, one guard A-B-A**: the redirect comparison joining a registration to a listener
    (`self.host == other.host && self.path == other.path`), mutated by dropping the path half. Intact `PASS`,
    mutant `FAIL`, restored `PASS`, restore byte-identical.
  - **Limits:** no client, no operation implementation, **no wire fixture**; the registered redirect form is
    unconfirmed against Google's console (Unresolved Question 7); `nonce` is not validated, so the ID token is
    received but unverified; nothing is callable, because no `ToolDefinition` is derived from the operation ids;
    and **no token has been exchanged**, so `token_endpoint` and `revocation_endpoint` are transcribed and
    asserted but never used.
  - **This round added the provider's decisions and the documented directory shape.**
    `crates/jarvis-connectors/src/google/` is now `mod.rs` (the contract), `client.rs` (the provider's
    decisions), `tests.rs` and `client_tests.rs` (16 new tests, so **181 in the crate**), which is
    `repository-layout.md`'s integration shape. **`ADR-0058`.**
  - **The decisions are pure functions of a response, because this crate has no HTTP stack** — and that is the
    point rather than an accident. `classify` maps a status and reason to a `RetryDecision`; `next_page` bounds
    a page token; `advance_gmail_history` and `advance_calendar_sync` map a response to a successor or to a
    resync. Nothing here can send a request, so every branch is testable without one, and the transport binding
    stays a named step instead of becoming the shape the rules are written against. `jarvis-models`' injected
    `Transport` is the precedent, and the Google transport binding is now a bounded piece of work.
  - **The classification table is a function of Google's own error vocabulary, not of its status codes.**
    `GmailErrorReason` is the closed set the error page names, and a **403 is classified by its `reason`**
    because four documented reasons share that status with three different remedies. The case that justifies the
    table is **`domainPolicy`** — "the domain administrators have disabled Gmail apps" — which arrives as a
    `403` exactly like the two throttling reasons and whose remedy is a conversation with an administrator, not
    a retry. A status-only classifier retries an administrator's decision forever.
  - **An unknown reason is representable and classified conservatively.** `GmailErrorReason::Unrecognised`
    exists because Google adds reasons and refusing to parse one would turn "a new error code" into "a
    connector that cannot read its own errors"; its classification is the **status's**, so it can never loosen a
    decision. For a 403 that means `Permanent`/`DoNotRetry`, argued in the code: a 403 is a refusal with **no
    effect**, so `Unknown`/`Reconcile` would send a caller to establish whether an effect happened when the
    status already says it did not. A new *throttling* reason appearing as a 403 is the cost, and it fails in
    the direction that cannot cause a second effect.
  - **`GmailErrorBody` has no field for the error message**, so the classification cannot be derived from message
    text even by accident — the structural form of `P3-008c`'s "do not derive a safety flag from message text".
    `jarvis-models`' `McpToolListing` uses the same technique for a server's annotations.
  - **Two guards that encode provider-specific judgement, both falsified A-B-A with compiling mutants:**
    the 403-permanent arm (mutated to throttled: intact `PASS`, mutant `FAIL`, restored `PASS`), and
    `gmail_history_status_is_pruned` (mutated `status == 404` → `status < 404`: same shape). A **third** guard,
    the backwards-`historyId` refusal (`next_id < previous_id` → `false`), was also falsified. **The first
    attempt at two of the three was worthless and the record says why**: one mutant did not compile (so the
    verdict was `VACUOUS`, not a weak guard) and one multi-line probe matched nothing because PowerShell
    here-strings are CRLF while these files are LF — both traps this workspace has recorded before, hit again.
  - **A monotonic cursor may not move backwards, and an opaque one may not be compared at all.**
    `advance_gmail_history` refuses a smaller `historyId` because `historyId` increases, so a smaller value is a
    stale or foreign response and storing it would re-walk processed history — a repeat, for a connector that
    acts on changes. `advance_calendar_sync` performs **no** ordering check, because `nextSyncToken` is opaque
    and comparing two would invent a property the provider never offered.
  - **Google's numbers are constants with tests around their relationship**, not prose:
    `GMAIL_BATCH_LIMIT` (50) and `GMAIL_MAX_RESULTS_CAP` (500) with an assertion that the first is below the
    second, because batching is what makes a full sync affordable *and* is itself a rate-limit trigger —
    confusing the two would ask for 500 sub-requests at once.
  - **Limits:** no request has been sent and **no response has ever been parsed**, so the tests prove the code
    implements the *record* and nothing about the record matching Google; there is no transport, no token
    source, and no operation, so nothing builds an authorization request or calls `users.messages.list`;
    `GOOGLE_MAX_BACKOFF_SECONDS` has **no caller and is carried by no decision**, because
    `RetryGuidance::BackoffSeconds` states a starting delay and a ceiling would need a field the shared type does
    not have; and `SyncAdvance::Refused` is **never constructed**, which by `P5-001`'s standard is a variant that
    reads as a live condition.
  - **This round derived the model-facing tool definitions from the manifest.**
    `crates/jarvis-connectors/src/google/definitions.rs` + `definitions_tests.rs` (14 new tests, so **195 in the
    crate**). **`ADR-0059`.** `definitions()` reads the manifest and produces one `ToolDefinition` per declared
    operation, so every effect, risk, scope and idempotency value comes from the operation it belongs to and a
    definition **cannot disagree with the manifest** — the rule `P3-006d` established for a native adapter,
    extended to a connector. Only what the manifest has no field for is stated here: the schemas, the title,
    the timeout and the retry policy.
  - **The identifier prefix is load-bearing.** `google.<operation id>`, because `ToolSource::from_namespace`
    classifies by the **leading segment** — so the prefix is what makes these `Connector` tools rather than
    `Native` ones, and a connector tool that could declare itself `Native` would be claiming JARVIS wrote the
    adapter for a third party's API. A test asserts the classification, which is also what stops a future edit
    from renaming the namespace.
  - **The two idempotency vocabularies are mapped explicitly and totally.** The distinction that matters is
    between the two *safe* variants: `Declared` means the provider makes a repeat a no-op, so **no JARVIS key
    is needed**, while `ProviderKey` means the caller must supply one. Swapping them would either demand a key
    the provider ignores or omit one it requires. `Unknown` and `NotIdempotent` both collapse to
    `Unsupported`, which refuses repeats — safe to collapse, because neither claims a repeat is safe.
  - **⭐ A retry gate that the existing check does not cover.** `RetryPolicy::blind` refuses a retry only for a
    **mutating** effect, so a read-only operation passes it whatever its idempotency says — meaning the check
    alone would let this module retry everything. `retry_declaration` therefore also consults
    `ProviderIdempotency::permits_automatic_retry`, because **a blind retry is only free when repeating the
    call is free**. An operation whose provider behaviour is `Unknown` gets no automatic retry however
    harmless its effect looks.
  - **⭐ A check and a test were both written and then removed as UNREACHABLE, and the removal is checked.**
    `definitions.rs` had a risk-ceiling refusal, and its test built an over-ceiling operation directly. But
    `ValidatedOperation` has **private fields** and is built only by `ConnectorManifest::new`, which already
    refuses a risk above the platform ceiling — so the refusal could never fire. An unreachable refusal reads
    as protection while enforcing nothing, the defect `P5-001` and `P5-003` each recorded from a different
    direction. Its place is taken by a test asserting the **reachable** path still enforces the ceiling, so
    the deletion is verified rather than asserted.
  - **⭐ A test's own premise was wrong and the failure surfaced it.** `the_schemas_are_the_2020_12_dialect…`
    asserted that *every* input schema has a `required` keyword. It failed on `gmail_messages_list` — whose
    arguments are all legitimately optional, because calling it with nothing means "the newest messages",
    which the schema's own `query` description says. **The claim was too broad and the schema was right.** It
    now asserts per operation: the two resource-addressing reads must name their message or calendar, and the
    list must require **nothing**.
  - **The schemas are deliberately narrower than the APIs.** `format=raw` is absent, because it returns the
    unparsed MIME message and nothing in this connector parses it; `max_results` is capped at Gmail's
    documented 500; `calendar_events_read` takes an opaque `sync_token` whose description records that a 410
    requires a full resync. `additionalProperties: false` throughout, so a model inventing a field gets a
    refusal rather than a silent ignore — asserted by validating an accepted and a refused instance.
  - **Input and output classifications differ on purpose.** A message id is `Internal`; the message is
    `Confidential`. A single field would have to be the maximum, which would over-restrict the input and hide
    what the tool consumes, and `ToolSensitivity::ceiling()` is what a placement decision reads.
  - **Three guards falsified A-B-A with compiling mutants:** the idempotency mapping's `Declared` direction,
    the retry gate (`permits_automatic_retry()` → `true`), and the title-table fallback (`_ => None` → an
    invented title). **One of the three first attempts hit the CRLF trap again** — a multi-line here-string
    probe matched nothing against these LF sources — and was retried with an LF-joined anchor. The trap is the
    third occurrence in this workspace; it is recorded again because the cost is a whole wasted run.
  - **The timeout and backoff are recorded as an unfixed problem rather than smoothed over.**
    `TOOL_TIMEOUT_SECONDS` is 30 (a JARVIS choice; Google documents no per-request deadline) and
    `TOOL_BACKOFF_CEILING_SECONDS` is 32 (Google's lower published figure), so **two attempts at 32 s cannot
    both complete inside a 30 s deadline — only the first retry is reachable.** The test states that
    consequence instead of asserting a comfortable inequality, and the fix (a longer deadline or a smaller
    ceiling) needs a measured provider latency, which needs a live call.
  - **Limits:** the definitions are **registered nowhere**, so no model can discover or call them; **no
    executor** implements `ToolExecutor` for this connector; the schemas are this connector's construction and
    have never been validated against a real response; `output_schema` describes a normalised JARVIS shape
    rather than Gmail's message resource, so it cannot be checked against a provider document; and
    `GoogleToolError::NoSchema` **cannot be reached from a test** because the operation it would be built from
    cannot be constructed — verified by construction rather than by exercise, which is stated rather than
    implied.
  - **This round built the requests, which is the other half of the client that needs no socket.**
    `crates/jarvis-connectors/src/google/request.rs` + `request_tests.rs` (18 new tests, so **213 in the crate**).
    **`ADR-0060`.** A request is a **value** here — method, URL, ordered parameters — and nothing sends it, the
    same split `client.rs` makes. Three request builders (`gmail_messages_list`, `gmail_messages_get`,
    `calendar_events_list`) and three response parsers.
  - **⭐ THE ENCODING IS A SECURITY CONTROL, NOT TIDINESS.** A Gmail query is model-chosen text, and each
    hazardous character changes the **request** rather than the query: `&` starts a new parameter, so
    `is:unread&maxResults=999` **replaces the connector's own bound with the model's**; `=` reads as an
    assignment; `#` ends the query; `%` starts an escape of the value's choosing; `?` starts a second query
    string. Every value goes through `percent_encode` (RFC 3986 unreserved set, **uppercase** hex per §6.2.2.1
    normalisation), and the falsification mutated the unreserved set to admit `& = # %` and the injection test
    failed. **A space is `%20` and a literal `+` is `%2B`**, because the form-urlencoded convention is
    ambiguous in both directions and `%20` is correct in every query position.
  - **⭐ `HttpRequest` HAS NO FIELD A CREDENTIAL COULD GO IN — the absence is the control.** Google's own page
    offers `?access_token=` and adds that "query strings tend to be visible in server logs", so the parameter
    is a supported way to do the one thing that leaks a token into every artifact a debugging session
    produces. The type is a method, a URL, ordered parameters and an `Accept` value: no header map, no token,
    and **no body** (all three operations are `GET`s, so a body field would be a shape nothing uses). The
    header's *name* and scheme are constants so a transport knows where a credential belongs; the value is not
    in this crate. Asserted on every request by searching each rendered URL for `access_token`/`token=`/`key=`.
  - **`Display` reports parameter NAMES and never a value**, because a rendering reaches a log line and a
    value is model-chosen text. That is why `url()` (path only) and `url_with_query()` are split: a diagnostic
    can render the shape while the transport builds the target.
  - **`status` is checked BEFORE the body is parsed, in every parser.** The failure mode is specific: an error
    document parsed as a page reports "no results", and a caller cannot then tell a successful empty mailbox
    from a refused request. The refusal names `client::classify` as where the outcome belongs. Falsified by
    making the status check `if false`.
  - **`nextSyncToken` and `nextPageToken` are separate fields and are not interchangeable.** Google's sync
    guide says the sync token "is present only on the very last page" while the page token continues the
    current walk, so a caller storing the page token as a cursor would store something that expires with the
    walk.
  - **`format=raw` is not representable.** `MessageFormat` has no `Raw` variant, because that format returns
    the unparsed MIME message including attachments and nothing in this connector parses it — a type that
    cannot express the value is stronger than a check that refuses it.
  - **`maxResults` is bounded per API** (500 Gmail, 2 500 Calendar), because one shared bound would be wrong
    for one of them; zero is refused rather than read as "unlimited"; and the value **at** each cap is
    accepted so neither bound is unreachable. Falsified by dropping the zero check.
  - **⭐ MY OWN TEST'S PREMISE WAS WRONG AND THE FAILURE SURFACED IT.** The injection test asserted that a
    hazardous character "must not survive encoding" — which is **false for `%`**, whose escape is `%25` and
    therefore *contains* `%` as the escape marker. The property happened to hold for `&`, `=`, `#` and `?`,
    which is why it looked right. Replaced with the **exact** expected encoding per character plus the real
    property (`%` only as the marker). **A property that holds for every case you tried is not yet a rule.**
  - **Three guards falsified A-B-A with compiling mutants:** the encoding's unreserved set, the status-first
    check, and the zero-`maxResults` refusal. **Two probes needed a second attempt** — both were multi-line and
    hit the CRLF here-string trap (4th and 5th occurrences in this workspace), retried with **LF-joined
    anchors**.
  - **Limits:** **no request has been sent and nothing performs one**, so `HttpRequest` has **no production
    caller**; the response parsers have never seen a real response, so they prove the code reads the *record's*
    shape; `parse_id_page` accepts a body with no `messages` array as an empty page — correct for
    `{"resultSizeEstimate": 0}` and indistinguishable from a *successful* wrong-shaped document, recorded
    rather than fixed because the fix would be a stricter schema nobody has observed; **no retry, pacing or
    budget accounting happens here**, so nothing connects a request to a `classify` decision; and only `Accept`
    is stated, so a transport still has to supply `Authorization` and whatever else a live call needs — a
    coverage gap rather than a decision.
  - **This round built the credential boundary.** `crates/jarvis-connectors/src/google/credential.rs` +
    `credential_tests.rs` (10 new tests, so **223 in the crate**). **`ADR-0061`.**
  - **⭐ THE FINDING: ADR-0060 CLOSED ONE ROUTE AND NOT THE ONES THAT ACTUALLY LEAK.** Giving `HttpRequest`
    no credential field makes a token unrepresentable *in a URL*, but a token reaches an artifact through an
    **impl**, not a data structure: a derived `Debug` (one `{:?}` in a log renders whatever the struct holds),
    a `Serialize` into a durable row or a wire DTO, a `Display` in a transport's error wrapper, or a plain
    `String` argument that every holder can print. So `AccessToken` implements **neither `Serialize` nor a
    derived `Debug`** — the `Debug` is hand-written to `[REDACTED]` plus the character count, which is not the
    value and is what distinguishes two credentials in a diagnostic.
  - **The absence of `Serialize` is ASSERTED, not described, and the assertion was falsified.** A
    `compile_fail` doctest serializes the token, so a later `#[derive(Serialize)]` breaks the build. **A
    `compile_fail` test can pass for the wrong reason** — a bad import path also fails to compile — so I replaced
    the serialization line with a call that *should* compile and the doctest then **failed**, which is what
    proves it fails for the serialization. Second such assertion in the workspace, after `SecretRef`.
  - **The bytes are reachable through one accessor and its name is the warning.** `with_exposed(|token| ..)`
    takes a **closure** rather than returning a `&str`, so the borrow cannot outlive the call and a caller
    cannot move the material somewhere a later `Debug` could reach; and the closure gets only the text, not the
    token, so reaching it requires writing the word `exposed`.
  - **The header is rendered by the token, not by the transport** — `authorization_header_value()` is the
    single place the bytes and the scheme meet. A transport building `"Bearer " + value` itself is where a
    missing space or a **doubled** scheme comes from, and the doubled scheme is a real shape because a pasted
    value may already carry it.
  - **A pasted `Authorization` header reports removing the scheme, not the whitespace.** `Bearer <token>`
    contains both, so the **scheme check runs first** and the message says what to do; a whitespace-first check
    would report “contains whitespace” — true, useless, and it sends a reader hunting an invisible character.
    `P5-004` records the same ordering decision for an API key, and the variants are separate because the
    remedies differ. A 20-character floor then makes a client-id paste mistake surface at construction rather
    than at the provider, where a generic auth error points at the credential's *validity* instead of its
    *shape*.
  - **The type may name which credential it came from without naming what it is.** `origin: Option<SecretRef>`
    is the one field a diagnostic prints — “the stored refresh exchange for this account failed” — and a
    `SecretRef`'s own redaction is **asserted** rather than assumed. Every refusal's message is asserted not to
    render the value it refused, and for **every variant at once**, because a new variant is the case that
    would forget.
  - **⭐ MY OWN MEASUREMENT WAS WRONG AND THE ARITHMETIC CAUGHT IT.** A workspace run appeared to report
    “30 suites, 1 failed, 1 compile error”, which sent me looking for a failure that did not exist. The cause:
    **`Select-String` is CASE-INSENSITIVE by default**, so a pattern of `FAILED` matched the `0 failed` in every
    one of the 48 `test result` lines — inflating the “failed” count and truncating the file I was reading into
    a stale fragment. The real totals are **48 suites / 1477 passed / 0 failed**. **Use `-CaseSensitive` for a
    verdict search**, and prefer counting `test result:` lines and parsing `N passed` / `N failed` from each.
  - **Limits:** **nothing holds an `AccessToken` in production code** — no transport, no token source, no
    exchange, so the type has no caller outside its tests; **the material is not zeroized on drop** (`zeroize`
    is not a dependency and a partial answer about which types zeroize would imply coverage it lacks);
    `authorization_header_value()` **allocates a second buffer holding the credential**, which is a small
    widening recorded rather than hidden; `Debug` on a `Vec<u8>` derived from the header would still print it,
    because this type controls its own rendering and not anything derived from it; the length floor is a
    heuristic and proves nothing about validity; and **nothing checks the token is for Google**, because
    provenance is not a property a string carries.
  - **This round built the transport port and the read operations — the connective tissue that makes every
    earlier slice reachable.** `crates/jarvis-connectors/src/google/transport.rs` + `transport_tests.rs` and
    `operations.rs` + `operations_tests.rs` (28 new tests, so **251 in the crate**). **`ADR-0062`.**
  - **⭐ I SHIPPED A SYNCHRONOUS PORT AND `ToolExecutor::execute` IS `async` — so the port could never back an
    adapter.** Written, formatted, tested, gated, and committed as `416fd94` before the flaw was noticed: a
    synchronous `send` cannot be called from an `async fn` without blocking a runtime worker for the whole round
    trip, which is the failure `P2-007` records for a blocking read inside a stream. The reasoning that produced
    it — "defer the async binding to whoever owns a runtime" — is internally consistent and still produced an
    **unreachable** port: the one interface it exists to satisfy is async, so the deferral meant "never". Fixed
    by making the port `#[async_trait]` **and** by implementing `ToolExecutor` for the adapter, which is what
    turns "the port exists" into "the port can be used". Recorded because a plausible deferral is the easiest
    way to build a component nothing can consume.
  - **⭐ THE FINDING: "THE REQUEST FAILED" IS NOT ONE CONDITION, AND THE TWO DIRECTIONS ARE NOT SYMMETRIC.**
    `TransportFailure` is `Connect | Send | Body | Timeout | Refused`, and the variants encode **when** the
    failure happened rather than what it was called. `Send` exists separately from `Connect` because "the
    request was written and then the connection broke" is not "the request could not be sent"; conflating them
    is exactly how an ambiguous failure becomes a certain one. `may_have_reached_the_provider()` is `false` for
    `Connect`/`Refused` and `true` for `Send`/`Timeout`/`Body`, and it is deliberately **not** named
    `is_certain_nothing_happened` — that reading invites a `true` default in a `match` fallback, and a new
    variant would then silently become retryable. It is a **wildcard-free `match`**, so a new variant is a
    compile error rather than a default.
  - **The subtle case is `Body`: the provider DID answer.** A response whose body could not be read means the
    request was certainly received and nothing is known about what it did — so grouping it with `Connect` would
    be the precise mistake the ambiguous variant exists to prevent. Its consequence is asserted, not just its
    class: `AmbiguousAfterReaching`, which **refuses an automatic retry** — because for a non-idempotent effect
    a retry is a second effect.
  - **A refusal is a RESULT, not an error, and the reason is the provider's machine-readable code.** A `403` was
    received and refused, so it becomes a `ToolCallResult` with a `Failed` outcome. Collapsing a non-2xx into a
    transport error would lose the status and the `errors[].reason` code, which is everything the next decision
    needs — and the prose is never carried, because `P3-008c` forbids deriving a decision from message text. A
    body that cannot be parsed still yields a reason naming the **status**, which is a fact even when the body
    is not: the difference between "we know little" and "we know nothing".
  - **⭐ A 200 WHOSE BODY IS UNREADABLE IS `Unknown` — AND A RESULT, NOT AN ERROR.** The status proves the
    request was answered; the body says nothing about what it produced. Reporting `Confirmed` would claim an
    effect from a status code and reporting `Failed` would claim nothing happened, so `Unknown` is the only
    honest reading. It is reached through a returned result rather than an error **because the provider did
    answer**, and `Unknown` refuses an automatic retry — the consequence that matters.
  - **The port keeps the credential boundary `ADR-0061` built.** `send` takes `request` and `token` as
    **separate parameters** rather than one authenticated-request type, because `HttpRequest` may be rendered
    (its `Display` prints the path and parameter *names*) and `AccessToken` may not; merging them would make a
    single `{:?}` leak the token. `HttpMethod` is a **closed enum with one variant** — `POST`/`PATCH`/`PUT`/
    `DELETE` are absent rather than present-and-unused, because a variant nothing constructs is a method a
    reader assumes is reachable — so `P5-009`'s write grows the enum and turns every `match` into a compile
    error, which is what makes a new method a deliberate edit.
  - **The adapter is a `ToolExecutor`, and its test drives the real trait with a request the REAL policy engine
    authorized.** The receipt's digest is recomputed from the arguments the request carries and the decision
    comes from an actual `evaluate` over the tool's own derived definition, because `AuthorizationReceipt::new`
    and `ToolExecutionRequest::new` verify everything — a fixture cannot stand in an invented digest or a
    fabricated decision. The actor holds exactly `mail.read` + `calendar.read`, the scopes the manifest
    declares, so the decision is allowed by the connector's own contract rather than by a wildcard.
  - **The deadline is checked before anything is sent, and refused rather than failed.** `RefusedBeforeReaching`
    is honest — nothing was sent, so nothing happened — and `AmbiguousAfterReaching` would send a reader
    investigating an effect that never existed. `jarvis-tools`' filesystem adapter makes the same choice before
    its first read. The pure `run` method takes `now` as a parameter while the trait `execute` reads the clock,
    so the deterministic layer can assert an exact `reported_at` and the logic is implemented once.
  - **The operation layer renders the tool's DECLARED output, not the provider's resource.** `read_output`
    emits `message_ids`/`event_ids` plus the declared schema's fields, because a parser returning Gmail's
    `Message` would make `ADR-0059`'s schema a fiction. `next_sync_token` is rendered **separately** from
    `next_page_token`: the sync token positions a **future** incremental sync while the page token continues the
    current walk, so merging them would store a cursor that expires with the walk.
  - **An argument that is supplied but wrong is REFUSED rather than dropped** — the defect `P5-004` records from
    the other direction, where a value that is present and ignored is indistinguishable from one that was
    honoured. A `null` is treated as *absent* (that is what a serializer emits for an unset optional field) and
    accepted; a wrong-typed or out-of-range value is refused. An unknown tool is `NotImplemented` rather than a
    default, because a fallback would make a mistyped name silently read a mailbox.
  - **⭐ FOUR GUARDS FALSIFIED WITH COMPILING MUTANTS.** Making `Timeout` certain → **4 tests detected**;
    making `Body` certain → **3 detected**; ignoring the status in the response classifier → **2 detected**;
    deleting the deadline check → **1 detected**; turning the deadline refusal into `AmbiguousAfterReaching` →
    **1 detected**. Each mutant **compiled**, so `FAIL` is a real detection rather than a build failure — the
    `VACUOUS`/`FAIL` distinction kept, and verified with a `git diff --stat` afterwards because an earlier
    aborted run had left one mutant in place.
  - **One new dependency edge: `async-trait`** (already a workspace dependency, so nothing new is fetched), for
    an object-safe `async` trait method. Still **no `reqwest` and no socket** in this crate.
  - **Limits:** **there is no transport implementation** — the port has a test double and nothing else, so **no
    request has been sent to Google and no response has been parsed from Google**, and every fixture is
    constructed from the research record, which means these tests prove the layer implements the *record*, not
    that the record matches the provider; **the port's own requirements (no redirect, no retry, no proxy from
    the environment) are unenforced** because the only implementations are test doubles — they become testable
    only when a real transport is written; **no credential is minted, refreshed, or expiry-checked** (`run`
    takes an `AccessToken` it did not obtain, so a stale token becomes a provider refusal); **the deadline is
    read from the system clock**, so the trait boundary can only be tested with a *lapsed* deadline; **no
    deadline is passed to the transport**, so "an implementation must bound its own wait and report `Timeout`"
    is a requirement on an implementation that does not exist; **`Retry-After` is carried and never
    interpreted**, so a transport could report it and a caller could ignore it with no test failing;
    **`evidence_from` has no caller in this crate** (provider evidence locates an *effect* and a read produces
    none, so it exists for `P5-009`); ~~the output rendering is not validated back through the schemas it claims
    to match~~ (**closed this round** — see below); and **nothing constructs the adapter in a binary** — no
    registry, executor, or route offers
    `google.gmail_messages_list`, so the adapter is reachable from a test and not from a run.
  - **This round added the wire fixtures — and the interesting part is what they are NOT.** Six files under
    `crates/jarvis-connectors/tests/fixtures/google/` plus `tests/google_fixtures.rs` (9 new tests, so **262 in
    the crate**). **`ADR-0063`.**
  - **⭐⭐ THE FINDING: A HAND-BUILT RESPONSE IS EASY TO MAKE INTERNALLY CONSISTENT AND IMPOSSIBLE TO MAKE
    REALISTIC IN ITS CONSTRAINTS — AND MY OWN EARLIER TEST PROVED IT.** I had written a test asserting a Calendar
    page carrying **both** `nextPageToken` and `nextSyncToken`. It passed. **Google cannot produce that body.**
    The reference page documents the two as **mutually exclusive**: `nextPageToken` is "Omitted if no further
    results are available, in which case nextSyncToken is provided", and `nextSyncToken` is "Omitted if further
    results are available, in which case nextPageToken is provided". The constraint lives in the **field
    description**, not in the sample JSON — so reading the *example* tells you the shape while reading the
    *prose* tells you what may co-occur. The test now asserts the two states **apart** (a mid-walk page with a
    page token and no sync token; a last page with a sync token and no page token) and the fixtures are that pair.
  - **⭐ "WE HAVE FIXTURES" MUST NOT BE READABLE AS "WE HAVE A RECORDING".** The acceptance text says "with
    recorded wire fixtures", and a directory of clean JSON looks identical whether it was copied from a live
    response or assembled from documentation. Writing shapes and calling them recordings is the failure mode the
    ADR exists to prevent, and it is the same shape as `ADR-0056`'s scaffold inventing a completed checklist. So
    each fixture carries `_not_a_capture: true` and `_shape_documented_at: <url>` **in the data**, the suite
    **asserts the marker** (a file that dropped it fails rather than passing as an apparent capture), and the
    research record repeats it. A fixture that was never captured proves the **reader**; only a capture proves
    the **record**.
  - **A fixture states its own point, because a reader cannot recover it from the bytes.**
    `_the_point_of_this_fixture` records *why* a payload exists — that the Calendar pair exists because the
    tokens are exclusive, and that the two 403 fixtures are a **pair whose only difference is the reason code**.
    Without that, a maintainer "simplifying" the pair into one file deletes the only test that distinguishes a
    Workspace administrator's decision from a throttling limit.
  - **The 403 pair makes the reason-vs-status split falsifiable, and both files are a 403.** One says
    `domainPolicy` and classifies as permanent; the other says `rateLimitExceeded` and classifies as retryable.
    The load-bearing assertion is that the two decisions **differ** — a classifier switching on the status alone
    would give them the same answer. The `domainPolicy` fixture's `message` deliberately invites a retry ("This
    looks like a transient error…") while its `reason` code forbids one, so a classifier reading prose instead
    of the code classifies it **wrongly**.
  - **The error fixture also proves the prose never reaches a decision.** `GmailErrorBody` has no field for
    `message`, so the fixture's text parses and then becomes **unreachable** — asserted by checking the derived
    `Debug` does not contain it. That is `P3-008c` as a property of the type rather than a convention.
  - **⭐ TWO MORE GUARDS FALSIFIED WITH COMPILING MUTANTS.** Flipping `_not_a_capture` to `false` → **3 tests
    detected**; deleting `nextPageToken` from the mid-walk page → **3 tests detected**. The second is the
    important one: it shows the mutual-exclusion property is genuinely load-bearing rather than incidentally
    true. Tree verified clean with `git diff --stat` afterwards.
  - **⭐ ONE RECORDED LIMIT WAS CLOSED RATHER THAN REPEATED.** Three different rounds had listed "the output
    rendering is not validated back through the schemas it claims to match" as a limit. A limit that is recorded
    three times and never closed is a **todo wearing a limit's clothes**, so it was fixed:
    `every_rendered_output_satisfies_the_schema_the_definition_declares` renders a page through `read_output`
    for **all three** operations and validates the result against `ToolDefinition::output_schema()`, which is
    **derived from the manifest and not restated**. The empty-page case is included, because an omitted
    `message_ids` and an empty `message_ids` are different documents and only the second satisfies the schema's
    `required`. Falsified with two compiling mutants — renaming a rendered field and omitting a required array —
    each detected by the new test **and** by the existing shape test.
  - **The research record's Verification Plan is now auditable.** It had eleven items and said "none exist yet";
    each is now marked **WRITTEN** or **Not written**, with the one written item named and the six fixtures
    listed as hand-built. A plan where every line is unmarked reads as done; a plan where every line is marked
    can be checked. The live smoke test remains **not written**.
  - **NEW LIMITS:** **no fixture is a capture and no live call has been made** — no credential exists, no Cloud
    project was created, no Google API was contacted; **a hand-built fixture cannot reveal a constraint nobody
    documented**, so a rule Google enforces but does not write down would still be invisible; **the fixtures do
    not cover every declared operation** — `history.list` (the operation Finding 2 most depends on), the batch
    endpoint, and the `format=metadata` envelope have **no** fixture at all; **the sweep checks provenance and
    not shape**, so a fixture is not validated against the schema it feeds (the *rendering* now is, which is a
    different join); and **`_shape_documented_at` is a URL nothing re-checks**, so a fixture can drift from the
    page it cites with no assertion failing.
  - **This round built the token exchange — the "connection setup" half, which was the least built part.**
    `crates/jarvis-connectors/src/google/token.rs` + `token_tests.rs` (25 new tests, so **278 in the crate**).
    Until now a connector could *start* a flow and *classify* a refresh and could not complete a first
    authorization. **`ADR-0064`.**
  - **⭐ THE FINDING: OAUTH ANSWERS FROM THE BODY AND NOT FROM THE STATUS — THE OPPOSITE OF THE REST OF THIS
    CRATE.** Everywhere else here a refusal is read from the status: `client::classify` switches on the code and
    `TransportResponse::is_success` is `status == 200`. RFC 6749 §5.1 puts the token parameters in a *successful*
    body, and §5.2 makes a failure an `error` parameter with those parameters **omitted** — so the presence of
    `error` is what makes an answer a refusal, and a `400` **without** one is a proxy's page or a misrouted
    request. A status-first reading would report "the server refused" and attribute a decision to a server that
    never made one. The status keeps exactly one bit: `5xx` marks the refusal transient, because RFC 6749 §5.2's
    codes describe the *request* and cannot say whether the server is unwell — that is the transport's
    observation.
  - **⭐⭐ I WROTE THE EXACT BUG `ADR-0061` EXISTS TO PREVENT, IN THE FIRST DRAFT OF THIS MODULE.**
    `parse_answer` read `access_token` into an owned `String` and then dropped it, with a comment saying so. It
    satisfies the letter of the rule and breaks its purpose: an owned copy existed, **with a lifetime**, in a
    function whose other outputs are `Debug`-printed. The rule is about **copies, not lifetimes**, so the draft
    was replaced — `parse_answer` now asks only whether a non-empty token is *present* (`.is_some_and`, producing
    a `bool`) and never binds the bytes to a named value, and `Granted` has **no field for them**: a field would
    be a second, unredacted copy of the credential in a value the rest of the crate prints. A test renders a
    granted answer and asserts **neither** the access token **nor** the refresh token appears.
  - **A refusal is a VARIANT, not an error, and the three outcomes stay apart.** An **unreadable** `400` (HTML,
    or JSON with neither a token nor an error) is `TokenRequestError::Body` — not `Refused`, which would invent a
    provider decision, and not a transport failure, which would claim unreadability when the provider answered.
    A grant that *completed* but is unusable (`token_type` not `Bearer`, or a lifetime past the bound) is
    `UnusableGrant` rather than `Refused`, because saying the provider declined would be false. And
    `TokenEndpointAnswer`'s two accessors are **disjoint** — exactly one of `response()` and `failure()` is
    `Some` — which is asserted, because it is the property the whole split rests on.
  - **The transient check precedes the error code, and both halves are asserted.** A provider behind a proxy can
    answer a `503` through the protocol's own channel carrying `invalid_grant` — a real shape — so reading the
    code first would send a user to a consent screen **during an outage**, which finds the same failure and looks
    like a broken connector. The test asserts the ordering **and** the control: the same code without the outage
    **is** the user's problem, so the rule is not simply "ignore the code".
  - **The rotation rule is about ARRIVAL, never about storage.** `invalid_grant` covers "invalid, expired,
    revoked, does not match the redirection URI, or was issued to another client" and the protocol does not say
    which — so `vendor_says_revoked` is a **parameter**, and a refresh is `Rotated` by whether new material
    arrived rather than by whether the caller stored it. A caller that failed to store one has a defect of its
    own, and reporting `Refreshed` would hide the half of the exchange that makes replay detectable.
  - **The request is a parameter LIST, not an `HttpRequest`, and there is no `client_secret` field anywhere.**
    `HttpRequest` is a `GET` with no body and no credential field *by design*, so forcing a credential-bearing
    form `POST` into it would undo `ADR-0060`. The list has no `Display` and no `Serialize`. The parameter set is
    asserted as an exact **set**, so an added parameter is a failing test — and the manifest's **empty
    `secret_fields`** and this request now cannot disagree, because there is no field to put a secret in.
  - **`Secret` refuses the paste mistake at construction, ordered by actionability.** Empty, oversized,
    whitespace-containing, and control-containing are four separate reasons; the specific case is a **trailing
    newline from a paste**, which reaches the provider as a different string and surfaces as a generic auth
    failure that sends a reader to debug the credential's *validity* instead of its *shape*. A PKCE verifier is
    separately checked against RFC 7636's 43–128 bounds **and** its unreserved alphabet, because a verifier
    outside it produces a challenge mismatch that looks like a PKCE bug.
  - **⭐ FOUR GUARDS FALSIFIED WITH COMPILING MUTANTS.** Ignoring the `error` parameter → **8 tests detected**;
    letting the outage lose precedence to the error code → **2**; never reporting a rotation → **2**; dropping
    the credential-shape whitespace check → **2**. Tree verified clean with `git diff --stat` afterwards.
  - **NEW LIMITS:** **no request is sent and no token has ever been obtained** — there is no transport for a
    form `POST`, and the fixtures do **not** yet include a token response; **the access token deliberately never
    reaches a value, so a caller must read it from the response body itself**, which means those bytes exist
    outside this module's boundary where it cannot enforce anything about them; **no ID-token verification**, so
    the `nonce` `P5-002` carries is still unvalidated; **no DPoP**, which needs a non-exportable key and is a
    decision rather than a header; **`redirect_uri` is not validated here** (the flow owns the registered value,
    so a caller could build a request the flow would refuse); **the parameter list is never tested as a body**,
    because nothing joins it — the encoding is asserted per value and the joining is not; and the **`client_id`
    is only checked for non-emptiness**, so a pasted URL or a project number is accepted here and fails at the
    provider.
  - **This round built revocation — the teardown half of connection setup — and found a contract contradiction.**
    `crates/jarvis-connectors/src/google/revocation.rs` + `revocation_tests.rs` (11 new tests, so **289 in the
    crate**). **`ADR-0065`.**
  - **⭐⭐ THE FINDING: `RevocationKind::requires_reauth_afterwards()` IS WRONG FOR GOOGLE, AND THE SHARED TYPE IS
    RIGHT ABOUT THE PROTOCOL.** `P5-002` reads RFC 7009 correctly: `token_type_hint` **selects which token** is
    revoked, so revoking the access token leaves the grant intact and `AccessToken.requires_reauth_afterwards()`
    returns `false`. Google's own page (last updated **2026-09-14**) says the opposite twice: revoking an access
    token **also revokes the paired refresh token**, and revocation "removes **all** OAuth 2.0 scopes previously
    granted to a **project**, invalidating any issued access or refresh tokens for **all clients registered under
    that project**". So on Google **no** revocation kind leaves the account usable, and a caller that trusted the
    shared `false` would tell a user their connection was disconnected-but-authorised while the account in fact
    needs a new consent. The shared type is **not changed** — it is provider-agnostic and correct — and the
    provider's answer lives in `effect_of`, with a test asserting **both sides and their divergence**, so a change
    to either fails rather than silently re-opening the gap.
  - **⭐ THE LINT IMPROVED THE DESIGN, AND IT WAS A REAL SIGNAL RATHER THAN NOISE.** `clippy::struct_excessive_bools`
    fired on a five-bool `RevokedEffect`, and it was right: `refresh_invalidated: false` could not distinguish
    *the refresh token survived* from *the account never had one* — two facts with opposite consequences, one of
    which would tell a user their refresh token was revoked when it never existed. So `MaterialState` has three
    variants (`Invalidated`, `Absent`, `Untouched`), where `Untouched` is never produced for this provider and
    exists so "we do not know" is representable rather than rounded. Likewise `EffectTiming { Immediate,
    MayTakeTime }` rather than a `bool`: "it might take some time" is the whole content of the distinction.
  - **`requires_reauth` is DERIVED from the grant's removal, not stored beside it.** With the scopes gone there is
    no consent to reuse and no refresh token to mint from, so needing a person follows from `scopes_removed`. A
    separate stored field would be a second value that must agree with the first — the defect class this repo
    keeps recording — and a mutant that hard-codes it is detected by 3 tests.
  - **A `200` means ACCEPTED, not IN FORCE.** "Following a successful revocation response, it might take some
    time before the revocation has full effect." A caller must not treat it as proof that a concurrent call will
    now fail, and **a test that asserted it would be flaky against the provider itself** — which is why the
    timing is a state a caller reads rather than a comment.
  - **An unreachable provider is never reported as revoked.** `parse_revocation_answer` takes `reached`
    **separately from the status**, because a transport cannot say "the provider refused" and "nothing answered"
    with one value. `Unreachable.is_withdrawn()` is `false`, so a `200` that was never received cannot read as
    success — the overclaim direction worth designing against. Likewise a **gateway's `502`** classifies as a
    refusal (the provider *was* reached, through something) and `status_is_documented` lets a caller tell the
    documented `200`/`400` pair from a layer in front of the endpoint.
  - **A `200` cannot report "already invalid", and the gap is stated rather than papered over.** RFC 7009 §2.2
    makes `200` cover both success and "the client submitted an invalid token", so the status carries **no signal
    about whether the token was live**. `AlreadyInvalid` stays reachable out of band but is **not derived from a
    response**, because inventing it from the status would read a fact out of a response that does not carry it.
  - **⭐ FOUR GUARDS FALSIFIED WITH COMPILING MUTANTS.** Hard-coding `requires_reauth` to `false` → **3 tests
    detected**; collapsing `Absent` into `Invalidated` → **2**; claiming an immediate effect → **2**; letting an
    unreachable provider read as `Revoked` → **2**. One first attempt was `VACUOUS` (a move error, not a test
    failure) and was replaced with a compiling mutant — the `VACUOUS`/`FAIL` distinction kept, tree verified
    clean with `git status` afterwards.
  - **NEW LIMITS:** **no request is sent and nothing has been revoked** — there is no transport for a form `POST`,
    so no `200` has been seen and the behaviour is derived from documentation and the RFC; **the divergence is
    documented, not measured** — confirming that Google revokes the paired refresh token would need a live grant
    and deliberately destroying it, so a provider whose *behaviour* differed from its *documentation* would not be
    caught; **`has_refresh_token` is the caller's claim about its own store**, and answering `false` incorrectly
    narrows the reported effect; **`RevocationKind::Grant` cannot be performed in one call to a strictly
    conforming server** (RFC 7009 revokes one token at a time), so relying on Google's single-call behaviour is a
    recorded provider property rather than an encoded one; **nothing handles the multi-account consequence** — the
    module reports the blast radius is the project, and no type holds the set of *other* accounts whose grants
    were withdrawn; **`takes_effect_later` has no duration**, so nothing can schedule a verification or re-check a
    revocation; and **revocation is not wired to anything** — no route, command, or teardown path calls this
    module, so a disconnect is still not something a deployment can perform.
  - **This round fixed the sync advance path, and found that a documented remedy was UNREACHABLE.** `SyncSignal`
    added to `client.rs`, `advance_gmail_history`/`advance_calendar_sync` rewritten to take it, and a false
    predicate replaced (2 new lib tests + 1 new fixture test, so **291 in the crate**). **`ADR-0066`.**
  - **⭐⭐ THE FINDING: `SyncAdvance::HistoryPruned` AND `TokenInvalidated` COULD NOT BE PRODUCED BY ANY INPUT.**
    `SyncAdvance` has a `HistoryPruned` variant whose own doc calls it "the finding this module was written
    around" — and `advance_gmail_history` took only `next_history_id`, never a status or any failure signal, so
    **no caller could reach the resync path at all**. Calendar's `TokenInvalidated` was unreachable the same way.
    This is the "refusal that can never fire" pattern `P5-001`/`P5-003` each recorded, in its **inverse** form: a
    *remedy* that can never be produced. The fix is `SyncSignal { Advanced { history_id }, CursorUnusable,
    Refused(RetryDecision) }`, which also makes the third case explicit so a refusal that is neither an advance
    nor a dead cursor is carried with its classification rather than swallowed.
  - **The signal is a parameter because the INFERENCE DEPENDS ON THE METHOD, and only the caller knows it.** A
    `404` on `users.history.list` may mean pruned history; a `404` on `users.messages.get` means the message does
    not exist, and resyncing on that would discard a whole sync over one missing message. Requiring the signal
    turns a caller's inference into an explicit act instead of a comparison hidden inside a function that never
    saw the request.
  - **⭐⭐ A PREDICATE ASSERTED A DISTINCTION THE PROVIDER DOES NOT PUBLISH — AND RESEARCHING IT PROPERLY MADE
    THAT WORSE, NOT BETTER.** `gmail_history_status_is_pruned` returned `status == 404`, claiming that a 404 on
    `history.list` IS pruned history while Finding 2 says the two causes are indistinguishable. The error guide
    (`handle-errors`, updated **2026-09-15**) settles it the wrong way for the predicate: its status summary
    lists `404 - Not Found` and it then has **NO 404 subsection at all** (its sections are 400, 401, 403, 429,
    5xx), so Google publishes **no `reason` code** for a 404. The ambiguity is **irreducible from the response**,
    so no predicate can resolve it and I removed the one that pretended to. It is replaced by
    `gmail_history_status_cannot_prove_usable`, which claims only what is readable.
  - **⭐ WHY RESYNCING ON AN AMBIGUOUS 404 IS SAFE: THE WRONG READING IS SELF-CORRECTING.** For **both**
    documented causes the first correct action is a full sync — if the history was pruned that is the documented
    remedy, and if the account is gone the full sync's own first call fails and surfaces *that*. So the wrong
    reading costs **one extra `messages.list` call**, while the alternative (resuming from a rejected position)
    yields a store that reports itself in sync while missing everything. Cost asymmetry decides the direction,
    the same reasoning `ADR-0062` uses for an ambiguous transport failure. And a dead cursor now carries
    **no cursor forward**, so a caller cannot resume from the position the provider just rejected.
  - **`429` and `5xx` are explicitly NOT dead cursors.** The predicate is true for `404` alone, asserted false for
    the retryable family — because a resync on a transient failure discards a working store, which is the
    opposite mistake and a far more expensive one.
  - **⭐ THREE GUARDS FALSIFIED WITH COMPILING MUTANTS.** Carrying a rejected cursor forward → **2 tests
    detected**; making the predicate unfailable → **2**; reporting a refusal as an advance → **2**. The
    multi-line anchors needed LF-joined strings built in the script, because here-strings are CRLF — the trap
    this session records, hit again.
  - **⭐ A VERIFICATION-PLAN ITEM WAS FALSIFIED AND THE RECORD SAYS SO.** The plan's "404-is-staleness test"
    required a fixture showing two causes produce opposite outcomes; that comparison needs information the
    response does not carry, so the test **cannot be written as stated**. The item is marked **WRITTEN** and
    **corrected**: the new fixture `gmail_history_404_no_reason.json` asserts the **absence of an `errors` array
    and therefore of a `reason`**, and the research record explains that the discrimination it asked for is
    impossible. A plan item being disproved is a result rather than a failure — `ADR-0063` records the same
    outcome for a different assumption.
  - **NEW LIMITS:** **no request is sent** — the signal is produced by a caller that does not exist, since there
    is no `history.list` transport, no sync loop, and no resync orchestration; **nothing can PERFORM the full
    sync**, so a caller reaching `HistoryPruned` has a verdict and no remedy; **the 404 reading is still an
    inference, now named as one** — the doc argues it is safe because self-correcting, not that the response
    distinguishes the cases, so a provider that changed its 404 behaviour would not be caught; **Calendar's
    `400`-is-a-query-error path still has no fixture**, so nothing pins which reason code accompanies it; and the
    signal does **not** protect against calling the wrong `advance_*` function — a mismatched cursor is caught by
    `SyncCursorKind`, not by the signal.
  - **This round added the `history.list` read — the producer the previous round's signal was missing.**
    `request::{gmail_history_list, parse_history_page, HistoryPage}`, `client::gmail_history_signal`, the
    `gmail_history_list` manifest operation and tool definition, and two fixtures
    (`gmail_history_list.json`, `gmail_history_list_last_page.json`). **4 new lib tests + 1 new fixture test,
    so 295 in the crate and 11 in the fixtures suite.** **`ADR-0067`.** Both new guards were falsified A-B-A
    with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: `ADR-0066` FIXED THE VARIANT FORM OF THE DEFECT AND REPRODUCED IT AS A PARAMETER FORM.**
    That round widened `advance_gmail_history` to take a `SyncSignal` so the resync remedy became producible —
    but **nothing could build a signal from a response**, because there was no `history.list` request in the
    crate. The only construction sites were test fixtures, so the whole enum was fixture-only. Widening a
    function's signature does not supply the value, and the check `P5-001`/`P5-003` record — *for every variant,
    grep the constructing sites, not the definition* — applies to a parameter as much as to an enum. Applying it
    showed `Advanced` and `Refused` had no producer either.
  - **The asymmetry that carries the weight is the predicate's ORDER.** `gmail_history_signal` checks
    `gmail_history_status_cannot_prove_usable` **first**, so the retryable family (`429`, `5xx`) can never fall
    into the dead-cursor arm. A resync on a transient failure discards a working store — the opposite mistake
    from the one the 404 heuristic tolerates, and the more expensive one. Asserted over `400/403/429/500/502/503/504`.
  - **⭐ TWO DEFECTS IN MY OWN FIRST DRAFT, both found by fetching the live method reference before writing the
    code — and both living in field *descriptions* rather than the example**, the `ADR-0063` lesson hit again.
    (1) I made `history_id` **required** in the output schema, assuming a success always states the new
    position; the reference says the id "can be stored … for a future request" when no `nextPageToken` is
    returned, which is about *when it is usable*, not *when it is present*. Requiring it would have made the
    connector's declaration stricter than the provider's. (2) I nearly treated `historyId` and `nextPageToken`
    as Calendar's mutually-exclusive pair; they are **not** — `historyId` is present on every success while
    `nextPageToken` appears only mid-walk, so a reader copying the Calendar pattern would take its durable
    cursor from the field that expires when the walk ends. The two fixtures are that pair.
  - **`historyTypes[]` is deliberately not offered.** The parameter filters the change kinds returned, so a sync
    using it would silently drop the kinds it excluded; the general `messages` field is populated on every
    change and is read instead. The operation also gets its **own** rate-limit entry, because `history.list`
    costs **2** quota units against a message read's 20 and reusing the read limit would over-state a sync.
  - **Falsified, two guards A-B-A with compiling mutants.** `gmail_history_signal`'s 404 arm → `if false`
    (detected by `a_history_status_becomes_the_signal_the_cursor_decision_consumes`); `parse_history_page`'s
    status check → `if false` (detected by `a_history_page_keeps_the_cursor_and_the_page_token_apart`). Both
    restored and verified byte-identical with the mutant string absent from the file.
  - **NEW LIMITS:** **no request is sent and no response has been parsed from Google** — the transport slice is
    still unbuilt, so these tests prove the layer implements the *record*; **the operation is registered
    nowhere**, so no model can reach it; **the signal producer has no caller** — a sync loop would be the
    consumer, and no sync loop exists; the fixtures are hand-built, so a shape Google sends but does not
    document is still invisible; and **`history_types` filtering and the `labelId` parameter are unimplemented**
    rather than absent-as-a-decision.
  - **This round built the transport implementation — the limit every previous slice of `P5-005` recorded.**
    `crates/jarvis-connectors/src/google/http.rs` + `http_tests.rs` (6 new tests, so **301 in the crate**),
    `reqwest` declared on the crate (resolving to the already-locked `=0.13.5`, so **no new package**), and a
    `#[cfg(test)]` `HttpRequest::rebase_to` seam so a transport test drives a **real product request** against a
    loopback server. **`ADR-0068`.** **50 suites / 1566 workspace tests, 0 failed, 0 ignored.** Both new guards
    falsified A-B-A with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: `ADR-0062`'s PORT REQUIREMENTS WERE UNENFORCEABLE AND READ AS SATISFIED.** The port listed
    four things an implementation "must not do" — follow a redirect, retry, read a proxy from the environment,
    return an error for a non-2xx — and the only implementations were **test doubles**, which have no redirect
    policy, no proxy configuration and no retry to disable. So all four were true **by construction** and none
    was tested. The crate's own limits admitted this ("they become testable only when a real transport is
    written"), which is the honest form — but a requirement that *reads* as met is worse than one nobody wrote
    down. `ADR-0067`'s lesson (a parameter with no producer) applies to a **constraint with no implementation**.
  - **The ordering in the failure map is the substance, and it is not the obvious order.** `classify_error`
    checks `is_timeout()` **before** `is_connect()`, although `is_connect()` is the more specific answer where
    both apply. The reason is the *failure direction*: a timeout is **ambiguous** (the request may have been
    written), so choosing it where the certain variant might also fit can only make a caller *less* willing to
    retry — whereas the reverse mistake would let a non-idempotent effect repeat. A pure connect failure is not
    a timeout, so DNS and refused-connection failures still report `Connect` and the specific case is not lost.
    `Send` is the final arm, because an unrecognised failure is exactly where certainty is unwarranted.
  - **A branch was deliberately NOT written, and the reason is a recorded defect class.** There is no
    `error.is_redirect()` arm: with `Policy::none()` no redirect produces an error, so the arm could never fire
    — the unreachable refusal `P5-001` and `P5-003` each recorded. The redirect is instead classified by the
    caller from the `302` the transport now *returns*, which is also the control that proves nothing was
    followed: **the redirect target server is asserted to have received zero connections**.
  - **Four port requirements became controls with tests**, each against a hand-written HTTP/1.1 server (the
    `jarvis-mcp-transport` precedent — a framework would share assumptions with the client under test): no
    redirect followed; no retry (exactly one request per `send`); no environment proxy (`no_proxy()` explicit,
    because `reqwest`'s `system-proxy` default is ON); and a `403` arriving as a `TransportResponse` rather than
    an error. The timeout case is driven by a server that accepts and **never answers**, so the client's own
    deadline is what ends it rather than an error the test invented.
  - **The dependency was measured before it was declared.** `reqwest` resolves to the `=0.13.5` already in the
    lock file through `jarvis-cli`, `jarvis-models` and `jarvis-mcp-transport`, so `cargo tree` shows one new
    edge and `Cargo.lock` gained **one line** — no package joined the tree, and `cargo deny check` is
    advisories/bans/licenses/sources **all ok**. `default-features = false` with `rustls` keeps the
    bundled-TLS policy.
  - **Falsified, two guards A-B-A with compiling mutants.** The timeout-first ordering → `if false`, detected
    by `a_refused_connection_is_certain_and_a_timeout_is_not` (`left: Send, right: Timeout`); the redirect
    policy → `Policy::limited(10)`, detected by
    `a_redirect_is_not_followed_and_the_target_is_never_contacted` (`left: 200, right: 302`). Both restored and
    verified byte-identical.
  - **NEW LIMITS:** **no request has been sent to Google and no Google response has been parsed** — every
    response is written by the test file, so these tests prove the transport's own controls rather than the
    record; **the transport has no production caller**, because no composition root constructs the connector
    (the daemon owns that); **no body-size bound**, since `TransportFailure` has no "too large" variant and a
    streaming cap needs `P5-009`'s output policy; **no token refresh**, so a stale token becomes a provider
    refusal; **no `Retry-After` interpretation** (it is carried and never acted on); and the timeout/connect
    pair is a **JARVIS choice**, because the port requires a bound while Google publishes no deadline.
  - **⚠ AND THE DEPENDENCY COST A LOCAL CHECK, WHICH IS RECORDED RATHER THAN WORKED AROUND.** `reqwest`'s `rustls`
    backend pulls `aws-lc-sys`, so `cargo clippy -p jarvis-connectors --target x86_64-unknown-linux-gnu` now
    fails with `ToolNotFound` even with the zig linker shim — the crate joins `jarvis-models` in the set that
    **cannot be cross-linted locally**. Native CI still lints it on all three OSes, so this narrows local
    verification rather than the gate; a `#[cfg(unix)]`-only defect would now surface only on CI.
  - **This round tested the SEAM — the join that had no test in either half.** `http_tests.rs` gained 4 tests (so
    **305 in the crate**) driving `GoogleReadTool` against a real `ReqwestTransport` over a socket, plus two
    recorded `#[cfg(test)]` seams: `HttpRequest::rebase_to` and `GoogleReadTool::run_with_origin`. **`ADR-0069`.**
    **50 suites / 1570 workspace tests, 0 failed, 0 ignored.**
  - **⭐⭐ THE FINDING: EVERY PART HAD A GREEN TEST AND THE JOINT HAD NONE.** The adapter was tested against a
    **`Scripted`** transport; the transport was tested against a **hand-written request** it was handed. Neither
    had ever met the other, so a URL the adapter builds that the transport sends to the wrong place — or a
    header one sets and the other drops — was invisible to both suites. This is `P3-006a`'s shape (each slice
    self-consistent; the defect lives between two correct modules) and the third variant of the same class this
    phase has found: `ADR-0067` a variant with no producer, `ADR-0068` a constraint with no implementation,
    `ADR-0069` a **joint with no test**. **Rule: for every pair that must agree, ask which test drives them
    TOGETHER.**
  - **⭐ THE CROSS-OPERATION TEST IS THE ONE WITH THE TEETH, and the single-operation seam tests would not have
    caught a mis-route.** Those assert that the expected data came back — and an operation wrongly routed to
    another endpoint that still answered with a parseable body would satisfy them. So one test drives **all
    four** operations and compares a **path per operation**. Verified rather than asserted: mutating the history
    builder's path to `/users/me/messages` failed with
    `google.gmail_history_list must address /gmail/v1/users/me/history, got /gmail/v1/users/me/messages?startHistoryId=12345`.
  - **⭐ TWO ROUTING MUTANTS WERE `VACUOUS`, NOT `FAIL`, AND THAT IS ITSELF A FINDING.** Pointing
    `gmail_history_list` at the messages *builder* failed to compile twice (`?` has incompatible types; 3
    arguments vs 4), because changing a builder's identity changes its argument types. So a routing mistake
    **inside** one module is largely unrepresentable — which is exactly why the seam (where the routing is
    *selected* and the request is *sent*) is where the defect can live. The `VACUOUS`/`FAIL` distinction kept the
    reading honest rather than counting a compile error as a detection.
  - **Both seams are `#[cfg(test)]` on BOTH sides**: `apply_origin` has a `#[cfg(not(test))]` sibling, so a
    shipped build contains no reference to the test origin at all — a single branch would have forced the seam
    to exist in every build for a test's sake.
  - **`HttpRequest`/`GoogleReadTool` assertions on the wire**: method, the encoded query (`q=is%3Aunread`,
    `maxResults=10`), the `Accept` header, a `Bearer ` authorization header, and no credential in the URL. The
    request **target** includes the query, so paths are compared with `split('?').next()` — the same split
    `url()`/`url_with_query()` makes; my first assertion compared the whole target and was wrong, not the code.
  - **NEW LIMITS:** still **no request to Google** — every response is written by the test file, so this proves
    the seam and not the record; the transport still has **no production caller** (no composition root builds
    the connector); the live smoke test remains unwritten.
  - **This round built the callback reader — the listener half of the flow had no implementation.**
    `Callback::from_request_target` in `authorization.rs`, plus `FormParameters` and `form_decode`, and two new
    `AuthRefusal` variants (so **310 in the crate**, 5 new tests). **`ADR-0070`.** **50 suites / 1575 workspace
    tests, 0 failed, 0 ignored.** Two guards falsified A-B-A with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: THE CALLBACK DECODING IS THE OPPOSITE OF THE REQUEST ENCODING, AND THE MISTAKE LOOKS
    LIKE AN ATTACK.** RFC 6749 §4.1.2 says the response parameters are added "using the
    `application/x-www-form-urlencoded` format, per Appendix B" — where a space is **`+`**. The request side
    (`ADR-0060`) encodes a space as `%20` **and a literal `+` as `%2B`**, deliberately, because RFC 3986 has no
    form semantics. So the same crate holds both rules and they disagree about one character. A `state` of
    `a b` arrives as `state=a+b`, and a decoder that kept the `+` would compare `a+b` against `a b` and refuse
    a **legitimate** response — surfacing as `StateMismatch`, i.e. as a possible forgery, while the cause is one
    character of decoding. **The worst shape a security refusal can take is a false one that reads as an
    attack.**
  - **⭐ A TEST FORCED A VARIANT SPLIT, and the failure was the finding.** `CallbackUnparsable` was one variant
    for both "the bytes could not be read" and "a parameter was repeated", and the codetable test failed on
    `indicates_forgery` (`left: true, right: false`) because one variant had to answer for both. They are now
    `CallbackMalformed { reason }` (a listener defect — a truncated escape, invalid UTF-8, or a target that
    names no loopback redirect) and `ParameterRepeated` (the shape an appended value takes). `indicates_forgery`
    is `true` only for the four mismatch/repeat variants. **`clippy::struct_excessive_bools`' lesson again: two
    values standing for more than two situations must be an enum.**
  - **A repeated parameter is REFUSED, not last-wins.** RFC 6749 §3.1 and §3.2 both require a parameter "MUST
    NOT be included more than once", and taking the last value is exactly how a `state` check is defeated —
    the server's own value comes first and an appended one second. A malformed escape or invalid UTF-8 is
    **refused rather than lossily decoded**, because a lossy decode turns a corrupted `state` into a *different*
    string that merely fails to match, hiding a transport fault behind a security refusal.
  - **`Callback`'s own doc claimed the decode lived elsewhere — and that division left it UNWRITTEN.** Every
    `Callback` in the tree was hand-assembled in a test, so no code here had ever read a request target. That is
    `ADR-0068`'s shape (a requirement with no implementation) and `ADR-0069`'s (a joint nobody drives) one layer
    further out: the *listener* had no reader. The `received_on` value is recovered **portless**, because an
    origin-form target carries no port — the `Host` header does — and `consume` still compares it against the
    transaction's ported registration with `matches_except_port`, so the port check is not lost, only moved to
    where both values exist.
  - **⚠ ONE OF MY OWN TEST PREMISES WAS WRONG AND THE FAILURE SURFACED IT.** I asserted a non-loopback target
    (`https://evil.example/cb?state=x`) would be refused; it is **accepted**, because the prefix check strips
    `http://` and the rest parses as a *path*. The claim was too strong; it was replaced by the accurate one
    (a target naming no loopback redirect is refused, asserted through the absolute-form case that genuinely
    fails) rather than by weakening the code.
  - **NEW LIMITS:** **no listener is bound**, so a callback has still never arrived over HTTP — what is closed
    is that the decode exists and is tested; the **form-POST transport for the token exchange** is still
    unbuilt, so `token.rs`'s exchange remains caller-driven; and nothing wires `from_request_target` into a
    server, because a loopback listener belongs to the composition root.
  - **This round moved the codec to one module and finished the rule — the encoder half was still missing.**
    New `crate::form` (`encode_component`, `decode_component`, `encode_body`, `CONTENT_TYPE`) with
    `form_tests.rs` (7 tests, so **318 in the crate**), and `authorization.rs`'s local `form_decode` deleted in
    favour of the shared decoder. **`ADR-0071`.** **50 suites / 1583 workspace tests, 0 failed, 0 ignored.**
    Two guards falsified A-B-A with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: THE ENCODING RULE IS NARROWER THAN `ADR-0070` RECORDED — the alphabet is
    ALPHANUMERICS ONLY.** `ADR-0070` established that a space is `+` rather than `%20`, and stopped there.
    HTML 4.01 §17.13.4 (which RFC 6749 Appendix B points at) actually says: "Space characters are replaced by
    `+', and then reserved characters are escaped ... **Non-alphanumeric characters are replaced by `%HH`'"**.
    So `~ - . _` are escaped too (`%7E %2D %2E %5F`), where RFC 3986's unreserved set — and this crate's
    `percent_encode` — leave them alone. The **decoder** accepted them either way, so round one could not see
    it; the gap appears only when an encoder exists. Escaping *more* is interoperable in both directions, while
    escaping less produces a value the server reads differently, so the wider escape is the safe one.
  - **TWO IMPLEMENTATIONS OF THE SAME ONE-CHARACTER RULE WERE ABOUT TO EXIST.** The token endpoint's form
    `POST` body needs this encoding in the **opposite direction** from the callback, so writing the encoder
    inside `token.rs` would have put a second copy of the space rule where the callback already had one —
    `P3-006a`'s "two values that must agree, with nothing holding both", for a codec rather than a struct. One
    module now holds both directions, and a test asserts the form encoder and `percent_encode` produce
    **different** output for the same input, so a refactor that merged them fails.
  - **⚠ MY OWN NEW TEST PREMISE WAS WRONG AND THE FAILURE SURFACED IT, exactly as in `ADR-0070`.** I wrote
    `encode_body`'s expectation as `"grant_type=authorization_code&code=a+b"` — with the parameter **names
    unescaped**. The code produced `"grant%5Ftype=authorization%5Fcode"`, and the code was right: the rule is
    stated about "control names **and** values". A named test now covers a name holding `&` and `=`
    (`"a&b=c"` → `"a%26b%3Dc"`) and fails if only values are encoded. **The reasonable-looking expectation is
    the one to check.**
  - **⭐ THE MODULE IS EXPORTED BECAUSE THE ENCODER HAS NO CALLER YET, AND CLIPPY SAID SO.** `pub` items inside
    a private module are unreachable and therefore dead code — the defect `P5-001` recorded and this crate hit
    when `google.rs` was private. Marking the encoder `#[cfg(test)]` instead would have hidden a function the
    next slice is meant to use, so the module is `pub` with the reason in its own doc.
  - **Falsified, two guards A-B-A with compiling mutants.** The space rule → a literal space (detected by
    **three** tests, incl. `left: "code=a b", right: "code=a+b"`); the name escaping → the raw name
    (detected by **two**, incl. `left: "a&b=c=v"`). One attempt printed `NOCHANGE` because the formatting had
    been refactored — the string-changed probe caught that rather than reporting a false pass.
  - **NEW LIMITS:** the **encoder has no caller** — `encode_body` is exercised only by its tests, because the
    token endpoint's form `POST` transport is unbuilt; HTML 4.01's line-break normalisation is deliberately
    **not** performed (a codec that rewrote the bytes would make a caller's refusal unobservable, and no OAuth
    parameter admits CR or LF anyway); and the codec does not validate what it encodes, so a value that is
    invalid for OAuth (a line break, an over-long token) encodes faithfully and is refused — or not — by the
    caller that owns the rule.
  - **This round found and fixed a LATENT double-encoding defect in the token exchange, one round after the
    encoder that would have exposed it was written.** `ExchangeIdentity::parameters()` no longer pre-encodes;
    `token::{body, content_type}` and `ExchangeIdentity::endpoint()` are added, and the test that asserted the
    pre-encoded form is **reversed** (3 new tests, so **320 in the crate**). **`ADR-0072`.** **50 suites /
    1585 workspace tests, 0 failed, 0 ignored.** One guard falsified A-B-A with a compiling mutant.
  - **⭐⭐ THE FINDING: THE TOKEN EXCHANGE WAS ESCAPING ITS VALUES WHERE THEY WERE STORED, NOT WHERE THEY WERE
    RENDERED — a double-encoding trap with no caller.** `ExchangeIdentity::parameters()` ran
    `percent_encode` over `client_id` and `redirect_uri`, and a test asserted `redirect_uri ==
    "http%3A%2F%2F127.0.0.1%2F"`. `ADR-0071`'s `encode_body` escapes everything it is given, so the two together
    produce `http%253A%252F%252F127.0.0.1%252F` — and Google answers a double-encoded `redirect_uri` with
    **`redirect_uri_mismatch`**, which names **client registration** rather than the encoding. **A fault whose
    symptom is one layer away from its cause is the one that costs a day.**
  - **⭐ THE DEFECT WAS LATENT UNTIL THE ENCODER EXISTED, AND THAT IS THE ARGUMENT FOR BUILDING THE RENDERER
    BEFORE THE TRANSPORT.** Nothing built a body, so the wrong-encoding path had no caller and the crate was
    green for several rounds. The moment `encode_body` landed, the two layers disagreed. Same family as
    `ADR-0068` (a requirement with no implementation) and `ADR-0069` (a joint nobody drives) — here the two
    halves are the *producer* and the *renderer* of one value, and **only a pipeline test covers them.**
  - **The rule this makes explicit: a value is encoded where it is RENDERED, never where it is STORED.** A list
    of values with an encoding already applied is a *half-rendered request*: every consumer must know whether
    it has been rendered, and the one that guesses wrong produces this failure. The general assertion is now
    part of the tests — **a `%25` anywhere in a rendered form body means some value was escaped where it was
    stored** — and it is checked for both the identity parameters and the whole exchange.
  - Two accessors make the rendering layer reachable without a socket: `token::body(&[(String, String)])` and
    `token::content_type()`, plus `ExchangeIdentity::endpoint()`, which takes the host **from the manifest**
    rather than restating it. That preserves `P5-004`'s recorded oddity — the consent screen and the exchange
    are on **different hosts** (`accounts.google.com` vs `oauth2.googleapis.com`), so a reader "tidying" them
    into one constant would break every exchange — and a test asserts the literal.
  - **Falsified, one guard A-B-A with a compiling mutant.** Restoring the pre-encoding was detected by **two**
    tests, and the failure printed the whole doubled body (`redirect%5Furi=http%253A%252F%252F127%2E0%2E0%2E1%252F`)
    so the message names its own cause. Restored byte-identical.
  - **NEW LIMITS:** nothing sends the request — `HttpMethod` has one variant, so a form `POST` is not
    expressible by the read transport and the caller still drives the exchange; the endpoint, parameter lists,
    body and answer reader are all present and tested, so the transport slice is now a thin binding; and the
    `client_id` is still only checked for non-emptiness, so a pasted URL is accepted here and fails at the
    provider.
  - **This round built the exchange over a transport — and `TokenRequestOutcome` finally has a producer.**
    `request::FormRequest`, `GoogleTransport::send_form` with its `reqwest` arm, `token::exchange`, and
    `TransportFailure::reason` (a `const fn`, because `NeverSent` holds a static reason); a new
    `token_exchange_tests.rs` (6 tests, so **326 in the crate**). **`ADR-0073`.** **50 suites / 1591 workspace
    tests, 0 failed, 0 ignored.** Two guards falsified A-B-A with compiling mutants and restored byte-identical.
  - **⭐⭐ THE FINDING: `TokenRequestOutcome` WAS DEAD CODE WEARING A CONTRACT, AND TWO THINGS BLOCKED IT.** Its
    own doc called the `NeverSent`/`SentAnswerUnknown` distinction consequential (RFC 9700 §4.2.4: a retry of a
    lost-answer request may consume the code **and destroy a working grant the first attempt issued**), and
    **every construction site was in `token_tests.rs`** — the `P5-001` defect, and the third instance this
    phase of *a declaration nothing produces* (`ADR-0067` the variant form, `ADR-0068` the constraint form,
    this the **outcome** form). The second blocker was structural: the exchange is a form `POST` and
    `HttpMethod` deliberately has one variant, so the port could not express the method — the type had neither
    a producer nor the **ability** to have one.
  - **⭐ A SEPARATE `FormRequest` TYPE, because the two request shapes have OPPOSITE credential boundaries.**
    `HttpRequest` exists so it **cannot** hold a credential in its URL (`ADR-0060`); a token request's body
    **is** the credential-bearing text. A union would either give `HttpRequest` a field a credential goes in or
    force every caller to prove which kind it holds. `FormRequest` takes the **already-rendered** body, so it
    cannot encode and therefore cannot double-encode (`ADR-0072`), and its `Debug` is hand-written to
    `[REDACTED]` + a character count with **no `Display`** — `ADR-0061`'s rule, since a derived `Debug` would
    render the body through any `{:?}`. A test asserts the rendering and the count.
  - **⭐ `send_form` IS A SECOND PORT METHOD, NOT A THIRD `HttpMethod` VARIANT.** `HttpMethod`'s own doc says
    adding a write is `P5-009`'s decision; this is not that — it is the token endpoint's *framing*, which
    authenticates by its body's `client_id` and has **no bearer token**. Folding it into `send` would give the
    read path a body it does not have and the token path a credential parameter it does not use: one method
    with two disjoint modes. Both test doubles gained a `send_form` arm that **panics** when the wrong path
    calls it, so a mistake is a test failure rather than a silent success.
  - **⭐ `reqwest`'s `form` FEATURE IS DELIBERATELY NOT ENABLED.** `.form()` exists but is feature-gated, and
    enabling it would add the dependency's form encoder beside `crate::form` — two implementations of the one
    character `ADR-0071`/`ADR-0072` exist to keep single, disagreeing **invisibly** because both produce a
    plausible body. So the request is built with `.body(rendered)` and an explicit `Content-Type`, and a test
    asserts the recorded body contains no `%25`.
  - **The retry-safety cases are asserted AGAINST `may_have_reached_the_provider`, not against a hand-written
    list of which failures are ambiguous.** A new `TransportFailure` variant therefore cannot be classified by
    omission: the fixture fails first if the predicate and the expectation disagree. And an unreadable body is
    an **`Err`**, not `Refused` — folding it in would send a user to a consent screen when the fault is a body
    this client cannot parse, which is a different thing from the provider answering "no".
  - **Falsified, two guards A-B-A with compiling mutants.** Collapsing the ambiguity to `NeverSent` was caught
    with `Send may have been written, so a retry could repeat an effect` (`left: NeverSent` / `right:
    SentAnswerUnknown`); turning an unreadable body into a refusal was caught by the exchange test. Restored
    byte-identical.
  - **NEW LIMITS:** **the exchange has no caller** — nothing constructs a `FormRequest` outside a test, because
    no composition root builds the connector, so a live call needs credentials and a composition root rather
    than more code here; the `refresh` path is not yet driven through `exchange` (only the code exchange is);
    and the request carries no deadline of its own, so it relies on the client's configured timeout.
  - **This round gave the refresh path a producer — and found a doc claiming a delegation the code never made.**
    `token::refresh_with` (so `RefreshExchange` has a caller outside a test), the shared `send_and_parse` join
    plus an `ExchangeFailure` enum keeping transport-failure and unreadable-body apart, and `classify_refresh`
    reduced to a one-line delegation; the private `refresh_outcome` copy deleted. 4 new tests, so **330 in the
    crate**. **`ADR-0074`.** **50 suites / 1595 workspace tests, 0 failed, 0 ignored.** Two guards falsified
    A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING, AND IT WAS FOUND BY A MUTATION SURVIVING: `classify_refresh`'s DOC NAMED THE SHARED
    CLASSIFIER AS THE AUTHORITY AND ITS BODY CALLED A PRIVATE COPY.** Its own text said "The classification
    itself is [`RefreshExchange::classify`]'s", while the code called `refresh_outcome(&self, ..)` — a local
    `match` restating **both** rules the shared module owns: the rotation test, *with its own copy of the
    "arrival, not storage" comment*, and the transient-first ordering, *with its own copy of the "a 503 can
    carry `invalid_grant`" comment*. Two implementations of an ordering whose whole content IS the ordering,
    and a doc naming the other as the authority. This is `P5-004`'s class: **a doc saying "the rule lives in
    X" is a claim about code, and X has to be read.**
  - **⭐⭐ A SURVIVING MUTATION IS NOT ALWAYS A WEAK TEST — it can be a ROUTING signal, and the two are told
    apart by checking REACHABILITY.** The mutant was `if failure.transient` → `if false` **in `token.rs`**, and
    it survived every test including the one written to assert that ordering. A weak test and a wrong path look
    identical from the result; the way to distinguish them is to ask whether the mutated code is reachable from
    the assertion — **grep its CALLERS, not its definition**. After the dedup the same mutant fails with
    `left: Expired, right: Transient`. So the test was sound and only blind while production took another path.
  - **⚠ A FALSIFICATION CAN REPORT A MISLEADING RESTORE, AND MINE DID.** One probe printed `RESTORED=True`
    while a **second** mutant was still in the file: the `.bak` had been captured after the first mutation was
    applied, so the "byte-identical" check compared against a corrupted baseline. It surfaced as three failing
    tests **after** the refactor. **Verify the mutant TEXT is absent (`Select-String`) rather than trusting a
    restore flag that compares to a snapshot you may have taken at the wrong moment.**
  - **The `#[cfg(test)]` import is now a compile-time witness.** `RefreshOutcome` is needed by this module only
    through the shared classifier, so its import is test-only; if a future edit reintroduces a local
    classification the import becomes unused and the build fails. Clippy caught the first version either way.
  - **⭐ THE TRANSPORT-FAILURE ASYMMETRY, asserted through the producer rather than the classifier.** A refresh's
    ambiguous transport failure is `Transient` — and that is **safe** because a refresh presents the *stored*
    token: if a rotation silently landed, the stored reference is invalid and the next attempt answers
    `invalid_grant` → `Expired`/`Revoked` → `needs_user`. So it **self-corrects over one extra call**, where
    retrying a lost *code* exchange could revoke tokens. The test drives both steps rather than arguing the
    point, and records that a rotation arriving unread is a limit of `RefreshOutcome`'s vocabulary.
  - **Falsified, two guards A-B-A with compiling mutants.** The rotation rule, mutated to read the caller's
    storage (`response.has_refresh_token && new_reference.is_some()`), caught with `left: Refreshed, right:
    Rotated`; the transient ordering, caught as above. The first attempt at each targeted the wrong FILE
    (`google/token.rs`, where the copy lived) — which is itself the evidence for the finding.
  - **NEW LIMITS:** **`refresh_with` has no caller outside its tests** — the composition root owns the secret
    store, so minting and storing a rotated reference is still unbuilt; a **rotation that arrives unread** is
    invisible until the next attempt (a `RefreshOutcome` vocabulary gap, not a defect in this function); and
    the refresh branch is reachable only through the producer, so `exchange`'s own refresh handling is
    untouched by these tests.
  - **This round wired the retry-classification table into the refusal path — it had no production caller.**
    `google::operations::refusal` now calls `client::classify` and appends the class and the stated delay to the
    bounded reason; 2 new tests (so **332 in the crate**). **`ADR-0075`.** **50 suites / 1597 workspace tests,
    0 failed, 0 ignored.** Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE TABLE `ADR-0058` EXISTS FOR DECIDED NOTHING.** `client::classify`'s own doc states
    the case for its existence — "**a 403's meaning comes from its `reason`, never from the status** … a
    classifier switching on the status alone would retry an administrator's decision" — and **nothing in
    production called it**. `refusal` read the machine-readable reason code and formatted it, so `RetryClass`
    and `RetryGuidance` reached nothing, and **`TransportResponse::retry_after_seconds` was carried, validated,
    documented as "whether a stated delay is honoured is `client::classify`'s decision", and then dropped**. A
    `429` with `Retry-After: 37` produced a reason naming neither the class nor the delay. This is the fourth
    form of *a declaration nothing produces* this phase: the variant (`ADR-0067`), the constraint (`ADR-0068`),
    the outcome (`ADR-0073`) and now the **policy table**.
  - **⭐ THE DELAY IS APPENDED ONLY WHEN `guidance.delay_seconds()` IS `Some`, and the test proves it with a
    CONTROLLED case.** The permanent/authentication/reconcile arms carry no seconds, so a refusal that must not
    be retried cannot show a delay **even when the wire carried one** — a caller seeing "retry after 60s"
    beside `domainPolicy` would back off and retry an administrator's decision. The test sends a
    `403 domainPolicy` **with** `Retry-After: 60` and asserts the absence, which is the case a naive
    "append the header" implementation gets wrong.
  - **The class rides in the reason because `jarvis-tools`' `AdapterError` has no field for a connector's own
    retry vocabulary** — a real constraint rather than a preference, and the reason `ToolOutcomeRecord`'s
    bounded reason is the one channel that reaches a caller. Verified shape:
    `the provider refused the call: rateLimitExceeded (throttled); retry after 37s`.
  - **An unreadable body is still classified by its status**, so a `503` with HTML stays `provider_fault` and an
    unrecognised `418` says `unknown` **and carries no delay** (`Reconcile` offers none, because a retry is not
    the action). That extends "we know little versus we know nothing" to the retry decision.
  - **Falsified, two guards A-B-A with compiling mutants.** Dropping the stated delay (caught, and the failure
    printed the fallback floor `retry after 1s`, so the mutation was a genuine loss); removing the class from
    the reason (caught, printing `rateLimitExceeded ()`).
  - **NEW LIMITS:** **`RetryDecision::provider_request_id` is still unpopulated in production**, and that is a
    real gap rather than an oversight: `TransportResponse` carries no headers at all. (A previous version of
    this limit asserted that Google "returns it in a response **header**"; Gmail's `handle-errors` guide, read
    2026-09-15, names **no request-id header**, so that was an assumption stated as a finding — corrected in
    `ADR-0076`. Whether Google supplies one is **unverified**.) A `TransportResponse` change with its own
    falsifying test is what it needs, not a fix here; the reason is a
    **rendering** of the decision rather than the decision itself, because `ToolExecutor`'s contract has no
    place for a connector's vocabulary (ADR-0047's boundary); and `GOOGLE_RETRY_FLOOR_SECONDS` is a JARVIS
    choice where Google publishes a range, so a delay of that value is indistinguishable in the reason from a
    provider-stated one.
- [ ] `P5-005` **(continued — `Retry-After` is three situations, not two)**: `TransportResponse::retry_after` is
    `Option<RetryAfter>` (`Seconds(u32)` | `NotSeconds`) instead of `Option<u32>`, both `reqwest` sites use the
    new `google::transport::parse_retry_after`, and `client::classify`'s `429` arm keeps *absent*, *stated as
    seconds*, and *stated but unreadable* apart, rendering the third as
    `… (throttled); retry after 1s (the provider stated a delay this client could not read)`. `RetryGuidance`
    gained `BackoffAfterUnreadableDelay`. **1 new test (so 333 in the crate).** **`ADR-0076`.** Two guards
    falsified A-B-A with compiling mutants.
  - **⭐⭐ THE DEFECT: TWO VALUES STANDING FOR THREE SITUATIONS.** `RFC 9110` §10.2.3 defines
    `Retry-After = HTTP-date / delay-seconds`, and `Option<u32>` cannot tell *the header was absent* from
    *the header was present in the date form*. The transport parsed digits only, so a conforming `HTTP-date`
    became `None` — **the same value as no header at all** — and both fell to the floor. That is the direction
    that retries **too soon**, and §5.6.7 makes the date form one a recipient **MUST accept**. The old test had
    already recorded the shape of the gap in the words chosen for it — "an unreadable value is **absent**, not
    zero" — which was true of the old type and is exactly the conflation the new type removes. Fifth form of
    *a declaration whose values do not cover its situations* this phase, and the first **value** form.
  - **⭐ THE VARIANT IS NAMED FOR WHAT IS READABLE, NOT FOR THE CONCLUSION.** `HttpDate` was the obvious name
    and would be a **lie**: `delay-seconds = 1*DIGIT` has **no upper bound**, so
    `Retry-After: 99999999999999999999` is `delay-seconds` by grammar and does not fit a `u32` — a plain
    `parse::<u32>().ok()` returns `None` for it, which reads as "retry now". Both land in `NotSeconds`, because
    the only distinction a retry decision needs is *"can this client express the stated delay in seconds"*;
    naming it after the diagnosis would have hidden the case where the diagnosis is wrong.
  - **The wait is the floor for both non-numeric cases; the WORDS are what distinguish them.** A `RetryGuidance`
    variant rather than a different number, because honouring the date form needs a clock and a transport that
    read its own clock would make the retry decision the port's own doc says it must not. **Refusing** to retry
    was rejected (throttling is the most retryable class); a **silent** fallback was rejected because it presents
    a JARVIS floor as the provider's instruction.
  - **The correction that makes this slice's record honest.** `ADR-0075` asserted Google "returns the identifier
    in a response **header**". Re-fetching the source it was written from (Gmail `handle-errors`, via
    `developers.google.com`): the page names **no request-id header**, so that was an **assumption stated as a
    finding**. Corrected in `ADR-0075`, here, and in the code comment; **whether Google supplies one is now
    recorded as unverified** rather than as a premise. No reader was built — settling an unverified name first
    is the point.
  - **An empty value is absent.** Both grammars require content (`1*DIGIT`, `HTTP-date`), so a present-but-empty
    value carries nothing to interpret. The one case where "present" and "absent" legitimately coincide.
  - **NEW LIMITS:** the date form is **recognised but not converted to a delay** — the conversion needs a clock
    and a policy for a skewed or hostile one (RFC 9110 §8.8.1 is explicit that a validator is not a trust
    mechanism), so it is a separate decision with its own falsifying test. `provider_request_id` remains
    unpopulated **and unverified** as above. `parse_retry_after` accepts exactly `1*DIGIT` — not `+30`, not
    `30.5` — which matches the grammar but means a provider that sent a signed or fractional value lands in
    `NotSeconds` rather than being coerced.
- [ ] `P5-005` **(continued — a bound that is documented but not applied is not a bound)**: `RetryGuidance`
    gained `DeferSeconds(u32)` plus a `deferred_seconds()` accessor, and `RetryGuidance::for_stated_delay` is the
    **one** path from a provider's stated number to retry guidance, applying `MAX_RETRY_AFTER_SECONDS`. The dead
    `RateLimitError::RetryAfterTooLong` variant was removed. `classify`'s `429` arm uses the constructor, and the
    reason renders the over-ceiling case as `defer for 18000s (above the 3600s this caller will hold)`. **2 new
    tests (so 334 in the crate).** **`ADR-0077`.** Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: `MAX_RETRY_AFTER_SECONDS` WAS A RULE NOTHING APPLIED.** Its doc states a *rule*, not a
    figure — "a longer value is **refused** rather than clamped: a clamp would silently retry **sooner** than the
    provider asked" — and a repository search found the identifier in exactly **three** places: its definition,
    a `pub use`, and a doc link. **No code read it.** The companion `RateLimitError::RetryAfterTooLong` made the
    same claim ("enforced by the constructor a caller would use") and had **no constructor**; its only
    construction in the tree was inside a test that asserted the message **formatting**, which passed for as long
    as the type existed and proved nothing. **A construction that exists only in a test is the signature: the
    type can hold the value, no code produces it.**
  - **⭐ `ADR-0076` made the gap load-bearing.** Routing the provider's stated delay into `RetryAfterSeconds`
    *without* the bound meant a `429` answering `Retry-After: 18000` produced `retry after 18000s` — a **five-hour
    wait presented as an ordinary retry delay**. Google documents exactly this: a daily-limit `429` "might result
    in these errors for multiple hours". So this is a new form of the phase's recurring defect: not a *type* that
    can hold a value nothing produces (`0067`/`0068`/`0073`/`0075`/`0076`), but a **constant whose stated rule
    nothing applied** — declaration and behaviour one comment apart, the comment the only thing that knew.
  - **`DeferSeconds` answers `None` from `delay_seconds()` — and that is the load-bearing choice.** That accessor
    answers "how long before the automatic retry", and this guidance **forbids** the automatic retry; a number
    there would make `delay_seconds().is_some()` mean "retryable" and reintroduce the hazard. The provider's
    number moves to a separate `deferred_seconds()`, and a test asserts the two accessors are **disjoint for
    every variant**.
  - **The refusal is a VARIANT, not an error.** A provider asking to wait five hours is a real response meaning
    *defer the work* — a state this vocabulary already names (`BudgetOutcome::Exhausted` is the budget-side twin).
    A `Result` every caller immediately converted into `DeferSeconds` would be ceremony, and a caller could `?` a
    plan to wait into a hard failure, so `for_stated_delay` is **total**.
  - **The dead error variant was removed with the reason recorded in place**, because keeping it would leave a
    second declaration of the same rule that nothing constructs — the defect this slice is about.
  - **NEW LIMITS:** no consumer reads `RetryDecision` outside `jarvis-connectors` yet, so nothing *schedules* the
    deferred work — the deferral reaches an operator's eyes through the bounded reason, and the pipeline half is
    the same run-record gap `ADR-0075` recorded for `provider_request_id`. The ceiling is a **platform constant**
    rather than a per-provider or policy value, so it cannot be tuned without a code change.
- [ ] `P5-005` **(continued — a scope's category is a review burden)**: `google::scopes` adds `ScopeCategory`,
  `VerificationBurden`, `AssessmentRequirement`, `ScopeAccounting` and the pure `account` function, plus
  `google::gmail_scope_categories()` as the dated table transcribed from the scopes page. **8 new tests (so 342
  in the crate).** **`ADR-0078`.** Two guards falsified A-B-A with compiling mutants. **This writes the research
  record's own "scope-category tests" item, which was open.**
  - **⭐⭐ THE FINDING: THE CATEGORY DECIDES A SECURITY ASSESSMENT, AND IT LIVED ONLY IN A DOC COMMENT.** The
    scopes page states that a restricted scope requires "restricted scope OAuth App Verification" **and** that
    "**if you store restricted scope data on servers (or transmit), then you must go through a security
    assessment**". `SCOPE_GMAIL_READONLY`'s comment said "**Restricted** rather than sensitive, which is the fact
    that governs what deploying this connector costs" — correct, and the **only** place the fact existed. Nothing
    could check it, and a new scope could be added without anyone deciding its category. Re-fetched the page: the
    recorded categories were **correct**, and are now a dated table.
  - **⭐ THE ACCOUNTING IS OVER THE REAL MANIFEST, NOT A FIXTURE LIST — and it reports a partial reading honestly.**
    The record's own item asked for scopes "all *accounted for* … so adding a scope forces a decision", which is
    **not** the same as "assert these three are restricted" (that passes when a fourth is added). Two of the four
    declared scopes — `openid` and `calendar.readonly` — have **no recorded category**, so the deployment's
    burden is `Unestablished` rather than "restricted" inferred from the two that are known. A partial maximum
    would present a partial reading as a complete one.
  - **⭐ AN UNLISTED SCOPE IS NOT A CHEAP SCOPE.** `Unknown`'s burden is `Unestablished`, **not** `BasicReview`.
    Under-reporting is the direction that harms: an operator told "basic review" who ships a restricted scope has
    skipped an assessment Google requires, and nothing would have said so. The assessment is therefore a
    **three-valued** `AssessmentRequirement`, because a `bool` would make `NotRequired` and `NotEstablished` the
    same value — the same conflation as `RateLimitEvidence`'s `Documented` vs `Observed`.
  - **The scope match is EXACT.** A scope string goes into an authorization request, so a prefix match would let
    a longer scope inherit a **cheaper** category from a scope that is its prefix — under-reporting again.
  - **A defect in this slice's own first draft, caught by its own test.** `AssessmentRequirement` originally had
    `is_required_regardless`, whose comment claimed "both answer `false`" while the code returned `true`; once
    fixed to match the comment, the predicate was **false for every value** — Google's rule is conditional for
    the only category that requires an assessment, so nothing is ever required *regardless*. Replaced by
    `may_require_an_assessment`, which fails closed and a caller can branch on. **Same class as `0067`–`0077`: a
    declaration no input can exercise** — found in the slice that was writing about that class.
  - **A date inconsistency corrected.** `ADR-0076` and two research-log rows were dated **2026-09-28** while the
    current date and every other record in the session are 2026-09-27. Corrected.
  - **NEW LIMITS:** the Calendar and `openid` scope categories are **not established** (the pages read do not
    state one), recorded as Unresolved Question 2 and surfaced by the accounting rather than assumed away. The
    **internal-app exemption** is deliberately **not** modelled — it is a property of the consent screen's
    audience setting, not of the scope set, so a field for it could only be set wrongly.
- [ ] `P5-005` **(continued — a rate limit without its unit)**: `RateLimit` gains a required `unit`
  (`RateLimitUnit::{Requests, CostUnits}`) and a `sustained_requests_per_second()` that answers `None` for a
  cost-unit limit; `ConnectorOperation` gains `quota_cost` (`QuotaCost::{Documented(u32), Unstated}`) with a
  `calls_per_window` conversion; the four operations declare their published costs. The two byte-identical
  `gmail_read_rate_limit`/`gmail_history_rate_limit` functions collapse to one `gmail_rate_limit()`. **3 new
  tests (so 345 in the crate).** **`ADR-0079`.** Two guards falsified A-B-A with compiling mutants. **This writes
  the research record's own "quota-cost test" item**, and it fixed two more defects found against the docs.
  - **⭐⭐ THE FINDING: A QUOTA-UNIT FIGURE WAS BEING READ AS A REQUEST RATE, 20× OVER.** `RateLimit`'s fields and
    `sustained_per_second()` both said **requests**; Google publishes 1,200,000 and 6,000 as **quota units**,
    defining them as "an abstract unit of measurement representing Gmail resource usage", with per-method costs
    from 1 to 100. So the read path reported **20,000/s** where the true `messages.get` rate is **1,000/s** — and
    the error is **silent**: the connector behaves correctly until the provider starts refusing calls. The
    divergence is per operation, not per connector: the same ceiling is 60,000 `messages.get`/min and 600,000
    `history.list`/min, a **10× spread**.
  - **⭐ THE DUPLICATE THAT NAMED A REAL DISTINCTION IN THE WRONG PLACE.** Two rate-limit functions were
    **byte-identical**, and the second's doc justified itself by "the two calls have different documented costs".
    The costs *do* differ (20 vs 2) — the reason was **true** — but a per-call cost is not a property of a rate
    limit, and `RateLimit` had no field for it. So two identical limits carried a distinction that belonged to
    the operation. Moving it there is what lets one ceiling be declared once.
  - **`sustained_requests_per_second()` answers `None` for a cost-unit limit.** A cost-unit limit has **no**
    request rate until an operation's cost is known, so the value a scheduler would most easily mistake is not
    offered at all. `sustained_per_second` survives with its doc corrected to say the result is **in the limit's
    own unit**.
  - **`QuotaCost::Unstated` is not `Documented(1)` — and it is the `Default`.** Reading an unstated cost as 1
    computes the whole allowance as a request rate and over-plans by the real cost. `calendar_events_read`
    **ships** `Unstated`, because this page publishes no Calendar cost; claiming 1 would invent a figure the
    provider never stated.
  - **Two further defects found by reading code against its own docs.** (a) `SCOPE_OPENID`'s doc said
    `users.getProfile` "is the operation declared below" — **no profile operation exists**; the doc now says the
    scope is requested and the operation is not yet declared. (b) `RateLimit::new`'s error message said "1 to
    1000000 **requests** per window" while the figure may be cost units; corrected to "per window".
  - **NEW LIMITS:** `RateLimit` has **no daily window**, so Google's 80,000,000-unit daily threshold (which
    **cannot be raised**) stays in the research record rather than in a declaration — a window kind is a
    separate decision with its own falsifying test. The cost-unit conversion is only tested as arithmetic; no
    run has planned against a real provider yet, so a conversion that still over-plans because the *per-user*
    ceiling binds is not observable here. And `sustained_requests_per_second()`, `QuotaCost::units()` and
    `QuotaCost::is_documented()` have **test-only consumers** — the same shape `ADR-0057` already records for
    `PollingInterval::is_documented`, since nothing schedules yet. Stated rather than left for a reader to
    discover: their callers appear when a scheduler consumes the conversion, not before.
- [ ] `P5-005` **(continued — a limit and a recommendation are two facts)**: `GMAIL_BATCH_LIMIT` (50, "the
  largest batch Gmail accepts") splits into `GMAIL_BATCH_HARD_LIMIT` (100) and `GMAIL_BATCH_RECOMMENDED` (50),
  with `BatchPlan`/`batch_plan` as the producer. **3 new tests (so 348 in the crate).** **`ADR-0080`.** Two
  guards falsified A-B-A with compiling mutants. **Writes the record's "full-sync budget test that asserts
  batching" item, and corrects its premise.**
  - **⭐⭐ THE FINDING: A FIGURE FROM A PAGE THE RECORD NEVER LISTED, FILED UNDER ONE THAT DOES NOT STATE IT.**
    The record's "Other limits" said "**Batch requests: no more than 50**", and its Verification Log attributed
    "the batch ceiling of 50" to the **quota page** — which states **no batch limit at all**. The **batch
    reference** (`guides/batch`) was **absent from the source table entirely**. Re-fetched: it says "You're
    limited to **100** calls in a single batch request" and separately "We recommend sending batches of no more
    than **50**". So the number was real, **half-right in value and half-right in meaning, attached to the wrong
    document** — the quietest form of the failure `external-research.md` warns about: not an invented number, but
    a real one from an unrecorded source. A reader checking the record against the quota page would not find it.
  - **⭐ ONE CONSTANT COLLAPSED A REFUSAL INTO A SLOWDOWN.** "Two documented facts, in tension" was the old doc's
    own words, and it resolved the tension by picking one figure and calling it a ceiling. But exceeding **100**
    *fails the request* while exceeding **50** *degrades throughput* — different failures, different remedies —
    so a caller holding one number cannot know whether a size between them is a bug or a trade-off. The names now
    carry it: `HARD_LIMIT` is refused above, `RECOMMENDED` is **allowed and reported**.
  - **`batch_plan` rounds the request count UP, and that is the arithmetic a naive planner gets wrong.** 101
    calls at 50 per batch is **three** requests, not two; floor division **drops the remainder's request** and
    therefore skips part of a sync *while reporting success*. `final_batch_size` is carried so the partial batch
    is a value rather than a recomputation.
  - **A size above the recommendation is permitted, not refused.** Refusing it would be stricter than Google and
    would hide the 50–100 range the API accepts. `is_within_recommendation()`'s `false` means "invites
    throttling", and it is asserted in both directions so it is not false for everything — the defect `ADR-0078`
    found in its own first draft.
  - **The batch page states a fact that changes how a sync is sized: batching saves connections and NO quota.**
    "A set of *n* requests batched together counts toward your usage limit as *n* requests, not as one request."
    Worth recording because "batch the sync to make it affordable" is the natural misreading, and the record's
    own `5 + 20N` estimate is **unchanged** by batching.
  - **`ADR-0058` was annotated**, because it introduced the wrong constant and credited it with being a
    provider fact rather than a correction.
  - **NEW LIMITS:** `batch_plan` is arithmetic and a type, not a scheduler — nothing yet *paces* batches, so
    `is_within_recommendation()` has a **test-only consumer**, the same pipeline-side gap `ADR-0075` records for
    `provider_request_id`. No delay is asserted for a recommended-size batch because the record states **no
    figure** for it, only that throttling is *likely* — a risk rather than a number, and inventing one would be
    the defect this slice is about.
- [ ] `P5-005` **(continued — Calendar's 410 needs its reason)**: `CalendarGoneReason`
  (`FullSyncRequired`/`ResourceAlreadyDeleted`/`Unrecognised`) plus `client::calendar_signal` as the producer;
  three Calendar error fixtures, including a **same-status 410 pair with opposite remedies**. **4 new tests (so
  348 in the crate; the fixture suite went 11 → 15).** **`ADR-0081`.** Two guards falsified A-B-A with compiling
  mutants. **Writes the record's "410-is-staleness test" item and corrects its premise.**
  - **⭐⭐ THE FINDING: "CALENDAR'S 410 HAS NO SUCH AMBIGUITY" WAS TRUE OF THE STATUS AND FALSE OF THE DECISION.**
    The record asserted it while contrasting Calendar with Gmail's unclassifiable 404. The Calendar **errors**
    page — a page the record's source table did not list for this fact — publishes **three** bodies for
    `410 Gone` and only two resync: `fullSyncRequired` ("wipe the store and re-sync"), `updatedMinTooLongAgo`
    (same), and **`deleted`**, whose suggested action is **"no further action is necessary"**. So a connector
    reading the status alone **wipes a whole sync store when a user deletes one event** — and `advance_calendar_sync`
    maps `CursorUnusable` to a full wipe with **no cursor**, so the wrong branch is the destructive one.
  - **⭐ THE `deleted` CASE IS A REFUSAL, NOT A SUCCESS.** A delete of an already-deleted event did not do what
    was asked, even though nothing needs repairing — so it is carried as `SyncSignal::Refused` rather than
    pretending the call succeeded. Same rule as `P3-005` for an outcome the adapter could not establish.
  - **⭐ AN UNREADABLE 410 STILL RESYNCS, AND THAT IS THE OPPOSITE OF THE CRATE'S USUAL RULE — deliberately.**
    `RetryClass::Unknown` refuses because a retry could send a **second effect**; here the thing at risk is a
    **store's liveness**. Resyncing needlessly costs a slower next sync; *not* resyncing a genuinely dead token
    costs a store that never syncs again and never says so. The direction is argued on the variant so a reader
    meets the reasoning rather than inferring a contradiction.
  - **The status-only predicate SURVIVES with its competence narrowed.** It cannot be deleted (an unparseable
    body still needs a recovery path) and cannot be made strict (that regresses the unreadable case), so two
    functions answer the same question for callers holding different information — normally a defect, and here
    the difference is exactly which facts the caller has. Both are asserted, including that they disagree on
    `deleted`.
  - **The fixture pair is what makes the claim falsifiable.** Two files with the **same status** and opposite
    remedies: a connector reading the status gives them the same answer, and the test asserts they differ. The
    three Calendar fixtures also completed the record's fixture table (corrected **nine → twelve**).
  - **NEW LIMITS:** `calendar_signal` is a producer with **no production caller** — there is no `events.list`
    request to obtain a status and body from — so the tests are what currently hold the distinction in place;
    the same pipeline-side gap `ADR-0075` records for `provider_request_id`. The record still lists **no source
    for `updatedMinTooLongAgo` beyond the errors page**, and the two sync-token causes share one variant because
    they share a remedy, so a diagnostic cannot distinguish them from the type alone.
- [ ] `P5-005` **(continued — one classifier for two APIs)**: `classify` takes the API, and the error vocabulary
  is renamed for what it is. **4 new tests (so 352 in the crate).** **`ADR-0082`.** Two guards falsified A-B-A
  with compiling mutants. **Adds Finding 6 to the record.**
  - **⭐⭐ THE FINDING: THE TWO ERROR PAGES PUBLISH DIFFERENT STATUS SETS, AND ONE CLASSIFIER ANSWERED FOR THE
    LESS INFORMATIVE ONE.** `GmailErrorReason`/`GmailError`* were named for Gmail but parsed **every** error body
    — Calendar's included — and `classify` had no API parameter. Gmail's error guide documents **no `410`
    subsection at all**; Calendar's documents `410 Gone` in detail. So a Calendar `410` — the dead sync token
    `ADR-0081` had just built a reason vocabulary around — fell to the catch-all and reached a caller as
    *"the provider answered 410 (unknown)"*, i.e. `Reconcile`, *establish what happened before doing anything
    else*, when the provider had already stated the cause and the remedy.
  - **The API is an input, not a second table.** Two tables would duplicate every arm the pages **do** agree on
    (`401`; the `5xx` family; `403`'s throttling reasons, which the Calendar page itself calls "functionally
    similar" across `403` and `429`). A duplicated table drifts, which is the defect this crate has found
    repeatedly, so `classify(api, status, …)` carries the API beside the status exactly as the status sits
    beside the reason.
  - **A Calendar `410` is `Permanent`/`DoNotRetry` and that does not contradict the resync remedy.** A resync is
    not a retry of *this* request — the same token cannot succeed — so the refusal is right, and the comment
    spells the distinction out because `DoNotRetry` read as "give up on the sync" is the mistake it prevents.
  - **The second divergence is recorded, not silently resolved.** Calendar's page suggests "use exponential
    backoff" for a `404`; Gmail's states no action. The crate keeps `DoNotRetry` for both — Calendar's own two
    `404` causes are a resource that never existed and a calendar the user cannot access, neither repaired by
    resending — and
    **asserts the divergence in a test that names which document wins** (`ADR-0081`'s technique applied to a
    documented disagreement). Logged as Unresolved Question 8, because the page's sentence and its own causes
    disagree and only a live call can settle it.
  - **The API is derived, not passed beside the operation name.** `api_of` reads the `gmail_`/`calendar_` prefix
    the manifest already uses, so `calendar_events_read` + `GoogleApi::Gmail` — the pairing that would
    reintroduce the defect — is unrepresentable rather than merely discouraged. The rename is part of the fix:
    a type named `GmailErrorReason` that Calendar responses are parsed into is a false statement about scope.
  - **NEW LIMITS:** the refusal message for an unparsed Calendar `410` renders `"the provider answered 410
    (permanent)"` and does **not** name the dead sync token — the class is right and the explanation is thin,
    because the words come from `reason` and a `410` need not carry one. `api_of` derives from the operation-id
    prefix, so a future operation not following that convention would silently get `Gmail`. Nothing outside
    `jarvis-connectors` consumes `RetryDecision` yet — the recurring pipeline-side gap also recorded for
    `provider_request_id`, `batch_plan` and `calendar_signal`.
- [ ] `P5-005` **(continued — a declared output field is bounded by what the request can return)**:
  `gmail_messages_read` now renders every field it declares, and its schema no longer declares a field the
  request cannot return. **4 new tests (so 356 in the crate).** **`ADR-0083`.** Two guards falsified A-B-A with
  compiling mutants. **Adds Finding 7 to the record.**
  - **⭐⭐ THE FINDING: THE TOOL DECLARED FOUR OUTPUT FIELDS AND RENDERED ONE — AND ONE OF THE FOUR WAS
    UNDELIVERABLE.** `GMAIL_READ_OUTPUT` promised `message_id`, `thread_id`, `label_ids` and `snippet`, while
    the renderer emitted only `message_id` from `parse_single_id`, so **three of four declared fields were
    unreachable**. The existing schema test did not catch it because **a validity check is one-directional**: it
    proves the rendering is inside the schema (`{"message_id":"m1"}` satisfies `required: ["message_id"]`) and
    never that the schema's fields are inside the rendering. This is `ADR-0082`'s defect in the *output*
    direction — a declaration nothing produces.
  - **⭐ AND `snippet` COULD NOT BE DELIVERED EVEN IN PRINCIPLE.** The `Format` enum page defines each value:
    `minimal` is "only email message ID and labels" and `metadata` is "only email message ID, labels, and email
    headers" — neither returns `snippet`. The tool offers all three formats, so a declared `snippet` would be
    absent for two of them. **The fix for that is to stop declaring it, not to make it optional**: for a field
    two of three formats never return, "optional" reads as unreliable rather than impossible, and a model cannot
    tell which it is.
  - **The reverse direction is now a test, and it is the one that finds this class of defect.**
    `no_declared_output_property_is_undeliverable` compares the declared property **names** to the keys of a
    maximal rendering, so a field added to the schema and forgotten in the renderer fails — and it needs no
    maintenance when a field is added, because it fails until the renderer catches up. Written as name-set
    equality rather than a required-field check, since the latter passes for an optional field never emitted,
    which is exactly the `snippet` case.
  - **`parse_single_id` becomes `parse_single_message` returning `GmailMessage`** — three fields, the ones the
    output declares, **not** Gmail's nine-field `Message` resource. Same rule `IdPage` and `CalendarPage`
    follow, so the declaration and the rendering cannot drift.
  - **⭐ AN ABSENT LIST IS NOT AN EMPTY LIST.** `label_ids` is `Option<Vec<String>>` and the key is emitted
    **only when the provider returned it**: `None` = "the field was not in the response", `Some([])` = "the
    message has no labels". A `Vec` with `#[serde(default)]` collapses both to `[]` and renders "no labels" for
    a response that never mentioned labels — the two-values-three-situations defect. `thread_id` and `label_ids`
    are optional in the schema for the same reason: a `minimal` read may not carry them, so a `required` field
    would make an honest response fail the tool's own validation.
  - **NEW LIMITS:** no request has been sent, so what `minimal`/`metadata` return is taken from the Format page
    and not observed; if `minimal` returns more than "ID and labels" the optionality is looser than reality,
    which fails safe but is a gap the live smoke test would close. `GmailMessage` models three fields of the
    resource's nine; a future read wanting `historyId` or `internalDate` must add them to the type and the
    schema together. The reverse test is written for `gmail_messages_read` (where the defect was) and not yet
    swept across every operation.
- [ ] `P5-005` **(continued — an argument pair the provider forbids)**: `calendar_events_read` now refuses
  `sync_token` with a time bound, in the builder and in the input schema. **3 new tests (so 359 in the crate).**
  **`ADR-0084`.** Two guards falsified A-B-A with compiling mutants. **Adds Finding 8 to the record.**
  - **⭐⭐ THE FINDING: THE INPUT SCHEMA ADVERTISED A COMBINATION THE PROVIDER DOCUMENTS AS A `400`.**
    `calendar_events_read` offers `time_min`, `time_max` and `sync_token`, and the builder sent all three
    together. The `events.list` reference says, under `syncToken`: *"There are several query parameters that
    cannot be specified together with nextSyncToken … These are: `iCalUID` `orderBy` `privateExtendedProperty`
    `q` `sharedExtendedProperty` **`timeMin`** **`timeMax`** `updatedMin`."* The sync guide gives the reason (a
    time range belongs to the **initial full sync**, and each incremental sync repeats the initial filters) and
    the consequence ("The response code for list queries containing disallowed restrictions is `400`").
  - **⭐ THE LIST HAS EIGHT ENTRIES AND THE CONNECTOR OFFERS TWO — the count is the reason to record it.**
    `timeMin`/`timeMax` are the two it can send; the other **six** (`iCalUID`, `orderBy`, `q`,
    `privateExtendedProperty`, `sharedExtendedProperty`, `updatedMin`) are not offered by the operation at all,
    so they need no check. A future slice adding `q` or `orderBy` joins the disallowed set and must be checked
    against the note — which is why the whole list is written down rather than just the two that bite today.
  - **A `DisallowedCombination` variant, not `Argument`.** Both values can be individually valid and the
    *pair* is the fault; a caller told "`time_min` is unusable" would remove the legitimate part of a full sync.
    The two checks are separate so the message names **which** bound conflicts, and it states both remedies
    (drop the bounds to continue the sync, or drop the token for a filtered full read).
  - **The schema carries it twice, on purpose.** The property descriptions state the restriction, because the
    schema is what a **model** reads and a model that saw all three advertised could reasonably choose the
    combination; an `allOf`/`not` constraint enforces it for a validator. Both are present because they serve
    different readers, and the test asserts rejection by the **document** — so removing the constraint while
    leaving the prose fails. A description a validator does not enforce is the "documented but not applied"
    defect (`ADR-0077`).
  - **The control is a filtered FULL sync**, which the sync guide's own sample performs ("we are only syncing
    events up to a year old"). Without it a schema that rejected every `time_min` would pass.
  - **Why refuse locally rather than let Google answer.** `ADR-0082` already classifies a `400` as
    `Permanent`/`DoNotRetry`, so the outcome is identical — except the round trip, the quota unit, and the fact
    that the `400` still does not tell the caller which argument to drop.
  - **NEW LIMITS:** the restriction is taken from the reference and **not observed**, so if Google accepts the
    pairing the connector refuses a call that would have worked (the safe direction — the guide's sample never
    combines them). Nothing outside this module consumes `DisallowedCombination`; `request_for` is the only
    caller and the tests hold the behaviour in place. The refusal lives in one builder, so a second
    `events.list`-shaped operation would need its own check against the same note.
- [ ] `P5-005` **(continued — a rendered token needs an input that can consume it)**: `calendar_events_read`
  gains `page_token`, so the `next_page_token` it renders can actually be used. **3 new tests (so 362 in the
  crate).** **`ADR-0085`.** Two guards falsified A-B-A with compiling mutants. **Adds Finding 9 to the record.**
  - **⭐⭐ THE FINDING: THE TOOL EMITTED A PAGE TOKEN AND HAD NO ARGUMENT TO FETCH THE PAGE WITH.**
    `calendar_events_read` declared `next_page_token` in its output, the renderer produced it, a test asserted
    it — and the **input declared no `page_token`** and the builder never sent `pageToken`. So the caller was
    told "there is a next page" with no way to fetch it. Both Gmail list operations pair `next_page_token` out
    with `page_token` in; Calendar was asymmetric.
  - **⭐ AND THE MISSING PAGE IS THE COMMON CASE.** The sync guide: *"you may find a `pageToken` **instead of** a
    `syncToken` … you'll need to perform the exact same list query … (with the exact same `syncToken`), append
    the `pageToken` to it and paginate through all the following requests until you find another `syncToken` on
    the last page."* A sync of a busy calendar returns a page token instead of a cursor, so the walk could not
    finish — **the cursor the whole sync mechanism exists to advance was unreachable exactly when it mattered.**
  - **⭐⭐ THE SAME PROVIDER FACT POINTS THE OPPOSITE WAY FROM `ADR-0084`.** `pageToken` is **not** on the
    disallowed-with-`syncToken` list; the guide **requires** it *with* a sync token (its example is
    `…&syncToken=…&pageToken=…`). So the eight-parameter restriction must be applied **per parameter** and never
    by shape: a conflict check added "for symmetry" with the time-range one would refuse the documented walk.
  - **⭐ THE CHECK IS A PAIRING, NOT A PER-TOOL LITERAL.** Each schema was internally consistent, which is why
    nothing caught this — only the **pairing** of one tool's input and output halves exposes a renderer that
    emits a token no argument can consume. `every_output_that_can_return_a_page_token_accepts_one_as_input`
    walks **every** definition, so a future paginated read cannot ship one-directional. `ADR-0083`'s method
    (assert the reverse direction) applied across a single tool's two halves.
  - **The description says why the two tokens ride together**, not just "continues a paginated read": the guide
    requires the *exact same* query, so a model must repeat the sync token with the page token.
  - **NEW LIMITS:** the walk is **possible but nothing performs it** — no code loops on `next_page_token`, so a
    caller issues the follow-up call itself. The connector implements the *ability* to paginate rather than an
    automatic walk, which is `P5-010`'s "pagination" item. No request has been sent, so the large-change-set
    shape is from the guide and not observed.
- [ ] `P5-005` **(continued — a refusal names the argument the caller sent)**: `search_query` takes the field
  it validates, `time_min`/`time_max` get their own `time_bound` validator and a declared bound, and every
  declared input bound is asserted equal to its constant. **3 new tests (so 365 in the crate).** **`ADR-0086`.**
  Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: A SHARED VALIDATOR HARD-CODED THE FIELD NAME IT REPORTED.** `search_query` always said
    `field: "query"`, but it had **three** callers — the Gmail list's `q`, and Calendar's `time_min` and
    `time_max`. So an oversized `time_min` on `calendar_events_read` was refused with *"the `query` argument is
    unusable: a search query may be at most 512 characters"*, **naming an argument the tool does not have** and
    calling an RFC 3339 instant a search query. A caller acting on that message hunts for a `query` parameter
    that does not exist. This is `ADR-0084`'s "remove the wrong argument" failure at a different layer: there a
    *combination* was reported against one field, here one field was reported as *another*.
  - **⭐ THE FIX IS A NEW VALIDATOR, NOT A RENAMED FIELD.** Passing the right field name through would still
    leave the *reason* wrong ("a search query" for a timestamp). `time_bound(field, value)` names its own noun
    and its own bound, because the fault's **kind** belongs in the message and not only the field it is about —
    the same distinction `DisallowedCombination` makes against `Argument`.
  - **⭐ `MAX_TIME_BOUND_CHARS = 64`, NOT THE QUERY'S 512.** An RFC 3339 instant is ~25 characters and at most
    ~30 with fractional seconds and an offset, so 64 admits every valid instant and still refuses a string that
    is plainly not a timestamp. The 512 was **the query's figure applied to a value of a different kind** —
    `ADR-0079`'s "a figure read as the wrong thing". Google publishes no length limit for `timeMin`/`timeMax`,
    so 64 is a **JARVIS** bound and the constant's doc says so.
  - **⭐⭐ THE SECOND DEFECT: THE DECLARED BOUNDS AND THE ENFORCED BOUNDS AGREED ONLY BY HAND.** The input
    schemas declare `maxLength`/`minimum`/`maximum`; `request.rs` enforces the same limits through constants;
    **nothing tied them**, and `time_min`/`time_max` had no declared bound at all. Now
    `every_declared_input_bound_matches_the_constant_that_enforces_it` reads each bound out of the schema by
    JSON pointer and compares it to the constant — 13 bounds plus 3 `minLength`s. The comparison is against the
    **constants**, so it cannot be satisfied by editing the schema alone, and it is the third instance this
    phase of one shape: two statements that must agree with nothing between them (`ADR-0083` output fields,
    `ADR-0085` token halves, this one numbers).
  - **NEW LIMITS:** `MAX_TIME_BOUND_CHARS` is a JARVIS bound, not a provider figure (no published limit). The
    drift test asserts **equality**, so it proves the two statements agree, not that either number is right — a
    bound changed in **both** places would pass with no provider evidence, which is why each constant carries
    its own justification rather than being asserted as "the value". The property table is explicit rather than
    a walk over every `maxLength`, so a new schema needs its row added.
- [ ] `P5-005` **(continued — a lease that lapses silently)**: new `google::watch` module — the watch lease's
  `expiration`, whether it is alive, and when to renew. **9 new tests (so 374 in the crate).** **`ADR-0087`.**
  Two guards falsified A-B-A with compiling mutants. **Adds Finding 10 to the record.**
  - **⭐⭐ THE FINDING: THE MANIFEST LINKED THE SEVEN-DAY BOUND AND NOTHING COULD APPLY IT.** The Webhooks doc
    link's purpose names "the seven-day renewal bound", but the crate had **no code that read a watch response
    or could decide whether a watch was still alive** — the "documented but not applied" defect (`ADR-0077`) on
    the one bound where it matters most, because **a Gmail watch fails silently**. The guide: *"You must call the
    `watch` method at least once every 7 days or you'll stop receiving updates for the user."* Nothing is raised
    and no notification announces that notifications stopped, so a lapsed watch is **indistinguishable from a
    quiet mailbox**.
  - **⭐⭐ `expiration` IS EPOCH MILLISECONDS IN A JSON STRING — two traps that raise nothing.** The reference:
    `"expiration": string (int64 format)`, *"epoch millis"*. A parser reading a JSON number refuses a
    **conforming** response; a parser scaling millis as seconds puts the watch's death **a thousand times too
    far in the future** — which is a valid instant, so nothing fails and the watch is silently dead. The unit is
    therefore pinned by a test against the reference's own value (`"1431990098200"` → 1,431,990,098 **seconds**
    *and* a rendered May-2015 instant), not by inspection.
  - **⭐ THE BOUND AND THE RECOMMENDATION ARE TWO FIGURES, ONE SENTENCE.** "At least once every 7 days" is when a
    watch *dies*; "We recommend calling `watch` once per day" is when to *renew*. `RenewalAdvice` is
    `Overdue | Recommended | NotYet` — ordered by **urgency**, which is why it is an enum and not a duration:
    a caller wants "renew now / soon / leave it", and a number would make it re-derive both thresholds here.
    Same split `ADR-0080` records for a rate limit and a recommendation.
  - **⭐ THE BOUNDARY IS DECIDED IN NANOSECONDS.** A lease ends at an *instant*, so comparing truncated seconds
    would call a watch with half a second left alive (keeping a dead watch) or lapsed (renewing early). The
    whole-second field is the magnitude computed **after** the direction, so `Lapsed { 0 }` is the exact expiry
    instant — **which counts as lapsed**, because the reference says the watch stops *at* that time and the safe
    direction is to treat the boundary as dead.
  - **`WatchLapse` is not a `bool`**: alive, just expired and long expired are three situations, and the elapsed
    time is what tells a caller whether this is a fresh problem or a mailbox unwatched for days (`ADR-0035`).
    **A renewal "in the future" is `NotYet`, not an error** — that is what a clock behind the renewal looks
    like, and the negative elapsed is carried rather than clamped so a caller can see the clock is ahead.
  - **FOUR unreadable shapes, four errors** (not JSON / absent-or-null / not a string / not an integer), because
    the remedies point at different layers. And a missing `expiration` is **refused**, not read as "never
    expires" — which would build a lease that never renews, the very failure being fixed.
  - **NEW LIMITS:** the module is **pure decisions with no caller** — nothing sends a `watch` request, so there
    is no response to parse and no scheduler to call `renewal_advice` (the same pipeline-side gap as
    `provider_request_id`, `batch_plan`, `calendar_signal`, `RetryDecision`). Neither the millis unit nor the
    inclusive boundary is **observed**; both are from the reference pages. `nanos_to_seconds` **truncates**, so a
    1.9-second difference reads as 1 — deliberate, since the direction is decided in nanoseconds first.
- [ ] `P5-005` **(continued — a field Google declares two encodings for)**: new `crate::base64` (extracted from
  `auth.rs`, decoder added) and `google::pubsub` (the notification envelope). **6 new tests (so 380 in the
  crate).** **`ADR-0088`.** Two guards falsified A-B-A with compiling mutants. **Adds Finding 11 to the
  record** and writes its "Pub/Sub envelope fixture" plan item.
  - **⭐⭐ THE FINDING: TWO OFFICIAL PAGES DECLARE `message.data`'s ENCODING DIFFERENTLY.** The Gmail push guide
    says it is *"a **Base64URL**-encoded string"*; the Cloud Pub/Sub `PubsubMessage` reference it links to types
    the field `string (bytes format)` and says *"A **base64**-encoded string"*. RFC 4648 §4/§5 differ in exactly
    two characters (`+`/`/` vs `-`/`_`), so **the disagreement is invisible on any value containing neither**.
  - **⭐⭐ AND ALMOST NO PAYLOAD CAN TELL THEM APART — which is why it is worth recording.** The guide's own
    example (`eyJlbWFpbEFkZHJlc3MiOiAidXNlckBleGFtcGxlLmNvbSIsICJoaXN0b3J5SWQiOiAiMTIzNDU2Nzg5MCJ9`) uses only
    `A-Za-z0-9` and decodes identically both ways — verified to yield exactly what the guide says. A sweep of all
    **95 printable ASCII** characters at all **four** base64 alignments inside a Gmail-shaped payload found only
    **three** (`>`, `?`, `~`, each after a one-character offset) that force the difference. So the readings agree
    on nearly every payload and diverge on the one that matters — where choosing wrongly **refuses a delivery**,
    which is a **missed change**.
  - **The decoder accepts both, tries URL-safe first, and says which matched.** URL-safe first because the guide
    is the more specific statement about *this* payload; both because the contradiction is the provider's and
    neither reading is provably wrong. `PubsubData` is returned rather than discarded so a real delivery can
    **settle the question from evidence** instead of the code resolving it silently in the provider's favour.
  - **⭐ THE FORCING FIXTURE IS SPELLED OUT, NOT GENERATED.** A second encoder would be a second thing that can be
    wrong, and the standard spelling is derived from the URL-safe one by RFC 4648 §5's substitution — with a test
    asserting the substitution actually changed the text, since a no-op substitution would test the same
    characters twice. Two earlier versions of the test failed here and each taught something: a hand-picked
    payload that happened to contain no `+`/`/` (so the test proved nothing), and an assertion that an unpadded
    value decoded (a fixture that did not contain the padding under test).
  - **Padding refused, not stripped**; **empty `data` refused as a shape**, not read as "nothing changed" (the
    silent direction); **four distinct errors** because the remedies point at different layers.
  - **Base64 now has one home.** The encoder moved from `auth.rs` to `crate::base64` **unchanged**, so the RFC 7636
    Appendix A test reaching it through `PkceVerifier` still guards the same lines. A `standard_padded` encoder
    was written and then **removed as unused** — a dead second implementation checks nothing.
  - **NEW LIMITS:** no delivery has been received, so which encoding a live subscription uses is **unknown** —
    exactly what reporting `PubsubData` leaves open. The envelope is decoded **only** as far as `message.data`;
    `messageId`, `publishTime`, `subscription` and the attributes map are not read, so there is no dedupe key and
    no ordering yet. `crate::base64` has no padded standard decoder, so a caller needing one must add it with a
    caller rather than as a spare function.
- [ ] `P5-005` **(continued — a rule from another context)**: padding is now **observed** rather than requested;
  the delivery envelope is parsed, with at-least-once dedupe and the five acknowledging statuses. **4 new tests
  (so 384 in the crate).** **`ADR-0089`.** Two guards falsified A-B-A with compiling mutants. **Adds Finding 12,
  which corrects Finding 11.**
  - **⭐⭐ THE FINDING: A RULE FROM ANOTHER CONTEXT WAS APPLIED TO A VALUE IT DOES NOT GOVERN.** `ADR-0088`
    refused `=` padding, justified by *"RFC 7636 requires padding omitted"* — an **OAuth PKCE** rule about the
    code verifier. But `message.data` is a **Cloud Pub/Sub** field, and the push page's **own minimum-value
    example** is `SGVsbG8gQ2xvdWQgUHViL1N1YiEgSGVyZSBpcyBteSBtZXNzYWdlIQ==` — **padded**, decoding to
    `Hello Cloud Pub/Sub! Here is my message!`. So the connector would have **refused Google's own published
    example**, which on a notification path is a **missed change**.
  - **⭐⭐ THE FIX REVEALS AN ASYMMETRY WORTH KEEPING: OBSERVABLE vs UNOBSERVABLE FACTS.** The alphabet needs to be
    a *parameter* because it is **unobservable** (the two differ in two characters, so a value containing neither
    decodes identically either way). The padding must **not** be a parameter because it is **observable** — it is
    the `=` at the end. Trying both paddings was *searching for something visible*, and it **cannot work**: a
    value whose unpadded length is already a multiple of four is valid either way, so the padded attempt is
    indistinguishable and can fail on a value that is legal. `decode_with(input, alphabet)` takes one axis;
    `Padding::of` reads the other.
  - **⭐ TWO DOCUMENTED FORMS ARE PREDICATES, NOT VARIANTS.** `PubsubData { alphabet, padding }` with
    `GMAIL_GUIDE = {UrlSafe, Absent}` and `PUBSUB_FIELD_TYPE = {Standard, Present}` as associated constants — a
    four-variant enum would make the documents into cases when they are points in one space.
  - **⭐ THE ENVELOPE, AND WHY `messageId` MATTERS.** Delivery is **at-least-once**: "A non-success response
    indicates that Pub/Sub must resend the messages", and a negative ack or an expired deadline resends. So
    `messageId` — "Guaranteed to be unique within the topic" — is the deduplication key, and **both spellings**
    (`messageId`/`message_id`, `publishTime`/`publish_time`) must be read, because the page's own examples show
    both and a `#[serde(default)]` field matching neither is `None` **with no error**.
  - **⭐ FIVE STATUSES ACKNOWLEDGE, NOT "2xx"**: `102`, `200`, `201`, `202`, `204`. A `203` or `206` is a success
    by HTTP's classification and a **negative acknowledgement** here, so a handler returning
    two-hundred-and-something would silently request redelivery of everything. `acknowledges_delivery` **fails
    closed**. Also recorded: **unwrapped** delivery (`payload-unwrapping`) has no `data` field, so it is refused
    as a shape rather than read as "no change".
  - **⭐ A FIELD IN THE WRONG PLACE IS SILENT.** The first attempt put `messageId`/`publishTime` at the **top
    level**; the provider puts them inside `message` (only `deliveryAttempt` and `subscription` are beside it).
    Every lookup returned `None` — silently, because these fields are `#[serde(default)]`. The test caught it by
    asserting `Some(...)` for the provider's own example rather than merely that the body parses.
  - **⭐ THE FIRST MUTANT SURVIVED, AND THAT WAS THE USEFUL PART.** Dropping the standard alphabet still passed
    the padded-example test, because `PUBSUB_PAGE_EXAMPLE` uses only `A-Za-z0-9` and decodes under **either**
    alphabet. The regression test now **forces the alphabet** as well, so a mutant cannot hide behind the
    provider's convenient example — the same lesson as Finding 11's sweep, applied to the test rather than the
    decoder. A dead-code smell was removed too: `decode_with` computed the padding and discarded it, so the first
    `Padding::of` mutant changed nothing.
  - **NEW LIMITS:** no delivery has been received, so which alphabet and padding a live subscription uses is
    **unknown** (which is what reporting `PubsubData` leaves open). `attributes` and `orderingKey` are not
    modelled — nothing in the Gmail path filters on an attribute and order is opt-in. `publish_time` is carried
    as **text**, not parsed, because nothing reads it and a parsed type would invite ordering logic the provider
    does not guarantee. Push **backoff** (100 ms–60 s, global, triggered by negative acks) is recorded in the
    research record but **not implemented**, since it is the provider's behaviour rather than a caller decision.
- [ ] `P5-005` **(continued — a refusal keeps the cursor)**: a refused cursor advance no longer discards the
  position. **2 new tests and 1 rewritten (so 386 in the crate).** **`ADR-0090`.** Two guards falsified A-B-A
  with compiling mutants, the second caught by **three** tests including a pre-existing one.
  - **⭐⭐ THE FINDING: A TRANSIENT FAILURE DISCARDED A WORKING CURSOR, SO A `429` COST THE WHOLE SYNC.**
    Both `advance_gmail_history` and `advance_calendar_sync` returned `cursor: None` for
    `SyncSignal::Refused(decision)` — a shape **copied from the `CursorUnusable` arm above them**, where it is
    right (the provider rejected the position) and here is wrong (the *request* failed and said nothing about the
    position). So a `429` on an incremental sync left the caller with no cursor: for Gmail a full resync of the
    mailbox, for Calendar a **full wipe of the store**.
  - **⭐⭐ AND IT IS THE EXACT MISTAKE `ADR-0067` WARNS AGAINST, COMMITTED IN THE ARM THAT HANDLES IT.**
    ADR-0067: *"a resync on a transient failure discards a working store, which is the opposite mistake and a
    much more expensive one."* That sentence is about the `404` heuristic; the `Refused` arm did worse, on the
    retryable family the sentence names. **The project's own table row contradicted the code**: it says
    `Refused` should "carry the classification; **never** a resync" — and dropping the cursor is what caused one.
  - **⭐ A TEST PINNED IT, WITH A COMMENT THAT WAS SOUND ABOUT THE WRONG SUBJECT.** The old test asserted
    `cursor.is_none()` justifying it as *"so a caller cannot store a new position on the strength of a failure"*.
    Correct reasoning, wrong referent: the previous cursor is **not a new position** — it is the caller's
    existing one, unchanged, and returning it unchanged satisfies the concern exactly. The test was
    **rewritten** with the old comment preserved so a reader sees why it changed.
  - **⭐ A REFUSAL AND AN UNCHANGED MAILBOX NOW GIVE THE SAME ANSWER, FOR ONE REASON.** `Refused` and
    `Advanced { None }` both return `Some(previous.clone())` because in both cases **nothing was learned about
    the position**. The three arms are now distinguishable and each is named: `CursorUnusable` → **none** (the
    position is dead), `Refused` → **the previous one** (the request failed), `Advanced { None }` → **the
    previous one** (nothing changed).
  - **⭐ A `429` HERE IS ROUTINE, NOT AN EDGE CASE.** `ADR-0089` established that push delivery is at-least-once
    and the push page documents backoff triggered by negative acknowledgements — so the arm that discarded the
    cursor sits on the common path of the feature this connector exists for.
  - **NEW LIMITS:** nothing consumes `CursorOutcome` outside this crate, so the benefit is proved by tests rather
    than observed in a deployment (the recurring pipeline-side gap). `Refused` still carries `RetryGuidance`
    inside it but **nothing reads the delay**, so a caller must schedule the retry itself.
- [ ] `P5-005` **(continued — a value redacted in one place and printed in another)**: four types stop printing
  what three other types already redact. **4 new tests (so 390 in the crate).** **`ADR-0091`.** Three guards
  falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE CRATE HAD A REDACTION CONVENTION AND LATER TYPES OPTED OUT OF IT BY DEFAULT.**
    `AccessToken`, `FormRequest` and `VerifiedAccount` each hand-write `Debug` to print `[REDACTED]`/`<redacted>`,
    each with a test asserting the marker — and `PubsubNotification` (a **mailbox address**),
    `PubsubMessageBody` (base64 whose bytes **decode to** that address), `SyncCursor` (the **token**) and
    `SyncCursorParts` (the same token) all **derived** `Debug` and printed their sensitive field.
    `#[derive(Debug)]` is the default and hiding a field is a deliberate act, so nothing made the omission
    visible.
  - **⭐⭐ AND THE POLICY WAS ALREADY WRITTEN DOWN IN ANOTHER MODULE.** `DiagnosticField::CursorObservedAt`:
    *"the token itself is never a field, because a cursor is provider-issued text that can address another
    account's data."* The rule existed; the type printed it under `{:?}` anyway, which is how a token reaches a
    log line. **A rule recorded in one module and violated in another is not a rule** — the fix is a test per
    sensitive type, not a note.
  - **⭐⭐ FIXING THE THREE KNOWN TYPES WAS NOT ENOUGH — AUDIT BY SHAPE, NOT BY THE OFFENDER LIST.**
    `SyncCursorParts` (the parts struct `P3-006a` introduced, with the token **moved into it** by
    `From<SyncCursor>`) is not `SyncCursor`, so it was not covered by the redaction one screen up — and it is
    reached by the crate's own advertised idiom, `let parts: SyncCursorParts = cursor.into();`, which the
    existing test uses verbatim. **A sibling type is not covered by its sibling's redaction**, and it was found
    by grepping for `Debug`-deriving structs with a token-shaped field — a query naming none of the three.
  - **⭐ THE SUBTLEST ONE LOOKED SAFE.** `PubsubMessageBody::data` is base64 — an opaque-looking blob carrying no
    credential — so a reader could reasonably print it. Its bytes decode to `{"emailAddress": …}`, so the
    disclosure is exactly the one the decoded form is redacted for. **Opacity is not safety**, and the *name* of
    a field is no guide to what it carries.
  - **⭐ THE REDACTION IS ASYMMETRIC ON PURPOSE.** `PubsubNotification` hides the address and **keeps
    `history_id`**, because a position names nobody and is what an operator debugging a stuck sync needs.
    `PubsubMessageBody` hides `data` and prints its **length**, matching `AccessToken`'s `chars: N`. A `Debug`
    that redacted everything would pass "the value is absent" and make every diagnostic useless — `A10` requires
    diagnostics to *"remain useful"* as well as redacted, which is why every test here has a **control** asserting
    the non-sensitive fields are still printed.
  - **⭐ `None` PRINTS AS `None`, NOT AS A REDACTION.** `SyncCursor::new` refuses a token on a `Start` cursor, so
    "this kind carries no token" is a fact worth seeing; a marker would make a start cursor look like a redacted
    one — the "two situations, one rendering" defect in miniature.
  - **⭐ THE DERIVED `Debug` THAT STAYS HAS A COMMENT SAYING WHY.** `PubsubDelivery`'s derive is safe **because
    its only sensitive field is inside the redacted `PubsubMessageBody`** — so the comment records the property
    to re-check ("does every field it holds refuse to print one"), not the conclusion, because it changes when a
    neighbour changes.
  - **NEW LIMITS:** this is a **convention with tests, not a mechanism** — nothing stops the next struct deriving
    `Debug` while holding a sensitive field, and the remedy is one test per type rather than a lint (a lint
    cannot tell which fields are sensitive). And `SyncCursor`/`SyncCursorParts` now print `kind`/`account`/
    `version`/`instant`, which is deliberate: the account is an `AccountReference`, JARVIS's own local identifier
    rather than the provider's id, so it names nothing about the mailbox.
- [ ] `P5-005` **(continued — a response field with no reader)**: a `users.watch` response carries **two** facts;
  only the lease was read. **3 new tests (so 393 in the crate).** **`ADR-0092`.** Two guards falsified A-B-A with
  compiling mutants, one of them the original defect re-introduced.
  - **⭐⭐ THE FINDING: THE WATCH RESPONSE'S `historyId` — THE ANCHOR A FIRST SYNC STARTS FROM — HAD NO READER.**
    The reference gives the response as `{ "historyId": string, "expiration": string (int64 format) }`; the module
    read `expiration` (with its two traps) and read past `historyId` entirely. The push guide says what it is for:
    *"The response contains the current mailbox `historyId` … Your client receives notifications for all changes
    **after** that `historyId`."* So a caller using the crate's one watch reader got a **lease with no anchor** —
    the first sync after a `watch` had nowhere documented to start, and there was no function to call and no
    error to handle. **A reader named for one field is not a reader for its response**: the name
    `parse_watch_expiration` is accurate, so nothing looked mislabelled — the gap was in the *shape of the API*.
  - **⭐⭐ AND THE GUIDE'S WORKED EXAMPLE USES TWO DIFFERENT NUMBERS, WHICH IS WHAT MAKES THE CONFUSION
    FALSIFIABLE.** *"Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can persist
    `9876543210` as the last known `historyId`"* — the response's id at one end, the post-sync position at the
    other, both strings, both spelled `historyId`. **A one-value example cannot falsify a conflation of two
    same-typed fields: every reading of it type-checks.** The test asserts the anchor is `1234567890` **and**
    `assert_ne!`s it against `9876543210`. Worth re-reading a guide for its **example** rather than only its
    field list — that is where the second number was.
  - **⭐ THE FAILURE MODE WOULD NOT TERMINATE.** Reading the response's id as "the new position" produces a
    plausible `historyId` and satisfies every assertion written against a single-number fixture — and an anchor
    does **not move when the mailbox does**, so a sync that stored it as the position would re-read the same
    window on every run. `advance_gmail_history` already computes the real successor, which is why the anchor is
    named `anchor` and not `history_id`: the API's own name is ambiguous between the two ends.
  - **⭐ THE STRUCT IS THE FIX, NOT THE SECOND READER.** `WatchResponse { anchor, expires_at }` with
    `parse_watch_response` as the reader to use; the two field readers stay public because each has traps worth
    testing in isolation. A caller holding a `WatchResponse` **has read both fields**, because the only way to
    build one is the function that reads both — so the omission cannot recur without changing the type.
  - **⭐ TWO NEW ERRORS, AND THE ORDER OF THE TWO READS IS A DECISION.** `MissingHistoryId`/`HistoryIdNotAString`
    are separate from the expiration's, because the remedies differ (an absent anchor makes the first sync
    unanchorable; an absent expiration makes the lease unreadable). `parse_watch_response` reads `expiration`
    **first**: a caller that cannot tell when the lease ends cannot use the anchor either.
  - **⭐ A MISSING ANCHOR IS REFUSED, NOT DEFAULTED.** Treating "no anchor" as "sync from the beginning" takes
    the most expensive path exactly when the provider failed to supply the cheap one — the fail-open direction.
  - **NEW LIMITS:** **no `users.watch` request is built**, so the anchor's consumer is still a future sync loop —
    this makes the anchor *available and correct*, not read in production. The anchor is also unvalidated as a
    cursor: `SyncCursor::new` applies the empty/oversized/control-character bound, and nothing here calls it, so
    an unusable `historyId` is caught at the cursor rather than at the response.
- [ ] `P5-005` **(continued — a request the provider accepts and ignores)**: the `users.watch` **request** — the
  crate's first body-bearing operation. **4 new tests (so 397 in the crate).** **`ADR-0093`.** Two guards
  falsified A-B-A with compiling mutants; a third issue was found by a **failing test**, not by review.
  - **⭐⭐ THE FINDING: THE DANGEROUS ARGUMENT IS THE ONE THE PROVIDER ACCEPTS.** `POST …/users/me/watch` carries
    a JSON body whose `labelFilterAction` the reference calls *"deprecated because it caused incorrect behavior
    in some cases"* and says is *"ignored"* when `labelFilterBehavior` is set — so sending the stale spelling is
    a **`200`**, not a `4xx`. And `labelFilterBehavior` is the *"filtering behavior of `labelIds` list
    specified"*, so sent with **no** list it governs nothing: the provider registers an **unfiltered** watch
    rather than refusing. A connector would then receive **every** change while believing it scoped the set —
    valid notifications, a healthy-looking lease (`historyId` + `expiration`), and only the *set* wrong. **A
    `400` is self-reporting; a `200` that ignored what you sent is not.**
  - **⭐⭐ THE REMEDY FOR A DEPRECATED SPELLING IS UNREACHABILITY, NOT DISFAVOUR.** `LabelFilterBehavior` is a
    two-variant enum producing exactly one field name, and `gmail_watch` has **no parameter** that reaches
    `labelFilterAction` — so it cannot be sent by accident, by a duplicated call, or by a "send both to be
    safe" habit a bare `Option<String>` would invite. **An absent parameter cannot be passed; a refused value
    can later be widened — and the absence is asserted** (the tests parse the body and check the field is
    absent, in the unfiltered *and* the filtered case, where sending it would look most plausible).
  - **⭐⭐ A CHECK AND A NORMALIZATION ON THE SAME VALUE MUST BE ORDERED DELIBERATELY — AND THIS WAS FOUND BY A
    FAILING TEST.** `watch_json_body` did `trim()` and **then** `is_control()` on the result, so a trailing
    `\n` was **deleted before the check looked for it** and `"projects/p/topics/t\n"` was silently accepted and
    sent as the clean string. **The check that exists to catch a newline must run before the operation that
    deletes one.** Trim is right for the emptiness/length checks and wrong as the input to the control check;
    the fix moved the check to the **raw** value, and the test was left as it was.
  - **⭐ A NEW ERROR VARIANT, BECAUSE THE FAILURE IS NEW.** `RequestError::Ignored` is distinct from `Argument`
    and `DisallowedCombination`: both of those describe requests the provider **rejects**, and this one it
    accepts. The message names the argument and the remedy (send labels, or omit the filter).
  - **⭐ AN EMPTY LIST IS REFUSED, NOT READ AS "NO FILTER".** After the emptiness check both render the same
    body, so accepting the empty one would equate a **loop-over-zero-labels** mistake with a deliberate choice.
    "No filter" is expressed by *omitting* the argument — the one rendering that cannot be produced by accident.
  - **⭐ A SECOND REQUEST TYPE BECAUSE THE CREDENTIAL BOUNDARY IS THE AXIS, NOT THE BODY.** `WatchRequest` holds
    a URL and a rendered JSON body and **no header map** (so no field could hold a bearer token — the `ADR-0060`
    property). It is **not** `FormRequest`: that type's whole justification is the credential its body carries,
    which is why its `Debug` redacts. A watch body holds a topic name in the caller's own project and Gmail's
    label vocabulary, so redacting it would make every watch diagnostic useless while protecting nothing.
  - **NEW LIMITS:** **no request is sent**, so whether Google accepts this body rests on the live smoke test that
    does not exist; the topic name's **shape** is not validated — the reference requires
    `projects/{project}/topics/{topic}` whose project *"must exactly match your Google developer project id"*,
    and checking the second needs a project id the connector does not hold; and a **host discrepancy** between
    two official sources (`gmail.googleapis.com` in the method reference vs `www.googleapis.com` in this
    record's Verified Contract) is **recorded and deliberately not resolved**, because moving a base on one
    page's example rendering is churn.
- [ ] `P5-005` **(continued — a negative acknowledgement is charged to the subscription)**: the push handler's
  **answer** becomes a decision with a cost. **4 new tests (so 401 in the crate).** **`ADR-0094`.** Two guards
  falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE ANSWER TO ONE DELIVERY IS PAID FOR BY EVERY DELIVERY ON THE SUBSCRIPTION.** The push
    page: *"Push backoff applies to all the messages in a subscription (global)"*, *"Push backoff can't be
    turned on or off"*, range **100 ms – 60 s**, *"calculated based on the number of negative acknowledgments"*.
    So `acknowledges_delivery → bool` read as "false: this one retries" while `false` also means **"and nothing
    else is delivered for up to a minute"**. A handler refusing a message it will *never* accept does not retry
    a message — it **slows every other mailbox on the subscription**, indefinitely, because the retry count is
    the subscription's policy and a push subscriber *"can't modify the acknowledgment deadline of individual
    messages"*. **Ask not "does this code acknowledge" but "who pays for this code".**
  - **⭐⭐ TWO TRIGGERS, AND ONE IS NOT A RESPONSE AT ALL.** An **expired acknowledgment deadline** triggers the
    same backoff — so a *slow* handler is indistinguishable, to the backoff, from a refusing one. That is why
    the decision type is about the delivery's fate rather than about a status code: the cost can be incurred
    without answering at all.
  - **⭐ `DeliveryAck` IS THREE ANSWERS, NOT A `bool`.** `Accept` / `Retry` / `AbandonAndAcknowledge` — exactly
    **one** of which refuses, which the test counts so a merge or a flipped arm fails loudly. A `u16` would let
    a caller re-derive `acknowledges_delivery` at the call site and hide the *reason* behind an integer.
  - **⭐ ABANDONING IS A NAMED CHOICE WITH ITS DOWNSIDE WRITTEN DOWN.** Acknowledge-and-record-the-drop loses
    the message — real cost — which is why it is a variant with the reasoning attached and not a default a
    caller falls into. `security.md`'s "fails closed" does not decide it: both answers are closed against
    *acting*, and the question is only whether the cost lands on this message or on the subscription.
  - **⭐ THE BOUND IS ON `deliveryAttempt`, AND `0` MEANS "NOT REPORTED" — NOT "EXHAUSTED".** No per-message
    deadline is readable, so the provider's own incremented count is the only per-message fact. Absent is not
    first, and **the direction is chosen**: `0` still gets a retry, keeping a possibly-new delivery alive,
    where the opposite reading would abandon a first delivery that merely arrived without an optional field.
    The push page's **minimum-value example omits `deliveryAttempt`**, so this is a shape that occurs.
  - **⭐ `MAX_RETRY_ATTEMPTS = 3` IS A JARVIS FIGURE AND SAYS SO.** The page publishes the backoff range and its
    global scope but **no retry count**, so this is this platform's policy — small and stated, because
    refusing without a bound is not a policy but the absence of one, paid for by every other mailbox.
  - **⭐ THE MIRROR OF `ADR-0093`, AND WORTH PAIRING.** There, an argument the provider **accepts and ignores**
    was dangerous because nothing reports it. Here, an answer the provider **honours** has a cost the sender
    cannot see — the same "real, untraceable from the call site" failure, from opposite directions.
  - **NEW LIMITS:** **nothing sends a response** — `DeliveryAck`/`decide_acknowledgement` are the *decision*, and
    the handler that maps a decision to a status code does not exist, so `acknowledges()` says *whether* to
    acknowledge and not *which* of the five codes to send (deliberate: they are interchangeable, and choosing
    one belongs with the response object); and **no dead-letter mechanism** is configured, so
    `AbandonAndAcknowledge` has nowhere to *send* the dropped delivery beyond recording it.
- [ ] `P5-005` **(continued — stopping notifications needs the grant that revoking destroys)**: account teardown
  is **two** operations with a forced order. **5 new tests (so 406 in the crate).** **`ADR-0095`.** Two guards
  falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE REVOKE SUCCEEDS AND THE STOP THAT FOLLOWS IS THE CALL THAT FAILS — ONE STEP LATER,
    INVISIBLY.** `users.stop` is an ordinary authenticated API call (the reference lists the **same four
    scopes** `users.watch` needs), while revocation **removes exactly those scopes** (the identity page: it
    *"removes all OAuth 2.0 scopes previously granted to a project"*). So **stop then revoke** works and
    **revoke then stop** does not — and the reversed order fails at the *second* call, where a reader is
    looking at a different operation. **Same shape as `ADR-0094` (report and cost in different places), in a
    sequence rather than one call.**
  - **⭐⭐ AND THE DAMAGE IS A SILENT PRIVACY EXPOSURE, NOT A FAILED CALL.** `stop`'s *"All new notifications
    should stop within a few minutes"* only applies if the stop **happened**. With the watch still registered
    and the grant gone, nothing ends the stream but **the lease lapsing** — `WATCH_RENEWAL_BOUND_SECONDS`, i.e.
    **up to seven days** of the mailbox address arriving at an endpoint the user believes is disconnected, with
    **no credential left to turn it off**. The leaked value is exactly what `PubsubNotification` redacts
    (`ADR-0091`).
  - **⭐ THE RULE IS A PAIRING FUNCTION, NOT A SEQUENCE.** `may_precede(first, second)` refuses exactly one
    pairing — a step that `withdraws_access()` before one that `needs_a_live_grant()` — and permits the rest
    (including a step before itself), so a **third** step (Calendar `channels.stop`, a subscription deletion)
    is checked by the same rule rather than by a reader remembering a comment. `TEARDOWN_PLAN` is the safe
    order and a test asserts **the rule and the plan agree**.
  - **⭐ THE ASYMMETRY THAT MAKES THE MISTAKE EASY:** `needs_a_live_grant()` is `true` for `StopWatch` and
    **`false` for `RevokeGrant`** — revocation accepts an already-dead token (RFC 7009 §2.2's `200` covers
    "the client submitted an invalid token"). So a caller that revoked first sees its revoke **succeed** and
    gets **no signal** that it has just made the next step impossible.
  - **⭐ THE TWO HALVES CARRY DIFFERENT FAILURE POLICIES, AND THE ASYMMETRY IS ARGUED.** `StopWatch` is
    `BestEffort` (a failed stop costs a bounded privacy window; aborting would leave a **working credential**
    because a *notification preference* could not be changed — the larger harm to avoid the smaller);
    `RevokeGrant` is `Required` (a failed revoke means the account is **not** disconnected, so the caller is
    told).
  - **⭐ `notification_exposure` IS AN ENUM BECAUSE ONLY ONE ANSWER IS A FIGURE THIS CRATE MAY STATE.** With the
    stop accepted the provider says *"within a few minutes"* — **no number**, so inventing seconds would
    fabricate a provider rule. Without the stop the exposure is the lease's bound, which Google **does** state,
    and it is **reused from `watch`** rather than restated so the two cannot drift.
  - **⭐ A GENERALISATION: WHEN TWO TEARDOWN STEPS EXIST, ASK WHICH ONE THE OTHER DISABLES.** And narrower: *a
    cleanup you cannot retry after you remove access must happen before you remove access* — the window between
    them is the exposure, and its length is a provider figure the crate already holds.
  - **NEW LIMITS:** **no request is sent and nothing calls this plan** — the teardown executor does not exist
    (as the push handler and sync loop do not), so the ordering is a **value with tests, not a mechanism**: a
    caller that ignored `TEARDOWN_PLAN` and revoked first would get no refusal from this module, only a plan to
    read; and **deleting the Cloud Pub/Sub subscription** is not modelled — it is shared by every watched
    account, so silencing one mailbox that way would stop notifications for all of them (a candidate
    **connector-level** third step).
- [ ] `P5-005` **(continued — a requirement with no consumer)**: the account-identity requirement had no
  operation that returned it. **5 new tests (so 411 in the crate).** **`ADR-0096`.** Two guards falsified A-B-A
  with compiling mutants.
  - **⭐⭐ THE FINDING: `tools-and-connectors.md` REQUIRED PROVIDER-VERIFIED IDENTITY AND NOTHING RETURNED IT.**
    The requirement is "account identity verified from the provider, not user-entered labels", `P5-004`'s
    mapping names `users.getProfile`'s `emailAddress` as the value — and the connector declared **no operation
    that reads a profile**, only four that return mail and calendar *content*, none of which says **which
    mailbox answered**. So `VerifiedAccount::new`'s required `provider_account_id` had **no producer** and the
    requirement was **unimplementable, not merely unimplemented**. The gap was invisible because it was an
    **absence**: no wrong field, no failing test, just a requirement nothing could satisfy.
  - **⭐⭐ AND A COMMENT DISCHARGED THE WORK THE CODE HAD NOT DONE.** `SCOPE_OPENID`'s doc said *"the
    `users.getProfile` response carries an `emailAddress`, and that is the operation **this scope exists for**"*
    — while the `users.getProfile` reference accepts `mail.google.com/`, `gmail.modify`, `gmail.compose`,
    `gmail.readonly`, `gmail.metadata` and **NOT `openid`**. A reader following the comment concludes the
    identity path needs nothing more. **A plausible statement substituted for a working link** — the same shape
    as `ADR-0092` (a reader named for one field while its sibling went unread), one level up.
  - **⭐⭐ GENERALISATION: A REQUIREMENT'S EVIDENCE IS THE OPERATION THAT RETURNS IT, NOT THE DOCUMENTATION
    THAT NAMES IT.** "Operation X satisfies requirement Y" is a claim to verify against the API's own **scope
    and response tables** — here it was false about the *scope* as well as absent about the *operation*.
  - **⭐ THE SCOPE IS CORRECTED, NOT DELETED.** `openid` *is* requested and *does* have an effect — it makes
    Google return an `id_token`, which the exchange receives and deliberately does not verify — so removing the
    constant would remove a real declaration to hide a false explanation. The doc now says `getProfile` does not
    accept it, that the Gmail read scope is what makes the profile readable, and that the ID-token check is
    **unbuilt: a prepared seam, not a working feature** (the `nonce` `P5-002` carries is what a future check
    would compare).
  - **⭐ NAMING ANOTHER MAILBOX IS UNREPRESENTABLE, NOT DISCOURAGED.** `gmail_profile()` takes **no argument**
    — it hardcodes `me`, because a `user_id` field would let a caller aim at a mailbox its own token cannot
    address, refused as a `403` rather than as an error naming the argument. The input schema is
    `"properties": {}` with `additionalProperties: false`, so an invented field is refused rather than dropped.
  - **⭐ THE ADDRESS IS REQUIRED AND THE COUNTS ARE NOT DECLARED.** `parse_profile` refuses a response with no
    usable address — including a **whitespace-only** one, which satisfies "the field was present" while
    denoting nothing — because the operation exists to establish *which* mailbox answered. `messagesTotal` and
    `threadsTotal` are deliberately **absent** from both the output schema and the renderer: they are mailbox
    counts nothing reads, and declaring them is `ADR-0083`'s defect. A test asserts the output contains
    **neither**, so the omission is checked in both directions.
  - **⭐ TWO NEW TESTS ASSERT THE CORRECTION, NOT JUST THE FIX.** One asserts the profile operation carries no
    `openid` JARVIS scope and that the granted scopes contain both the Gmail read (which makes the profile
    answer) and `openid` (for the id token), with `assert_ne!` on the two strings — the divergence-assertion
    shape the revocation module uses. The other asserts the **declaration** (exists, `mail.read`, `ReadOnly`,
    cost `Documented(1)`) so a future edit removing it fails with the reason it must not be removed.
  - **NEW LIMITS:** **no request is sent and nothing calls the operation** — the connect-time identity flow
    (call `gmail_profile_read`, build `VerifiedAccount`, store `AccountReference`) does not exist, so the
    requirement is now *satisfiable* rather than *satisfied in production*; and the `id_token` remains
    **received and unverified** — this closes the *identity* gap, **not** the *token-verification* one, and the
    ADR says so rather than conflating them.
- [ ] `P5-005` **(continued — a delivery names a mailbox and nothing mapped it to an account)**: the push
  path's missing join, as a pure function. **7 new tests (so 418 in the crate).** **`ADR-0097`.** Two guards
  falsified A-B-A with compiling mutants (the first caught by **four** tests).
  - **⭐⭐ THE FINDING: EVERY PIECE OF THE PUSH PATH EXISTED AND THE JOIN DID NOT.** A delivery could be
    **decoded** (`parse_delivery`), its lease **read** (`watch::parse_watch_response`), its answer **decided**
    (`pubsub::decide_acknowledgement`), a stale cursor **classified** (`advance_gmail_history`) — and nothing
    connected the payload's `emailAddress` to one of the connector's accounts. So a delivery said "a mailbox
    changed" with no way to learn **which of yours**, and the sync it triggers needs one account's stored
    credential. **An unattributable notification is an unactionable one.**
  - **⭐⭐ AND THE VALUE THAT MUST BE JOINED ON IS UNTRUSTED.** Finding 1: neither Google mechanism fits
    `WebhookSupport::Push` (OIDC bearer JWT; echoed channel token over a zero-length body), so the connector
    **cannot authenticate a delivery at all** — the address is a string from whoever posts to the endpoint. Two
    facts bound what a forged delivery can do, and both are load-bearing: the route selects **a mailbox to
    read, never a credential to use** (the sync uses that account's own token, so it reaches only mailboxes
    already authorised), and the notified `historyId` is **not a position the sync trusts** (`history.list`
    runs from the **stored** cursor, so a too-high forged id cannot cause a **missed** change). **An identifier
    arriving over an unauthenticated channel may SELECT but must not AUTHORISE.**
  - **⭐ BYTE-EXACT, AND THAT IS SECURITY RATHER THAN STRICTNESS.** Six near-misses are asserted, each of which
    defeats a looser rule: a prefix (`starts_with`), a superstring (`contains`), a suffix-domain, a different
    local part, and leading/trailing whitespace (trimming). The control asserts the exact value **does** route.
  - **⭐ A CASE-ONLY NEAR-MATCH IS A STATE, NOT A MATCH.** Addresses are case-insensitive in practice, so
    `Person@example.invalid` is *probably* the same mailbox — but Google publishes **no canonicalisation rule**
    for `emailAddress` in a push payload, and if the two spellings were two accounts then applying the route
    reads the wrong mailbox. `DeliveryRoute::CaseDiffers { accounts }` reports it and a person decides.
  - **⭐ FOUR VARIANTS, NOT AN `Option`.** `Exact(AccountReference)` / `Ambiguous { accounts }` /
    `CaseDiffers { accounts }` / `Unknown` — an `Option` has two states and the decision has four, and a reader
    of `None` could not tell "not my account" from "my account, spelled differently". **`Exact` is the only
    variant carrying a reference**; `Ambiguous` and `CaseDiffers` carry **counts**, so no accessor can return an
    arbitrarily chosen account (picking first/oldest/most-recent would sync one mailbox under another's
    identity).
  - **⭐ EVERY UNROUTABLE DELIVERY IS ACKNOWLEDGED, AND `Retry` IS UNREACHABLE HERE.** None of the three is
    repaired by another attempt — the account set is a **local** fact — and `ADR-0094` makes that decisive: a
    negative acknowledgement triggers a **subscription-global** backoff of up to 60 seconds, so refusing would
    slow **every other mailbox on the subscription** for a message that can never become routable.
    `unroutable_acknowledgement()` is `AbandonAndAcknowledge` for all three and `None` for `Exact` (the latter
    because a routable delivery **can** be processed, not that it **was**).
  - **⭐ THE ARGUMENT IS `&[VerifiedAccount]`, NOT ADDRESSES.** The address and the reference must belong to the
    **same** account; a `&[(AccountReference, String)]` would let a caller pair one account's reference with
    another's address and route to the wrong mailbox with nothing able to notice. `VerifiedAccount` is the type
    that already binds the two, so it is the argument.
  - **NEW LIMITS:** **no delivery has been received and no account connected**, so routing runs on types the
    crate owns rather than observed data; and the **authentication gap is untouched** — this makes a forged
    delivery's consequence **small and bounded**, it does **not** make forging impossible (Unresolved
    Question 1, which needs a contract change or an OIDC/JWKS verifier `P5-001` kept out of a pure path
    deliberately).
- [ ] `P5-005` **(continued — one address, one account)**: the connect-time step that `ADR-0096` and `ADR-0097`
  both named as unbuilt, and where the router's `Ambiguous` is prevented. **6 new tests (so 424 in the crate).**
  **`ADR-0098`.** Two guards falsified A-B-A with compiling mutants; a **third** defect was found by a failing
  test rather than by review.
  - **⭐⭐ THE FINDING: THE TWO ENDS OF ONE STEP WERE MISSING, AND THE STATE THE ROUTER CANNOT ACT ON IS CREATED
    THERE.** `ADR-0096` produced an identity (`gmail_profile_read` → `emailAddress`) and named the connect flow
    as unbuilt; `ADR-0097`'s router consumes stored identities and named `Ambiguous` as caused by "a reconnect
    that mints a new reference without retiring the old row". Nothing turned one into the other, so **this step
    is where `Ambiguous` is prevented or created** — which is why its rule is a refusal, not a deduplication.
    A duplicate makes routing **undecidable** (every notification for that mailbox stops being acted on until a
    person resolves it) while being **invisible** (two cursors, two schedules, a quota budget paid twice look
    exactly like two mailboxes).
  - **⭐⭐ THE CASE COMPARISON RUNS THE OPPOSITE WAY, AND BOTH ARE THE SAME RESTRAINT.** `ADR-0097` refuses to
    **act** on a case-only near-match (no canonicalisation rule is published; acting could read the wrong
    mailbox). This refuses to **create** one, for the same uncertainty and because the directions differ in
    cost — refusing asks a person (recoverable), a duplicate is silent. **Neither acts on an uncertain
    case-match.** One test asserts **both halves against one pair of spellings**, so loosening either shows up
    as a contradiction rather than as policy drift.
  - **⭐⭐ A THIRD DEFECT, FOUND BY A FAILING TEST AND IT IS `ADR-0091`'s CLASS AGAIN: REDACTION APPLIED IN ONE
    PLACE IS NOT REDACTION APPLIED IN ANOTHER — AND THE SECOND PLACE IS USUALLY AN ERROR PATH.** The
    `IdentityUnusable` refusal first carried `error.to_string()`, propagating `ConnectorError::Identifier`'s
    `Display` — which is `"the connector identifier `{value}` is unusable: {reason}"`, **interpolating the
    value it rejected**. So an unstoreable address would have been printed in full by the refusal that says an
    identity could not be *stored* — while `VerifiedAccount`'s `Debug` **redacts that same value**. The test
    asserting the refusal does not contain the address caught it; the fix was to carry the error's
    **`&'static str` `reason`** instead of its rendering, making the leak **unrepresentable** (a `&'static str`
    has nowhere to put a runtime value). **The move is structural, not a rule about not printing.**
  - **⭐ A SEPARATE REFUSAL VARIANT, BECAUSE THE SUBJECT DIFFERS.** `IdentityUnusable` ≠
    `AddressAlreadyConnected`: one is about the account set, one about the identity, and the remedies differ
    (retire an account vs. look at the provider response). Reporting the second as the first sends a person
    hunting for a duplicate that does not exist. `holder()` returns `Option` — `None` here — rather than
    fabricating a holder to make the signature uniform.
  - **⭐ THE COUNTERFACTUAL IS DEMONSTRATED, NOT DESCRIBED.** The duplicate test constructs the duplicate
    **directly** and asserts the same notification becomes `Ambiguous`, so the consequence the refusal prevents
    is shown through the router rather than claimed in prose.
  - **⭐ NO DISPLAY NAME, BECAUSE THE PROVIDER SENDS NONE.** `users.getProfile` returns `emailAddress`,
    `messagesTotal`, `threadsTotal`, `historyId` — passing the address as a display name would invent a
    provider statement, which is the field `VerifiedAccount` exists to keep honest.
  - **⭐ `resume_from` CARRIES THE CURSOR'S KIND WITH ITS POSITION.** The kinds differ in exactly the way this
    matters: a `MonotonicMarker` has detectable staleness and a defined recovery; an `OpaqueToken` may not be
    validated at all and only the provider's refusal is authoritative — so a caller need not remember which API
    it holds. A `Start` cursor is `FullSync` rather than an error (`SyncCursorKind::Start`: a full sync "is a
    decision with consequences … an absent value would make it the default a caller stumbles into"), and its
    **two documented causes are deliberately not separated**, because the cursor cannot support the
    discrimination and the caller that discarded the position already knows why (`ADR-0067`'s restraint).
  - **NEW LIMITS:** **nothing calls `establish_account` or `resume_from`** — no account store, no executor, no
    sync loop — so both are decisions with tests rather than enforced behaviour (the "convention with tests,
    not a mechanism" limit `ADR-0091`/`ADR-0095` carry); the duplicate check is a **scan of a slice** rather
    than a store uniqueness constraint; and the two causes of `FullSync` stay unseparated by design.
- [ ] `P5-005` **(continued — a delivery can be authenticated without covering the body)**: the webhook contract
  gains the two authenticators Google's push mechanisms use — `SignatureAlgorithm::OidcIdToken` (an RS256 JWT in
  the `Authorization` header, Gmail via Cloud Pub/Sub) and `EchoedChannelToken` (a client-set string echoed in
  `X-Goog-Channel-Token`, Calendar channels) — plus `covers_the_body`/`is_body_independent` and a refusal of a
  signature encoding for a body-independent authenticator. **2 new tests (so 426 in the crate).** **`ADR-0099`.**
  This **resolves Unresolved Question 1** of the Google record, which two adjacent slices had named as
  *blocking*. Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: ONE AXIS WAS BEING USED TO ANSWER TWO QUESTIONS, AND THE UNASKED ONE WAS THE PROVIDER'S.**
    `ADR-0054`'s contract had a single axis — *which MAC over the raw body* — and used it to answer a different
    question: *what authenticates this delivery*. A provider whose authentication is a **header token** (a
    bearer JWT, a shared string) therefore had **no representation at all**, and the finding was recorded for two
    rounds as "Gmail and Calendar push cannot be *expressed*". The finding's own name was the clue — it says
    "authenticated by an OIDC bearer JWT **or** an echoed channel token" — because **neither covers the body and
    both authenticate a delivery**. So the missing thing was not a value but an axis, and `covers_the_body` is
    it. **⭐ GENERALISATION: when a type answers two questions with one accessor, the unasked question is the one
    a new provider will need.** The variants are the consequence; the axis is the fix.
  - **⭐⭐ AND `ADR-0054`'s PROSE ALREADY KNEW THE AXIS, AND NAMED IT IN A SENTENCE WITH NO ACCESSOR.** Its doc
    for `is_keyed_mac` argues that "*a keyed MAC and an asymmetric signature both cover the body, but only the
    MAC requires the verifier to hold a secret*" — i.e. it states "*covers the body*" as a property in its own
    right, then exposes an accessor for only the second half. **A distinction stated in prose and absent from
    the code is the recurring defect of this phase (`0067`–`0098`)**, and this is the first instance where the
    prose was **correct and complete** and the code still could not answer the question: the sentence is a
    description, not an interface, and nothing forced it to be one.
  - **⭐ `covers_the_body` IS PROVABLY NOT `is_keyed_mac`, AND THE TWO DISAGREE IN BOTH DIRECTIONS.** Ed25519 is
    `is_keyed_mac == false` with `covers_the_body == true` (a public key still signs the bytes); an echoed
    channel token is `is_keyed_mac == false` with `covers_the_body == false` (a secret that covers no bytes).
    A single test asserts both cross-cases, so a later collapse of the two accessors fails rather than silently
    making one question stand for the other.
  - **⭐ `None` IS CORRECTED FROM "ANOTHER MECHANISM" TO "NO MECHANISM", AND THAT WAS THE TRAP.** Its doc had
    offered "*a bearer token in a header*" as an example of another way to authenticate that `None` made
    representable — but `None` means **nothing** authenticates the delivery, so the only honest way to represent
    a bearer-token authenticator was always a new variant. An author following the old doc would reach for
    `None` when the truth was "an authenticator I cannot name", producing a declaration that reads as *no
    control* — the webhook-spoof row with its control removed, in the one place the manifest refuses it.
  - **⭐ A BODY-INDEPENDENT AUTHENTICATOR MAY NOT CLAIM A SIGNATURE ENCODING (`SignatureError::Encoding`).**
    `encoding` describes how a **signature's** bytes are presented; a token presents an opaque header value, so
    there is nothing to hex-decode. `Raw` is the honest value and anything else is refused — `ADR-0057`'s rule
    (a field that cannot take an honest value for a variant is refused rather than defaulted) applied to a
    sibling field of the scheme. It is a distinct error from `Header` because the remedy differs: a bad name is
    a typo, an encoding on a token is a **misunderstanding of the mechanism**, and reporting it as a header
    problem sends a reviewer to the wrong half of the value.
  - **⭐ THE MANIFEST'S PUSH GUARD IS UNCHANGED AND STILL REFUSES ONLY `None`.** `!scheme.authenticates()`
    keeps its exact meaning ("no control is present"), and the acceptance test now round-trips **every**
    authenticator — including the two header tokens — so the guard is provably **not** a synonym for "not an
    HMAC". The falsification makes the point: mutating it to `!covers_the_body()` made a **truthful** OIDC push
    declaration refused, which is the over-broad-guard direction.
  - **⭐⭐ BUT THE CONNECTOR STILL DECLARES `Polling`, FOR A CORRECTED REASON, AND CORRECTING THE REASON WAS
    PART OF THE WORK.** Making the mechanisms expressible does **not** make Google's push declarable: one
    connector holds **one** `WebhookSupport` value while Google has **two** push mechanisms with different
    headers *and* different bindings, and **neither verifier is built** (no JWKS reader for the JWT, no stored
    value for the channel token). So flipping to `Push` would trade one incomplete declaration for another and
    would also demand webhook signature/replay readiness items nothing can satisfy. Three places of connector
    prose said the mechanisms "cannot be expressed" — now false — and each is corrected (`ADR-0074`/`ADR-0096`:
    a comment naming a module is a claim about code, and the code changed).
  - **⭐ A TEST'S PREMISE WAS CORRECTED RATHER THAN LEFT GREEN ON A STALE REASON.** `a_push_declaration_is_not_used_because_neither_google_mechanism_fits`
    asserted the *unexpressibility* that no longer holds. It is rewritten to **prove the mechanisms are
    expressible** (both schemes construct) and only then assert the manifest still declares `Polling` — so it
    fails for the reason that is still true rather than passing for one that is not.
  - **NEW LIMITS:** naming an authenticator is **not** verifying one — no JWKS fetching, certificate rotation,
    `aud`/`iss`/`exp` checking, or channel-token comparison exists, so the authentication gap `ADR-0097`
    recorded is **narrowed from "cannot be declared" to "is declared but unverified"** (Unresolved Question 9,
    which `P5-010` owns). `WebhookSupport` remains **one mechanism per connector**, which is why the two
    mechanisms cannot both be declared. And nothing *sends* or *receives* a delivery, so every rule here is a
    decision about values the crate owns.
- [ ] `P5-005` **(continued — a delivery is not always a change)**: new `google::channel` module — the
  **Calendar notification-channel push message**, read from `X-Goog-*` **headers** because the delivery has a
  **zero-length body**. `ResourceState { Sync, Exists, NotExists }`, `ChannelMessage` (with `is_sync()` and a
  redacted `channel_token()`), `parse_channel_message`, and `ChannelMessageError`. Two fixtures
  (`calendar_channel_message.json`, `calendar_channel_sync.json`) and 2 harness tests. **10 new lib tests (so
  436 in the crate).** **`ADR-0100`.** This writes the research record's own *"Calendar sync-message fixture"*
  item, which was open. Three guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: A PUSH CONSUMER MUST DISTINGUISH "A MESSAGE ARRIVED" FROM "A RESOURCE CHANGED", AND BOTH
    GOOGLE MECHANISMS MAKE THOSE DIFFER ON THE FIRST MESSAGE.** Calendar sends a **`sync`** message when a
    channel is created — *"to indicate that notifications are starting"*, and *"It's safe to ignore"* — so the
    first delivery is a **handshake, not a change**. Gmail says the same in its own words (`ADR-0092`: a
    successful `watch` *"immediately sends a notification, so the first delivery is not a change"*). A consumer
    that equated the two would do one spurious read the moment it began watching, on **both** providers.
  - **⭐⭐ AND `ADR-0099` HAD JUST NAMED A CONTROL WHOSE INPUT NOTHING COULD PARSE.** That round taught the
    webhook contract to *name* an echoed channel token as an authenticator — over a header that **no reader
    looked at**, because `google::pubsub` models the Gmail envelope and **nothing modelled the Calendar
    mechanism at all**. That is `ADR-0092`'s "a value with no reader" inverted: the *contract* produced the
    requirement and no code consumed the wire form. This slice closes the **read** layer; the **compare** layer
    (the verifier) stays open and is named.
  - **⭐ AN UNKNOWN RESOURCE STATE IS REFUSED, BECAUSE NEITHER DEFAULT IS SAFE.** Treating an unrecognised
    `X-Goog-Resource-State` as `exists` (a change) acts on a message the connector does not understand; treating
    it as a non-change ignores a possible change. Refusing names the value instead of choosing a direction for
    the caller — and it is the mutant the test kills (`unwrap_or(Exists)`, the fail-open direction).
  - **⭐ DETECTION IS BY THE DECLARED DISCRIMINATOR, NOT THE ACCIDENTAL ONE.** `X-Goog-Message-Number` *"is
    always 1 for sync messages"* — but also *"not sequential"*, so `number == 1` classifies any early message as
    a handshake. `is_sync()` reads the **state**, and the fixtures deliberately pair a `sync` numbered `1` with
    an `exists` numbered `10`, so the number cannot be what is under test.
  - **⭐ TWO EXPIRATIONS WITH CONTRADICTORY ENCODINGS, READ BY DIFFERENT CODE.** `X-Goog-Channel-Expiration` is
    *"human-readable format"* (a date string) while the Gmail watch lease's `expiration` is an **epoch-millis
    string** (`ADR-0087`). Sharing a reader would force it to guess an encoding, so this module keeps the value
    **as text** and does not share one — the `ADR-0082` shape ("one classifier for two APIs") applied to a pair
    of values.
  - **⭐ THE ECHOED TOKEN IS SURFACED AND REDACTED, AND SURFACING IS NOT VERIFYING.** `channel_token()` returns
    it (a verifier needs the value), the field is **private**, and the hand-written `Debug` prints
    `[REDACTED], N chars` (`ADR-0091`) — because the token is the anti-spoofing control and a value in a log is
    a value an attacker could replay. Comparing it is `P5-010`'s work, and this crate does not hold the stored
    value the comparison needs.
  - **⭐ HEADER READS DISTINGUISH ABSENT, AMBIGUOUS, AND NON-UTF-8.** `single_header` collapses all three into
    `None`; this module separates them, because an absent header is the provider sending less than documented,
    an ambiguous one is a wire attack, and a non-UTF-8 one is an encoding fault. Ambiguity is **refused**, so
    the wrong value cannot win.
  - **⚠ A TEST-AUTHORING DEFECT FOUND BY A FAILING TEST: A HARDCODED LENGTH THAT DUPLICATED A COMPUTED VALUE.**
    The redaction test asserted `"34 chars"` (then `"25"`); the token `target=myApp-myChannelDest` is **26**
    characters, so it failed for a mistyped constant, not for the behaviour. Fixed by deriving the length from
    the token (`token.len()`). **A literal that restates a value the code already computes is a second source of
    truth, and it is wrong exactly when nothing else is.**
  - **NEW LIMITS:** **nothing receives a Calendar notification** — no delivery endpoint, no channel registration,
    and no channel-token comparison, so this reads a message from headers the crate owns rather than observed
    data. `WebhookSupport` still declares `Polling` for the connector (the cardinality reason `ADR-0099`
    records). And **`not_exists` semantics were not established by the page read** — the guide lists the value
    but does not define it for a caller — so treating it as actionable rests on the fail-safe direction rather
    than on a quoted rule.
- [ ] `P5-005` **(continued — a missing control and a failed one are not the same answer)**: the channel-token
  **verifier** — `verify_channel_token(stored, delivery)` returning a four-variant `ChannelTokenCheck`
  (`Verified`, `Absent`, `Mismatch`, `TokenRequired`) with `may_be_acted_on()`/`is_rejection()`, plus
  `MAX_CHANNEL_TOKEN_BYTES`. **4 new tests (so 440 in the crate).** **`ADR-0101`.** Completes the "surfaced,
  not verified" limit `ADR-0099`/`ADR-0100` each recorded, and **closes the constant-time-comparison half** of
  the research record's Unresolved Question 9. Two guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: A `bool` CANNOT SEPARATE "A MISSING CONTROL" FROM "A FAILED ONE", AND THE TWO HAVE
    OPPOSITE READINGS.** A Calendar delivery has a **zero-length body**, so the echoed channel token is the
    **only** control — and `matches(stored, delivery) -> bool` would read `false` for both *"the channel was
    registered without a token, so there is nothing to check"* (`Absent`) and *"the channel is protected and
    this delivery failed the check"* (`Mismatch`/`TokenRequired`). One is a documented configuration that must
    **not** alert; the other is a forged or misrouted delivery that must. A predicate that cannot tell them
    apart makes a caller either **page on a correct configuration** or **accept a delivery that failed its only
    control**. `ADR-0035`'s "a boolean standing for more than two situations is an enum", where the collapsed
    states have **opposite operational readings**.
  - **⭐⭐ AND THE ANSWER IS TWO PAIRS, ONLY ONE MEMBER OF EACH A REFUSAL.** `Verified`/`Absent` may be acted on;
    `Mismatch`/`TokenRequired` may not. **`Absent` is not a refusal** — the token is optional (*"Only present if
    defined"*), so a delivery without one for an un-tokened channel is the documented shape, and refusing it
    would fail closed on a correct configuration. **`TokenRequired` ≠ `Mismatch`**: a wrong value versus no
    value, and a diagnostic saying "mismatch" for a delivery carrying nothing sends an operator hunting a value
    that was never sent.
  - **⭐⭐ ⚠ A GUARD IN MY OWN FIRST DRAFT COULD DECIDE NOTHING, AND ITS TEST COULD NOT TELL.** The first version
    checked `presented.chars().count() > MAX` and returned `Mismatch` before comparing — but
    `SecretValue::matches` **already** refuses a different-length candidate immediately, so the guard changed
    **no input's answer** while itself being an **unbounded `O(n)` walk of attacker input**: it added exactly the
    cost it claimed to prevent. The test I wrote for it **passed under a mutation removing the guard**, which is
    how the redundancy went unnoticed. **`ADR-0066`'s family in a new form: not a guard that can never *fire*,
    but one that can never *decide*.** Fix: removed the guard; renamed the constant to `MAX_CHANNEL_TOKEN_BYTES`
    and made its doc say **stated, not enforced, and why** (so a later reader does not add the guard back); and
    rewrote the test to assert the **observable** property — an over-long candidate is a `Mismatch` *because its
    length differs*, and a value **at** the documented maximum **verifies**, so the absence of a hidden ceiling
    is demonstrable. **⭐ Before adding a check, ask which input it changes the answer for — if none, it is not a
    check but a cost.**
  - **⭐ THE COMPARISON IS EXACT AND CONSTANT-TIME, REUSING THE CRATE'S OWN ROUTINE.** `SecretValue::matches`
    (`ADR-0055`, the OAuth `state`'s comparison) rather than `==`, because the stored token **is** a secret an
    attacker learns one byte at a time, and a short-circuiting compare leaks its prefix. Five near-misses
    pinned — prefix, superstring, case variant, trailing space, empty — each a rule a looser comparison would
    accept; and a **different but well-formed** token is a `Mismatch`, so `Verified` is not a constant.
  - **NEW LIMITS:** **nothing calls `verify_channel_token`** — no delivery endpoint and no channel store, so it is
    a decision with tests rather than enforced behaviour. The `OidcIdToken` (Gmail Pub/Sub) verifier is **still
    unbuilt** (no JWKS reader, no `aud`/`iss`/`exp` check), so Unresolved Question 9 is **half closed** (the
    constant-time comparison) and half open (the JWT). `WebhookSupport` still declares `Polling` (cardinality).
    And the verifier checks **only the token** — that a delivery names a resource this channel watches is a
    separate control it does not perform.
- [ ] `P5-005` **(continued — two pushes routed on keys of opposite provenance)**: `google::channel` gains the
  routing join — `ChannelRegistration` (channel id + account + optional token), `ChannelRoute`
  (`Exact`/`Ambiguous`/`Unknown`), and `route_channel`. **4 new tests (so 444 in the crate).** **`ADR-0102`.**
  Completes the attribution join `ADR-0100`/`ADR-0101` left open, the Calendar counterpart of `ADR-0097`. Two
  guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE TWO GOOGLE PUSHES JOIN ON KEYS OF OPPOSITE PROVENANCE.** A Calendar delivery names the
    **channel** (`X-Goog-Channel-ID`) — a value the **connector chose** — while a Gmail delivery names the
    **`emailAddress`** — a value the **provider sent**. So the same "which of mine is this" question is a
    **lookup in the connector's own records** for Calendar and a **comparison against untrusted text** for
    Gmail. Both want **byte-exact** matching, for **opposite** reasons: an untrusted provider string may only be
    compared as sent, and a connector-generated id is compared against the bytes it stored. **⭐ A key's origin
    decides how strictly it may be compared — and a reader who generalised one mechanism's rule to the other
    would add either a spurious case-differs state or a dangerous case-fold.**
  - **⭐ ROUTING AND VERIFICATION ANSWER DIFFERENT QUESTIONS, AND BOTH MUST RUN.** A delivery can route `Exact`
    and still **fail** verification (a mismatched/absent token); a delivery for an **unregistered** channel is
    `Unknown` and cannot be verified **at all**, because there is no stored value to compare against. So the
    sequence is route → `verify_channel_token(registration.token(), message)`, and a test asserts routing
    **ignores** the token entirely, so the two controls are provably independent — fusing them would make an
    unroutable delivery unverifiable by construction and hide which control failed.
  - **⭐ THE BINDING IS ONE TYPE, SO A ROUTE CANNOT MISPAIR.** `route_channel` takes `&[ChannelRegistration]`
    (id + account + token bound together) rather than an `(id, account)` list, for the reason `route_delivery`
    takes `&[VerifiedAccount]`: separate lists let a caller pair one account's reference with another's channel
    and route to the wrong mailbox with nothing able to notice. `token()` reaches the stored value without the
    field being public, and the hand-written `Debug` redacts it (`ADR-0091`).
  - **⭐ `token: None` IS A RECORDED CHOICE, NOT A MISSING VALUE.** The guide makes the token optional, so a
    registration with no token is one the connector **chose** not to protect — read as `ChannelTokenCheck::Absent`
    (not a refusal), and an `Option<SecretValue>` with a sentinel would have made "I forgot the token" the same
    as "there is none".
  - **⭐ A REGISTRATION COLLISION IS `Ambiguous`, AND A COUNT IS NOT A PICK.** Reachable only if a channel id is
    reused — the guide *recommends* a UUID "so it is unique", a recommendation not an enforcement — and
    undecidable without a person. `ADR-0098`'s "one address, one account" argument applied to a channel: a
    duplicate makes the route unactionable and looks exactly like two legitimate channels.
  - **⚠ A COMPILE ERROR CAUGHT A DERIVE/IMPL CLASH BEFORE ANY TEST RAN** (`E0119`): `ChannelRegistration` derived
    `Debug` **and** hand-wrote it to redact the token. The **intent was the redaction**, so the fix was to drop
    the derive, not the hand-written impl — the compiler refused a type that would have printed the channel
    token. And a **test-helper defect** was fixed before it could pass for the wrong reason: the first
    `message_for_channel` built headers from a non-`'static` slice, so the helper leaks the id (test-only)
    rather than widening the fixture types. **The fixture layer is where a test stops testing the code.**
  - **NEW LIMITS:** **nothing calls `route_channel` and nothing registers a channel** — no `watch` executor, no
    channel store — so the route is a decision with tests rather than enforced behaviour, and a collision is only
    reachable if a caller hands in a duplicate slice (a store could enforce uniqueness, the revisit condition
    `ADR-0098` names). The Calendar push path is now end-to-end **on paper** (name → read → verify → route); what
    is missing is a handler that receives a delivery, and the Gmail `OidcIdToken` verifier (Unresolved Question
    9, still open).
- [ ] `P5-005` **(continued — composing the push path is what decides, and the seam changed a type)**:
  `ingest_channel_delivery(delivery, registrations) -> ChannelIngest` composes read → route → verify →
  classify; `ChannelIngest { Unreadable, Unroutable, Rejected, Handshake, Changed{account} }` with
  `is_accepted`/`account_to_sync`/`acknowledges`; and **`ChannelRoute::Exact` now carries the whole
  `ChannelRegistration`** (with a `registration()` accessor). **9 new lib tests + 1 harness test (so 453 in the
  crate, 18 fixture).** **`ADR-0103`.** Completes the Calendar push path the previous three slices built one
  piece at a time. Three guards falsified A-B-A with compiling mutants.
  - **⭐⭐ THE FINDING: THE SEAM IS WHERE A *TYPE* HAD TO CHANGE, AND NO PER-PIECE TEST COULD SHOW IT.** Read
    (`ADR-0100`), verify (`ADR-0101`) and route (`ADR-0102`) were each green alone and **nothing called them
    together** — `ADR-0069`'s "two tested halves do not test the seam", and `ADR-0098`'s "the join was the
    missing step", a **third** time. Composing them showed that `ChannelRoute::Exact` carried an
    `AccountReference` but **the token that proves the delivery lives on the registration** — so a caller would
    route, then scan the registrations **again** to get the token: two lookups deciding one match, and the
    account acted on and the token verified could come from two scans that merely happened to agree. `Exact` now
    carries the registration. **⭐ General rule: when a composition forces a value to be *fetched* rather than
    *carried*, the missing field is the finding — here, the control that proves what the account may act on.**
  - **⭐⭐ AND NO SINGLE ANSWER COULD EXPRESS THE OUTCOMES.** A `Result<Option<AccountReference>, _>` would encode
    *handshake* and *unroutable* as the same `None`, and those need **opposite** handling (accept-and-do-nothing
    vs drop); a `Result` also cannot separate a refusal from a retry. `ChannelIngest` has **five** variants for
    the three questions a push handler asks — *well-formed?* *really ours?* *a change?* — and only **one**
    (`Changed`) starts work. `is_accepted` and `account_to_sync` are **different questions**: a handshake is
    *accepted* (the sender keeps the message) but *syncs nothing*.
  - **⭐ ROUTING RUNS BEFORE VERIFICATION, SO AN UNREGISTERED CHANNEL IS `Unroutable`, NEVER `Rejected`.** The
    token cannot be checked without the registration that holds it, so for an unregistered channel **no
    comparison happened** — and `Rejected` would claim a control failed when none was present (`ADR-0101`'s
    "missing control ≠ failed control", at the composition). The order is also why the `sync` state is read
    **last**: an unauthenticated delivery must not steer whether work happens, even into the "no-op" branch.
  - **⭐ AN UN-TOKENED CHANNEL STILL SYNCS ON A REAL CHANGE.** `ChannelTokenCheck::Absent` **may be acted on**, so
    a correctly configured channel registered without a token is not silently dead — but its `sync` message is
    still a `Handshake`, so accepting a change does not accept everything.
  - **⭐ EVERY OUTCOME ACKNOWLEDGES.** None of the four non-`Changed` results is repaired by retrying, and a
    negative ack is **subscription-global** (`ADR-0094`), so refusing would slow every other channel for a
    message that can never become actionable. `acknowledges()` is a method so a future variant that *should* be
    retried has a place to say `false`. `Changed` carries only the **account**, not the registration — a sync
    needs the account, and handing the token onward puts the anti-spoofing control into a component with no use
    for it.
  - **NEW LIMITS:** **nothing receives the delivery** — no endpoint, no channel store — so this is a decision with
    tests, not a running handler. It does not record the drop, sync anything, or deduplicate a redelivery
    (`X-Goog-Message-Number` is read but unused, per `ADR-0100`). The Gmail `OidcIdToken` verifier is still
    unbuilt (Unresolved Question 9), so the Gmail path has no equivalent composition.
- [ ] `P5-005` **(continued — a cause is not a marker, and the two pushes do not share a state machine)**:
  `google::routing` gains `GmailIngest { Unreadable(GmailBodyError), Unroutable(DeliveryRoute),
  Changed{account, history_id, message_id} }`, `GmailBodyError { Envelope, Payload }`, `ingest_gmail_delivery`,
  and `acknowledgement()`. **5 new tests (so 458 in the crate).** **`ADR-0104`.** Composes the Gmail push path
  (`ADR-0103`'s counterpart) **and corrects a shipped claim** in `channel.rs`. Two guards falsified A-B-A.
  - **⭐⭐ THE FINDING: A *CAUSE* IS NOT A *MARKER*.** `ADR-0092`/`ADR-0100` recorded Gmail's rule as *"a
    successful `watch` immediately sends a notification, **so the first delivery is not a change**"*. Re-fetching
    the guide (footer **2026-09-15**) to build on it shows only the **first** clause is stated: the notification
    `watch` sends is an **ordinary** one — the same `{emailAddress, historyId}` payload a change produces, with
    **no state field**. Google **causes** an opening Gmail notification but **marks** the opening Calendar one
    (`X-Goog-Resource-State: sync`, *"safe to ignore"*). So *"X is not a Y"* was true of **what happens** and
    false of **what a consumer can detect**, and a state drawn from that confusion would be one the wire cannot
    produce. **⭐ Ask of any "X is not a Y" rule: a fact about what happens, or about what the message says
    happened?**
  - **⭐⭐ AND THE FALSE CLAIM HAD ALREADY SHIPPED — composition is what surfaced it.** `channel.rs`'s module doc
    said the Calendar `sync` rule was *"the Calendar counterpart of the Gmail rule"* — attributing to Gmail a
    detectable handshake it does not have (the `ADR-0074`/`ADR-0096` class, reached from a new direction: the
    wrong thing was a **comparison between two mechanisms**). Building the Gmail ingest forced the question
    *"which of `ChannelIngest`'s five variants apply here?"* — and the answer was *four do not, and one of the
    four is missing for a reason the doc got backwards*. Corrected in place and labelled as a correction.
  - **⭐ `GmailIngest` HAS THREE VARIANTS AND NO `Handshake`.** Copying `ChannelIngest` would have **invented a
    state**: with no marker, a `Handshake` variant is unreachable by any input (the "a variant nothing
    constructs" defect this phase keeps finding) *and* reachable-looking, so a caller would branch on it and
    believe it was skipping the opening notification. The outcome space is exactly read-failure,
    routing-failure, routed-change.
  - **⭐ `Unreadable` KEEPS THE LAYER.** `GmailBodyError::Envelope` (a broken Pub/Sub wrapper) vs `Payload` (a
    broken Gmail payload inside a good wrapper) — the two point at different layers, and a single "bad body"
    would send a caller to the wrong one. The failure is destructured back out of `PubsubDeliveryError::Payload`
    rather than flattened.
  - **⭐ `message_id` IS CARRIED AS AN `Option`, BECAUSE PUB/SUB IS AT-LEAST-ONCE.** It is the **only** field that
    tells a redelivery from a new change (`ADR-0094`); `None` means *"this may be a repeat I cannot detect"*,
    which is why it is not a defaulted string.
  - **⭐ `acknowledgement()`: `Accept` for a routed change, `AbandonAndAcknowledge` for both failures.** Neither
    failure is repaired by retrying (the payload is what it is; the account set is local) and a negative ack is
    subscription-global (`ADR-0094`). **No `Retry`** — this decides *what the delivery IS*, not whether acting on
    it succeeded, so the transient-failure answer belongs to the caller that acts.
  - **⭐ NO `Rejected` OUTCOME, AND THE REASON IS A NAMED GAP.** Gmail's delivery is authenticated by an OIDC
    bearer JWT (`ADR-0099`) and **that verifier is not built** (no JWKS reader, no `aud`/`iss`/`exp`), so there
    is no control to reject on — inventing the variant would claim a check that does not happen. Unresolved
    Question 9, still open.
  - **NEW LIMITS:** **nothing receives the delivery** (no endpoint), so this is a decision with tests; the JWT
    verification gap stands (the same trust limitation `ADR-0097` records — a forged delivery can select a
    mailbox to *read*, never a credential, and cannot cause a *missed* change); and the opening Gmail
    notification still costs **one** spurious sync per `watch`, which is the honest, documented cost of the
    absent marker rather than something this code can remove.
- [ ] `P5-005` / `P5-010` **(continued — the documented remedy is two steps, and only the second was a type)**:
  new `google::recovery` — `CallRecovery { Refresh, Reauth{reason}, NotCredential }`,
  `RefreshRecovery { RetryCall, RetryCallAndStore, RetryRefresh, Reauth{reason} }`, `TokenState
  { PossiblyStale, JustRefreshed }`, `recover_from_call`, `recover_from_refresh`. **8 new tests (so 466 in the
  crate).** **`ADR-0105`.** The first piece of the reauth lifecycle `P5-010` owns. Two guards falsified A-B-A.
  - **⭐⭐ THE FINDING: THE PROVIDER'S REMEDY IS TWO STEPS AND OUR VOCABULARY NAMED ONLY THE SECOND.** The errors
    guide, verbatim: *"To fix this error, **refresh the access token** … **If this fails, direct the user through
    the OAuth flow**."* The connector's `RetryGuidance::Reauthenticate` renders the **second** half as the whole
    of it — so read literally it sends a user through a consent screen as the **first** response to an **expired
    token**, exactly the failure a silent refresh repairs. The first step **had no type anywhere**, so the
    documented sequence was not expressible. **⭐ Generalisation: a remedy enum that names the *last* step of a
    documented sequence makes the earlier steps unrepresentable.** Ask of any remedy vocabulary: *does it cover
    the sequence the provider documents, or only its end?*
  - **⭐⭐ AND ONE ERROR CODE MEANS TWO CAUSES, WHICH ONLY THE PRESCRIBED *FIRST ACTION* SPLITS.** The same page:
    *"the access token … is either expired or invalid. **Missing authorization for the requested scopes can also
    cause this error.**"* So `authError` is an expired token (refresh fixes it) **or** a scope the grant never
    had (only re-consent does) — and the refusal cannot tell them apart. **A refresh can**, because it cannot
    grant a scope: a call refused with a *fresh* token is not a token problem at all. That is the guide's own
    *"if this fails"* branch from the other direction, and it is why the decision **takes `TokenState` as an
    input** rather than deciding from the refusal alone (the inference/decision split the cursor and pub/sub
    decisions already use).
  - **⭐ `TokenState` IS A TYPE, NOT A `bool`.** `PossiblyStale` (a refresh may help) vs `JustRefreshed` (a
    refresh was already tried **in response to this same failure** and the call was refused again). The
    distinction is a fact about the **attempt**, not about the token, and a `bool` named `refreshed` would read
    as the latter — which is how the second step gets taken first.
  - **⭐ `Rotated` CARRIES A STORE OBLIGATION `Refreshed` DOES NOT (`RetryCallAndStore` vs `RetryCall`).** A
    caller that treats a rotation as an ordinary refresh keeps using a refresh token the provider has already
    invalidated, and the **next** attempt then reads as a broken account rather than as a missed store.
    `RefreshOutcome::Rotated` was already its own variant; this is where it reaches a caller as an **action**.
  - **⭐ `Transient` RETRIES THE *REFRESH*, NOT THE CALL (`RetryRefresh`).** The one non-obvious direction:
    retrying the *call* with a token that was never obtained fails identically, while retrying the *refresh* is
    what a rate-limited token endpoint eventually honours.
  - **⭐ `JustRefreshed` NAMES `ScopeLoss`, NOT `ProviderRefused`.** With the token excluded by freshness, the
    guide's documented remaining cause is a missing scope; a persistent client-level problem is a
    *possibility* the refusal does not establish, so the reason named is the documented one rather than the
    graver-sounding one (`P3-008i`: do not assert a cause you cannot know). The code says the two are **not
    distinguishable here**.
  - **⭐ ONLY AN `Authentication` REFUSAL REACHES A REFRESH.** A `429`, a `403` and a `5xx` are `NotCredential`
    for **both** token states — refreshing a token that is not the cause spends the token endpoint's budget and
    changes nothing. `NotCredential` says *this module has an opinion and it is that the credential is not the
    problem*, which is different from "not considered".
  - **NEW LIMITS:** **nothing calls these functions and nothing performs a refresh** — no credential store and no
    refresh loop exist, so both are decisions with tests rather than enforced behaviour, and the
    `Rotated`→`RetryCallAndStore` obligation is only **expressed** (a store that could discharge it does not
    exist). `ProviderRefused` is **unreachable from `recover_from_call`** by design — only a caller that knows
    the client itself was refused can produce it, and this function cannot see that.
- [ ] `P5-005` **(continued — the same expiry in two encodings, and renewal is a replacement)**: new
  `google::channel` watch-lease half — `parse_channel_watch_response` (`ChannelWatchResponse { channel_id,
  resource_id, expires_at }`), `ChannelWatchError { NotJson, Missing, WrongType, OutOfRange }`, `ChannelLease
  { Lapsed, Alive }` + `channel_lease`, `ChannelRenewal { ReplaceNow, ReplaceSoon, NotYet }` +
  `renewal_decision`, and `CHANNEL_REPLACE_LEAD_SECONDS`. Plus the fixture
  `calendar_channel_watch_response.json` and a harness test that drives it. **7 new crate tests (so 470 in the
  crate; 1743 in the workspace).** **`ADR-0106`.** Two guards falsified A-B-A. The half of the channel path
  that happens *around* a channel rather than on a delivery.
  - **⭐⭐ THE FINDING: ONE QUANTITY, THREE ENCODINGS, AND THE FORM WE ALREADY HELD WAS THE UNUSABLE ONE.** A
    Calendar channel's expiry arrives as (a) the notification header `X-Goog-Channel-Expiration`, *"expressed in
    human-readable format"* (`Tue, 19 Nov 2013 01:13:52 GMT`) — which `ChannelMessage` already parsed and
    **could not** compare to a clock without a date parser, locale and timezone handling; (b) the `watch`
    **response body's** `expiration`, *"a Unix timestamp (in milliseconds)"*, typed `long` by the `events.watch`
    reference — which **was never read**; and (c) Gmail's `watch` response `expiration`, an epoch-millis
    **string**, per `ADR-0087`. So the connector held the expiry in the one form arithmetic cannot use, and the
    form it *can* use was the one nothing read. **⭐ Generalisation: when a quantity is available in several
    encodings, check which one the code already holds and whether it is the one the *decision* needs — "we have
    this value" is not "we have this value in a usable form".** Three encodings also means **three parsers**:
    sharing one would force the encoding to become a parameter, and the provider's own words would then live
    only at the call sites.
  - **⭐⭐ AND THE GUIDE CONTRADICTS ITS OWN REFERENCE ABOUT THE FIELD'S TYPE — in prose, with a table one click
    away.** The push guide's request section calls `expiration` *"An `expiration` **property string** set to a
    Unix timestamp (in milliseconds)"*; the `events.watch` reference the guide links to types it **`expiration |
    long`**. The reference wins (it is the schema for the method, and the guide uses the same "property string"
    phrasing for `params.ttl`, which the reference types `string`), so the parser reads a **number** and refuses
    a string **by name**. A parser that accepted both would have erased which document declares which form —
    and a *string* here is one of the other two encodings. **Same class as `ADR-0088`'s two-base64-alphabet
    contradiction, and the same resolution: prefer the specific statement, and make the choice visible.**
  - **⭐⭐ RENEWAL IS A *REPLACEMENT*, NOT A REFRESH — and reusing Gmail's vocabulary would have hidden it.**
    Verbatim from the guide: *"Currently, there's no automatic way to renew a notification channel. When a
    channel is close to its expiration, you must **replace it with a new one** by calling the `watch` method. As
    always, you must use a **unique value for the `id` property of the new channel**. Note that there's likely
    to be an **'overlap' period** of time when the two notification channels for the same resource are active."*
    Four facts, each of which changes a type: the channel is not extended (a *second* one is created), the new
    id must be unique, the old one **keeps delivering during an overlap** (duplicate deliveries for one resource
    are expected, not a bug), and **the overlap has no published duration**. `ADR-0087`'s `RenewalAdvice` takes
    a *last-renewed* instant because Gmail's rule is a **cadence**; Calendar has no cadence, only an **expiry**,
    so `renewal_decision(expires_at, now)` takes a different input and is a different function — the same split
    `ADR-0104` records for the two pushes, one layer down.
  - **⭐ `CHANNEL_REPLACE_LEAD_SECONDS` IS A JARVIS FIGURE AND SAYS SO.** Google publishes the overlap as
    *"likely"* with **no number**, so the margin for "close to its expiration" cannot be quoted from a page. It
    happens to equal `WATCH_RENEWAL_RECOMMENDED_SECONDS` and is **stated separately anyway**, because reusing
    the Gmail constant would tie two independently documented mechanisms together — a change to one provider's
    text would silently move the other's behaviour. `ADR-0080`: a figure with no source is neither a limit nor
    a recommendation.
  - **⭐ THE REFERENCE ALSO PUBLISHES THE SAME 7-DAY FIGURE FROM A SECOND MECHANISM.** *"params.ttl | string |
    The time-to-live in seconds for the notification channel. **Default is 604800 seconds.**"* Numerically
    identical to Gmail's `WATCH_RENEWAL_BOUND_SECONDS` (`604_800`), arrived at by a different mechanism and
    documented on a different page. Recorded because it makes the figure *look* like a shared constant when the
    two documents are separate — the same trap the two expirations set.
  - **⭐ `resourceId` IS READ BECAUSE A CONSUMER NEEDS IT; `token` IS NOT, BECAUSE ONE ALREADY EXISTS.**
    `channels.stop` *"requires that you provide at least the channel's `id` and the `resourceId` properties"*,
    so `resourceId` is a value with a **consumer** — and the response is the only place it appears *before* a
    notification arrives (the `sync` message carries it too, but the guide warns it can arrive *before* the
    `watch` response, so a stop built on `sync` alone has a race the response does not). The echoed `token`, by
    contrast, would be a second source for a value this connector chose; `kind` and `resourceUri` have no
    consumer. `ADR-0092`'s rule applied twice in one struct with opposite outcomes.
  - **⭐ THREE RENEWAL STATES, NOT TWO, BECAUSE ONE IS AN INCIDENT AND ONE IS A TASK.** A **lapsed** channel has
    already stopped delivering — notifications are being **lost** — while a **nearly-lapsed** one must be
    replaced *before* it does. Same action, different operator meaning, so collapsing them would hide whether a
    gap has already begun. The exact expiry instant counts as **lapsed**, in nanoseconds, for the same reasons
    `watch_lapse` does (`ADR-0035`, `ADR-0087`).
  - **⭐ TWO GUARDS FALSIFIED A-B-A, both compiling.** (1) `checked_mul(1_000_000)` → `checked_mul(1_000)` (ms
    read as µs) → **detected** by the test pinning Google's own `1426325213000` to `1426325213` seconds. (2) the
    renewal boundary `<= CHANNEL_REPLACE_LEAD_SECONDS` → `<` → **detected** by the test asserting a channel
    exactly one lead away is `ReplaceSoon`. **The scale factor is the one error here that does not announce
    itself**: `1426325213000` read as micro- or nanoseconds is a valid `UtcTimestamp` in the wrong century, and
    no type can catch a plausible unit — so the value is pinned to the provider's own example.
  - **⭐ A DEFECT FIXED IN THE RECORD ITSELF.** Finding 10 said Calendar's channel `expiration` is *"an RFC 3339
    date-time string"*. **No Calendar page says that** — it is epoch millis, and RFC 3339 is what Google
    **error** bodies' metadata and Pub/Sub's `publishTime` use. The claim was unquoted, plausible, and survived
    because nothing read the field. Corrected in place, with the reason: **a comparison drawn from memory reads
    exactly like one drawn from a page, and only the quoted form can be checked.**
  - **NEW LIMITS:** **no `watch` call is issued and no live channel has been read** — a hand-built fixture
    marked `_not_a_capture` stands in. `expiration` may come back **shorter than requested** (*"determined
    either by your request or by any Google Calendar API internal limits or defaults (the more restrictive value
    is used)"*), which no offline test can exercise. **The replacement itself is not built** — no second `watch`
    with a new unique `id`, so the documented overlap is never produced, and no deduplication key for the
    duplicate deliveries that overlap implies has been decided. Same shape as `ADR-0087`: the *decision* is
    tested and the *caller that acts* does not exist.
- [ ] `P5-005` **(continued — a teardown step whose arity the provider decides)**: `google::request` gains
  `calendar_channel_stop` (a body of exactly `id` + `resourceId`) and `WatchRequest` is **renamed `JsonRequest`**;
  `ChannelRegistration` gains `resource_id` (read by `ADR-0106` and previously discarded);
  `TeardownStep::StopCalendarChannel` joins the plan as a **third** step; `notification_exposure` splits into
  `gmail_exposure` + `calendar_exposure` and `NotificationExposure` gains `AlreadyEnded`. **5 new tests (so 475
  in the crate; 1748 in the workspace).** **`ADR-0107`.** Two guards falsified A-B-A, one mutation proved a
  **no-op** and corrected a doc claim. Completes the channel path at its far end.
  - **⭐⭐ THE FINDING: A VALUE READ FOR A CONSUMER THAT DID NOT EXIST.** `ADR-0106` read `resourceId` out of the
    `watch` response and said in its own doc that it is *"what the `channels.stop` call needs"* — and
    `ChannelRegistration` had **nowhere to put it**, so the value was dropped one function after it was obtained.
    That is `ADR-0092`'s "a response field with no reader" in a **stronger form: a field with a reader and no
    holder**, where the doc states the consumer and the type cannot carry the value to it. **⭐ A registration is
    the only value that survives between the `watch` and the teardown, so an identifier missing from it makes
    the channel unstoppable** — and the omission is discovered at teardown, the worst possible place.
  - **⭐⭐ AND ONE "THIRD STEP" IS NOT ONE STEP: THE PROVIDER DECIDES THE ARITY.** `teardown.rs` predicted a
    future third step *"is checked by the same rule"*, and `ADR-0095` asked whether `may_precede` *"generalises or
    needs a per-API argument"*. **It generalises** — the new step was admitted **unchanged and unedited**, because
    the rule tests *authority* (`withdraws_access`, `needs_a_live_grant`) rather than the operation. But the
    prediction missed the step's **shape**: `users.stop` ends **the** mailbox watch (one resource, one call, no
    arguments), while `channels.stop` ends **a** channel and has *no per-user form* — *"there's only one `stop`
    method"* — so an account watching three calendars needs **three** calls. One variant would have misreported
    whichever mechanism it skipped. **⭐ Generalisation: before reusing a step variant, check its ARITY — how many
    calls it stands for — because a step that is "one call per account" and one that is "one call per instance"
    are the same effect only until an account has two instances.**
  - **⭐⭐ AND THE EXPOSURE FIGURE CANNOT BE SHARED, for a reason about the *sources*.** `notification_exposure`
    returned Gmail's `WATCH_RENEWAL_BOUND_SECONDS` (7 days) — correct for Gmail *because Google publishes a bound
    for the mechanism*. Google publishes **no equivalent bound for a channel**: its life is *"determined either by
    your request or by any Google Calendar API internal limits or defaults"*. So the only honest figure is the
    expiry the channel's own `watch` response reported — **the two mechanisms are documented differently, so one
    function cannot compute both**, and a flag would drag an `Option<ChannelLease>` that is meaningless in one
    branch.
  - **⭐ `AlreadyEnded` REMOVES AN OVERSTATEMENT.** A channel whose lease had **already** lapsed exposes nothing,
    whether or not a stop was attempted — and it **outranks** the stop's success, because the answer no longer
    depends on the call. A `bool` function would have reported `UntilTheLeaseLapses { seconds: 0 }`, describing a
    clean teardown as an open window. Reachable only for Calendar, because Gmail's figure is a constant rather
    than a lease — a wiring gap the ADR records.
  - **⭐ THE STOP PERMISSION RULE IS DOCUMENTED AND LOCALLY UNENFORCEABLE, and the reason is worth stating.**
    *"If the channel was created by a regular user account, only the same user from the same client (as identified
    by the OAuth 2.0 client IDs from the auth tokens) who created the channel can stop the channel. If the channel
    was created by a service account, any user from the same client can stop the channel."* **Two rules keyed on
    how the channel was created**, and the discriminant is the **`client_id` inside the token** — which
    `credential` exposes only as a rendered header value (`ADR-0061`), and which a JWT access token would carry as
    the **unverified** `aud`/`azp` claims (`ADR-0064`). Enforcing it locally would mean trusting an unverified
    claim **to be stricter than the provider**, the failure direction `ADR-0064` warns about. So it is a recorded
    limit, and a violation is the provider's `403`.
  - **⭐ `WatchRequest` → `JsonRequest`: A NAME DERIVED FROM ITS FIRST CALLER BECAME FALSE.** `channels.stop` is
    the call that *ends* a channel a watch created — not a watch. The axis separating this type from
    `FormRequest` is the **credential boundary** (`ADR-0093`), which both bodies share (neither carries one), so
    the name follows the axis. `ADR-0093`'s revisit condition asked exactly this and pre-supplied the criterion.
    It is also the safe moment to rename: the type has **no consumers** — the builders are the public surface.
  - **⭐ THE TWO IDENTIFIERS ARE OPAQUE AND TRANSPOSABLE, SO THE TEST PARSES RATHER THAN SUBSTRING-MATCHES.** A
    swapped `id`/`resourceId` pair is well-formed and would stop the wrong channel or none. Asserted by
    deserializing the body and comparing each field to its expected value, plus an exact field count — so the
    optional `token` the reference permits is *asserted absent*, because sending it would put the anti-spoofing
    control into a body a diagnostic renders, for no effect.
  - **⭐ TWO GUARDS FALSIFIED A-B-A, AND A THIRD MUTATION WAS A NO-OP THAT DISPROVED MY OWN DOC.** (1) Writing the
    **channel id into both** body fields → **detected** (`left: "channel-alpha"`, `right: "o3hgv1538sdjfh"`).
    (2) Removing the **`stop_succeeded` guard** from `calendar_exposure`'s live arm → **detected**
    (`left: SettlingWithinMinutes`, `right: UntilTheLeaseLapses { seconds: 90000 }`). (3) **Reordering the arms
    so a successful stop matched first changed NOTHING** — `Lapsed` and `Alive` are different variants, so the
    arms are mutually exclusive and **no order decides anything**. The doc had said *"that is why the arms are
    ordered lease-first"*, which asserted a fact about the source's **layout** as though it were a fact about the
    **behaviour** — unfalsifiable, therefore uncheckable. **The claim was corrected rather than kept**, and the
    load-bearing part is the guard on the arm below.
  - **NEW LIMITS:** **no request is sent and no teardown executes** — still no caller, as for the push handler and
    the sync loop. `users.stop` is deliberately **not** built (a different method with an empty body would need a
    third request shape for no gain). The **channel count is a runtime fact**, so a plan names the effect and the
    caller performs it per channel; a caller that stops fewer than all of an account's channels gets no refusal
    from this module, only a plan to read — unchanged from `ADR-0095`. `gmail_exposure` cannot report
    `AlreadyEnded` even though the caller holds the watch's `expiration`: a wiring gap, not a missing fact.
- [ ] `P5-005` **(continued — a rule stated in another field's description)**: `google::client` gains
  `HistoryPosition { Storable, UnfinishedWalk, Unstated }` + `of_page`; `gmail_history_signal` takes the type
  rather than an `Option<&str>`; `HistoryPage.history_id` becomes `position`; the `gmail_history_list` **output
  schema's** description states the condition. **2 new tests (so 477 in the crate; 1750 in the workspace).**
  **`ADR-0108`.** Two guards falsified A-B-A. A fifth claim was found while writing the control for the second.
  - **⭐⭐ THE FINDING: THE RULE THAT GOVERNS STORING A VALUE IS IN ANOTHER FIELD'S DESCRIPTION.** The
    `history.list` response's `historyId` is described only as *"The ID of the mailbox's current history
    record."* The condition is stated **once**, in **`startHistoryId`'s** description: *"If you receive no
    `nextPageToken` in the response, there are no updates to retrieve and you can store the returned `historyId`
    for a future request."* So a page's id is the mailbox's position **at the moment that page was produced**, and
    a walk with more pages has not consumed the changes up to it. **⭐ Ask of a rule you are relying on: which
    field's description actually states it — the field you are storing, or one you only ever send?**
  - **⭐⭐ AND FOUR LAYERS OF THIS CRATE ASSERTED THE UNCONDITIONAL FORM, each true of a final page and false of a
    continuing one.** (1) a renderer comment (*"the durable cursor"*); (2) `HistoryPage`'s field doc (*"the next
    sync cursor"*); (3) the **output schema's description** (*"This is the next sync cursor…"*); (4)
    `gmail_history_signal`'s doc inferring *"the mailbox was unchanged"* from an **absent** id the reference does
    not document as optional. **The third is the one that decides the design: an output schema's `description`
    is not documentation for a repository reader — it is an INSTRUCTION TO A MODEL**, delivered in the model's
    own channel. A model told "this is the next sync cursor" stores it. **⭐ A schema description is
    executable-adjacent prose: it is read by the component that acts, so an error in it is a behaviour, not a
    typo.**
  - **⭐ THE DEFECT WAS NOT THE WORDING — IT WAS THAT NO TYPE ASKED THE QUESTION.** The parser produced
    `Option<String>`, the renderer emitted it whenever it was `Some`, and the signal producer took
    `Option<&str>` — so every layer could *describe* the field as a cursor without any layer having to *decide*
    whether it was one. **Four rounds of correcting prose would not have caught this; one type that makes the
    question unskippable does.** The producer now cannot be called with a bare id.
  - **⭐ THREE VARIANTS, NOT A `bool`.** `UnfinishedWalk` and `Unstated` both fail to yield a position and differ
    in *why*: a stated-but-unusable id is **information a diagnostic about a stalled walk wants**, and the second
    must not be read as "the mailbox is unchanged" — which is exactly claim (4). Collapsing them would restore
    that inference in the type system's own vocabulary (`ADR-0035`).
  - **⭐ THE RULE NEEDS BOTH FIELDS, SO ITS ENCODING TAKES BOTH.** `of_page(id, next_page_token)`: an id with no
    token is storable, an id with a token is not, and no id is not a position — which is why folding it into an
    `Option<String>` at the parser loses the distinction **whichever way the `Option` points**. A blank id is
    treated as absent (the reading `VerifiedAccount::new` already takes). `storable()` is named for the
    **question** rather than the value, and there is deliberately **no** accessor returning the id in every state.
  - **⭐ THE RENDERER STILL EMITS THE ID ON A CONTINUING PAGE.** Withholding it would make *"the page stated an id
    you may not use"* identical to *"the page stated nothing"*; the condition now travels with the value through
    the description **plus** the presence of `next_page_token` — the exact fact the provider's rule keys on.
  - **⭐ TWO GUARDS FALSIFIED A-B-A, both compiling.** (1) `of_page` returning `Storable` regardless of the token
    → **detected by two tests.** (2) `storable()` returning the id for `UnfinishedWalk` → **detected by three**,
    including the end-to-end signal assertion (`left: Advanced { history_id: Some("12347") }`,
    `right: … { None }`).
  - **⭐ A FIFTH CLAIM WAS MINE, FOUND BY WRITING THE CONTROL.** My first version of the end-to-end test stated an
    id **smaller** than the fixture cursor's, and it failed — not on the storable rule but on
    `advance_gmail_history`'s **monotonic** guard, which refused a backwards move. So the test would have been
    checking a different guard and *passing for the wrong reason* had the id happened to be larger. The id is now
    greater than the fixture's, **with the reason recorded in the test**: a fixture that triggers a different guard
    is the same defect as one that cannot separate two behaviours.
  - **NEW LIMITS:** **nothing stores a cursor** — there is no sync loop — so the rule is representable and tested
    rather than enforced on a store. No live walk has run: the path is exercised against the two hand-built
    fixtures. A `200` whose page states **no** id advances nothing and is `Unstated`, explicitly *not* "the
    mailbox is unchanged"; the reference does not mark `historyId` optional, so that shape is not one the provider
    documents — which is why the inference is refused rather than guessed.
- [ ] `P5-005` **(continued — a constraint three layers knew and the test contradicted)**: `google::client`
  gains `CalendarContinuation { MorePages, WalkComplete, NothingFurther, Rejected }` + `of_page`;
  `CalendarPage.next_page_token`/`next_sync_token` become `continuation`; `calendar_signal` takes the type; the
  renderer emits **at most one** token; the `calendar_events_read` output schema's two tokens gain descriptions.
  **1 new test (so 478 in the crate; 1751 in the workspace)**, plus a shipped test **corrected**. **`ADR-0109`.**
  Two guards falsified A-B-A — and one of them **survived `--lib`**, which is the second finding.
  - **⭐⭐ THE FINDING: A CONSTRAINT THREE LAYERS KNEW AND NONE ENFORCED.** The `events.list` reference documents
    the two continuation tokens as mutually exclusive **in each field's own description** (`nextPageToken`
    *"Omitted if no further results are available, in which case `nextSyncToken` is provided"*; `nextSyncToken`
    *"Omitted if further results are available, in which case `nextPageToken` is provided"*). **Three layers of
    this repository already stated it** — the research record, the fixture's own
    `_the_point_of_this_fixture` prose, and the renderer's test helper — while `CalendarPage` carried both as
    independent `Option<String>` fields, `calendar_signal` took a bare `Option<&str>`, and the **output schema**
    declared both with **no description at all**. **⭐ Writing a rule down three times is not enforcing it once;
    and the layers that repeat it read as corroboration, so nobody looks for the layer that is missing.**
  - **⭐⭐ AND THE ONE TEST STILL ASSERTING THE IMPOSSIBLE THING WAS THE PARSER'S — which the fixture's own notes
    had already flagged.** The fixture says: *"An earlier test of mine asserted a body carrying both, which
    Google cannot produce."* **The correction reached the fixture, the record and the renderer's helper, and
    missed the parser's test.** So the slice is not "add a type": it is "**the same defect was fixed four times
    in three places that do not run, and left in the one that does**". **⭐ Generalisation: when a correction is
    applied to several copies of a claim, enumerate the copies and check the ones that EXECUTE first.**
  - **⭐⭐ THE SECOND FINDING IS ABOUT THE TEST SUITE, NOT THE CODE: A GUARD WHOSE ONLY DETECTOR LIVED IN ANOTHER
    TEST BINARY.** The mutation routing a **page token through as the sync position** **survived the entire
    `cargo test --lib` run** (477 passed), because `calendar_signal`'s 200 arm had **no unit test** while the
    **Gmail** producer's arm did — the asymmetry was invisible and nobody had a reason to look for it. Only
    `tests/google_fixtures.rs` caught it. **`cargo test --lib` is what a developer runs while iterating, so a
    guard checked only by an integration test is one refactor from being unguarded.** The missing unit test was
    written, and re-running the *same* mutant then fails in `--lib` — **confirmed by mutation, not assumed**.
  - **⭐ FOUR VARIANTS, AND `Rejected` IS A VALUE RATHER THAN A PANIC OR A PRECEDENCE.** The pair is impossible
    against a conforming provider, so the state names a provider change or a hand-built fixture error. A panic
    would turn reportable provider input into a crash; **silently preferring one token is the defect the type
    exists for** — a caller handed the sync token from such a page would store a position for a walk that has not
    finished. Both accessors return `None` for it, so an uninterpretable response yields neither a position nor a
    next page, and `is_nonconforming()` makes the state actionable.
  - **⭐ IT IS NOT `HistoryPosition`, BECAUSE THE RULES HAVE DIFFERENT SHAPES.** Gmail's is a condition on **one**
    value (*"storable only when the page token is absent"*), which leaves an id present and unstorable;
    Calendar's is a relation between **two** (*"exactly one token"*), which leaves no such state. Sharing a type
    would give Calendar a variant its provider cannot produce — the defect being fixed. **⭐ `ADR-0108`'s lesson
    was "no type asked the question"; the answer is not therefore the same type.**
  - **⭐ THE OUTPUT SCHEMA WAS THE LAYER WITH THE LEAST INFORMATION AND THE MOST AUTHORITY.** `ADR-0108` taught
    the **Gmail** output schema to state its condition, and left this one — in the same tool family, in the
    **same slice** — with bare `["string", "null"]`. A model reading it saw two optional tokens and could
    persist the one that expires. **⭐ A lesson applied to one tool and not its neighbour is indistinguishable
    from a lesson not learned.**
  - **⭐ TWO GUARDS FALSIFIED A-B-A, both compiling.** (1) `of_page` **preferring the sync token** when both
    arrive → **detected** by the parser's test. (2) routing a page token through as the sync position →
    **detected only by the integration test** (which is the finding above); after the missing unit test was
    written, the same mutant is **detected by `--lib`**.
  - **⭐ A DUPLICATED VERIFICATION-LOG ROW WAS FOUND AND REMOVED** while appending the new one — the previous
    slice's edit inserted its row twice, and the record had been carrying both. Found by inspection while adding
    to the table, which is the only moment a duplicate of that shape is visible.
  - **NEW LIMITS:** no request sent, no live walk run (two hand-built fixtures); **nothing stores a sync token**,
    so `storable`/`page_token` have no production consumer and each doc says so explicitly (the
    `seconds_from_edge` convention, rather than deleting accessors whose absence would leave a page token
    unreadable while the sync token is not); `Rejected` is unreachable against a conforming provider by design,
    so it is exercised only by the test that deliberately keeps the impossible body.
- [ ] `P5-005` **(continued — a first sync with two documented branches and one expressible)**: `google::connection`
  gains `ResumePoint::FromAnchor`, `SyncOrigin { WatchResponse, Profile }`,
  `resume_anchored`/`resume_ignoring_anchor`, `AnchoredAccount`, and the `establish_account_with_anchor` /
  `establish_account_from_watch` entry points; `ResumePoint::position()` returns `None` for `FromAnchor` and a new
  `anchor()` accessor returns it. **2 new tests (so 480 in the crate; 1753 in the workspace).** **`ADR-0110`.**
  Two guards falsified A-B-A. Joins the anchor that two modules had parsed and described to the step that
  decides where a sync begins.
  - **⭐⭐ THE FINDING: A FIRST SYNC HAD TWO DOCUMENTED BRANCHES AND THE CONNECTOR COULD EXPRESS ONE.** The push
    guide, immediately after defining the anchor, **forks**: *"Your client receives notifications for all changes
    **after** that `historyId`. **If you need to process changes before this `historyId`**, refer to Synchronize
    clients with Gmail."* Branch one is `history.list` **from the anchor** — the changes since the `watch`, then
    a position of its own. Branch two is the mailbox's **existing contents**. `resume_from` answered `FullSync`
    for every `Start` cursor, so **only branch two existed**, and a just-connected mailbox was read end to end at
    `5 + 20N` quota units to discover nothing had happened since the `watch`. **⭐⭐ Generalisation: when a
    provider's sentence contains an "if you need X instead", that is a FORK — enumerate the branches and check
    each is expressible, because implementing the second one looks complete from the inside.**
  - **⭐⭐ AND THE ANCHOR HAD A PARSER, A FIELD, TWO MODULES OF JUSTIFICATION, AND NO CONSUMER.** `watch.rs` calls
    it *"the anchor a first sync starts from"*; `request.rs` says the profile's `historyId` *"yields the mailbox's
    current position without consuming a message"* and that **either can seed a first sync**. **Neither could
    seed anything.** This is `ADR-0092`'s "a response field with no reader" from the other side: a value that IS
    read, is documented as the input to a step, and whose step cannot accept it. **⭐ A doc comment that says what
    a value is FOR is a claim about a consumer — check the consumer exists before believing it.**
  - **⭐ `position()` RETURNS `None` FOR AN ANCHOR, AND THAT IS THE POINT.** An anchor is not a position —
    nothing has been synced from it. The guide's trap is exactly that the two are distinct strings which
    plausibly fit each other (`1234567890` vs `9876543210`, both spelled `historyId`), so the type now refuses
    the reading the guide warns against: `FromAnchor` carries no `position`, and `anchor()` is separate.
  - **⭐ `requires_full_sync()` IS TRUE ONLY FOR `FullSync`.** An anchored start does **not** read the mailbox
    from the beginning, so a caller asking "is this expensive" gets the right answer for both incremental cases
    without knowing which it holds — and the property is what makes the branch a decision rather than a default.
  - **⭐ A STORED POSITION OUTRANKS AN ANCHOR, AND THE WRONG DIRECTION IS A SILENT DUPLICATE.** Once anything has
    been synced the anchor is historical; preferring it would re-read the window between the `watch` and the
    first stored position, and `history.list` would simply return records the store already holds — a duplicate,
    not a miss, and therefore invisible. **The opposite failure direction from `ADR-0090`'s, which is why it
    needed its own assertion.**
  - **⭐ TWO BRANCHES ARE TWO FUNCTIONS, NOT A FLAG.** The difference in cost is orders of magnitude, and a
    `bool` at a call site (`resume_from(cursor, false)`) says nothing about which branch is which. The expensive
    branch is `resume_ignoring_anchor` — a **name**, so a caller cannot reach it by omitting an argument — and it
    takes the anchor and discards it, which is what keeps the two signatures alike enough that neither is called
    by accident.
  - **⭐ `SyncOrigin` IS AN ENUM, NOT A `bool`** (`ADR-0035`): the sources differ in *cost* — a profile read needs
    only a credential and 1 quota unit, a `watch` also needs a Pub/Sub topic — so a caller choosing between them
    is choosing a cost, and the origin is carried rather than inferred so a log line says **where** a starting
    point came from. `establish_account` now shares one body with the two anchored entry points, and carries no
    anchor of its own — a deliberate limit rather than an oversight.
  - **⭐ TWO GUARDS FALSIFIED A-B-A, both compiling.** (1) `resume_anchored` **never anchoring** → detected
    (`left: FullSync`, `right: FromAnchor { … }`). (2) the **position-outranks-anchor** check disabled →
    detected (`left: FromAnchor { … }`, `right: FromPosition { … }`).
  - **⭐ A DOC COMMENT THAT WAS AN OVERSTATEMENT IS NOW CHECKABLE.** `GmailProfile`'s own doc said *"**either**
    can seed a first sync"* about the profile and watch anchors — true of the **values** and false of the
    **connector**, which could seed nothing from either. It is now true of the code, and the code is what runs.
  - **NEW LIMITS:** no request sent, no account stored, no sync runs, so `FromAnchor` reaches a real
    `history.list` only when a sync loop exists. The anchor's **age** is not modelled — a long-held anchor used
    now may have been pruned, which arrives as the documented `404`/resync path rather than as a local refusal.
    And **nothing records which branch a deployment chose**: a caller picks per call, so a connector that used
    branch two for every account would still pay the cost the branch exists to avoid.
- [ ] `P5-005` **(continued — a bound and a lease are different inputs)**: `google::teardown` gains
  `gmail_watch_exposure(lease: WatchLapse, stop_succeeded)` beside the unchanged `gmail_exposure(stop_succeeded)`;
  the `AlreadyEnded` variant doc, the module doc and a stale `notification_exposure` reference are corrected.
  **2 new tests (so 482 in the crate; 1755 in the workspace).** **`ADR-0111`.** Two guards falsified A-B-A.
  Closes the wiring gap `ADR-0107` recorded as its own revisit condition.
  - **⭐⭐ THE FINDING: A BOUND AND A LEASE ARE DIFFERENT *INPUTS*, NOT TWO ENCODINGS OF ONE.** `ADR-0107` built
    `gmail_exposure` from Google's **mechanism bound** — *"at least once every 7 days"* — and
    `calendar_exposure` from a channel's **own lease**, then recorded that Gmail's figure was one-sided: the
    caller **does** hold the watch's `expiration` (`parse_watch_response` reads it; `watch_lapse` turns it into
    `WatchLapse`), so `AlreadyEnded` *could* be reachable but was not. **⭐ "We hold the value" ≠ "the value the
    function takes"** — the same shape `ADR-0106` records for one expiry in three encodings, where the form the
    code held was the unusable one.
  - **⭐⭐ A SECOND FUNCTION, NOT AN `Option` PARAMETER.** `gmail_exposure(Option<WatchLapse>, bool)` would give
    every existing call site a `None` to pass and would make the mechanism's limit and this watch's expiry two
    spellings of one call — the flag-with-a-meaningless-`Option` shape `NotificationExposure`'s own doc already
    rejects for the mechanism itself. **⭐ `ADR-0107` removed a `PushMechanism` enum because a value that only
    distinguishes two mechanisms is a consumerless value; the same argument applies to the INPUT, so the
    function choice names which input is held.**
  - **⭐ THE BOUND-ONLY FIGURE IS KEPT, NOT REPLACED.** A scheduler deciding when to renew holds the mechanism's
    limit and **no** particular watch — that is the caller `WATCH_RENEWAL_BOUND_SECONDS` exists for. Two inputs,
    two functions, both asserted reachable, so neither becomes dead code.
  - **⭐ THE LIVE ARM REPORTS THE LEASE'S OWN SECONDS, NOT THE BOUND.** The reference warns a watch's actual
    expiry *"may return shorter than requested"*, so falling back to the bound for a live lease overstates a
    nearly-expired watch in the exact direction the bound-only figure was already wrong. A falsification
    replaces `for_seconds` with `WATCH_RENEWAL_BOUND_SECONDS` and the test detects it.
  - **⭐ TWO GUARDS FALSIFIED A-B-A**, both compiling: (1) inverting the `stop_succeeded` guard → detected
    (`left: SettlingWithinMinutes`, `right: UntilTheLeaseLapses { seconds: 120000 }`); (2) the live arm's
    `for_seconds` → `WATCH_RENEWAL_BOUND_SECONDS` → detected (`left: … { seconds: 604800 }`, `right: … {
    seconds: 120000 }`). A third mutation — reordering the arms — changes nothing, because `Lapsed` and `Alive`
    are disjoint variants: `ADR-0107`'s lesson applied rather than re-learned.
  - **⭐⭐ A SIBLING TEST ASYMMETRY WAS CLOSED, AND THE SAME MUTANT IS NOW CAUGHT IN `--lib`.** `ADR-0109` found
    that `calendar_signal`'s 200 arm had no unit test while Gmail's did, so a mutant routing a **page token
    through as the sync position** survived the whole `--lib` suite. `CalendarContinuation::storable` and
    `page_token` were still exercised only *through* `calendar_signal`; a test now asserts directly that the two
    accessors are **disjoint** (no value offers both a page to fetch and a position to store) and that `Rejected`
    yields neither. Making `storable` return the page token is now **detected by `--lib`**, confirmed by
    mutation.
  - **⭐⭐ TWO DEFECTS WERE FOUND IN THE RECORD ITSELF WHILE APPENDING TO IT.** (1) A **dangling intra-doc link
    to the `PushMechanism` enum that `ADR-0107` removed** — in the doc paragraph that *says* the enum was
    removed; `cargo doc` warnings are not denied, so nothing compiled it. (2) **The findings were numbered
    `…20, 22, 23`** — `ADR-0108`'s commit renamed the then-existing Finding 21 to 23 and inserted a new Finding
    22, leaving **no Finding 21**, and two subsequent slices appended without noticing. **⭐ A gap in a numbered
    series reads as a DELETED finding**, and a duplicate/omission of that shape is only visible while appending —
    which is what found it. Renumbered to contiguous `21`/`22`; nothing outside the record cites a Finding by
    number, which is what made the renumber safe.
  - **NEW LIMITS:** no teardown executor, so nothing computes the `WatchLapse` and calls either exposure
    function; `users.stop` still deliberately absent; **the stop permission rule still cannot be checked
    locally** (the client id lives inside the credential). A convenience that reads a `WatchResponse`'s
    `expiration` into a `WatchLapse` is **not** built, because it would be a second reader of a value with one
    caller — the consumerless-value defect the same slice's own argument invokes.
- [ ] `P5-005` **(continued — a prescribed call with no builder)**: `google::request` gains
  `calendar_channel_watch(calendar_id, channel_id, address, token)`; `JsonRequest`'s redaction becomes
  body-dependent (hand-written `Debug`, two private named constructors `renderable`/`sensitive`); `webhook_address`
  and the channel-id bound are new. **5 new tests (so 487 in the crate).** **`ADR-0112`.** Four guards falsified
  A-B-A. Closes the **create** half of the channel path.
  - **⭐⭐ THE FINDING: A PRESCRIBED CALL WITH NO BUILDER.** `channels.stop` was built and `renewal_decision`
    prescribes the remedy — *"you must **replace it with a new one by calling the `watch` method`"* — while **no
    `events.watch` builder existed**. So the connector could **end** a channel and had no way to **make** one,
    and `parse_channel_watch_response` plus the whole `ChannelLease`/`ChannelRenewal` chain were reachable only
    from a hand-built fixture. **⭐ `ADR-0092`'s "a value with no reader" from a new direction: not a decision
    without a caller, but a call the documentation names and the connector's own decision function prescribes,
    with no operation behind it.**
  - **⭐⭐ THE THIRD BODY MADE A TYPE'S OWN DOC FALSE.** `JsonRequest`'s `rendered_body` said *"there is no
    credential here"* and the type **derived `Debug`** — true of its two bodies (a topic name with label ids; a
    channel id with a resource id) and false of a **creation** body carrying the webhook `address` and the
    channel **`token`** (the anti-spoofing control `verify_channel_token` compares against). **⭐ A derived
    `Debug` is a claim that every field is printable, and the claim expires when a new field arrives.** The
    redaction is now body-dependent, and the choice is **forced** rather than defaulted: no public constructor,
    two named private ones, so a fourth builder must say which kind of body it produces — the
    "unrepresentable rather than checked" shape this module already uses for a credential in a URL.
  - **⭐ THE `address` IS VALIDATED FOR WHAT A STRING CAN PROVE, AND THE CERTIFICATE RULE IS A RECORDED LIMIT.**
    The guide requires HTTPS *and* a valid (non-self-signed, non-revoked, subject-matching) certificate —
    but a chain is a fact about a TLS handshake a request value with no socket cannot perform. So a callback
    with a bad certificate passes the builder and fails at **delivery** time; pretending to check it would be an
    unverifiable claim. `http://` is refused rather than downgraded, and the scheme compare is case-insensitive
    because a URI scheme is.
  - **⭐ GOOGLE'S 64, NOT THE MODULE'S GENERIC 256.** The channel `id` is capped at *"64 characters"* by the
    push guide, while every other identifier here uses a JARVIS bound of 256. A 65-character id passes the
    generic `resource_id` and is refused by the provider, so the builder checks Google's figure **on top of**
    the shared validator, and the boundary is exercised at 64 (ok) and 65 (refused).
  - **⭐ THE REDUNDANT-GUARD RULE APPLIED.** The token's 256 bound is already enforced by `SecretValue::new`, so
    the builder does **not** re-check it — a second check of the same bound is the guard `ADR-0066` records as
    one that can never decide anything the first did not.
  - **⭐ FOUR GUARDS FALSIFIED A-B-A**, all compiling: (1) `sensitive` setting `false` → the token appeared in a
    `Debug` rendering → **detected**; (2) `strip_https_scheme` accepting any non-empty scheme → `http://`
    accepted → **detected**; (3) the channel-id bound relented to `MAX_RESOURCE_ID_CHARS` → a 65-character id
    accepted → **detected**; (4) **the control** — `renderable` setting `true` → a printable body rendered
    `[REDACTED]` → **detected**, which is what stops a redact-everything `Debug` from satisfying the redaction
    test while destroying the diagnostic value of the two bodies that hold no secret.
  - **NEW LIMITS:** no request is sent and no channel is registered — the **certificate** rule and reachability
    are unverifiable offline; the stop **permission** rule (`ADR-0107`) is still unenforceable locally; **no
    expiry parameter** is offered (the figure acted on is the response's), and **nothing renews on a schedule**
    yet, so `CHANNEL_REPLACE_LEAD_SECONDS` and this builder are not exercised together.
- [ ] `P5-005` **(continued — the value that survives is the one that did not hold it)**: `google::channel`'s
  `ChannelRegistration` gains `expires_at` (the provider's reported expiry), `from_watch_response`, `expires_at()`
  and `renewal(now)`; the three test fixtures and the integration test gain the argument. **2 new tests (so 489
  in the crate)**, plus the end-to-end fixture test extended. **`ADR-0113`.** Three guards falsified A-B-A.
  - **⭐⭐ THE FINDING: THE RECORD THAT SURVIVES THE CALL DID NOT CARRY A VALUE A LATER DECISION NEEDS AS ITS SOLE
    INPUT.** `parse_channel_watch_response` reads the response's `expiration` into `ChannelWatchResponse::expires_at`
    — whose own doc says it *"can drive a renewal decision"* — and `renewal_decision` takes **that expiry as its
    only input**. But `ChannelRegistration`, documented as *"the value that survives between the `watch` and the
    teardown"*, had **no field for it**, so nothing in production held the input and the decision was reachable
    only from a test. **⭐ The same "a value read and then dropped" shape `ADR-0107` found for `resourceId` one
    round earlier, from the same call.**
  - **⭐⭐ AND THE TYPE'S OWN DOC COUNTED ITS FACTS AND THE COUNT WAS WRONG.** It said *"two of the four facts are
    the provider's and two are not"* — there were **five** (three from the response: `id`, `resourceId`,
    `expiration`; two the connector's own: account, token), and the one the count had no room for was the
    expiry. **⭐ A doc that enumerates its inputs is a claim about the set, and an omission inside the
    enumeration is invisible because the arithmetic adds up.**
  - **⭐ `from_watch_response` IS THE CONSTRUCTOR, BECAUSE IT MAKES THE SPLIT UNNECESSARY.** Three of the five
    facts come from the one response; taking the response whole makes the expiry travel with the two identifiers
    beside it rather than leaving a caller to remember a third — which is exactly how it was dropped. The plain
    `new` stays for tests and a caller holding loose values.
  - **⭐ A STORED FACT, NOT A STORED DECISION.** The field is the `UtcTimestamp` and **not** a `ChannelLease` or
    `ChannelRenewal`: those are answers to a question asked at an instant, and storing one would pin it to the
    second it was computed. `renewal(now)` is the bridge and **delegates** to `renewal_decision`, asserted equal
    so the bridge is provably not a second opinion.
  - **⭐ `from_watch_response` REFUSES A BLANK `resourceId`.** The parser checks presence and type but not
    usability, so a blank second stop identifier would build a registration that cannot end its own channel.
  - **⭐ THREE GUARDS FALSIFIED A-B-A:** (1) `renewal()` ignoring the registration's expiry → detected; (2) the
    blank-`resourceId` refusal removed → detected (`left: Ok(… resource_id: "   " …)`, `right: Err(Missing {
    field: "resourceId" })`); (3) `from_watch_response` not carrying the expiry → detected.
  - **NEW LIMITS:** still no caller that renews on a schedule, so `CHANNEL_REPLACE_LEAD_SECONDS` and this bridge
    are not exercised together; a registration is **not persisted**, so the new `UtcTimestamp` field has no
    stored column yet; the expiry is read back from the response rather than recomputed from the request,
    because the guide's *"more restrictive value is used"* means a recomputation would disagree exactly when
    Google shortened it.
- [ ] `P5-005` **(continued — a path identifier validated but not encoded)**: `calendar_events_list` and
  `calendar_channel_watch` now apply `percent_encode` to the calendar id; `resource_id`'s doc is corrected to
  separate log forging from request structure. **1 new test (so 490 in the crate).** **`ADR-0114`.** Two guards
  falsified A-B-A.
  - **⭐⭐ THE FINDING: BOTH CALENDAR PATH BUILDERS INTERPOLATED `calendar_id` RAW, WHILE THE GMAIL ONE ENCODED.**
    `gmail_messages_get` percent-encodes its path identifier, with a comment and a test asserting a `/` becomes
    `%2F` — while `calendar_events_list` and `calendar_channel_watch` put a `calendar_id` in the path **raw**. A
    Gmail message id is an opaque hex string so the omission was invisible; **a Calendar id is routinely a
    mailbox address (`user@example.com`) and a holiday calendar's id contains a literal `#`
    (`en.usa#holiday@group.v.calendar.google.com`)**, which starts a URL **fragment** and truncates the path to
    `/calendars/en.usa` — a calendar that does not exist.
  - **⭐⭐ AND THE ASYMMETRY IS WHAT HID IT.** A reader asking "does this module encode path identifiers?" finds a
    `yes` in the sibling that was written with a message id in hand — the same shape `ADR-0107` records for two
    teardown steps that should have matched. **⭐ Two builders that should behave alike and do not are invisible
    until they are compared side by side.**
  - **⭐ A VALIDATOR THAT BOUNDS LENGTH AND REFUSES CONTROL CHARACTERS DOES NOT MAKE AN IDENTIFIER URL-SAFE**, and
    that is the second half: `resource_id` **accepts** `@`, `#`, `%` and `/` because a real calendar id contains
    them, so it addresses **log forging** and not **request structure**. Its doc implied the check covered URL
    safety; it now states what it does and names `percent_encode` as the control for the other. **⭐ A doc that
    implies coverage is what makes the next builder omit the control.**
  - **⭐ THE WATCH HALF MATTERS MORE.** A wrong path on a read addresses the wrong calendar and fails; a wrong
    path on a `watch` **registers a channel against the wrong resource**, and the channel id is the join key a
    delivery routes on — so the mistake propagates past the call.
  - **⭐ THE SAME `percent_encode`, NOT A PATH VARIANT.** The unreserved set is identical for a query value and a
    path segment, so one function is correct in both positions and a second would be two implementations of one
    rule that could drift.
  - **⭐ TWO GUARDS FALSIFIED A-B-A:** (1) removing the encoding from `calendar_events_list` → detected
    (`…/calendars/en.usa#holiday@…/events`); (2) removing it from `calendar_channel_watch` → detected. The test
    asserts each structural character's **exact** encoding (`#`→`%23`, `?`→`%3F`, `/`→`%2F`, `%`→`%25`) on both
    builders, plus the control that `primary` is left alone, so a partial fix (escaping only the `#`, or only one
    builder) fails rather than passing.
  - **NEW LIMITS:** no request is sent, so the encoding is proved against the builder's own output rather than a
    provider's response; a fourth builder would justify extracting the encoding into a shared URL helper.
- [ ] `P5-005` **(continued — one acknowledgement vocabulary for both push mechanisms)**: `ChannelIngest`'s
  `acknowledges() -> bool` becomes `acknowledgement() -> DeliveryAck`, the same type `GmailIngest` returns; the
  four existing assertions are tightened to name the specific `DeliveryAck`. **2 new tests (so 492 in the
  crate).** **`ADR-0115`.** Three guards falsified A-B-A.
  - **⭐⭐ THE FINDING: ONE INGEST ANSWERED WITH A DECISION AND ITS TWIN WITH A `bool` THAT WAS ALWAYS `true`.**
    `GmailIngest::acknowledgement()` returns a three-state `DeliveryAck` (`Accept` / `Retry` /
    `AbandonAndAcknowledge`); `ChannelIngest::acknowledges()` returned `bool` and **always `true`**, so a caller
    could not tell **a delivery it acted on** from **one it deliberately dropped** — opposite downstream
    consequences (work done versus work irrecoverably discarded), and only one should be recorded as a success.
    The `bool`'s doc even said *"a future variant that should be retried … has a place to say `false`"*: it
    reasoned about the **retry** dimension (always `false` here) and never noticed the other dimension, *which
    kind of yes*, that its sibling carries. **⭐ The same sibling-asymmetry shape `ADR-0107` and `ADR-0114` found
    — two things that should behave alike, each internally consistent, invisible until read side by side.**
  - **⭐ A METHOD WHOSE RETURN IS CONSTANT IS THE SHAPE TO CHECK.** "All variants return `true`" is true of the
    *acknowledgement* and false of the *reason*, and the reason is the half a caller acts on. **A value that is
    always the same is either a fact worth asserting or a type that is missing a variant.**
  - **⭐ ONE METHOD, NOT TWO NAMES.** The richer type loses nothing because `DeliveryAck::acknowledges()` is the
    predicate, so a caller wanting "does this acknowledge" asks `outcome.acknowledgement().acknowledges()` — the
    old name is **not** kept as a shim, because two names for one question is the duplication this repository
    removes everywhere else.
  - **⭐ THE `ADR-0094` ARGUMENT IS UNCHANGED AND NOW CARRIED BY A TYPE.** Every variant still acknowledges
    (none of the three refusals is repaired by another attempt, so refusing would be charged to the whole
    subscription) — only **which reason** becomes visible, and a call site now reads `AbandonAndAcknowledge`
    instead of trusting a `true`.
  - **⭐⭐ THE PARITY IS ASSERTED AS A CROSS-MECHANISM EQUIVALENCE, NOT A COMMENT.** Two tests drive the **same**
    outcome kind through **both** mechanisms and assert the acknowledgements are equal — accepted
    (`Changed` ↔ `Changed`) → `Accept`; unroutable and refused → `AbandonAndAcknowledge` — so a future
    divergence fails a test. Reverting to the "always `Accept`" behaviour is **detected by that parity test**,
    confirmed by mutation.
  - **⭐ THREE GUARDS FALSIFIED A-B-A:** (1) the refusals reporting `Accept` → detected; (2) an accepted delivery
    reporting a drop → detected; (3) reverting to always-`Accept` → detected by the parity test.
  - **⭐ THE FOUR EXISTING ASSERTIONS WERE TIGHTENED, NOT RELAXED** — each `assert!(outcome.acknowledges())`
    became an `assert_eq!(outcome.acknowledgement(), …)` naming the **specific** value, so the tests pin the
    reason as well as the acknowledgement (the "assert the specific value, not `is_ok`" rule).
  - **NEW LIMITS:** no handler sends an acknowledgement yet, so the `Accept`/`AbandonAndAcknowledge` distinction
    reaches a metric only when a push handler exists; a transient store failure is still not an ingest outcome,
    so `DeliveryAck::Retry` has no user in either mechanism.
- [ ] `P5-005` **(continued — a field with a producer and no reader)**: `ConnectorHealth` gains
  `missing_scopes()`; `diagnostics_for` consults the state's list as well as the caller's and emits one finding;
  a dangling `is_stale_at` doc link is corrected. **2 new tests (so 494 in the crate).** **`ADR-0116`.** Two
  guards falsified A-B-A.
  - **⭐⭐ THE FINDING: `NeedsReauth`'s `missing_scopes` WAS WRITTEN EVERYWHERE AND READ NOWHERE.**
    `ConnectorHealth::NeedsReauth` carries the list, documented as *"the scopes that are missing, when the
    reason is a scope loss"* — and there was **no accessor**, while `diagnostics_for` matched
    `NeedsReauth { reason, .. }` (discarding the field) and emitted `MissingScopes` — described as *"the list a
    reauth prompt needs"* — from a **separate argument**. So a caller that built a `ScopeLoss` state carrying
    scopes and passed an empty shortfall produced a report with **no** missing-scope finding at all.
    **⭐ `ADR-0092`'s "a value with a producer and no reader", and the field's own doc made a claim about a
    consumer that did not exist.**
  - **⭐⭐ AND THE TWO LISTS HAD TO AGREE WITH NOTHING MAKING THEM** (`ADR-0021`): the state's list and the
    caller's described one fact, only one reached the report, and they could differ.
  - **⭐ BOTH SOURCES ARE CONSULTED, DELIBERATELY** — a shortfall **without** a reauth (a partial consent the
    account still runs under, `AccountStatus::ScopeShortfall`) is only ever in the caller's list, while a
    `ScopeLoss` records the scopes in the **state** (the value a persisted health record holds). The union is
    the honest answer and it closes the drop. `MissingScopes` is emitted **once**, because it describes the
    condition rather than each scope.
  - **⭐ THE ACCESSOR IS THE `reauth_reason()` SHAPE** — a slice, not an `Option<&Vec>`, so a caller branches on
    `is_empty` rather than matching every variant; an absent list and an empty one call for the same action.
  - **⭐ TWO GUARDS FALSIFIED A-B-A:** (1) `missing_scopes()` returning empty for `NeedsReauth` → detected by
    **both** the accessor test and the diagnostics test; (2) `diagnostics_for` reading only the caller's argument
    → detected. A `let _ = missing_scopes;` was removed — the tell that the parameter decided nothing.
  - **NEW LIMITS:** nothing renders the report yet, so the union's contents are asserted rather than shown; a
    shortfall without a reauth still has no `ConnectorHealth` variant, so the caller's argument is not yet
    redundant.
- [ ] `P5-005` **(continued — a diagnostic that contradicts the predicate the platform gates on)**:
  `DiagnosticField::HealthStale` added; `diagnostics_for` takes `now` and `freshness_seconds` and derives the
  health severity from `permits_calls_at`. **2 new tests (so 496 in the crate).** **`ADR-0118`.** Three guards
  falsified A-B-A.
  - **⭐⭐ THE FINDING: THE REPORT RANKED A STATE BY THE WRONG PREDICATE, AND THE TWO FACTS NEEDED TO SEE THE
    CONTRADICTION WERE NOT IN THE REPORT.** `security.md`'s "missing or stale evidence fails closed" is enforced
    by `ConnectorHealth::permits_calls_at(now, bound)` — whose module doc calls it *"the method a caller should
    use"* and says calling `permits_calls` on an unfresh state *"is the defect this exists to prevent"* — and
    `diagnostics_for` derived `HealthState`'s severity from **`permits_calls()`** alone. So a `Connected`
    observation from yesterday was reported as `connected` at **`Info`** (the severity that means *nothing to
    do here*) about a state that permits **no call**.
  - **⭐⭐ THE REPORT CARRIED `HealthObservedAt` AND NEITHER THE BOUND NOR *NOW*.** So an operator could not even
    *see* the contradiction: the rule turns on `elapsed` versus a bound, and one of the two operands was in the
    report and the other two were not. **⭐ `ADR-0092`'s "a value with a producer and no reader" — the instant
    was there and nothing about it.** **⭐ A diagnostic that contradicts the predicate the rest of the platform
    gates on is worse than a missing one: it is read *instead of* the truth.**
  - **⭐ `DiagnosticField` IS DOCUMENTED AS A CLOSED SET OF FACTS A DIAGNOSTIC MAY REPORT, CHOSEN SO
    `is_loggable()` IS TRUE FOR ALL OF IT.** "This state is too old to act on" is such a fact, and the set had
    no variant — so the platform's central rule was the one thing the report could not say. The count assertion
    (20 → 21) was updated in the same change, so the new field is covered by the loggability, model-exposure,
    renderability and distinctness assertions.
  - **⭐ `now` IS A PARAMETER, NOT A CLOCK READ** — the reason the health module's own `is_fresh_at` records:
    a value that asked the system clock about its own age could not be checked against a supplied instant, and
    `jarvis_core::Clock` exists so time is injectable. The daemon owns the clock; this function owns the policy.
  - **⭐⭐ A MUTANT SURVIVED THE FIRST VERSION OF THIS CHANGE, AND THAT IS THE SECOND FINDING.** With
    `stale = !health.permits_calls_at(now, bound)` in place of `!is_fresh_at(now, bound)` **the entire suite
    passed** — every state the staleness test used was `Connected`, where the two predicates **agree**. They are
    different facts: a `NeedsReauth` observed a moment ago **permits no call** and **is fresh**, and the mutant
    would have told an operator "nobody has checked since" about a state that was just checked. The missing
    detector is a state that **refuses and is fresh**; `staleness_and_unusability_are_two_dimensions_and_a_
    mutant_is_why_this_exists` is it, and it was confirmed to fail under that mutant **after** being written.
    **⭐ A predicate with two conjuncts is exercised by a value where they DIFFER, and a fixture where they
    agree cannot see the difference.**
  - **⭐ THREE GUARDS FALSIFIED A-B-A:** (1) the freshness inputs ignored entirely → detected by **both** tests;
    (2) staleness conflated with unusability → detected by the **second** test only, after the gap was closed;
    (3) the supplied bound ignored (`u64::MAX`) → detected by both.
  - **⭐ THE SEVERITY IS DERIVED, NOT STORED, AND THE TWO FINDINGS NEITHER SUPPRESS NOR IMPLY EACH OTHER** — a
    stale healthy state is an error **and** stale; a fresh refusal is an error and **not** stale; a stale
    refusal reports both. `HealthStale` is emitted **only when true** (the `MissingScopes` rule), because a
    negative finding that is always present is one a reader stops seeing.
  - **NEW LIMITS:** nothing renders the report yet, so the two severities are asserted rather than shown together;
    the report has no ordering rule beyond emission order; a second source of staleness (a cursor's age, a
    lease's expiry) would repeat "value plus bound plus now" a third time and may deserve one type.
- [ ] `P5-006` Research Microsoft identity platform and Microsoft Graph mail/calendar, subscriptions, delta queries, and limits; record findings.
- [ ] `P5-007` Implement Microsoft connection setup and Outlook/Calendar read tools with recorded wire fixtures.
- [ ] `P5-008` Research and implement GitHub authentication and read tools.
- [ ] `P5-009` Add draft/write operations behind policy and approval with provider idempotency where available.
- [ ] `P5-010` Add reauth, token expiry, revoked scope, pagination, throttling, webhook replay, and redacted diagnostics tests.


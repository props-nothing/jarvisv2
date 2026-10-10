# Architecture Decision Records

ADRs preserve why durable choices were made. Accepted records are historical; change a decision by adding a new ADR that marks the old one superseded.

## Statuses

- `Proposed`: under review; implementation should not depend on it yet
- `Accepted`: current decision
- `Superseded by ADR-NNNN`: replaced, retained for history
- `Deprecated`: still present only for compatibility
- `Rejected`: considered but not selected

## Required Sections

Each ADR includes status/date, context, decision, consequences, alternatives, and conditions that would justify revisiting it. Link implementation and migration evidence when available.

## Index

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](0001-rust-control-plane.md) | Rust owns the durable control plane | Accepted |
| [0002](0002-daemon-client-topology.md) | Use a daemon with thin clients | Accepted |
| [0003](0003-sqlite-local-postgres-server.md) | SQLite local, PostgreSQL server | Accepted |
| [0004](0004-jarvis-owned-memory.md) | JARVIS owns canonical memory | Accepted |
| [0005](0005-canonical-tools-and-mcp.md) | Canonical tool gateway; MCP at the boundary | Accepted |
| [0006](0006-isolated-agent-runtimes.md) | External agent runtimes are isolated adapters | Accepted |
| [0007](0007-native-workflows-before-temporal.md) | Build a native durable workflow engine first | Accepted |
| [0008](0008-provider-neutral-voice.md) | Voice is provider-neutral; ElevenLabs is an adapter | Accepted |
| [0009](0009-clean-room-prototype-migration.md) | Migrate prototype behavior clean-room | Accepted |
| [0010](0010-model-based-turn-detection.md) | Turn detection and interruption are model-based capabilities | Accepted |
| [0011](0011-run-events-and-http-transport.md) | Run events are durable; HTTP is a first-class daemon transport | Accepted |
| [0012](0012-cli-runs-over-http.md) | The CLI reaches runs over HTTP with a loopback-only endpoint type | Accepted |
| [0013](0013-restart-settles-interrupted-runs.md) | A restart settles interrupted runs truthfully instead of resuming them | Accepted |
| [0014](0014-conversations-are-runs-in-a-session.md) | A conversation is runs sharing a session, and the session is a trust boundary | Accepted |
| [0015](0015-tool-contract-consistency-and-offline-schemas.md) | Tool contracts derive their source, refuse remote schema references, and validate cross-field consistency at construction | Accepted |
| [0016](0016-supplied-schema-documents-and-registry-boundaries.md) | A schema composes only against JARVIS-supplied documents; external manifests are self-contained | Accepted |
| [0017](0017-policy-evaluation-outcomes-and-ownership.md) | Policy evaluation is a pure adapter-crate function with three outcomes, deny overrides, and channel-capped authentication | Accepted |
| [0018](0018-an-approval-is-bound-to-the-exact-arguments.md) | An approval is bound to the exact arguments it was asked about (simplified by 0136) | Accepted |
| [0019](0019-tool-calls-are-a-durable-lifecycle.md) | A tool call is a durable lifecycle row; the idempotency ledger is a unique index and a terminal outcome is final | Accepted |
| [0020](0020-filesystem-confinement-is-a-handle.md) | Filesystem confinement is a directory handle rather than a validated path | Accepted |
| [0021](0021-an-authorization-receipt-derives-from-its-decision.md) | An authorization receipt is derived from its policy decision and cannot be built from a refusal | Accepted |
| [0022](0022-cancellation-carries-no-version.md) | A cancellation request carries no version, because operator intent cannot be stale | Accepted |
| [0023](0023-tool-pipeline-composition-root.md) | The tool pipeline is composed in the daemon over the adapter's own definitions | Accepted |
| [0024](0024-a-server-does-not-name-itself.md) | A server does not name itself, and two MCP tool names never become one identifier | Accepted |
| [0025](0025-mcp-effects-come-from-an-operator.md) | An MCP server's effects and risk come from an operator, never from the server | Accepted |
| [0026](0026-the-host-join-between-discovery-and-authority.md) | Discovery is joined to authority only where both halves are visible | Accepted |
| [0027](0027-a-remote-endpoint-is-a-validated-value.md) | A remote MCP endpoint is a validated value, and its HTTP client is built here | Accepted |
| [0028](0028-an-adapters-outcome-mapping-is-the-honesty-boundary.md) | An adapter's outcome mapping is the honesty boundary, and every row of it needs a test | Accepted |
| [0029](0029-the-mcp-host-role-is-a-narrow-configuration-surface.md) | The MCP host role is a narrow configuration surface, and one parser per document | Accepted |
| [0030](0030-the-daemon-dispatches-by-tool-identity.md) | The daemon dispatches by tool identity, and MCP servers live in their own document | Accepted |
| [0031](0031-a-dependencys-default-is-not-a-policy.md) | A dependency's permissive default is not a policy, and an origin is a tuple | Accepted |
| [0032](0032-a-transitive-tool-is-not-re-exposed.md) | A transitive tool is not re-exposed, and exposure is not the catalog | Accepted |
| [0033](0033-filtering-a-list-is-not-authorization.md) | Filtering a list is not authorization | Accepted |
| [0034](0034-a-delegated-check-that-cannot-express-the-rule.md) | A delegated check that cannot express the rule is not a control | Accepted |
| [0035](0035-a-documented-invariant-with-no-test-is-a-convention.md) | A documented invariant with no test is a convention | Accepted |
| [0036](0036-a-self-reported-name-is-never-a-permit.md) | A self-reported name is evidence or nothing, and never a permit | Accepted |
| [0037](0037-an-admitted-request-is-a-type.md) | An admitted request is a type, and the check order is a decision | Accepted |
| [0038](0038-a-network-request-is-never-a-local-caller.md) | A network request is never a local caller, and a policy refusal is a wire answer | Accepted |
| [0039](0039-a-remote-mcp-call-has-no-run.md) | A remote MCP call has no run, and its enforcement is the same enforcement | Accepted |
| [0040](0040-conformance-is-measured-against-the-protocol-schema.md) | Conformance is measured against the protocol's schema, not against the SDK | Accepted |
| [0041](0041-a-sandbox-guarantee-is-named-and-refused-when-unenforceable.md) | A sandbox guarantee is a named capability that is refused when it cannot be enforced | Accepted |
| [0043](0043-a-decision-carries-its-approver.md) | An approval decision records who answered and through which surface (simplified by 0136) | Accepted |
| [0044](0044-a-deleted-memory-stores-a-hash-of-its-key.md) | A deleted memory stores a hash of its key, a claim triple is three columns, and the stored status reaches the constructor | Accepted |
| [0045](0045-a-correction-is-declared-not-inferred.md) | A correction is declared, because a search key cannot detect one | Accepted |
| [0046](0046-retrieval-filters-then-explains-itself.md) | Retrieval eligibility is a filter, and the ranking's explanation is its arithmetic | Accepted |
| [0047](0047-a-vector-is-compared-with-its-metadata-or-not-at-all.md) | A vector is compared with its metadata or not at all, and the vector is not a number | Accepted |
| [0048](0048-semantic-similarity-is-one-conditional-signal.md) | Semantic similarity is one conditional signal, and an incompatible vector is refused rather than approximated | Accepted |
| [0049](0049-untrusted-content-is-fenced-not-detected.md) | Untrusted content is fenced, and the fence is not a promise the model will obey | Accepted |
| [0050](0050-forgetting-is-a-purge-and-the-receipt-names-what-it-cannot-reach.md) | Forgetting is a purge that writes a tombstone first, and the receipt names what it cannot reach | Accepted |
| [0051](0051-the-query-is-shaped-by-the-index.md) | The query is shaped by the index, and a vector the index cannot hold is refused | Accepted |
| [0052](0052-an-entity-is-read-from-the-store-not-invented.md) | An entity is read from the store, never invented from a request | Accepted |
| [0053](0053-pgvector-indexes-a-partial-cast-expression.md) | pgvector indexes a partial cast expression, and the vector's width is a capability | Accepted |
| [0054](0054-a-connector-is-described-before-it-is-trusted.md) | A connector is described before it is trusted, and its declarations are checked against each other | Accepted |
| [0055](0055-the-authorization-transaction-is-consumable-once.md) | The authorization transaction is consumable once, and the redirect cannot be anywhere but loopback | Accepted |
| [0056](0056-a-checklist-is-evidence-of-a-kind.md) | A checklist item is evidence of a kind, and a scaffold refuses to invent what it cannot know | Accepted |
| [0057](0057-a-declaration-that-cannot-be-made-honestly-is-a-missing-variant.md) | A declaration that cannot be made honestly is a missing variant, not a default value | Accepted |
| [0058](0058-a-provider-decision-belongs-in-the-crate-that-does-not-own-a-socket.md) | A provider decision belongs in the crate that does not own a socket | Accepted |
| [0059](0059-a-tool-definition-is-derived-from-the-connectors-manifest.md) | A tool definition is derived from the connector's manifest, never written beside it | Accepted |
| [0060](0060-a-url-query-is-built-from-encoded-parts.md) | A URL's query is built from encoded parts, and the type has no field for a credential | Accepted |
| [0061](0061-a-credential-cannot-be-rendered-or-serialized.md) | A credential is a type that cannot be rendered, serialized, or reached by accident | Accepted |
| [0062](0062-an-unanswered-request-is-classified-by-whether-it-reached-the-provider.md) | An unanswered request is classified by whether it may have reached the provider | Accepted |
| [0063](0063-a-wire-fixture-declares-whether-it-is-a-capture-or-a-shape.md) | A wire fixture declares whether it is a capture or a shape | Accepted |
| [0064](0064-a-token-answer-is-read-from-its-body-and-its-token-is-never-copied.md) | A token endpoint's answer is read from its body, and the token it grants is never copied into a value | Accepted |
| [0065](0065-a-protocol-type-stays-correct-when-a-provider-disagrees.md) | A protocol type stays correct when a provider disagrees with it | Accepted |
| [0066](0066-a-cursor-decision-takes-a-signal-not-a-status.md) | A cursor decision takes a signal, not a status, because only the caller knows the method | Accepted |
| [0067](0067-a-staleness-signal-needs-a-producer.md) | A staleness signal needs a producer, not only a parameter | Accepted |
| [0068](0068-a-transport-port-is-enforced-by-an-implementation.md) | A transport port is only enforced by an implementation that exists | Accepted |
| [0069](0069-two-tested-halves-do-not-test-the-seam.md) | Two tested halves do not test the seam between them | Accepted |
| [0070](0070-the-callback-decoder-is-not-the-request-encoder.md) | The callback decoder's encoding is the opposite of the request encoder's | Accepted |
| [0071](0071-one-form-codec-both-directions.md) | One codec, both directions, because the rule is narrower than RFC 3986 | Accepted |
| [0072](0072-a-value-is-encoded-where-it-is-rendered.md) | A value is encoded where it is rendered, never where it is stored | Accepted |
| [0073](0073-a-retry-safety-variant-needs-a-producer.md) | A retry-safety variant needs a producer, and the token exchange is it | Accepted |
| [0074](0074-a-doc-naming-another-module-is-a-claim.md) | A doc naming another module is a claim about code, and the code never went there | Accepted |
| [0075](0075-a-classification-table-with-no-caller.md) | A classification table with no caller decides nothing | Accepted |
| [0076](0076-retry-after-is-three-situations-not-two.md) | `Retry-After` is three situations, not two | Accepted |
| [0077](0077-a-bound-that-is-documented-but-not-applied.md) | A bound that is documented but not applied is not a bound | Accepted |
| [0078](0078-a-scope-category-is-a-review-burden.md) | A scope's category is a review burden, and an unlisted scope is not a cheap one | Accepted |
| [0079](0079-a-rate-limit-without-its-unit.md) | A rate limit without its unit is a figure read as the wrong thing | Accepted |
| [0080](0080-a-limit-and-a-recommendation-are-two-facts.md) | A limit and a recommendation are two facts, and a figure with no source is neither | Accepted |
| [0081](0081-calendars-410-needs-its-reason.md) | Calendar's 410 is unambiguous only once the reason is read | Accepted |
| [0082](0082-one-classifier-for-two-apis.md) | One classifier for two APIs answered for the less informative one | Accepted |
| [0083](0083-a-declared-field-is-bounded-by-what-can-return-it.md) | A declared output field is bounded by what the request can return | Accepted |
| [0084](0084-an-argument-pair-the-provider-forbids.md) | An argument pair the provider forbids is refused before it is sent | Accepted |
| [0085](0085-a-rendered-token-needs-an-input-that-consume-it.md) | A token the output renders needs an input that can consume it | Accepted |
| [0086](0086-a-refusal-names-the-argument-the-caller-sent.md) | A refusal names the argument the caller sent, and a declared bound equals the one enforced | Accepted |
| [0087](0087-a-lease-that-lapses-silently.md) | A lease that lapses silently needs a code that can say so | Accepted |
| [0088](0088-a-field-google-declares-two-encodings-for.md) | A field Google declares two encodings for | Accepted |
| [0089](0089-a-rule-from-another-context.md) | A rule from another context, and the observable axis | Accepted |
| [0090](0090-a-refusal-keeps-the-cursor.md) | A refusal keeps the cursor, because the position was never rejected | Accepted |
| [0091](0091-a-value-redacted-in-one-place.md) | A value redacted in one place and printed in another | Accepted |
| [0092](0092-a-response-field-with-no-reader.md) | A response field with no reader, and a worked example that uses two numbers | Accepted |
| [0093](0093-a-request-the-provider-accepts-and-ignores.md) | A request the provider accepts and silently ignores | Accepted |
| [0094](0094-a-negative-acknowledgement-is-charged-to-the-subscription.md) | A negative acknowledgement is charged to the subscription | Accepted |
| [0095](0095-stopping-notifications-needs-the-grant-revoking-destroys.md) | Stopping notifications needs the grant that revoking destroys | Accepted |
| [0096](0096-a-requirement-with-no-consumer.md) | A requirement with no consumer, and a scope justified by the wrong operation | Accepted |
| [0097](0097-a-delivery-names-a-mailbox-and-nothing-mapped-it.md) | A delivery names a mailbox, and nothing mapped it to an account | Accepted |
| [0098](0098-one-address-one-account.md) | One address, one account — and the opposite direction from routing | Accepted |
| [0099](0099-a-delivery-can-be-authenticated-without-covering-the-body.md) | A delivery can be authenticated without covering the body | Accepted |
| [0100](0100-a-delivery-is-not-always-a-change.md) | A delivery is not always a change | Accepted |
| [0101](0101-a-missing-control-and-a-failed-one-are-not-the-same-answer.md) | A missing control and a failed one are not the same answer | Accepted |
| [0102](0102-two-pushes-routed-on-keys-of-opposite-provenance.md) | Two pushes routed on keys of opposite provenance | Accepted |
| [0103](0103-composing-the-push-path-is-what-decides.md) | Composing the push path is what decides, and the seam changed a type | Accepted |
| [0104](0104-a-cause-is-not-a-marker.md) | A cause is not a marker, and the two pushes do not share a state machine | Accepted |
| [0105](0105-the-documented-remedy-is-two-steps.md) | The documented remedy is two steps, and only the second was a type | Accepted |
| [0106](0106-the-same-expiry-in-two-encodings.md) | The same expiry in two encodings, and renewal is a replacement | Accepted |
| [0107](0107-a-teardown-whose-arity-the-provider-decides.md) | A teardown step whose arity the provider decides, and a value read for a consumer that did not exist | Accepted |
| [0108](0108-a-rule-stated-in-another-fields-description.md) | A rule stated in another field's description, and a cursor the schema told a model to store | Accepted |
| [0109](0109-a-constraint-three-layers-knew-and-the-test-contradicted.md) | A constraint three layers knew and the test contradicted, and a guard whose only detector was in another binary | Accepted |
| [0110](0110-a-first-sync-with-two-documented-branches.md) | A first sync with two documented branches and one expressible, and an anchor with a reader and no consumer | Accepted |
| [0111](0111-a-bound-and-a-lease-are-different-inputs.md) | A bound and a lease are different inputs, so the exposure that took one could not report the state the other reaches | Accepted |
| [0112](0112-a-prescribed-call-with-no-builder.md) | A prescribed call with no builder, and a body that made a request's own doc false | Accepted |
| [0113](0113-the-value-that-survives-is-the-one-that-did-not-hold-it.md) | The value that survives the `watch` is the one the renewal needs, and it was the one that did not hold it | Accepted |
| [0114](0114-a-path-identifier-validated-but-not-encoded.md) | A path identifier that was validated but not encoded, and the asymmetry that hid it | Accepted |
| [0115](0115-one-acknowledgement-vocabulary.md) | One mechanism's acknowledgement was a decision and its twin's was a bool that was always true | Accepted |
| [0116](0116-a-field-with-a-producer-and-no-reader.md) | A field with a producer and no reader, and the two values that had to agree | Accepted |
| [0117](0117-a-skill-is-a-procedure-not-a-permission.md) | A skill is a procedure, not a permission | Accepted |
| [0118](0118-a-diagnostic-that-contradicts-the-predicate.md) | A diagnostic that contradicts the predicate the platform gates on | Accepted |
| [0119](0119-the-model-may-request-a-tool-and-rust-decides.md) | The model may request a tool; deterministic policy still decides whether it runs | Accepted |
| [0120](0120-a-decided-approval-continues-the-run.md) | A decided approval continues the run that parked on it | Accepted |
| [0121](0121-a-providers-credential-is-a-file-path-not-a-value.md) | A provider's credential is a file path, and a partial provider is refused rather than guessed | Accepted |
| [0122](0122-an-approval-override-can-only-tighten.md) | An operator's approval override can only tighten, and the risk vocabulary moved to core | Accepted |
| [0123](0123-the-control-plane-reads-the-policy-in-force.md) | The control plane reads the policy in force, and a preview is a decision not a prediction | Accepted |
| [0124](0124-memory-admission-is-a-column-pair.md) | A memory's admission is a column pair, and its cross-column rule lives in the decode | Accepted |
| [0125](0125-a-summary-is-a-memory-plus-a-span.md) | A session summary is a memory plus a span table, and its retention is an explicit verb | Accepted |
| [0126](0126-an-entity-is-its-own-surface.md) | An entity is its own surface, and identity is not trust | Accepted |
| [0127](0127-the-naming-strategy-is-the-documents.md) | The naming strategy is the document's, not the caller's | Accepted |
| [0128](0128-a-container-backend-confines-without-ffi.md) | A container backend confines without FFI, and claims three of five guarantees | Accepted |
| [0129](0129-a-fetch-is-checked-on-the-address-it-connects-to.md) | A fetch is checked on the address it connects to, and a model-chosen URL was held for a person by default (amended by 0133) | Accepted |
| [0130](0130-a-pending-approval-holds-what-it-is-waiting-on.md) | A pending approval holds what it is waiting on, so a person can decide it (amends 0013) | Accepted |
| [0131](0131-model-authored-code-runs-in-a-disposable-container.md) | Model-authored code runs in a disposable container, and is held for a person unless the owner trusts it (amended by 0133) | Accepted |
| [0132](0132-a-schedule-is-a-request-to-start-an-ordinary-run.md) | A schedule is a request to start an ordinary run, fired at most once and never piled up | Accepted |
| [0133](0133-approval-is-for-what-can-hurt-and-the-owner-can-decide-once.md) | Approval is for what can hurt, and the owner can decide once (amends 0129, 0131) | Accepted |
| [0134](0134-a-sub-agent-is-an-ordinary-run-one-level-deep.md) | A sub-agent is an ordinary run, one level deep | Accepted |
| [0135](0135-the-console-is-a-static-page-the-daemon-serves.md) | The console is a static page the daemon serves, and it is where you talk, watch, answer and stop | Accepted |
| [0136](0136-an-approval-is-a-yes-or-no-from-the-owner.md) | An approval is a yes or no from the owner | Accepted |
| [0137](0137-file-writes-create-or-append-an-edit-replaces-one-exact-text.md) | File writes create or append; an edit replaces one exact text | Accepted |
| [0138](0138-the-voice-is-synthesized-by-the-daemon.md) | The voice is synthesized by the daemon | Accepted |
| [0139](0139-settings-are-changed-by-the-owner-through-one-validated-path.md) | Settings are changed by the owner, through one validated path | Accepted |
| [0140](0140-remembering-is-conversational-and-defaults-to-the-owner.md) | Remembering is conversational, and a claim with no subject is about the owner | Accepted |
| [0141](0141-the-console-is-a-face-with-a-conversation.md) | The console is a face with a conversation; operations are their own page | Accepted |
| [0142](0142-a-run-is-bounded-by-progress-and-by-you.md) | A run is bounded by progress and by you, not by small counts | Accepted |
| [0143](0143-a-command-tool-and-a-run-that-remembers-its-turn.md) | JARVIS can run a command in your folder, and a run that waits for you remembers what it was doing | Accepted |
| [0144](0144-one-executable-the-daemon-is-jarvis-daemon.md) | One executable: the daemon is `jarvis daemon` | Accepted |
| [0145](0145-a-tool-permission-changes-at-once-and-jarvis-can-schedule.md) | A tool's permission changes at once, and JARVIS can schedule work from a conversation | Accepted |
| [0146](0146-reasoning-effort-notifications-and-web-search.md) | A default reasoning effort, desktop notifications for scheduled results, and web search | Accepted |
| [0147](0147-install-itself-and-search-memory.md) | JARVIS installs itself, and the model can search its memory | Accepted |
| [0148](0148-the-model-can-search-files.md) | The model can search the text of the files in a granted folder | Accepted |
| [0149](0149-sign-in-with-google-and-read-mail-and-calendar.md) | Sign in with Google from Settings, and read mail and calendar | Accepted |
| [0150](0150-google-actions-opt-in-and-a-stop-that-does-not-throw-work-away.md) | Sending mail and creating events is opt-in and always asked, and a stop does not silently throw work away | Accepted |
| [0151](0151-a-project-tells-a-run-what-it-is-for-and-never-what-it-may-do.md) | A project tells a run what it is for, and never what it may do | Accepted |
| [0152](0152-the-console-shows-what-jarvis-is-doing-with-the-links-it-found.md) | The console shows what JARVIS is doing as it happens, with the links it found | Accepted |
| [0153](0153-jarvis-can-create-projects-and-keep-its-memory-straight.md) | JARVIS can create and change projects itself, and keep its memory straight, with the owner's yes | Accepted |
| [0154](0154-the-face-stands-alone-and-the-work-stays-on-screen.md) | The face stands alone, and what JARVIS did stays on screen as a constellation | Accepted |
| [0155](0155-restarts-keep-work-the-owner-hears-and-leads-live-in-rows.md) | A restart no longer drops work, the owner hears when it matters, and prospect data lives in rows | Accepted |
| [0156](0156-rate-limits-are-waited-out-and-do-not-contact-is-enforced.md) | A run waits out a provider rate limit, and the Gmail send tool honours do_not_contact | Accepted |
| [0157](0157-email-can-carry-files-answer-in-thread-and-save-attachments.md) | Email can carry files, answer in its thread, and save what arrives | Accepted |
| [0158](0158-jarvis-reads-pdf-word-spreadsheet-and-slide-files.md) | JARVIS reads PDF, Word, spreadsheet and slide files | Accepted |
| [0159](0159-files-can-be-moved-and-a-limited-model-can-be-swapped.md) | Files can be moved, and a limited model can be swapped for another | Accepted |
| [0160](0160-web-pages-are-read-in-parts-and-pdfs-and-word-notes-are-read.md) | Web pages are read in parts, PDFs on the web are read, and Word notes are read | Accepted |
| [0161](0161-an-email-can-be-saved-as-a-draft-for-the-owner-to-review.md) | An email can be saved as a draft for the owner to review | Accepted |
| [0162](0162-pictures-and-scanned-pdfs-are-read-by-the-owners-tesseract.md) | Pictures and scanned PDFs are read by the owner's tesseract | Accepted |
| [0163](0163-the-fallback-model-may-be-at-a-different-provider.md) | The fallback model may be at a different provider | Accepted |
| [0164](0164-the-brain-settings-choose-a-provider-and-a-model-from-a-list.md) | The Brain settings choose a provider and a model from a list | Accepted |
| [0165](0165-a-project-can-cap-the-tokens-its-scheduled-work-uses-in-a-day.md) | A project can cap the tokens its scheduled work uses in a day | Accepted |

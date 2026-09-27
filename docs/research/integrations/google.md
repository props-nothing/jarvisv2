---
integration: google
status: researched
last_verified: 2026-09-27
owners: []
selected_spec_version: "Gmail API v1; Calendar API v3 (both unversioned REST revisions, last updated 2026-09-03..18 per page footers)"
selected_sdk: none selected — this record assumes direct HTTP and did NOT check Google's client-library list for a Rust SDK
---

# Google Workspace: identity, Gmail, Calendar, push notifications, quotas, restricted scopes

## Scope

**In scope:** the facts `P5-005` needs to build and operate a Google connector — OAuth identity and the scope
taxonomy, the Gmail and Calendar read operations a first connector would expose, Gmail push notifications via
Cloud Pub/Sub and Calendar push notifications via channels, the incremental-sync mechanisms and their staleness
signals, quota and rate-limit values, the error taxonomy, and the verification burden restricted scopes carry.

**Out of scope:** write and send operations beyond what is needed to classify them, Drive, Contacts, Tasks,
Meet, Chat, the admin SDK, and every Google Cloud product other than Pub/Sub. `P5-009` owns write operations
"behind policy and approval"; this record only records what it must know about Gmail's sending semantics.

**This record does not implement anything.** `P5-004` is a research task. `P5-005` is where a connector exists.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| Google Workspace `llms.txt` | **`not found`** — `https://developers.google.com/llms.txt` and `https://developers.google.com/gmail/api/llms.txt` both returned **HTTP 404** | 2026-09-27 | discovery — recorded as absent, not invented |
| Gmail API overview | https://developers.google.com/workspace/gmail/api/guides (page footer: last updated **2026-09-18**) | 2026-09-27 | the resource model and vocabulary |
| Gmail push notifications | https://developers.google.com/workspace/gmail/api/guides/push (last updated **2026-09-15**) | 2026-09-27 | `users.watch`, the Pub/Sub handshake, `history.list`, the notification envelope, rate and reliability limits |
| Gmail sync | https://developers.google.com/workspace/gmail/api/guides/sync (last updated **2026-09-15**) | 2026-09-27 | full vs partial sync, `historyId`, the failure signal |
| Gmail usage limits | https://developers.google.com/workspace/gmail/api/reference/quota (last updated **2026-09-10**) | 2026-09-27 | quota units, per-method costs, the **May 2026 model change**, the daily billing threshold |
| Gmail scopes | https://developers.google.com/workspace/gmail/api/auth/scopes (last updated **2026-09-10**) | 2026-09-27 | the non-sensitive / sensitive / restricted taxonomy |
| Gmail error handling | https://developers.google.com/workspace/gmail/api/guides/handle-errors (last updated **2026-09-15**) | 2026-09-27 | the status and `reason` taxonomy, backoff guidance |
| Gmail MCP server reference | https://developers.google.com/workspace/gmail/api/reference/mcp (last updated **2026-07-21**) | 2026-09-27 | the first-party MCP endpoint and its toolset |
| Calendar push notifications | https://developers.google.com/workspace/calendar/api/guides/push (last updated **2026-09-11**) | 2026-09-27 | channel creation, the `X-Goog-*` headers, the sync message, renewal, delivery semantics |
| Calendar sync | https://developers.google.com/workspace/calendar/api/guides/sync (last updated **2026-09-11**) | 2026-09-27 | `nextSyncToken`, the `410 Gone` signal |
| Calendar scopes | https://developers.google.com/workspace/calendar/api/auth (last updated **2026-09-03**) | 2026-09-27 | the Calendar scope list |
| OAuth consent and scope categories | https://developers.google.com/workspace/guides/configure-oauth-consent (last updated **2026-09-03**) | 2026-09-27 | which review each scope category requires |
| Cloud Pub/Sub push authentication | https://docs.cloud.google.com/pubsub/docs/authenticate-push-subscriptions (last updated **2026-09-24**) | 2026-09-27 | **the JWT-bearer mechanism** — see Finding 1 |
| Cloud Pub/Sub push subscriptions | https://cloud.google.com/pubsub/docs/push | 2026-09-27 | the envelope and the acknowledgement rule |
| Google OIDC discovery document | https://accounts.google.com/.well-known/openid-configuration | 2026-09-27 | **machine-readable**: every endpoint, the PKCE methods, and whether the `iss` response parameter is supported |
| Google OAuth 2.0 for native apps | https://developers.google.com/identity/protocols/oauth2/native-app (last updated **2026-09-14**) | 2026-09-27 | the installed-app flow: the loopback method, the token and refresh exchanges, the response fields, DPoP, revocation |

The shared OAuth protocol facts — PKCE, loopback redirects, rotation, revocation — are recorded once in
[oauth2-pkce-native-apps.md](oauth2-pkce-native-apps.md) and are not restated here.

## Verified Contract

### Operations And Transport

Both APIs are REST over HTTPS with JSON bodies, at `https://www.googleapis.com/gmail/v1/...` and
`https://www.googleapis.com/calendar/v3/...`. Both use `me` / `primary` as self-referential path segments. Both
paginate with `pageToken`/`nextPageToken` and a server-chosen `maxResults` (Gmail's is capped at 500).

**Read operations a first connector would expose**, with their quota costs:

| Operation | Endpoint | Units |
| --- | --- | --- |
| Profile / account identity | `users.getProfile` | 1 |
| List message ids | `users.messages.list` | 5 |
| Get a message | `users.messages.get` | 20 |
| Get an attachment | `users.messages.attachments.get` | 20 |
| List threads / get a thread | `users.threads.list` / `.get` | 10 / 40 |
| List labels | `users.labels.list` | 1 |
| History (incremental) | `users.history.list` | 2 |
| Watch (push) | `users.watch` | 100 |
| Stop push | `users.stop` | 50 |
| Calendar list / events list | `calendarList.list`, `events.list` | not published on this page |
| Calendar events watch | `events.watch` | not published on this page |

Write operations, recorded only so `P5-009` can classify them: `messages.send` **100**, `drafts.send` **100**,
`messages.insert` 25, `drafts.create` 10, `messages.modify` 5, `messages.trash` 20, `messages.delete` 10,
`messages.batchModify` and `messages.batchDelete` **50 each**. `messages.get` costs 20 units versus
`messages.list` at 5, so a full sync of *N* messages costs roughly `5 + 20N` units — **the single most
important number for sizing a first sync**, and the reason a connector must batch (see Limits).

### Authentication And Authorization

- **OAuth 2.0 Authorization Code.** No Google-specific deviation from the shared record; Google supports PKCE
  and native-app loopback redirects, and the scope set is what changes between calls. A Google connector
  therefore reuses `P5-002`'s transaction unchanged.
- **Scope taxonomy is a review burden, not just a string.** Every scope is non-sensitive, sensitive, or
  restricted, and the category decides the verification:

  | Category | Requirement |
  | --- | --- |
  | Non-sensitive | basic app verification |
  | Sensitive | **additional** app verification |
  | Restricted | additional verification **and**, if the data is stored or transmitted through a server, **a security assessment** |

- **Gmail's categories are counter-intuitive.** `gmail.labels` is the only generally useful *non-sensitive*
  scope. `gmail.send` is **sensitive**. And every read scope a mail connector wants is **restricted**:
  `gmail.readonly`, `gmail.metadata`, `gmail.modify`, `gmail.compose`, `gmail.insert`, `mail.google.com/`,
  `gmail.settings.basic`, `gmail.settings.sharing`.

  **`gmail.metadata` is restricted too**, which matters because it is the *least* privileged way to read a
  mailbox (labels and headers, not bodies) and an author would reasonably assume it is the cheap option. It is
  not: it still needs the assessment. `mail.google.com/` is the only scope that permits permanent deletion
  bypassing the trash, and the documentation advises requesting it only for that.
- **Calendar** publishes a flat scope list without per-scope categories on the page fetched;
  `calendar.readonly` and `calendar.events.readonly` are the read scopes, and the *consent* page's category
  table implies the read scopes are sensitive-or-restricted. **Recorded as a gap:** the record does not claim a
  category for a Calendar scope, because the page it was read from does not state one. See Unresolved Question 2.
- **The internal-app exemption.** For an app used only inside one Google Workspace organization, "scopes aren't
  listed on the consent screen and use of restricted or sensitive scopes doesn't require further review by
  Google". This is the single fact that decides whether a personal JARVIS deployment needs the security
  assessment at all — and it is about the *consent screen's audience setting*, not about scope choice.

#### The endpoint set, read from the discovery document

The values below come from `https://accounts.google.com/.well-known/openid-configuration`, which is the one
source here that is **machine-readable** rather than prose — so these are not the documentation's examples but
the server's own published configuration.

| Field | Value |
| --- | --- |
| `issuer` | `https://accounts.google.com` |
| `authorization_endpoint` | `https://accounts.google.com/o/oauth2/v2/auth` |
| `token_endpoint` | `https://oauth2.googleapis.com/token` |
| `revocation_endpoint` | `https://oauth2.googleapis.com/revoke` |
| `userinfo_endpoint` | `https://openidconnect.googleapis.com/v1/userinfo` |
| `device_authorization_endpoint` | `https://oauth2.googleapis.com/device/code` |
| `jwks_uri` | `https://www.googleapis.com/oauth2/v3/certs` |
| `code_challenge_methods_supported` | `["plain", "S256"]` |
| `id_token_signing_alg_values_supported` | `["RS256"]` |
| `authorization_response_iss_parameter_supported` | **`true`** |
| `token_endpoint_auth_methods_supported` | `client_secret_post`, `client_secret_basic`, and more that appeared **truncated in capture** |
| `grant_types_supported` | `authorization_code`, `refresh_token`, the device-code URN, and the JWT-bearer URN |
| `subject_types_supported` | `public` |

- **The authorization, token and revocation endpoints in the connector are transcribed from here**, which is
  what makes them checkable: `the_authorization_endpoint_matches_the_discovery_document` asserts the constant
  against these values, so a future change is a deliberate edit rather than a silent drift.
- **`authorization_response_iss_parameter_supported: true`** is the fact that decides whether
  `AuthorizationTransaction::with_issuer` has anything to check. `P5-002` stores the issuer and reports
  `MixUpDefence::IssuerConfirmed` only when the response carries `iss`; Google supporting the parameter means
  the confirmation is available rather than perpetually `NotSatisfied`.
- **`plain` is offered as a code-challenge method**, which is the downgrade `P5-002` refuses to leave to a
  server's defaulting: `AuthenticationTransaction::parameters` always sends `code_challenge_method` even for
  `S256`, so a server reading an omission as `plain` cannot weaken the proof.
- **The arrays marked truncated were truncated in the capture**, and that is recorded rather than glossed: the
  record does **not** claim a complete `token_endpoint_auth_methods_supported` list, so nothing here concludes
  whether `none` (a public client's auth method) is advertised. It does not need to: `P5-002`'s flow is the
  authorization-code grant with PKCE, and the token endpoint's authentication method is what a *confidential*
  client uses.

#### The loopback redirect, and the exchange's own contract

- **Loopback is the recommended method for macOS, Linux and Windows desktop**, from the page's own table: "if
  your platform supports it, this is the recommended mechanism for obtaining the authorization code". The
  application type is set to **Desktop app**.
- **`localhost` is permitted but discouraged by the provider too**: "It is also possible to use `localhost` in
  place of the loopback IP, but this configuration may cause issues with client firewalls." `P5-002` refuses
  `localhost` for its own reasons (RFC 8252 §8.3), and Google's advice points the same way — two independent
  reasons for one refusal.
- **Custom URI schemes are no longer supported**, "due to the risk of app impersonation", and the OOB
  copy/paste method is deprecated. So the loopback redirect is not merely recommended but the only supported
  non-embedded option for a desktop client.
- **`redirect_uri` "must exactly match one of the authorized redirect URIs"**, and a mismatch is the
  `redirect_uri_mismatch` error. The page's own exchange example uses a **ported** form
  (`redirect_uri=http://127.0.0.1:9004`). **What the Cloud Console accepts as the *registered* value is not
  stated on this page** — see Unresolved Question 7.
- **`client_secret` is `Optional`** on both the code exchange and the refresh, and the refresh section adds
  "not applicable to requests from clients registered as Android, iOS, or Chrome applications". So a public
  native client sends neither, which is what the manifest's **empty `secret_fields`** relies on.
- **The token response's fields:** `access_token`, `expires_in`, `id_token` (**only when an identity scope such
  as `openid`, `profile` or `email` was requested**), `refresh_token` ("always returned for installed
  applications"), `refresh_token_expires_in` (only for a time-based access grant), `scope` (space-delimited,
  case-sensitive — so the granted scopes are checkable and may be **fewer** than requested), and `token_type`
  ("always `Bearer`", even under DPoP).
- **An `id_token` is therefore expected on this connector's exchange**, because the manifest requests `openid`.
  `P5-002` records that `nonce` is carried but **not validated**, which needs JWKS verification against
  `jwks_uri`; the discovery document supplies that URI, and the verification is still unbuilt.
- **Refresh-token issuance is limited, and the consequence is that older tokens stop working**: "one limit per
  client/user combination, and another per user across all clients … If your application requests too many
  refresh tokens, it may run into these limits, in which case older refresh tokens will stop working." A
  deployment that re-authorizes repeatedly can silently invalidate the token it was relying on.
- **Refreshing does not return a new refresh token** in the documented sample, so `P5-002`'s rotation detection
  (`has_refresh_token`) reports `false` and no rotation here is expected. Rotation remains correct to detect: a
  provider that *does* return one would otherwise go unnoticed.
- **Revocation is `https://oauth2.googleapis.com/revoke` with the token as a parameter**, HTTP 200 on success and
  400 on error, and "the token can be an access token or a refresh token. If the token is an access token and it
  has a corresponding refresh token, the refresh token will also be revoked".
- **⚠⚠ THE FINDING THAT DECIDES HOW A DISCONNECT MUST BE REPORTED: Google's revocation is NOT RFC 7009's, and
  `RevocationKind::requires_reauth_afterwards()` IS WRONG FOR THIS PROVIDER.** RFC 7009 has `token_type_hint`
  select *which token* is revoked, so revoking the access token leaves the grant intact — and
  `RevocationKind::AccessToken.requires_reauth_afterwards()` returns `false` on exactly that assumption. Google's
  own page (last updated **2026-09-14**) says the opposite in two places: revoking an access token **also
  revokes the paired refresh token**, and revocation "removes **all** OAuth 2.0 scopes previously granted to a
  **project**, invalidating any issued access or refresh tokens for **all clients registered under that
  project**". So on Google no revocation kind leaves the account usable, and a connector that trusted the shared
  `false` would tell a user their connection was disconnected-but-authorised when the account in fact needs a new
  consent. The shared type is **correct about the protocol** and is therefore **not changed**; the provider's
  answer lives in `google::revocation::effect_of`, and a test **asserts the divergence** so that a change to
  either side fails rather than silently re-opening the gap.
- **⚠ The blast radius is wider than one account, and it is not immediate.** The documented unit is the
  **project**, so a connector that revokes to disconnect *one* account must not assume the effect is limited to
  that account's grant — it can withdraw grants belonging to other accounts and other clients in the same Cloud
  project. And "it might take some time before the revocation has full effect", so a `200` means **accepted**
  rather than **in force**: a caller must not treat it as proof that a concurrent call will now fail, and a test
  that asserted it would be flaky against the provider itself.
- **A `200` also covers "the client submitted an invalid token"** (RFC 7009 §2.2, which this endpoint follows),
  so the status carries **no signal about whether the token was live**. `RevocationOutcome::AlreadyInvalid`
  therefore cannot be derived from a response and is not invented from one.
- **DPoP is optional and recommended, and it changes the key-material obligation rather than a parameter.** A
  successful token request with a `DPoP` header binds the **refresh token** to the key while access tokens keep
  `token_type: Bearer`. For a code exchange the `jti` must be `BASE64URL(SHA256(AUTHORIZATION_CODE))`, and a
  refresh needs a unique per-request `jti`. A missing or unacceptable nonce yields `use_dpop_nonce` with a fresh
  `DPoP-Nonce` header to cache. The documentation recommends storing the private key "in a way that it cannot be
  copied off-device, for example by using TPMs, Secure Enclaves, or other hardware-backed keystores".

  **So DPoP is a decision, not a default, and the reason is not the extra header.** It requires JARVIS to hold a
  non-exportable signing key and to sign every token request, which is exactly the sender-constraining `P5-002`
  records as **not implemented** — and its own ADR calls that the strongest argument for the work. Bypassing it
  is safe (DPoP is optional); adopting it is a slice, not a flag.

### Limits And Failure Semantics

**Quota model, changed 1 May 2026.** This is the most operationally consequential part of the record and the
part most likely to be stale in any secondary source:

| Limit | Value |
| --- | --- |
| Per minute per project | 1,200,000 quota units |
| Per minute per user per project | 6,000 quota units |
| Per day per project (daily billing threshold) | 80,000,000 quota units |

- **Projects created on or after 1 May 2026 are subject to the new quotas; projects that used the API between
  November 2025 and April 2026 keep their previously-set quotas.** So two installations of the same connector
  can have different limits depending on the age of the Cloud project, and the connector cannot assume either.
- Exceeding the daily threshold is currently free but **"planned to incur charges to your Google Cloud billing
  account later in 2026"**, with at least 90 days' notice, and the threshold **cannot be raised**. A connector
  therefore has a hard ceiling per project per day that no quota request can lift.
- **Per-user limits cannot be increased at all** ("You can't increase per-user limits").
- **A service account is one user** for quota purposes: "API calls by a service account are considered to be
  using a single accounting". So a domain-wide-delegation connector reading 50 mailboxes pays the *per-user*
  6,000 units/minute as if it were one mailbox, which is a hard throughput ceiling rather than a per-mailbox one.

**Other limits:**

- **500 recipients per email message.**
- **Batch requests: no more than 50**, and "larger batch sizes can trigger rate limiting" — so batching is
  required for a full sync *and* is itself a rate-limit trigger.
- **Gmail push: a maximum notification rate of one event per second per watched user; "The service drops any
  user notifications exceeding that rate."** Dropping is silent, not a signal.
- **Gmail push reliability: "in rare situations, notifications might be delayed or dropped."** The documented
  remedy is a periodic `history.list` fallback after a quiet period.
- **Calendar push reliability: "Notifications are not 100% reliable. Expect a small percentage of messages to
  get dropped under normal working conditions."**
- Gmail history records are "typically available for at least one week and often longer… the time period
  available might be significantly shorter".

**Error taxonomy** — 403 and 429 carry different meanings and need different handling:

| Status | `reason` | Meaning | Remedy |
| --- | --- | --- | --- |
| 401 | `authError` | expired/invalid token **or missing scopes** | refresh; if that fails, reauthorize |
| 403 | `dailyLimitExceeded` | project quota reached | raise the project quota; **not** retryable in-loop |
| 403 | `rateLimitExceeded` | project request rate | backoff |
| 403 | `userRateLimitExceeded` | per-user rate | backoff, or fewer requests |
| 403 | `domainPolicy` | the domain admin disabled the app | surface to the user; **retrying never helps** |
| 429 | — | mail-sending limit, bandwidth limit, **or** per-user concurrency | backoff; the response carries a retry time |
| 429 | — | daily limit exceeded | "might result in these errors for multiple hours" |
| 5xx | `backendError` | unexpected server error | exponential backoff from **at least one second** |

- **`domainPolicy` is a permanent, user-actionable refusal that looks like a rate limit** if a connector only
  switches on the status code. It must be classified from `reason`.
- **429 conflates three unrelated causes** (sending quota, bandwidth, concurrency), and the documented remedy
  for all three is "retry or split across multiple Gmail accounts" — which for a connector means backoff, since
  JARVIS will not be spreading one account's work over several.
- **Backoff guidance is explicit**: start at ≥1 second, `min(2^n + random(0..1000ms), max_backoff)`, with
  `max_backoff` "typically 32 or 64 seconds", and stop retrying at some point.

### Data And Compliance

- **Restricted-scope data stored on or transmitted through a server requires a Google security assessment.**
  For a Gmail read connector this is not optional and not a formality; it is an external review with a
  turnaround, and it is the largest non-engineering cost in this record.
- Gmail message content is at minimum `Confidential` and, for health/financial/legal mail, `Restricted` under
  JARVIS's own vocabulary. **A connector cannot classify per-message in the manifest** — the manifest declares
  the connector's ceiling — so a Gmail connector declares at the connector level and must not lower it for a
  particular call.
- Google's own MCP server for Gmail exists (Finding 3), which changes the data-flow question: the model-facing
  tool calls would go to Google's endpoint rather than to JARVIS's connector code.

### Versions And Deprecations

- Neither API publishes a versioned revision the way MCP does; the pages carry "last updated" dates, the newest
  of which is **2026-09-18**. There is no changelog page for either API in the sources fetched.
- **The quota model changed on 2026-05-01** and a pricing change is announced as pending. Both are dated facts
  in this record rather than assumptions.
- **The first-party Gmail MCP server is in Developer Preview** (Workspace Developer Preview Program: "grants
  early access to certain features"). Preview status means no stability guarantee — recorded, not glossed.
- The Gmail `watch` method must be re-called at least every 7 days or updates stop, so a connector that treats
  a watch as permanent will silently stop receiving push and must have a polling fallback anyway.

## JARVIS Mapping

| Google concept | JARVIS type | Note |
| --- | --- | --- |
| Gmail account identity | `VerifiedAccount` from `users.getProfile` | `emailAddress` is provider-verified, not user-entered — satisfies `tools-and-connectors.md`'s "account identity verified from the provider" |
| Gmail `historyId` | `SyncCursorKind::MonotonicMarker` | numeric and monotonically increasing |
| Calendar `nextSyncToken` | `SyncCursorKind::OpaqueToken` | base64-ish, opaque |
| **Gmail stale cursor (404)** | `CursorError` → require full resync | see Finding 2 — a *404 on a read*, not a dedicated code |
| **Calendar stale cursor (410)** | `CursorError` → require full resync | see Finding 2 |
| Gmail push (Pub/Sub) | **`WebhookSupport::Push` — cannot be expressed today** | Finding 1 |
| Calendar push (channels) | **`WebhookSupport::Push` — cannot be expressed today** | Finding 1 |
| `gmail.readonly` etc. | connector **OAuth** scopes, not JARVIS `Scope`s | `P3-001` draws this line explicitly: a JARVIS scope is `resource.action` and a provider scope is the connector's business |
| Quota units | `RateLimit { per_window, window_seconds, scope }` | the *project* limits are `Project`-scoped and the *user* limits per-account; a connector must declare both, and the daily threshold is a third |
| `reason` codes | `RetryClass` | `domainPolicy` → `Permanent`; `rateLimitExceeded`/`userRateLimitExceeded` → `Throttled`; 5xx → `Transient`; 429 → `Throttled` with the stated delay |

## Decisions

**Nothing is decided here.** This slice records external facts; `P5-005` decides what to build. The three
findings below are recorded as findings, and Findings 1 and 3 are named as *blocking* questions rather than
quietly resolved, because both would change a contract rather than a call site.

## Findings That Affect Slices Other Than `P5-005`

### Finding 1 — Neither Google push mechanism fits `WebhookSupport::Push`, which is HMAC-over-body only

`ADR-0054` defined `SignatureScheme { algorithm, header, encoding }` with `SignatureAlgorithm::HmacSha256` as
the algorithm, and `WebhookDelivery`/`SingleHeader` semantics built around verifying a MAC over the **raw
body**. Google's two push mechanisms are neither:

**Gmail (via Cloud Pub/Sub)** authenticates with a **Bearer JWT in the `Authorization` header**:

- The Pub/Sub service signs a JWT with **RS256** and sends it in `Authorization: Bearer <jwt>`.
- Verification means checking the **signature** (against Google's rotating certificates, or by calling
  `https://oauth2.googleapis.com/tokeninfo?id_token=<token>`) and then checking that the **`email` and `aud`
  claims match what the subscription was configured with**.
- **The body is not signed.** There is no MAC, no `X-...-Signature` header, and therefore nothing our
  `SignatureScheme` can name.
- Tokens "may be up to an hour old", so the mechanism is not replay-proof on its own; the documented controls
  are signature validity plus the audience/email match.

**Calendar (via notification channels)** has **no request signature at all**. It sends `X-Goog-*` headers:

- `X-Goog-Channel-ID` — the channel id the client chose, echoed back.
- `X-Goog-Channel-Token` — **an arbitrary client-set string, echoed back**, presented by the documentation as
  what you use "to verify that each incoming message is for a channel that your application created—to ensure
  that the notification is not being spoofed". That is a **shared-secret-in-a-header comparison**, not a MAC.
- `X-Goog-Resource-State` (`sync` | `exists` | `not_exists`), `X-Goog-Resource-ID`, `X-Goog-Resource-URI`,
  `X-Goog-Message-Number` (monotonic, non-sequential, always `1` for the `sync` message),
  `X-Goog-Channel-Expiration`.
- The message **has no body** at all (`Content-Length: 0`), so there is nothing to sign even in principle.
- The `sync` message can arrive **before the `watch` response**, so a connector cannot assume it has the channel
  metadata before the first message.

Consequences to record honestly:

1. **A Gmail or Calendar connector cannot set `WebhookSupport::Push` and be truthful.** `SignatureScheme::new`
   requires a header and an algorithm, and `SignatureAlgorithm` is HMAC-family only. There is no variant for
   "OIDC bearer JWT" and none for "echoed channel token".
2. **`WebhookSupport::Polling` is accurate for both today**, and both providers document that push is unreliable
   enough to need a polling fallback anyway — Gmail's drop rate is "a small percentage" and the recommended
   remedy is periodic `history.list`; Calendar's is the same. So the *honest* declaration is `Polling` plus an
   optional acceleration.
3. **Widening the webhook contract is a real decision with a real cost**, and it is not obviously worth it:
   an OIDC-JWT verifier needs JWKS fetching, certificate rotation, `aud`/`iss`/`exp` validation, and a network
   dependency inside a webhook path that `P5-001` deliberately kept as a pure function of its arguments. A
   channel-token check needs only a constant-time comparison, but it authenticates *the channel* rather than
   the *body*, so it belongs to a different threat model than the one `WebhookRejection` encodes.

**This is recorded as blocking for the push half of `P5-005`** (Unresolved Question 1), not resolved here.

### Finding 2 — Stale sync cursors arrive as ordinary HTTP status codes on a read

Both providers signal "your cursor is no longer usable" by making a normal read fail:

- **Gmail**: if `startHistoryId` "is outside the available range of history records, the Gmail API returns an
  **HTTP 404** error response. In this case, your client must perform a full sync."
- **Calendar**: an invalidated sync token yields **HTTP 410 Gone**, which "should trigger a full wipe of the
  client's store and a new full sync." A disallowed query restriction on an incremental sync is **HTTP 400**.

**404 is the finding.** A `history.list` that answers 404 because history was pruned is indistinguishable, by
status code alone, from a `history.list` for a mailbox that does not exist — and the required responses are
opposite (resync everything vs. the account is gone). `P5-001`'s `SyncCursorKind::can_be_detected_as_stale()`
already asks this question; this record supplies Gmail's answer as **"only from the response to the read, not
from the cursor's shape"**, and a connector must classify a 404 on `history.list` specifically as staleness
rather than as absence. Calendar's 410 has no such ambiguity.

### Finding 3 — Google publishes a first-party Gmail MCP server, which is an alternative to a connector

`https://gmailmcp.googleapis.com/mcp/v1` is a **Google-operated** MCP server, currently in **Developer Preview**,
with one toolset: `create_draft`, `get_message`, `get_thread`, `label_message`, `label_thread`, `list_drafts`,
`list_labels`, `search_threads`, `unlabel_message`, `unlabel_thread`. There is a Calendar MCP server as well
(linked from the Calendar navigation). Its quotas are published separately and mirror the API's
(1,200,000/min/project, 6,000/min/user/project) with per-tool costs.

**Why this matters enough to record:** `jarvis-mcp-transport` already exists and consumes remote MCP servers,
so "use Google's MCP server" is a path with far less work than a connector — *and it is a different product*.
The differences are not incidental:

| | Google's MCP server | A JARVIS connector |
| --- | --- | --- |
| Tool source | third-party → `ToolSource::Mcp` | first-party, JARVIS-written |
| Effect/risk classification | operator declares it, `ADR-0025`'s `unclassified` posture by default: risk 3, held for approval, no retry | the connector declares per-operation effects in the manifest |
| Coverage | 10 Gmail tools, no send, no `watch`, no `history.list` | whatever the connector implements |
| Incremental sync | none | `historyId`/`syncToken` |
| Stability | Developer Preview | our own |
| Data flow | model-facing calls go to Google's endpoint | calls go through JARVIS's adapter, quota, and audit |

So the MCP server is **not a substitute for this connector** — it cannot sync, cannot send, and its tools would
arrive unclassified and held — but it is a **legitimate second surface** and a possible fast path for
"search my mail" before a connector exists. Recorded as an option `P5-005` may take up, not as a recommendation.

### Finding 4 — A 200 from a Gmail send does not mean the mail was sent

From the error page, verbatim: **"You can't assume that a 200 response means the email was successfully sent."**
The sending pipeline is complex, the quota is shared with the user's web client and IMAP, "once the user exceeds
their quota, there can be a delay of several minutes before the API begins returning 429 error responses", and
the daily limit can persist "for multiple hours".

**Why this belongs in a *read* slice:** it is the cleanest real-world case of the outcome-honesty problem
`P3-001` and `P3-005` are built around, and it is *worse* than the generic case. A tool call reaching Google and
getting `200` is `AdapterError::Confirmed` by every signal JARVIS has, and the effect may still not have
happened — so `P5-009`'s send operation must be declared `ProviderIdempotency::Unknown` or `NotIdempotent` and
must never be automatically retried, and its outcome must be reported as *submitted*, never *delivered*. The
`messages.send` quota cost of 100 units also means a send is 20× a read, which is relevant to `P5-009`'s budget.

### Finding 5 — The quota model changed on 2026-05-01, and a pricing change is pending

Recorded as a finding because it is the fact most likely to be wrong in any secondary source: the numbers above
came from a page last updated 2026-09-10 that explicitly says **"As of May 1, 2026, the usage limits for this API
were updated"**, that projects used between November 2025 and April 2026 keep the *old* limits, and that
exceeding the daily threshold is "planned to incur charges… later in 2026". A connector must therefore treat
quota as **configuration rather than a constant**, and must not hard-code either the old or the new numbers.

## Rejected Alternatives

- **The Gmail MCP server instead of a connector.** Rejected *for this slice's purpose* for the reasons in
  Finding 3: no sync, no send, ten tools, held by default as an unclassified third-party source, and in
  Developer Preview. Kept as a recorded second surface.
- **`mail.google.com/` as the Gmail scope.** It is the only scope permitting permanent deletion bypassing the
  trash and the documentation advises requesting it only for that. A read connector has no use for it.
- **`gmail.metadata` as "the cheap read scope".** It is restricted like the rest, so it carries the same
  verification and assessment burden; choosing it does not avoid the review. It remains the right scope for a
  metadata-only connector because it *requests less data*, which is a different argument.
- **Polling only, with no push.** Not rejected outright — it is what `WebhookSupport::Polling` would declare
  and it is honest — but both providers' own documentation says push is unreliable enough to need a polling
  fallback, so polling-only loses the latency benefit without removing any of the reliability work.
- **Treating the Calendar push as HMAC-verifiable.** It has a zero-length body. There is nothing to MAC.
- **Assuming the old quota numbers.** See Finding 5.

## Verification Plan

`P5-004` is research, so these are the tests `P5-005` must write. **Status is marked per item**, because a plan
where every line looks equally done is a plan nobody can audit.

- **Scope-category tests.** A test that a Gmail connector's declared scopes are all *accounted for* against a
  table of Google's categories, so adding a scope forces a decision about the verification burden. The table is
  a fixture with a date, because Google can recategorise. **Not written.**
- **A quota-cost test.** `users.getProfile` = 1 and `messages.get` = 20 as fixtures, and a computed estimate for
  a full sync of *N* messages (`5 + 20N`) — the cheapest test that would catch a connector that planned its
  budget from message counts alone. **Not written.**
- **A full-sync budget test that asserts batching.** A full sync must batch (≤50 per batch) *and* must respect
  that batches trigger rate limiting, so the test asserts both the batch size and the delay. **Not written.**
- **The 404-is-staleness test.** A `history.list` fixture returning 404 must produce "resync from scratch", not
  "account not found" — and a fixture for a genuinely absent account must produce the opposite. **This is the
  cheapest test that would disprove the central assumption of Finding 2**, and it is the one most worth
  writing first. **Not written** — the *classification* it would test is decided and tested
  (`client::gmail_history_status_is_pruned`, `advance_gmail_history`), but no 404 fixture exists.
- **The 410-is-staleness test** for Calendar, plus 400-is-a-query-error. **Not written** as fixtures; the
  classification is tested (`client::calendar_status_requires_resync`, `advance_calendar_sync`).
- **A `domainPolicy` test.** A 403 with `reason: domainPolicy` must be `Permanent` and must not be retried,
  which a status-code-only classifier cannot distinguish from `rateLimitExceeded`. **WRITTEN** — see below.
- **A recording fixture for the Pub/Sub envelope** — a sanitized delivery with `message.data` Base64URL-decoded
  to `{"emailAddress":…,"historyId":…}` — so the envelope handling is tested without a Pub/Sub subscription.
  **Not written.**
- **A Calendar sync-message fixture**, including that it can arrive *before* the `watch` response and that
  `X-Goog-Message-Number` is `1` for it. **Not written.**
- **An unauthenticated-delivery test** for whichever push mechanism is chosen, asserting the refusal **fails
  closed** — the same shape as `P5-001`'s falsified guard. **Not written.**
- **A renewal test.** A watch whose lease is near expiry must be renewed before it lapses, and the Gmail 7-day
  bound must be asserted, because a lapsed watch stops notifications **silently**. **Not written.**
- **Opt-in live smoke test** behind credentials and a cost gate, as `tools-and-connectors.md` requires.
  **Not written.**

### Fixtures present, and what they are not

`crates/jarvis-connectors/tests/fixtures/google/` holds six files, tested by `tests/google_fixtures.rs`:

| Fixture | Shape source |
| --- | --- |
| `gmail_messages_list.json` | `users.messages.list` response |
| `gmail_messages_get.json` | the `Message` resource |
| `calendar_events_list_page.json` | `events.list`, a **mid-walk** page |
| `calendar_events_list_last_page.json` | `events.list`, the **last** page |
| `gmail_error_403_domain_policy.json` | the error resource, 403 + `domainPolicy` |
| `gmail_error_403_rate_limit.json` | the error resource, 403 + `rateLimitExceeded` |

**Every one is hand-built, not captured**, and each file says so twice: in `_not_a_capture: true` and in prose.
The test suite **asserts the marker**, so a file that dropped it fails rather than passing as an apparent
recording. `docs/development/external-research.md` asks to "validate against official schemas or sanitized real
wire fixtures"; these are the first kind and not the second, and no live smoke test has run.

**A finding the fixtures produced.** Google documents `nextPageToken` and `nextSyncToken` on `events.list` as
**mutually exclusive** — `nextPageToken` is "omitted if no further results are available, in which case
nextSyncToken is provided", and `nextSyncToken` is "omitted if further results are available, in which case
nextPageToken is provided". A page with more results therefore **cannot** carry a sync token. An earlier version
of my own test asserted a body carrying both, which the provider cannot produce; it now tests the two states
apart (a mid-walk page and a last page), and the fixtures are that pair. The lesson generalises: **a hand-built
response is easy to make internally consistent and impossible to make realistic in its constraints**, so a
fixture must be checked against the *field's own documentation and not just the example*. The constraint
lives in the field description ("omitted if…"), not in the sample JSON.

**No live test was run for this slice and none is claimed.** No credentials were used, no Google API was called,
and no Cloud project was created.

## Unresolved Questions

1. **How should JARVIS declare a webhook that is authenticated by an OIDC bearer JWT or an echoed channel
   token, when `WebhookSupport::Push` names only an HMAC over the body?** *Impact:* Gmail and Calendar push
   cannot be declared truthfully, so `P5-005` must either declare `Polling` (correct, and loses latency) or a
   change to `jarvis-connectors`' webhook contract is required first. *Blocks:* the push half of `P5-005`, and
   `P5-010`'s "webhook signature/replay tests" for any provider using either scheme — which is both Google
   APIs, so it is not a corner case. *Not decided here*: widening the contract adds a network dependency
   (JWKS) to a path `P5-001` kept pure, and the alternative is a permanent polling declaration.
2. **What category does each Calendar read scope have?** The Gmail page states categories per scope; the
   Calendar page lists scopes without them, and the consent page gives the category *table* but not the
   mapping. *Impact:* the connector cannot state its verification burden, and `P5-006` will hit the same gap
   for Microsoft unless it is solved once. *Blocks:* nothing yet, because it changes the *effort estimate* for
   shipping a Calendar connector rather than its code. It must be answered before a Calendar connector is
   released to anyone outside its own Workspace organization.
3. **Does a self-hosted, single-user JARVIS qualify for the internal-app exemption?** The documentation says an
   app "used only internally by your Google Workspace organization" needs no further review, and that the test
   is the consent screen's audience setting. A personal JARVIS with an `External` consent screen would not
   qualify even with one user. *Impact:* potentially the difference between days and months of lead time before
   a Gmail connector can be used by its own author. *Blocks:* using a Gmail connector at all in a deployment
   whose Cloud project predates this decision.
4. **Is the first-party Gmail MCP server usable with JARVIS's own MCP client today?** It is a remote MCP server
   over Streamable HTTP, which `jarvis-mcp-transport` supports, but the Developer Preview enrolment, the exact
   auth flow for an MCP client, and whether its tool schemas satisfy `P3-008a`'s untrusted-schema rules were
   **not** verified here. *Impact:* a possible fast path for search-style Gmail access. *Blocks:* nothing in
   `P5-005`; it is an alternative that would need its own research record.
5. **What is the actual `quotaUser`/`userIp` behaviour, and does it let a connector ask for a specific user's
   quota accounting?** Both APIs accept these parameters and this was **not verified**. *Impact:* a service
   account is counted as one user (6,000 units/minute) regardless of how many mailboxes it reads, so a
   multi-account connector's real throughput ceiling depends on whether quota can be attributed per
   end-user. *Blocks:* any connector that reads more than one mailbox under one credential.
6. **Does the pending pricing change alter anything a connector can control?** The daily threshold "can't be
   increased" and charges are "planned". *Impact:* the honest answer may be that no connector design avoids it
   except by reading less. *Blocks:* nothing; recorded so a later reader does not mistake the current no-charge
   state for a guarantee.
7. **Which exact redirect URI string does Google's console accept as *registered* for a Desktop-app client?**
   The native-app page's exchange example uses the **ported** form (`redirect_uri=http://127.0.0.1:9004`), and
   RFC 8252 §7.3 says a server must accept **any** port, but the page does not state what may be entered in the
   console — a portless `http://127.0.0.1/`, a loopback with a specific port, or the `localhost` equivalent.
   *Impact:* the connector's registered form is unconfirmed, so a deployment may hit `redirect_uri_mismatch`
   for a flow that is otherwise correct, and diagnosing it means reading a console page rather than a protocol
   requirement. *Blocks:* nothing in the flow's code — `P5-002`'s `LoopbackRedirect::registered` produces the
   portless form and `matches_except_port` joins it to a listening port, so **both sides of the comparison are
   implemented and tested**; what is unverified is one string a human types. It must be confirmed before a live
   smoke test, which is where a mismatch would surface.

## Verification Log

| Date | Check | Result |
| --- | --- | --- |
| 2026-09-27 | `https://developers.google.com/llms.txt` and `https://developers.google.com/gmail/api/llms.txt` fetched | **HTTP 404** for both. No `llms.txt` exists for Google Workspace; recorded as `not found` and the official guide pages used instead, per `external-research.md`. |
| 2026-09-27 | Gmail push guide fetched and read in full | `users.watch` request/response shapes; the immediate notification on a successful watch; the **7-day renewal bound** with daily recommended; the `PubsubMessage` envelope with `message.data` as **Base64URL-encoded JSON** decoding to `{"emailAddress","historyId"}`; the **1 event/second/user cap with excess dropped**; "notifications might be delayed or dropped"; the polling fallback. |
| 2026-09-27 | Gmail sync guide fetched | Full vs partial sync; `startHistoryId`; history available "at least one week"; **a `startHistoryId` out of range returns HTTP 404 and requires a full sync**. |
| 2026-09-27 | Gmail quota page fetched | The **May 2026 model change** and its grandfathering; 1,200,000/min/project, 6,000/min/user/project, 80,000,000/day/project; the **full per-method cost table**; 500 recipients/message; the batch ceiling of 50; the daily threshold cannot be raised; per-user limits cannot be increased; a service account is one user for quota. |
| 2026-09-27 | Gmail scopes page fetched | The non-sensitive/sensitive/restricted split, with **`gmail.readonly` and `gmail.metadata` both restricted** and `gmail.send` sensitive; the rule that storing or transmitting restricted-scope data requires a security assessment; the internal-app exemption. |
| 2026-09-27 | Gmail error page fetched | The 401/403/429/5xx taxonomy by `reason`; the four 403 reasons; the three distinct causes behind a 429; the exponential-backoff recipe with `max_backoff` 32–64 s; **"You can't assume that a 200 response means the email was successfully sent."** |
| 2026-09-27 | Calendar push guide fetched | Channel creation with `id`/`type: web_hook`/`address`/`token`/`expiration`; the full `X-Goog-*` header table; **`X-Goog-Channel-Token` as the anti-spoofing control**; the **zero-length body**; the `sync` message that can arrive before the watch response; success codes `200/201/202/204/102`; 5xx retried; **"no automatic way to renew"** with an expected overlap; "Not 100% reliable. Expect a small percentage of messages to get dropped"; per-calendar vs per-user subscription granularity. |
| 2026-09-27 | Calendar sync guide fetched | `nextSyncToken`; incremental sync with `syncToken`; **HTTP 410 Gone for an invalidated token** requiring a full wipe; **HTTP 400 for disallowed query restrictions**; the pagination rule of repeating the exact same query with `pageToken`; `modifiedSince` recorded as deprecated in favour of sync tokens. |
| 2026-09-27 | Pub/Sub push authentication page fetched | **JWT (RS256) in `Authorization: Bearer`**; the claim set (`aud`, `azp`, `email`, `sub`, `iss`, `exp`, `iat`); validation = signature + **email and audience claims matching the subscription configuration**; tokens "may be up to an hour old"; no body signature. This is the finding that `WebhookSupport::Push` cannot express. |
| 2026-09-27 | Gmail MCP reference fetched | `https://gmailmcp.googleapis.com/mcp/v1`, **Developer Preview**, ten tools with per-tool query costs; a Calendar MCP server linked from the Calendar navigation. |
| 2026-09-27 | `https://accounts.google.com/.well-known/openid-configuration` fetched | **Machine-readable**, so the endpoint values are the server's own published configuration rather than documentation examples: `authorization_endpoint`, `token_endpoint`, `revocation_endpoint`, `userinfo_endpoint`, `jwks_uri`; `code_challenge_methods_supported` = `plain`+`S256`; **`authorization_response_iss_parameter_supported: true`**; `token_endpoint_auth_methods_supported` **truncated in capture** and recorded as such. |
| 2026-09-27 | Google OAuth 2.0 for native apps fetched | Loopback is the **recommended** desktop method and custom schemes are **no longer supported**, so loopback is the only non-embedded option; `redirect_uri` must match an authorized URI **exactly**; **`client_secret` is Optional** on both exchanges; the token response's fields including `refresh_token` "always returned for installed applications" and `id_token` only with an identity scope; **refresh-token limits make older tokens stop working**; the `revocation_endpoint` and its HTTP 200/400 contract; **revocation removes grants for the whole project**; DPoP's key-storage obligation. |
| 2026-09-27 | Cross-check against `jarvis-connectors` | `AuthFlow::new` requires a **`https://` authorization endpoint** and a redirect URI for an OAuth method, so the discovery document's values are what make the flow constructible. `AuthorizationTransaction::begin` requires the listener's redirect to **match the flow's registration on host and path, ignoring the port** — so the registered form and the listening form are both needed, and the unverified question is what a human types into the console (Unresolved Question 7). |
| 2026-09-27 | Cross-check against `jarvis-connectors` (webhook and cursor) | `SignatureAlgorithm` has no OIDC/JWT variant and `SignatureScheme` requires a signed body, so **Finding 1 is a contract gap rather than a connector mistake**; `SyncCursorKind` has `MonotonicMarker` and `OpaqueToken`, so both cursor shapes are already representable and only the **staleness signal** (Finding 2) is new. |

**No Google API was called, no credentials were used, no Cloud project was created, and no live test was run.**

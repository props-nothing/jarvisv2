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
| Gmail batch requests | https://developers.google.com/workspace/gmail/api/guides/batch (last updated **2026-09-10**) | 2026-09-30 | the **hard** batch limit (100), the **recommended** size (50), and that a batch counts as *n* requests against quota |
| Gmail scopes | https://developers.google.com/workspace/gmail/api/auth/scopes (last updated **2026-09-10**) | 2026-09-27 | the non-sensitive / sensitive / restricted taxonomy |
| Gmail error handling | https://developers.google.com/workspace/gmail/api/guides/handle-errors (last updated **2026-09-15**) | 2026-09-30 | the status and `reason` taxonomy, backoff guidance — and that its sections are 400/401/403/404/429/5xx, i.e. it documents **no `410`**, unlike the Calendar page (Finding 6) |
| Gmail MCP server reference | https://developers.google.com/workspace/gmail/api/reference/mcp (last updated **2026-07-21**) | 2026-09-27 | the first-party MCP endpoint and its toolset |
| Calendar push notifications | https://developers.google.com/workspace/calendar/api/guides/push (last updated **2026-09-11**) | 2026-09-27 | channel creation, the `X-Goog-*` headers, the sync message, renewal, delivery semantics |
| Calendar sync | https://developers.google.com/workspace/calendar/api/guides/sync (last updated **2026-09-11**) | 2026-09-27 | `nextSyncToken`, the `410 Gone` signal |
| Calendar error handling | https://developers.google.com/workspace/calendar/api/guides/errors (last updated **2026-09-11**) | 2026-09-30 | the per-status JSON bodies, **including the three distinct `410` causes** — one of which needs no action |
| Calendar scopes | https://developers.google.com/workspace/calendar/api/auth (last updated **2026-09-03**) | 2026-09-27 | the Calendar scope list |
| OAuth consent and scope categories | https://developers.google.com/workspace/guides/configure-oauth-consent (last updated **2026-09-03**) | 2026-09-27 | which review each scope category requires |
| Cloud Pub/Sub push authentication | https://docs.cloud.google.com/pubsub/docs/authenticate-push-subscriptions (last updated **2026-09-24**) | 2026-09-27 | **the JWT-bearer mechanism** — see Finding 1 |
| Cloud Pub/Sub push subscriptions | https://docs.cloud.google.com/pubsub/docs/push (last updated **2026-09-24**) | 2026-09-30 | the wrapped envelope, **at-least-once delivery**, the **five** acknowledging statuses, unwrapped delivery, push backoff — and the **padded** example that corrects Finding 11 (`ADR-0089`) || Cloud Pub/Sub `PubsubMessage` | https://docs.cloud.google.com/pubsub/docs/reference/rest/v1/PubsubMessage (last updated **2026-05-14**) | 2026-09-30 | the envelope's field **types** — including `data`'s, which **contradicts** the Gmail guide (`ADR-0088`) |
| Google OIDC discovery document | https://accounts.google.com/.well-known/openid-configuration | 2026-09-27 | **machine-readable**: every endpoint, the PKCE methods, and whether the `iss` response parameter is supported |
| Google OAuth 2.0 for native apps | https://developers.google.com/identity/protocols/oauth2/native-app (last updated **2026-09-14**) | 2026-09-27 | the installed-app flow: the loopback method, the token and refresh exchanges, the response fields, DPoP, revocation |
| Gmail `users.history.list` method reference | https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.history/list (page footer: last updated **2026-04-15**) | 2026-10-01 | the incremental-sync read: **the query parameters, the required `startHistoryId`, and the response body `{ history[], nextPageToken, historyId }`** — the shape the `gmail_history_list` fixtures reproduce. **The storing rule is in `startHistoryId`'s description, not in `historyId`'s**: *"If you receive no nextPageToken in the response, there are no updates to retrieve and you can store the returned historyId for a future request."* (`ADR-0108`, Finding 22) |
| Gmail `Format` enum + `Message` resource | https://developers.google.com/workspace/gmail/api/reference/rest/v1/Format (last updated **2026-03-24**) and https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages | 2026-09-30 | **which fields each `format` returns** — the fact that bounds what a `messages.get` output can declare (`ADR-0083`) |
| Calendar `events.list` method reference | https://developers.google.com/workspace/calendar/api/v3/reference/events/list (last updated **2026-07-29**) | 2026-09-30 | the full parameter list and **the eight parameters that cannot be combined with `syncToken`** (`ADR-0084`), the `maxResults` default and ceiling, and the `nextPageToken`/`nextSyncToken` exclusivity |
| Calendar `events.watch` method reference | https://developers.google.com/workspace/calendar/api/v3/reference/events/watch (last updated **2026-05-12**) | 2026-10-01 | the **response** `expiration` typed **`long`** — *"a Unix timestamp, in milliseconds"* — which **contradicts the guide's "property string"** for the same field; `params.ttl` *"Default is 604800 seconds"*; the channel fields and the `resourceId`/`resourceUri` distinction (`ADR-0106`, Finding 20) |
| Calendar `events.watch` **request body** (method reference + push guide, *Make watch requests*) | https://developers.google.com/workspace/calendar/api/v3/reference/events/watch (last updated **2026-05-12**) and https://developers.google.com/workspace/calendar/api/guides/push (last updated **2026-09-11**) | 2026-10-01 | **the fields a channel *creation* carries** — the fact that made channel creation constructible at all (`ADR-0112`, Finding 23): the **required** trio (`id` ≤ 64 chars, *"echoed back in the `X-Goog-Channel-Id`"*; `type` = `web_hook`; `address`, **https-only with a valid certificate**), the **optional** pair (`token` ≤ 256 chars for anti-spoofing, `expiration` as a **Unix timestamp in milliseconds**), and that `address` and `token` are **credential-adjacent** fields a rendered body would disclose |
| Calendar `channels.stop` method reference | https://developers.google.com/workspace/calendar/api/v3/reference/channels/stop (last updated **2025-04-01**) | 2026-10-01 | the stop body is exactly `{ id, resourceId, token? }` with **no `expiration`**, so the `resourceId` a `watch` response returns is what makes teardown constructible (`ADR-0106`) |

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
- **Batch requests: a hard limit of 100, and a recommended size of 50** — and these are **two different
  facts**, which an earlier version of this record conflated into one "ceiling of 50". The batch reference
  says "You're limited to **100** calls in a single batch request. If you must make more calls than that, use
  multiple batch requests", and separately "Larger batch sizes are likely to trigger rate limiting. We
  recommend sending batches of no more than **50** requests." So 100 is a **refusal** and 50 is a **slowdown**:
  a batch of 60 is accepted and merely unwise, while a batch of 101 fails. **Corrected 2026-09-30** — the figure
  had been attributed to the quota page, which states no batch limit at all, and the batch page was missing
  from the source table (see `ADR-0080`).
- **Batching saves connections and no quota.** The same page: "A set of *n* requests batched together counts
  toward your usage limit as *n* requests, not as one request." A full-sync budget is therefore unchanged by
  batching, which is worth stating because "batch the sync to make it affordable" is the natural misreading.
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
- **A `429` can state a delay of hours, and that is documented rather than hypothetical.** The error page says a
  daily-limit `429` "might result in these errors for multiple hours" (recorded above), and the quota page notes
  that after an account reaches its quota "there can be a delay of several minutes before the API begins
  returning 429 error responses". So a stated `Retry-After` above one hour is a **real response** meaning *defer
  the work*, not a malformed one: `RetryGuidance::DeferSeconds` represents it and `MAX_RETRY_AFTER_SECONDS`
  bounds what a caller will hold inside a retry loop (`ADR-0077`). **WRITTEN.**

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
| **Calendar stale cursor (410)** | `CursorError` → require full resync | see Finding 2 — the status alone is not enough; the `reason` decides (`ADR-0081`) |
| Gmail push (Pub/Sub) | **`WebhookSupport::Push` — cannot be expressed today** | Finding 1 |
| Calendar push (channels) | **`WebhookSupport::Push` — cannot be expressed today** | Finding 1 |
| `gmail.readonly` etc. | connector **OAuth** scopes, not JARVIS `Scope`s | `P3-001` draws this line explicitly: a JARVIS scope is `resource.action` and a provider scope is the connector's business |
| Quota units | `RateLimit { per_window, window_seconds, scope }` | the *project* limits are `Project`-scoped and the *user* limits per-account; a connector must declare both, and the daily threshold is a third |
| `reason` codes | `RetryClass` | `domainPolicy` → `Permanent`; `rateLimitExceeded`/`userRateLimitExceeded` → `Throttled`; 5xx → `Transient`; 429 → `Throttled` with the stated delay. **The status is classified per API** (`GoogleApi`), because the two pages document different status sets — see Finding 6 and `ADR-0082` |

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

**RESOLVED 2026-10-01 (`ADR-0099`), and the resolution splits the finding in two.** The page was re-read the
same day, so the facts above are unchanged; what changed is the contract. The gap's own name was the clue: it
asked for "a webhook authenticated by an OIDC bearer JWT or an echoed channel token", and **neither mechanism
covers the body while both still authenticate a delivery.** `SignatureAlgorithm` had one axis — which MAC over
the body — and used it to answer a different question, "what authenticates this delivery", for which it is the
wrong axis. `OidcIdToken` and `EchoedChannelToken` are now variants, and `covers_the_body` names the property
that was being asked for all along.

- **Consequence 1 is closed.** Both schemes are constructible, so a push endpoint authenticated by either can
  now be *declared*. `ADR-0057`'s rule is what made the missing variants the right fix rather than a widened
  `None`.
- **Consequence 3 is partly answered and partly still open.** Naming an authenticator is not verifying one, so
  the JWKS/certificate-rotation work and the channel-token comparison are **still unbuilt** — they move from
  "blocked on an inexpressible declaration" to `P5-010`'s "webhook signature/replay tests", which is exactly
  the item this finding was said to block.
- **Consequence 2 stands, and for a corrected reason.** One connector holds **one** `WebhookSupport` value and
  Google has **two** push mechanisms with different headers *and* different bindings, so declaring one would be
  as incomplete as declaring neither; neither verifier existing is a second reason. The connector therefore
  keeps `Polling`, which is still true of what runs today — but the claim that the mechanisms *cannot be
  expressed* is now false and has been corrected in the connector's prose.

**The contract half is resolved; the verification half is recorded as blocking for `P5-010`, not for `P5-005`'s
declaration, and Unresolved Question 1 is superseded by Unresolved Question 9 below.**

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
rather than as absence.

> **Correction (ADR-0081, 2026-09-30).** An earlier version of this paragraph ended *"Calendar's 410 has no such
> ambiguity."* That was **true of the status and false of the decision**, which is the distinction that matters.
> The Calendar **errors** page publishes **three** bodies for `410 Gone`, and only two of them resync:
> `fullSyncRequired` ("wipe the store and re-sync") and `updatedMinTooLongAgo` (same), but also **`deleted`**
> ("Resource has been deleted"), whose suggested action is **"no further action is necessary"**. So a connector
> reading the status alone wipes a whole sync store when a user deletes one event. The corrected statement:
> **a Calendar `410` is unambiguous once the `reason` is read** — and the errors page, which this record's source
> table did not list for this fact, is where that is established.

**⚠ And the ambiguity cannot be resolved from the response, which took a third pass to establish.** The error
guide (`handle-errors`, last updated **2026-09-15**) lists `404 - Not Found — The requested resource couldn't be
found` in its status summary and then **has no 404 subsection at all**: the guide's sections are 400, 401, 403,
429 and 5xx. So Google publishes **no `reason` code** for a 404, and a connector has nothing machine-readable to
switch on. The two causes are therefore genuinely indistinguishable at the response level.

**The resolution is the *consequence*, not the classification.** For **both** causes the first correct action is
a full sync: if the history was pruned that is the documented remedy, and if the account is gone the full sync's
own first call fails and surfaces *that*. So reading a 404 as "resync" is safe because the wrong reading is
**self-correcting**, and its worst case is one extra `messages.list` call rather than a store that resumes from a
dead position while reporting itself in sync. `client::advance_gmail_history` therefore resyncs on a
caller-supplied signal, and the predicate that used to assert "pruned" was **removed** — it claimed knowledge the
response does not carry, which is worse than the ambiguity it pretended to settle.

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

### Finding 6 — The two error pages publish different status sets, so one classifier cannot serve both

Gmail's and Calendar's error handling are documented on **separate pages with separate status summaries**, and
they do not agree on the set of statuses:

| Status | Gmail page | Calendar page |
| --- | --- | --- |
| `400`, `401`, `403`, `429`, `5xx` | documented, by `reason` | documented, by `reason` |
| `404` | in the summary; states no action | in the summary; **suggests "use exponential backoff"** |
| `410` | **no subsection at all** | **documented in detail** — three causes, two of which resync |

**A classifier with no API parameter must answer for the less informative case**, and this one did: a Calendar
`410` fell to the catch-all and reached a caller as *"the provider answered 410 (unknown)"* — `Reconcile`, i.e.
*establish what happened before doing anything else* — when the provider had already stated what happened and
what to do. The remedy is to make the API an input to the decision, and to note that the arms the two pages **do**
share (`401`; the `5xx` family; `403`'s throttling reasons, which the Calendar page explicitly calls
"functionally similar" across `403` and `429`) are shared **because the pages agree**, not for brevity.

**The second divergence, recorded rather than resolved.** Calendar's page suggests exponential backoff for a
`404`; Gmail's states no action. The crate keeps `DoNotRetry` for both, on the ground that Calendar's own two
documented `404` causes are *"the requested resource … has never existed"* and *"accessing a calendar that the
user can not access"* — and neither is repaired by resending the identical request. The disagreement is asserted
in a test that names which document the code follows, so a reader comparing the arm with the page meets the
decision instead of a surprise. See `ADR-0082`.

**Neither fact was obtainable from a secondary source**, and both are the kind of thing a "the Google APIs"
mental model flattens: the two APIs are separate products with separate error contracts that happen to share a
host and an OAuth server.

### Finding 7 — `format` decides which fields `messages.get` returns, so an output field is bounded by the request

`users.messages.get` takes a `format` and returns **different top-level fields for each value**. From the
**Format** enum page (last updated **2026-03-24**), verbatim:

| Format | What it returns (the page's own words) |
| --- | --- |
| `minimal` | "Returns only email message ID and labels; does not return the email headers, body, or payload." |
| `metadata` | "Returns only email message ID, labels, and email headers." |
| `full` | "Returns the full email message data with body content parsed in the `payload` field; the `raw` field is not used." |
| `raw` | "Returns the full email message data with body content in the `raw` field as a base64url encoded string." |

The `Message` **resource** lists `id`, `threadId`, `labelIds`, `snippet`, `historyId`, `internalDate`,
`payload`, `sizeEstimate`, `raw` and `classificationLabelValues`. The consequence a per-format reading makes
visible: **`snippet` ("A short part of the message text") is not in the `minimal` or `metadata` definitions**,
so a tool offering those two formats cannot reliably return it. `labelIds` is named for both; `threadId` is in
the resource but the Format page says nothing about it for either format, so it is not guaranteed.

**`full` and `raw` "cannot be used when accessing the api using the `gmail.metadata` scope."** The connector
requests `gmail.readonly`, not `gmail.metadata`, so this is not a live constraint for it — but it is recorded
because a future metadata-only connector would find its `full` reads refused by that rule.

**This is the fact a declared-output contract must respect**, and it was found by reading the *Format* page and
not the method page: the method page only says "the response body contains an instance of `Message`", which
suggests every field is always returned. A declaration promising a field that two of three accepted formats do
not produce is a promise the connector cannot keep — see `ADR-0083`.

### Finding 8 — Eight query parameters cannot be combined with Calendar's `syncToken`, and two of them are inputs the connector offers

The `events.list` reference states, under `syncToken`, verbatim:

> "There are several query parameters that **cannot be specified together with nextSyncToken** to ensure
> consistency of the client state. These are: `iCalUID` `orderBy` `privateExtendedProperty` `q`
> `sharedExtendedProperty` **`timeMin`** **`timeMax`** `updatedMin`."

The sync guide explains the reason and the consequence: a time range belongs to the **initial full sync** and
"each incremental sync should use the same set of query parameters, including the initial request"; the guide's
own sample sets `timeMin` only in the full-sync branch and `syncToken` only in the incremental one. "The
response code for list queries containing disallowed restrictions is `400`."

**The finding for this connector is narrower than the list.** `calendar_events_read` offers **two** of those
eight — `time_min` and `time_max` — and its input schema advertised them alongside `sync_token` with no
restriction, so `{calendar_id, sync_token, time_min}` was a request the schema called legal, the builder built,
and the provider documents as a `400`. The other **six** (`iCalUID`, `orderBy`, `q`, `privateExtendedProperty`,
`sharedExtendedProperty`, `updatedMin`) are simply **not offered by the operation**, so they need no check — a
parameter the connector never sends cannot conflict. That distinction is why the list is recorded: a future
slice adding `q` or `orderBy` to `events_read` joins the disallowed set and must be checked against this note
rather than discovered as a `400`.

The pairing is now refused in `request::calendar_events_list` and by an `allOf`/`not` constraint in the input
schema — see `ADR-0084`.

### Finding 9 — A large incremental sync returns a `pageToken` *instead of* a `syncToken`, so a page token must be sent alongside the sync token

The sync guide, verbatim:

> "In cases where a large number of resources have changed since the last incremental sync request, you may find
> a `pageToken` instead of a `syncToken` in the list result. In these cases you'll need to perform the exact same
> list query as was used for retrieval of the first page in the incremental sync (with the exact same
> `syncToken`), append the `pageToken` to it and paginate through all the following requests until you find
> another `syncToken` on the last page."

The guide's own example is `GET /calendars/primary/events?maxResults=10&singleEvents=true&syncToken=…&pageToken=…`.

**Two facts follow, and they point in opposite directions from Finding 8.** First, `pageToken` is **not** on the
disallowed-with-`syncToken` list — `timeMin`, `timeMax`, `q`, `orderBy` and four others are, and a page token is
required *with* a sync token. So the eight-parameter list must be read per parameter rather than applied by
shape. Second, the connector's `calendar_events_read` rendered `next_page_token` while declaring **no**
`page_token` input, so a sync of a busy calendar — where the provider returns a page token rather than a
cursor — could not be walked to its end. The input is added and the two halves are now asserted in step
(`ADR-0085`).

**Why the pairing matters more than the two fields.** Each schema was internally consistent, so no per-tool
check found this; only the pairing of the input and output halves of one tool exposes a renderer that emits a
token no argument can consume. The check is therefore written over every definition, not for this tool alone.

### Finding 10 — A Gmail watch lapses silently, and its `expiration` is epoch **milliseconds** in a JSON string

The push guide's renewal sentence is two facts in one line:

> "You must call the `watch` method at least once every 7 days or you'll stop receiving updates for the user. We
> recommend calling `watch` once per day."

- **The bound is seven days; the recommendation is one day.** They answer different questions — when a watch
  *dies* versus when to *renew* — and reporting one as the other either renews six days late or hides the cadence
  to use. The same limit/recommendation split as `ADR-0080`.
- **The failure is silent.** Nothing is raised when the lease ends and no notification announces that
  notifications have stopped, so a lapsed watch is indistinguishable from a quiet mailbox. That is why the
  connector must be able to decide for itself.

**And the value that says when the lease ends is doubly mistyped.** The `users.watch` reference publishes the
response as `{ "historyId": string, "expiration": string (int64 format) }` and describes `expiration` as *"When
Gmail will stop sending notifications for mailbox updates (epoch millis)"*:

- **It is a string, not a number.** Reading a JSON number would refuse a *conforming* response; the string is
  Google's convention for a 64-bit integer JSON cannot carry exactly.
- **It is milliseconds, not seconds.** A seconds value read as millis puts the watch's death a thousand times too
  far in the future — which raises nothing, and produces exactly the silent dead watch this fact is about. The
  connector scales millis → the timestamp's nanoseconds (× 1,000,000) and asserts a real documented value
  (`"1431990098200"`) both as seconds and as a rendered instant, so the unit is pinned by a test rather than by
  inspection (`ADR-0087`).

**A Calendar comparison worth noting for `P5-006`-style work — and corrected by `ADR-0106`, which needed it.** An
earlier version of this paragraph said Calendar's channel `expiration` is *"an RFC 3339 date-time string"*. **It
is not, and no Calendar page says so.** It is epoch millis (see Finding 20); what *is* an RFC 3339 date-time
string is Google **error** bodies' `error.details[].metadata` / Pub/Sub's `publishTime`, which is a different
field. The claim was plausible, unquoted, and wrong, and it survived because nothing read the field. The lesson
is narrower than "expiration is not a shared shape": **a comparison drawn from memory reads exactly like one
drawn from a page, and only the quoted form can be checked.**

### Finding 11 — Google declares `message.data`'s encoding two different ways, and almost no payload can tell them apart

The Gmail push guide and the Cloud Pub/Sub reference — a page the same guide links to — **disagree** about the
encoding of the notification payload:

| Source | Statement |
| --- | --- |
| Gmail push guide | *"The `message.data` field is a **Base64URL**-encoded string that decodes to a JSON object containing the email address and the new mailbox history ID."* |
| `PubsubMessage` reference | `data \| string (bytes format) \| … A **base64**-encoded string.` |

RFC 4648 §4 (standard, `+`/`/`) and §5 (URL-safe, `-`/`_`) differ in **exactly two characters**, so:

- **The guide's own example cannot tell them apart.** `eyJlbWFpbEFkZHJlc3MiOiAidXNlckBleGFtcGxlLmNvbSIsICJoaXN0b3J5SWQiOiAiMTIzNDU2Nzg5MCJ9` uses only
  `A-Za-z0-9` and decodes identically under both — and it decodes to exactly what the guide says,
  `{"emailAddress": "user@example.com", "historyId": "1234567890"}` (verified by decoding it).
- **Neither can almost any real notification.** A sweep of all 95 printable ASCII characters at all four base64
  alignments, inside a Gmail-shaped JSON payload, found only **three** — `>`, `?`, `~`, each after a
  one-character offset — whose standard encoding contains `+` or `/`.

**So the two readings agree on nearly every payload, and diverge on the one that matters** — where choosing
wrongly means **refusing a delivery**, i.e. a missed change. The connector therefore decodes under **both**
alphabets, tries URL-safe first (the guide is the more specific statement), and **reports which matched** so a
real delivery can settle the question rather than leaving it resolved in the provider's favour silently. See
`ADR-0088`.

**A generalisation worth keeping:** two pages describing the same field's *type* is a disagreement about
**values**, not prose, and the test that catches it is one whose input forces the difference — not the
provider's convenient example, which is chosen to be readable rather than discriminating.

### Finding 12 — The push payload is **padded** base64, delivery is **at-least-once**, and only five statuses acknowledge

Three facts from the push page, each of which changes behaviour, and one of which **corrects Finding 11**.

**The payload is padded, and the code refused it.** The page's own minimum-value example of `message.data` is
`SGVsbG8gQ2xvdWQgUHViL1N1YiEgSGVyZSBpcyBteSBtZXNzYWdlIQ==` — it ends in `==` and decodes to
`Hello Cloud Pub/Sub! Here is my message!`. So the field is **padded standard** base64, not the bare "Base64URL"
Finding 11 recorded from the Gmail guide, and a decoder that refused padding would have **refused Google's own
example**. Finding 11's alphabet contradiction stands; its implied *padding* reading did not, and the cause was
that **RFC 7636's no-padding rule is an OAuth rule applied to a Pub/Sub field** (`ADR-0089`).

**Delivery is at-least-once, so `messageId` is required.** *"A non-success response indicates that Pub/Sub must
resend the messages"* and *"If you send a negative acknowledgment or the acknowledgment deadline expires,
Pub/Sub resends the message."* A repeat is therefore normal rather than exceptional, and the envelope's
`messageId` — *"Guaranteed to be unique within the topic"* — is what distinguishes a redelivery from a new
change. The page's own examples show it both as `messageId` **and** as `message_id`, and `publishTime`
**and** `publish_time`, so a parser must read either spelling; `deliveryAttempt` is **top-level** beside
`message` rather than inside it.

**Only five status codes acknowledge: `102`, `200`, `201`, `202`, `204`.** *"To send a negative acknowledgment
for the message, return any other status code."* So a `203` or `206` — successes by HTTP's classification —
are **negative acknowledgements** here and cause a redelivery. The page also documents **unwrapped** delivery
(`payload-unwrapping`), in which the raw payload is the whole body and there is no `data` field at all, and
`push backoff` (100 ms–60 s, triggered by negative acknowledgements, global to the subscription) which is
independent of the subscription retry policy.

**And the envelope is a *sensitive* value, which the page does not say.** The payload is *"a JSON object
containing the email address and the new mailbox history ID"* — a mailbox address, i.e. a person's identity,
arriving at a **public HTTP endpoint**. The connector treats it accordingly: `PubsubNotification` redacts the
address in `Debug`, and so does `PubsubMessageBody` for `data`, whose bytes decode to the same address — a
field that looks like a harmless opaque blob and is not (`ADR-0091`). The `historyId` beside it is a *position*
and stays printable, because a diagnostic about a stuck sync needs it and it names nobody.

### Finding 13 — A `watch` response carries two facts, and the guide's worked example uses a different number for each

The `users.watch` response is `{ "historyId": string, "expiration": string (int64 format) }` — two fields, and
the push guide says what the first is for:

> "The response contains the current mailbox `historyId` for the user. **Your client receives notifications for
> all changes after that `historyId`.**"

So the response's `historyId` is the **anchor** a first sync starts from — a `startHistoryId` — and not a
position the mailbox has reached. The guide then makes the distinction concrete with its own worked example,
whose two sentences use **two different numbers**:

> "Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can persist `9876543210` as the
> last known `historyId` for future use cases."

`1234567890` is the watch response's `historyId`; `9876543210` is the position the resulting `history.list`
reached. **The two ends of one operation, both spelled `historyId`, both strings, both plausible as the other.**
Reading the response's id as "the new position" type-checks and is wrong, and it fails without terminating: an
anchor does not move when the mailbox changes, so a sync that stored it as the position would re-read the same
window on every run.

**And the guide documents TWO branches for a first sync, not one.** The sentence immediately after the anchor's
definition is a fork, and the connector could express only the second:

> "The response contains the current mailbox `historyId` for the user. **Your client receives notifications for
> all changes after that `historyId`.** If you need to process changes **before** this `historyId`, refer to
> Synchronize clients with Gmail."

- **Branch one** — `history.list` *from the anchor*, which returns the changes since the `watch` was set
  (typically none) and yields a position of its own. The mailbox is not read.
- **Branch two** — the mailbox's **existing** contents, which is what *"changes before this `historyId`"*
  means, at `5 + 20N` quota units for *N* messages.

The anchor had a parser, a field on [`WatchResponse`] and two modules' worth of justification, and **no consumer**
— `resume_from` answered `FullSync` for every `Start` cursor, so a just-connected mailbox was read end to end.
Implemented as `ResumePoint::FromAnchor` + `SyncOrigin` + `resume_anchored`/`resume_ignoring_anchor` and the
anchored `establish_account_*` entry points (`ADR-0110`). **The two branches are two functions rather than a
flag**, because a `bool` at a call site says nothing about which branch is which while the difference in cost is
orders of magnitude.

**Why the example is the load-bearing part.** A one-value example cannot falsify a conflation of two
same-typed fields — every reading of it passes. The `1234567890`/`9876543210` pair can, which is why the
connector's test asserts the anchor is the first **and** `assert_ne!`s it against the second. The
`google::watch` module had a reader for `expiration` and none for `historyId` at all, so a caller using the
crate's one watch reader got a lease with no anchor; both fields are now read together (`ADR-0092`).

### Finding 14 — The `watch` **request** advertises a deprecated field the provider does not reject, and a filter that governs nothing without a label list

`POST /gmail/v1/users/{userId}/watch` is the only operation in `P5-005` that carries a **JSON body**, and the
reference publishes two fields whose misuse produces a `200` rather than an error. The reference's body shape:

```json
{ "labelIds": [ string ], "labelFilterAction": enum (LabelFilterAction),
  "labelFilterBehavior": enum (LabelFilterAction), "topicName": string }
```

**The deprecated spelling is accepted and ignored, not refused.** The reference says of `labelFilterAction`:
*"deprecated because it caused incorrect behavior in some cases; use `labelFilterBehavior` instead"*, and of the
newer field: *"This field replaces `labelFilterAction`; if set, `labelFilterAction` is ignored."* So sending the
old field is not a `4xx` — it is either **ignored** (when the new one is present) or produces the "incorrect
behavior" the provider itself names. Neither is reportable downstream.

**And the filter is relative to a list, with no error when the list is absent.** `labelIds` is the thing that
*"dictates which labels are required for a push notification to be generated"*, and `labelFilterBehavior` is the
*"filtering behavior of `labelIds` list specified"*. So `include`/`exclude` sent with no list select nothing —
and the provider registers an **unfiltered** watch rather than refusing. A connector would then receive every
change while believing it had scoped the set, with valid notifications and a healthy-looking lease
(`historyId` and `expiration` as usual); only the *set* would be wrong.

**Two values per enum**, both named in the reference: `include` — *"Only get push notifications for message
changes relating to labelIds specified"* — and `exclude` — *"Get push notifications for message changes except
those relating to labelIds specified"*. The connector models the closed vocabulary and produces only the new
field name, so the deprecated spelling is unreachable rather than discouraged (`ADR-0093`).

**A host discrepancy between two official sources, recorded rather than resolved.** The method reference gives
the request as `POST https://gmail.googleapis.com/gmail/v1/users/{userId}/watch`, while this record's Verified
Contract and the API's error pages use `https://www.googleapis.com/gmail/v1`. Both hosts serve the API, so
neither reading is wrong — and the connector's base is `www.googleapis.com` ([`GMAIL_API_BASE`]), which is the
host the rest of this record was verified against. **Not changed here**, because moving a base on the strength
of one page's example rendering — against a source that does not contradict the host the other pages use — is
churn; recorded so a reader comparing the two pages knows the difference was seen and deliberately left.

### Finding 15 — A negative acknowledgement is charged to the **whole subscription**, and the subscriber cannot opt out

The push page's acknowledgement rule is one sentence, and this record already carries half of it: *"To
acknowledge the message, return one of the following status codes: `102`, `200`, `201`, `202`, `204`."* The
other half is what makes the answer a **decision** rather than a verdict on one message:

> "If a push subscriber sends too many negative acknowledgments, Pub/Sub might start delivering messages using
> a push backoff. When Pub/Sub uses a push backoff, it stops delivering messages for a predetermined amount of
> time. This time span can range between 100 milliseconds to 60 seconds."

and, in the page's own list of considerations:

> "• Push backoff can't be turned on or off. You also can't modify the values used to calculate the delay.
> • Push backoff triggers on the following actions: When a negative acknowledgment is received. When the
> acknowledgment deadline of a message expires.
> • **Push backoff applies to all the messages in a subscription (global).**"

**Four facts, and each has a consequence:**

- **Global.** One delivery's answer delays *every* account on the subscription, so the cost of refusing is not
  bounded by the message. A connector watching several mailboxes pays for one bad payload with all of them.
- **100 ms – 60 s, exponential**, *"calculated based on the number of negative acknowledgments that push
  subscribers send"* — so a message refused forever keeps the delay near its ceiling rather than decaying.
- **Not modifiable.** There is no subscription setting to disable it, unlike the retry policy, which the page
  notes is a **separate** feature whose delay **adds** to the backoff's ("the total delay is the maximum
  combined value of both").
- **Two triggers**, and one of them is not a response at all: an **expired acknowledgment deadline** also
  triggers it — so a *slow* handler that never answers is indistinguishable, to the backoff, from one that
  refuses. And a push subscriber *"can't modify the acknowledgment deadline of individual messages"*.

**What this forces.** A delivery whose payload this connector will never decode — an unwrapped subscription, a
changed shape — fails identically on every attempt, so a refuse-and-retry handler answers a negative code every
time and holds the subscription's backoff up indefinitely. The bounded answer is to **acknowledge it and record
the drop**, which loses the message but not the subscription. The connector models the three answers as
`DeliveryAck::{Accept, Retry, AbandonAndAcknowledge}` and bounds `Retry` by the provider's own
`deliveryAttempt` (`ADR-0094`). The bound is a **JARVIS figure**, because the page publishes the backoff range
and the global scope but no retry count a subscriber should use — the distinction this record keeps elsewhere.

### Finding 16 — `users.stop` requires an authorization that revocation destroys, so teardown has a forced order

Disconnecting a mailbox is **two** operations, and their order is not a preference. `users.stop`:

> "Turn off push notification delivery for the given user mailbox. … `POST
> https://gmail.googleapis.com/gmail/v1/users/{userId}/stop`"

It requires *"one of the following OAuth scopes"*: `mail.google.com/`, `gmail.modify`, `gmail.readonly`,
`gmail.metadata` — **the same four `users.watch` needs**, so it is an ordinary authenticated call. Revocation,
whose project-wide effect this record already carries, **removes exactly those scopes**. So:

- **stop then revoke** — both succeed;
- **revoke then stop** — revocation succeeds (RFC 7009 §2.2's `200` covers a dead token), and the stop goes out
  with an invalidated token and fails.

**And the failure is a silent privacy exposure, not a failed call.** `stop`'s own page says *"All new
notifications should stop within a few minutes"* — but that only applies if the stop happened. With the watch
still registered and the grant gone, nothing ends the stream but **the lease lapsing**: the push guide's
*"at least once every 7 days"*, i.e. up to a week of the mailbox's address arriving at an endpoint the user
believes is disconnected, with no credential left to turn it off. The connector models this as a **plan** with
the ordering rule enforced by a function, the stop best-effort and the revoke required (`ADR-0095`).

### Finding 17 — `users.getProfile` supplies the account identity, and it accepts **no OIDC scope**

The JARVIS Mapping above names the account identity as `VerifiedAccount` from `users.getProfile`, and that
operation is what makes `tools-and-connectors.md`'s "account identity verified from the provider, not
user-entered labels" satisfiable. The method reference:

> "Gets the current user's Gmail profile. … `GET https://gmail.googleapis.com/gmail/v1/users/{userId}/profile`"

with the response as `{ "emailAddress": string, "messagesTotal": integer, "threadsTotal": integer,
"historyId": string }` and `emailAddress` described as *"The user's email address"*.

**Its authorization is where a plausible reading goes wrong.** The reference requires *"one of the following
OAuth scopes"*: `mail.google.com/`, `gmail.modify`, `gmail.compose`, `gmail.readonly`, `gmail.metadata` —
**`openid` is not among them.** So the scope the connector requested "so that the granting account's *identity*
is available" is not the scope that makes the identity available: a caller holding only `openid` would be
refused by `getProfile`. The Gmail read scope the connector already has is what the profile read needs.

`openid` is still requested, for a different reason than the one recorded: it is what makes Google return an
`id_token`, which the token exchange receives and this crate deliberately does not verify — and the `nonce`
`P5-002` already generates and carries is what a future ID-token check would compare, so the pairing is a
**prepared seam, not a working feature** (`ADR-0096`).

**The profile response carries a mailbox position as well as an identity.** `historyId` is *"The ID of the
mailbox's current history record"*, so a profile read yields a sync anchor without consuming a message — the
same *kind* of value `users.watch`'s response supplies, and like that one it is the mailbox's position at the
moment of the read rather than the position a sync ends at (Finding 13). `messagesTotal` and `threadsTotal` are
mailbox counts, and neither the operation's output schema nor its renderer declares them because nothing reads
them.

### Finding 18 — a notification's only routing key is a mailbox address, on a channel that cannot be authenticated

The push payload is `{"emailAddress": "user@example.com", "historyId": "9876543210"}` — and **`emailAddress` is
the only field that names a mailbox**. Nothing else in the delivery identifies the account: `messageId` is
*"a Cloud Pub/Sub message ID, unrelated to Gmail messages"*, `historyId` is a position that is meaningful only
*within* a mailbox, and the envelope carries **no message content and no change detail**. So attributing a
delivery to one of the connector's accounts means matching that address against the addresses it holds.

**And the channel it arrives on cannot be authenticated.** Finding 1 establishes that neither Google mechanism
fits `WebhookSupport::Push` (an OIDC bearer JWT, and an echoed channel token over a zero-length body), so the
address is **untrusted input**. The consequence is bounded by two facts rather than by the address being
trustworthy: the route selects **which mailbox to read** (the sync uses that account's own stored token, so a
forged delivery reaches only mailboxes the connector was already authorised to read), and the notified
`historyId` is **not trusted as a position** (`history.list` runs from the stored cursor, so a forged id cannot
make the connector skip changes) — `ADR-0097`.

**The address is matched byte-exactly, and a case-only near-match is reported rather than applied.** Google
publishes no canonicalisation rule for `emailAddress`, so `User@example.com` against a stored
`user@example.com` is *evidence* about identity and not identity; promoting it would read the wrong mailbox if
the two spellings were two accounts.

### Finding 19 — one address, one account: the duplicate that makes a delivery unroutable

The connect-time flow the previous two findings named is where a stored identity is created, and **the profile's
`emailAddress` becomes the account's `provider_account_id`** — the value Finding 18's router matches a
notification against. So this step decides whether that lookup is single-valued, and a second account for one
address is **refused** rather than allowed. The cost of allowing it is not tidiness: once two rows carry one
address the router answers `Ambiguous`, which may not be applied without a person, so every notification for
that mailbox stops being acted on — while the duplicate itself looks exactly like two mailboxes (two cursors,
two schedules, a quota budget paid twice).

**The comparison ignores ASCII case, which is deliberately the opposite direction from routing.** Finding 18
refuses to *act* on a case-only near-match because no canonicalisation rule is published; this refuses to
*create* one, for the same uncertainty and because the directions differ in cost — refusing asks a person
(recoverable), while creating a duplicate is silent and needs one to be undone. **Both are the same restraint:
neither acts on an uncertain case-match.**

**And the profile carries no display name.** `users.getProfile` returns `emailAddress`, `messagesTotal`,
`threadsTotal` and `historyId` — so the account is stored with a `None` display name rather than the address
passed as one, which would be inventing a provider statement. Both facts are `ADR-0098`.

### Finding 20 — One channel expiry, three encodings: an epoch-millis number, an epoch-millis string, and a human-readable date

A notification channel's expiry is one quantity and Google publishes it in **three** forms, in three documents,
across two push mechanisms:

| Where | Type and unit (verbatim) | Document |
| --- | --- | --- |
| `watch` **response** `expiration` (**Calendar**) | *"The channel expiration time as a **Unix timestamp in milliseconds**."*; the method reference types it `expiration \| long` | Calendar push guide (*Watch response*) / `events.watch` reference |
| notification header `X-Goog-Channel-Expiration` (**Calendar**) | *"Date and time of notification channel expiration, expressed in **human-readable format**."* (e.g. `Tue, 19 Nov 2013 01:13:52 GMT`) | Calendar push guide (*Headers*) |
| `watch` **response** `expiration` (**Gmail**) | *"When Gmail will stop sending notifications for mailbox updates (epoch millis)"*, declared `string (int64 format)` | `users.watch` reference; Finding 10 |

**Only one of the three can be compared against a clock without a date parser**, and it is the response body's
number — so the form `ChannelMessage` already **held** (the header, on every delivery) is the one form a renewal
decision *cannot* use, and the form that *can* be used was the one nothing read. Implemented as
`google::channel::parse_channel_watch_response`, which scales millis → nanoseconds by × 1,000,000 and pins the
factor against Google's own `1426325213000` (`ADR-0106`).

**A contradiction inside the Calendar guide's own request section.** The guide's *Optional properties* calls
`expiration` *"An `expiration` **property string** set to a Unix timestamp (in milliseconds)"* while the
`events.watch` reference — a page the guide links to — types the same field `expiration | long`. **The reference
wins**, on two grounds: it is the machine-readable schema for the method being called, and `long` is a type while
"property string" is prose that also appears immediately above for `params.ttl`, which the same reference types
`string`. So the connector reads a **number** and refuses a string with a named error rather than accepting both
— a *string* here is one of the other two encodings in the table above, and silently accepting it would erase
which document declared which form.

**The ttl default is the same seven days as Gmail's renewal bound, by a different mechanism.** The `events.watch`
reference: *"params.ttl | string | The time-to-live in seconds for the notification channel. **Default is 604800
seconds.**"* So Calendar's default channel life is `604800` — **numerically identical** to Gmail's
`WATCH_RENEWAL_BOUND_SECONDS`, with the same "no number for the margin, just an expected overlap" quality on the
renewal side. Two mechanisms, one figure, arrived at independently: worth recording because it makes the figure
look like a shared constant when the documents are separate.

**Renewal is a replacement, verbatim.** The push guide, *Renew notification channels*:

> "Currently, there's no automatic way to renew a notification channel. When a channel is close to its
> expiration, you must **replace it with a new one** by calling the `watch` method. As always, you must use a
> **unique value for the `id` property of the new channel**. Note that there's likely to be an **'overlap' period
> of time** when the two notification channels for the same resource are active."

**Four facts, and each changes what a type has to be:** the channel is not extended (a *second* channel is
created), the new id must be **unique**, the old channel keeps delivering during an **overlap** (so duplicate
deliveries for one resource are expected, not a bug), and **the overlap has no published duration** — so the
"close to its expiration" margin is JARVIS's own figure and must be labelled as such rather than attributed to a
page (`ADR-0080`). Implemented as `CHANNEL_REPLACE_LEAD_SECONDS` with `ChannelRenewal::{ReplaceNow, ReplaceSoon,
NotYet}` (`ADR-0106`).

**And `resourceId` is what the stop call needs, which is why the response is parsed rather than discarded.** The
guide: *"This method requires that you provide at least the channel's `id` and the `resourceId` properties"*, and
the stop body is exactly those two. The response is also the only place `resourceId` appears **before** a
notification arrives — the `sync` message carries it too, but the guide warns it *"is possible to receive the
`sync` message even before you receive the `watch` method response"*, so a stop built on the `sync` message alone
has a race the response does not.

### Finding 21 — the rule that governs storing a `historyId` lives in *another* field's description, and four layers of this crate said otherwise

The `users.history.list` response's `historyId` field is described as nothing more than *"The ID of the
mailbox's current history record."* **It says nothing about storing it.** The condition that governs storing is
stated **once**, in `startHistoryId`'s description instead:

> "If you receive no `nextPageToken` in the response, there are no updates to retrieve and you can store the
> returned `historyId` for a future request."

So a page's `historyId` is the mailbox's position **at the moment that page was produced**, and a walk with more
pages to come has not consumed the changes up to it. Four statements in this crate asserted the unconditional
form, in four layers, and **each is true of a final page and false of a continuing one**: a renderer comment
(*"the durable cursor"*), `HistoryPage`'s field doc (*"the next sync cursor"*), the **output schema's**
description (*"This is the next sync cursor…"* — which a **model** reads and acts on), and
`gmail_history_signal`'s doc, which inferred *"the mailbox was unchanged"* from an **absent** id that the
response does not document as optional. Implemented as `HistoryPosition` (`Storable` / `UnfinishedWalk` /
`Unstated`) with `of_page` encoding the rule from **both** fields, and the signal producer taking the type rather
than a bare id (`ADR-0108`).

**The third layer is the one that decides the design.** An output schema's `description` is not documentation for
a reader of this repository — it is an **instruction to a model**, delivered in the model's own channel. A model
told "this is the next sync cursor" stores it, and a mid-walk id stored as a cursor positions the next sync
**past** the changes still sitting in the un-walked pages. The loss is silent in both directions at once: a page
token that expired un-walked, and a cursor asserting everything before it was consumed.

**And the field ordering is a trap worth recording**, because it is what made the rule hard to find: the
condition is in the **request** parameter's description, while the value it governs is in the **response** body.
A reader checking the field they intend to store finds a sentence with no bearing on the question.

### Finding 22 — two push mechanisms, two stops of different arity, and a permission rule that cannot be checked locally

Ending notifications is **not one operation per account**. Google's two mechanisms are ended by calls of
different arity, and only one of them has a per-user form:

| Mechanism | Call | How many |
| --- | --- | --- |
| Gmail mailbox watch | `POST …/gmail/v1/users/me/stop` | **one** per account — the watched resource *is* the mailbox |
| Calendar channel | `POST …/calendar/v3/channels/stop` | **one per channel**, and an account may hold several |

The guide is explicit about the second: *"This method requires that you provide at least the channel's `id` and
the `resourceId` properties… **Note that if the Google Calendar API has several types of resources that have
`watch` methods, there's only one `stop` method.**"* Combined with *"Each notification channel is associated
both with a particular user and a particular resource (or set of resources)"*, the consequence is that a channel
is identified by a **pair** — the channel id says *which channel*, the `resourceId` says *which watched
resource* — and an account watching three calendars needs three calls. **A teardown that performed one stop and
reported "notifications stopped" is therefore wrong for whichever mechanism it skipped**, which is why
`ADR-0107` splits `TeardownStep::StopWatch` from `TeardownStep::StopCalendarChannel` rather than treating the
second as an instance of the first.

**And the exposure figure cannot be shared between them, for a reason that is about the sources rather than the
numbers.** Gmail's lease has a bound Google states for the mechanism (Finding 10, `604_800` seconds). Calendar
publishes **no equivalent bound for a channel**: its life is *"determined either by your request or by any
Google Calendar API internal limits or defaults (the more restrictive value is used)"* — so the only honest
figure for a channel is the expiry its own `watch` response reported, which is the epoch-millis number of
Finding 20. The two functions take different inputs because the two mechanisms are documented differently.

**The stop permission rule is documented and locally unenforceable.** The guide:

> "Only users with the right permission can stop a channel. In particular: If the channel was created by a
> regular user account, **only the same user from the same client (as identified by the OAuth 2.0 client IDs
> from the auth tokens) who created the channel** can stop the channel. If the channel was created by a service
> account, any user from the same client can stop the channel."

That is **two rules keyed on how the channel was created** (a regular user account versus a service account),
and the discriminating fact is the `client_id` **inside the token**. This crate deliberately exposes a
credential only as a rendered `Authorization` header value (`ADR-0061`), so the client id is not readable
without unpacking or introspecting the token — and a *JWT access token's* `aud`/`azp` are claims this crate has
already recorded as **unverified** (`ADR-0064`). So the rule is recorded as a limit rather than implemented as a
check: a violation surfaces as the provider's `403`, and a local refusal would have to trust an unverified claim
to be stricter than the provider — the failure direction `ADR-0064` warns about. **The permission is also a
property of the token, not of the channel**: the same channel id and `resourceId` presented with a credential
from another client cannot stop it, which means a stop is only ever attempted with the credential the `watch`
used.

### Finding 23 — the connector could **stop** a Calendar channel and could not **create** one, and the create body carries two credential-adjacent fields

`channels.stop` was built (`ADR-0106`/`ADR-0107`) and `renewal_decision` prescribes the remedy for a lapsed
channel — *"you must replace it with a new one by calling the `watch` method"* — while **no `events.watch`
request builder existed**. So the connector could end a channel and had no way to make one, which is the
"a remedy with no operation" shape from a new direction: not a decision without a caller, but a **prescribed
call with no builder**. The response parser (`parse_channel_watch_response`) and the lease types were built
around a `watch` that nothing could issue.

**The request body is four fields, and two of them are credential-adjacent.** The `events.watch` reference gives
the body as `{ id, token, type, address, params }`, and the push guide states which are required:

| field | required? | documented value |
| --- | --- | --- |
| `id` | **yes** | *"A UUID or similar unique string that identifies this channel… **Maximum length: 64 characters**"*, **echoed** as `X-Goog-Channel-ID` |
| `type` | **yes** | `"web_hook"` (*"or `webhook`"*) |
| `address` | **yes** | *"the URL that listens and responds to notifications… **must use HTTPS**"* |
| `token` | no | *"an arbitrary string value… **Maximum length: 256 characters**"*, echoed as `X-Goog-Channel-Token` |
| `params.ttl` | no | *"The time-to-live in seconds… **Default is 604800 seconds**"* |

So the body carries **the webhook address** (an endpoint, potentially internal) **and the channel token** (the
anti-spoofing secret `verify_channel_token` compares against). That matters because
[`JsonRequest::rendered_body`](crate::google::request::JsonRequest::rendered_body) is documented as *"not
redacted… there is no credential here"* and derives `Debug` — a claim that was true of the two bodies it had
(topic name + label ids; channel id + resourceId) and **false of the third**. A derived `Debug` on the third
body would print the token, the same defect `ADR-0091` records for four types that held a sensitive field and
printed it. So the type's redaction has to become **body-dependent**, which is a change a derived `Debug`
cannot make.

**The `address` is validated for shape only, and that is a boundary rather than an omission.** The guide
requires HTTPS *and* a valid (non-self-signed, non-revoked, subject-matching) certificate at the receiving host,
but **a certificate is not observable at request-construction time**: whether the host presents a valid chain is
a fact about a TLS handshake this crate's request value (which has no socket) cannot perform, and whether the
address is even *reachable* is untestable offline. So the builder enforces what is knowable — a non-empty
`https://` absolute URL with a host, no control characters, a bounded length — and records the certificate rule
as the provider's own `400`-class refusal rather than pretending to check it.

**The `id` echo is what makes the create response the registration's other half.** The `watch` response returns
`id`, `resourceId`, `resourceUri`, `token` and `epoch-millis expiration`; the connector already parses `id`,
`resourceId` and `expiration` (`ADR-0106`/`ADR-0107`). The `id` the connector **chose** comes back, which is
what lets a created channel be bound to a registration: the two identifiers a `channels.stop` needs are the one
the connector sent and the one the provider returned, so creation and teardown are two halves of one record.


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
  a fixture with a date, because Google can recategorise. **WRITTEN.** `google::scopes` carries the category,
  burden and assessment types plus `account`, and `gmail_scope_categories()` is the dated table. The check runs
  against the **connector's real manifest scopes**, so adding a scope is what makes it speak — and it reports
  `openid` and `calendar.readonly` as **unaccounted**, which is the honest result rather than a default. The
  table's date is asserted equal to this record's `last_verified`. See `ADR-0078`.
- **A quota-cost test.** `users.getProfile` = 1 and `messages.get` = 20 as fixtures, and a computed estimate for
  a full sync of *N* messages (`5 + 20N`) — the cheapest test that would catch a connector that planned its
  budget from message counts alone. **WRITTEN.** The per-call cost is now a declared `QuotaCost` on each
  operation rather than prose inside a `description`, and the test asserts `1,200,000 / 20 = 60,000` calls per
  window for `messages.get` against `600,000` for `history.list` — the 10× spread a single shared limit erased.
  `calendar_events_read` declares `Unstated`, because this page publishes no Calendar cost. See `ADR-0079`,
  which also found that the ceiling figures were being read as **requests** when they are quota units.
- **A full-sync budget test that asserts batching.** A full sync must batch (≤50 per batch) *and* must respect
  that batches trigger rate limiting, so the test asserts both the batch size and the delay. **WRITTEN, and the
  premise was wrong.** The "≤50" was not a limit: the batch reference states a **hard limit of 100** and a
  **recommendation of 50**, so `batch_plan(calls, size)` now takes the size and reports both the request count
  (rounding **up**, so the partial final batch is not dropped) and whether the size is within the recommendation.
  A 1,001-call first sync is asserted to be 21 requests of 50 with the last holding one. No delay is asserted,
  because the record states none for a recommended size — it states that throttling is *likely*, which is a risk
  rather than a figure (see `ADR-0080`).
- **The 404-is-staleness test.** A `history.list` fixture returning 404 must produce "resync from scratch", not
  "account not found" — and a fixture for a genuinely absent account must produce the opposite. **This is the
  cheapest test that would disprove the central assumption of Finding 2**, and it was the item most worth
  writing first. **WRITTEN, AND IT DISPROVED THE ASSUMPTION.** `tests/fixtures/google/gmail_history_404_no_reason.json`
  plus `the_gmail_history_404_fixture_carries_no_reason_code` establish that the fixture carries **no `errors`
  array and therefore no `reason` code**, so the test the plan asked for — "404 must produce resync rather than
  account-not-found" — **cannot be written as stated**, because the response does not carry the information a
  discrimination would need. What replaced it: a fixture asserting the *absence*, and code that resyncs on a
  caller-supplied signal whose wrong reading is self-correcting (see Finding 2). The predicate
  `gmail_history_status_is_pruned`, which claimed the discrimination, was **removed**.
- **The 410-is-staleness test** for Calendar, plus 400-is-a-query-error. **WRITTEN.** Three fixtures now
  exist: `calendar_error_410_full_sync_required.json` and `calendar_error_410_resource_deleted.json` (a
  **same-status pair with opposite remedies**, which is what makes "every 410 wipes the store" falsifiable) and
  `calendar_error_400_time_range_empty.json`. The classification is `CalendarGoneReason` with
  `client::calendar_signal` as its producer, asserted in both directions — and the premise was corrected: the
  errors page shows **three** `410` causes, not one, and the `deleted` one says *"no further action is
  necessary"*. See `ADR-0081`.
- **A `history.list` producer for the staleness signal.** The item `ADR-0066` left open: it made the signal a
  caller's parameter but nothing could *build* one from a response, so the resync remedy was still unreachable
  outside a fixture. **WRITTEN.** `client::gmail_history_signal(status, history_id, refusal)` produces all
  three `SyncSignal` cases from a status, and `gmail_history_list.json` /
  `gmail_history_list_last_page.json` supply the two shapes. The signal is asserted in both directions: a `404`
  becomes `CursorUnusable` (reaching `SyncAdvance::HistoryPruned`), and the retryable family (`429`, `5xx`)
  becomes `Refused` with its classification — because a resync on a transient failure discards a working store.
- **The `Retry-After` field has two forms, and only one is a number.** Fetched from the specification rather than
  a Google page, because the field is defined by HTTP and not by Google: **RFC 9110 §10.2.3** states
  `Retry-After = HTTP-date / delay-seconds` with `delay-seconds = 1*DIGIT`, and §5.6.7 makes the date form one a
  recipient **MUST accept** (three formats, with `IMF-fixdate` preferred). A connector that parsed digits only
  therefore read a conforming `HTTP-date` as **no `Retry-After` at all** — the direction that retries too soon,
  since the field exists to say *do not ask again before this time*. **WRITTEN** as `google::transport::RetryAfter`
  plus `parse_retry_after`, with the classifier keeping *absent*, *stated as seconds*, and *stated but unreadable*
  apart (`ADR-0076`). The date form is **recognised but not converted to a delay**, which remains a recorded
  limit because the conversion needs a clock.
- **The `history.list` response shape.** The method reference documents  `{ "history": [ { object (History) } ], "nextPageToken": string, "historyId": string }` where `historyId` is
  "The ID of the mailbox's current history record", and it states that when **no** `nextPageToken` is returned
  "there are no updates to retrieve and you can store the returned `historyId` for a future request". Two
  consequences are now pinned by the fixtures: (a) `historyId` is present on **every** success while
  `nextPageToken` appears only mid-walk — so they are **not** a mutually-exclusive pair the way Calendar's
  `nextPageToken`/`nextSyncToken` are, and a reader may not treat them as one; and (b) `startHistoryId` is
  **Required**, so unlike `messages.list` an incremental sync has no meaningful empty-argument form.
- **A correction my own draft carried, found by reading the field descriptions rather than the example.** My
  first `gmail_history_list` output schema made `history_id` **required**, on the assumption that the response
  always carries the mailbox's new position. The reference does not say that: it says the id may be stored when
  no page token is returned, which is a statement about *when it is usable* rather than about *when it is
  present*. Requiring it would have made the connector's own declaration stricter than the provider's, so the
  field is optional and absent-with-a-page-token is representable. Same family as the Calendar
  `nextPageToken`/`nextSyncToken` finding: **the constraint lives in the field description, not the sample.**
- **A `domainPolicy` test.** A 403 with `reason: domainPolicy` must be `Permanent` and must not be retried,
  which a status-code-only classifier cannot distinguish from `rateLimitExceeded`. **WRITTEN** — see below.
- **A recording fixture for the Pub/Sub envelope** — a sanitized delivery with `message.data` Base64URL-decoded
  to `{"emailAddress":…,"historyId":…}` — so the envelope handling is tested without a Pub/Sub subscription.
  **WRITTEN**, and the decode found a **contradiction in the sources**: the guide says `message.data` is
  Base64URL while the `PubsubMessage` reference it links to types the field `string (bytes format)`, "a
  base64-encoded string". The guide's own example value is the fixture, asserted to decode to both documented
  fields — and because it uses only `A-Za-z0-9` it cannot distinguish the two alphabets, so the test that
  catches the difference uses one of the three printable-ASCII payloads that force it. See `ADR-0088` and
  Finding 11.
- **A Calendar sync-message fixture**, including that it can arrive *before* the `watch` response and that
  `X-Goog-Message-Number` is `1` for it. **WRITTEN** by `ADR-0100`: `google::channel` reads the notification
  from `X-Goog-*` **headers** (the delivery has a zero-length body), `is_sync()` distinguishes the handshake
  from a change, and two fixtures (`calendar_channel_sync.json`, `calendar_channel_message.json`) are swept by
  the fixture-declaration test. The "arrives before the watch response" and "number is 1" facts are recorded in
  the module doc and carried by the sync fixture — but **detection is by the state, not the number**, because
  the number is documented as non-sequential. The **channel-token comparison** is now built (`ADR-0101`, below);
  what is **not** built is anything that *receives* a notification — no endpoint and no channel registration.
- **An unauthenticated-delivery test**, asserting the refusal **fails closed** — the same shape as `P5-001`'s
  falsified guard. **WRITTEN for the Calendar mechanism** by `ADR-0101`: `verify_channel_token` is exercised
  **both ways**, so it is not a guard that refuses everything — a matching token **verifies**, a mismatched or
  absent one is **refused**, and a channel registered **without** a token is `Absent` (not a refusal). The
  Gmail mechanism's **`OidcIdToken`** half remains unbuilt (no JWKS reader, no `aud`/`iss`/`exp` check), so its
  fail-closed test is still unwritable — and the two halves must ship together for that mechanism or the test
  would prove only that everything is refused.
- **A renewal test.** A watch whose lease is near expiry must be renewed before it lapses, and the Gmail 7-day
  bound must be asserted, because a lapsed watch stops notifications **silently**. **WRITTEN** by `ADR-0087`:
  `WATCH_RENEWAL_BOUND_SECONDS` is asserted equal to `604_800` and its relationship to
  `WATCH_RENEWAL_RECOMMENDED_SECONDS` (`604_800 > 86_400`) is what makes `Overdue` reachable, and
  `every_state_except_not_yet_asks_for_a_renewal` pins the cadence. What is **not** built is the caller that
  acts on the advice — no scheduler renews anything yet.
- **A Calendar channel-lease test**, added once `ADR-0106` read the `watch` **response** and found the expiry
  arrives there in a *different encoding* from the notification header's. **WRITTEN** by `ADR-0106`:
  `parse_channel_watch_response` reads `id`/`resourceId`/`expiration` (**a number** of milliseconds, per the
  `events.watch` reference's `expiration | long`) and the test pins Google's own `1426325213000` to
  `1426325213` seconds, so a wrong scale factor cannot pass. `channel_lease`/`renewal_decision` then answer
  `ReplaceNow`/`ReplaceSoon`/`NotYet` against the **expiry** rather than a cadence, with the exact expiry instant
  **lapsed**. The fixture `calendar_channel_watch_response.json` drives it end to end. What is **not** built:
  the replacement itself — no second `watch` with a new unique `id`, so the documented overlap is not exercised
  and no duplicate-delivery deduplication key has been decided. The margin `CHANNEL_REPLACE_LEAD_SECONDS` is a
  **JARVIS figure** because Google publishes no number for the overlap.
- **A teardown test for the Calendar channel stop**, which is where the ordering rule `ADR-0095` built met its
  first new step. **WRITTEN** by `ADR-0107`: `TeardownStep::StopCalendarChannel` is admitted by `may_precede`
  **without the rule being edited** (which is the answer to `ADR-0095`'s revisit question — the rule tests
  *authority*, not the operation), `calendar_channel_stop` builds a body of **exactly** `id` and `resourceId`
  which are asserted by parse-and-compare rather than substring, and `calendar_exposure` reports a live channel's
  **own** lease rather than Gmail's constant — with a third state, `AlreadyEnded`, for a channel whose lease had
  already lapsed, which a `bool` function would have reported as an open window. What is **not** built: the
  teardown executor, `users.stop` (a different method with an empty body), and any check of the stop **permission**
  rule — the client id it depends on lives inside the token, which this crate exposes only as a rendered header
  value. A test that counted channels is also impossible here, because the **count is a runtime fact**: the plan
  names the effect and the caller performs it once per channel.
- **A test that the history cursor is stored only from a completed walk**, added once the record's own field
  descriptions were checked against the reference and found to state the rule **unconditionally**. **WRITTEN** by
  `ADR-0108`: `HistoryPosition::{Storable, UnfinishedWalk, Unstated}` with `of_page` encoding the rule from the
  page token **and** the id together, `gmail_history_signal` taking that type rather than a bare `&str`, and the
  output schema's description carrying the condition and the direction of the mistake. The two fixtures
  (`gmail_history_list.json` mid-walk, `gmail_history_list_last_page.json` complete) assert the split in both
  directions, and the `UnfinishedWalk` case is asserted **through** `advance_gmail_history` to show the previous
  cursor surviving. What is **not** built: nothing stores a cursor — there is no sync loop — so the rule is
  representable and tested rather than enforced on a store.
- **A test that the two Calendar continuation tokens cannot arrive together**, added after the constraint was
  found to be recorded in **three** layers and enforced in **none**. **WRITTEN** by `ADR-0109`:
  `CalendarContinuation::{MorePages, WalkComplete, NothingFurther, Rejected}` with `of_page` deciding from
  **both** fields, `CalendarPage` carrying one value rather than two optionals, `calendar_signal` taking the
  type, the renderer emitting **at most one** token, and the output schema's two descriptions carrying each
  half of the rule. The parser test keeps the impossible body **deliberately** and asserts it is *reported*
  rather than resolved, so `Rejected` has an exercise. **Two guards falsified A-B-A**, and the second is a
  finding about the **suite** rather than the code: the mutation routing a page token through as the sync
  position **survived the whole `--lib` suite**, because `calendar_signal`'s 200 arm had no unit test while the
  Gmail producer's did — its only detector was the integration test. The missing unit test was written and the
  same mutant then fails in `--lib` too, **confirmed by mutation rather than assumed**. What is **not** built:
  no sync loop, so `storable` and `page_token` have no production consumer and each says so.
- **A test that a just-connected account syncs from its anchor rather than reading the whole mailbox**, added
  once the anchor's documented consumer was checked and found not to exist. **WRITTEN** by `ADR-0110`:
  `ResumePoint::FromAnchor` + `SyncOrigin` + `resume_anchored`/`resume_ignoring_anchor`, and the
  `establish_account_with_anchor` / `establish_account_from_watch` entry points returning `AnchoredAccount`. Both
  documented branches are asserted — the anchored start **and** the full sync the guide names for *"changes
  before this `historyId`"* — plus the guard that a **stored position outranks a historical anchor**, because the
  wrong direction there is a silent duplicate rather than a miss. **Two guards falsified A-B-A**: `resume_anchored`
  never anchoring, and the position check disabled. What is **not** built: no request is sent, no account is
  stored, no sync runs, and the anchor's **age** is not modelled — a
  long-held anchor used now may have been pruned, which arrives as the documented `404`/resync path rather than
  as a local refusal.
- **A test that a Gmail exposure is computed from the watch's own lease rather than the mechanism's bound**,
  added once `ADR-0107`'s recorded wiring gap was closed. **WRITTEN** by `ADR-0111`: `gmail_watch_exposure(lease,
  stop_succeeded)` reports `AlreadyEnded` from a **lapsed** `WatchLapse` and the lease's **own** remaining
  seconds from a live one — never `WATCH_RENEWAL_BOUND_SECONDS`, which would overstate a nearly-expired or
  already-dead watch. `gmail_exposure` (the bound-only figure) is **kept**, because a scheduler holds the
  mechanism's limit and no particular watch; both are asserted reachable. **Two guards falsified A-B-A**: the
  `stop_succeeded` guard inverted, and the live arm's `for_seconds` replaced by the bound. A third mutation —
  reordering the arms — changes nothing, since `Lapsed` and `Alive` are disjoint variants (`ADR-0107`'s lesson,
  applied rather than re-learned). What is **not** built: the teardown executor, so nothing computes the
  `WatchLapse` and passes it.
- **A test that a channel creation carries the required trio and redacts a token-bearing body**, added once the
  remedy `renewal_decision` prescribes was found to have **no builder**. **WRITTEN** by `ADR-0112`:
  `calendar_channel_watch(calendar_id, channel_id, address, token)` builds `{id, type, address}` or, with a
  token, `{id, type, address, token}`; `type` is the constant `web_hook` because Google describes no other
  mechanism (**Findings 1 and 20** name the two delivery shapes); **no expiry parameter** is offered, because the
  figure acted on is the **response's** (`ADR-0106`). The **token** made `JsonRequest`'s "there is no credential
  here" doc false, so the type's redaction became **body-dependent**: `Debug` is hand-written and the two
  constructors are private and named (`renderable`/`sensitive`), so a new builder must say which kind of body it
  produces. Four guards falsified A-B-A: the sensitive constructor un-redacted (**the token appeared in a
  `Debug`**), `strip_https_scheme` weakened (**`http://` accepted**), the channel-id bound relented to the generic
  256 (**a 65-character id accepted**), and the **non**-sensitive constructor redacting (**the control**: a
  printable body rendered `[REDACTED]`). The `address` **certificate** rule is a **recorded limit** — a valid
  chain is not observable at request-construction time (Finding 23). What is **not** built: no request is sent,
  no channel is registered, and the stop **permission** rule (`ADR-0107`) is still unenforceable locally.
- **A test that the value surviving the `watch` carries the input the renewal needs**, added once the create
  builder (`ADR-0112`) made the response obtainable and the surviving record was checked for the expiry
  `renewal_decision` takes as its **only** input. **WRITTEN** by `ADR-0113`: `ChannelRegistration` gains
  `expires_at` (the provider's reported instant, **Finding 20**'s epoch-millis number), `from_watch_response`
  (which makes the provider/connector split unnecessary so the third provider fact travels with the two
  identifiers), `expires_at()` and `renewal(now)` (which **delegates** to `renewal_decision`, asserted equal so
  the bridge is not a second opinion). **Three guards falsified A-B-A**: `renewal()` ignoring the registration's
  expiry, the blank-`resourceId` refusal removed, and `from_watch_response` not carrying the expiry. What is
  **not** built: no scheduler renews, so the margin and the bridge are not exercised together, and a registration
  is not persisted, so the new `UtcTimestamp` field has no stored column yet.
- **Opt-in live smoke test** behind credentials and a cost gate, as `tools-and-connectors.md` requires.
  **Not written.**

### Fixtures present, and what they are not

`crates/jarvis-connectors/tests/fixtures/google/` holds **fifteen** files, tested by `tests/google_fixtures.rs`:

| Fixture | Shape source |
| --- | --- |
| `gmail_messages_list.json` | `users.messages.list` response |
| `gmail_messages_get.json` | the `Message` resource |
| `calendar_events_list_page.json` | `events.list`, a **mid-walk** page |
| `calendar_events_list_last_page.json` | `events.list`, the **last** page |
| `gmail_error_403_domain_policy.json` | the error resource, 403 + `domainPolicy` |
| `gmail_error_403_rate_limit.json` | the error resource, 403 + `rateLimitExceeded` |
| `gmail_history_404_no_reason.json` | `users.history.list`, the 404 (no `reason` code) |
| `gmail_history_list.json` | `users.history.list`, a **mid-walk** page (both tokens present) |
| `gmail_history_list_last_page.json` | `users.history.list`, the **last** page (no page token) |
| `calendar_error_410_full_sync_required.json` | the Calendar error resource, 410 + `fullSyncRequired` (**resync**) |
| `calendar_error_410_resource_deleted.json` | the Calendar error resource, 410 + `deleted` (**no action**) |
| `calendar_error_400_time_range_empty.json` | the Calendar error resource, 400 + `timeRangeEmpty` |
| `calendar_channel_message.json` | a Calendar **push notification** (a change): `X-Goog-*` headers, **zero-length body** |
| `calendar_channel_sync.json` | a Calendar **push notification** (the `sync` handshake): state `sync`, number `1` |
| `calendar_channel_watch_response.json` | the **`watch` response body**: `id`, `resourceId`, `expiration` as an epoch-**millis number** |

The fixture count has been raised four times as files were added (six, then nine, twelve, fourteen, now fifteen),
and it is corrected each time rather than left to drift: a fixture directory whose stated size is stale reads as
"nothing changed" to the next reader. The three Calendar error fixtures are a **same-status pair plus a
control** — the two `410`s have opposite documented remedies, so they are what makes "every 410 wipes the store"
falsifiable. The two **channel notification** fixtures are the opposite wire **shape** from every other file
here: they are a **request** (method, path, headers) rather than a response body, because a Calendar
notification has no body — so they are the one pair whose `headers` object is parsed as headers rather than as
JSON the code reads (`ADR-0100`). The third channel fixture is the opposite again — a response body with **no
headers at all** — which is the point of it: the channel's expiry arrives as a header for a delivery and as a
body field for the `watch`, in two encodings, so the two shapes have to be held apart (`ADR-0106`).

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

**That correction was applied to three layers and missed the fourth, and `ADR-0109` is the record of it.** The
paragraph above, the fixture's own `_the_point_of_this_fixture` text and the renderer's test helper all state
the rule — while the **parser's** test went on asserting the impossible body, `CalendarPage` carried both tokens
as independent `Option<String>` fields, `calendar_signal` took a bare `Option<&str>`, and the **output schema**
declared both with no description at all. So three layers knew the constraint and none made it impossible, which
is the same "no type asked the question" shape `ADR-0108` records one tool over. Implemented as
`CalendarContinuation` (`MorePages` / `WalkComplete` / `NothingFurther` / `Rejected`), decided in `of_page` from
**both** fields, with the schema descriptions carrying each token's half of the rule.

**A second finding, about the test suite rather than the code.** The mutation that routed a **page token
through as the sync position** survived the entire crate-library suite, because `calendar_signal`'s 200 arm had
no unit test — while the Gmail producer's did. Its only detector was the integration test in
`tests/google_fixtures.rs`. **A guard whose only detector lives in a different test binary is one refactor from
being unguarded**, since `cargo test --lib` is what a developer runs while iterating. The missing unit test was
written and the same mutant then fails in `--lib` as well, confirmed by mutation rather than assumed.

**No live test was run for this slice and none is claimed.** No credentials were used, no Google API was called,
and no Cloud project was created.

### The transport, and what it does and does not add

The port `GoogleTransport` had only test doubles until now. `crate::google::http` implements it against
`reqwest` 0.13.5, pinned by the workspace with `default-features = false` and `rustls` — the same version and
feature set `jarvis-models` and `jarvis-mcp-transport` already resolve, so declaring it added **no package** to
the lock file (one dependency edge, verified with `cargo tree`). `repository-layout.md` names `client.rs` as the
"provider HTTP client", and the decision layer in this crate still names no `reqwest` type, so the split between
*decisions* and *transport* is preserved.

**Four port requirements became enforceable controls, each with a test against a hand-written HTTP server**
(the `jarvis-mcp-transport` precedent: a framework would share assumptions with the client under test):

| Requirement | Control |
| --- | --- |
| Do not follow a redirect | `redirect(Policy::none())`; a `302` arrives as a `TransportResponse`, and the redirect *target* server is asserted to have received **zero** connections |
| Do not retry | exactly one request per `send`; there is no retry branch to disable |
| Do not read a proxy from the environment | `no_proxy()` called explicitly (the `system-proxy` default is on) |
| Return a non-2xx as a response, not an error | a `403` fixture is asserted to be a `TransportResponse` carrying its status and reason |

**What this does NOT close.** No request has been sent to Google and no Google response has been parsed. The
server every transport test uses is written by the test file, so these tests prove the *transport's own
controls*; the live smoke test below is still the only thing that would prove the record matches Google. The
transport also has **no production caller** — nothing constructs a `ReqwestTransport` outside a test, because
no composition root builds the connector.

**Recorded limits of the implementation** (each stated rather than implied): **no body-size bound**, because
`TransportFailure` has no variant meaning "too large" and a streaming cap needs a policy that belongs with
`P5-009`'s output handling; **no token refresh**, so a stale token becomes a provider refusal; **no `Retry-After`
interpretation**, since honouring a delay is a retry decision; and the timeout/connect-timeout pair is a
**JARVIS choice** (the port requires a bound; Google publishes no per-request deadline).

## Unresolved Questions

1. ~~**How should JARVIS declare a webhook that is authenticated by an OIDC bearer JWT or an echoed channel
   token, when `WebhookSupport::Push` names only an HMAC over the body?**~~ **RESOLVED 2026-10-01 by
   `ADR-0099`.** The contract now names both: `SignatureAlgorithm::OidcIdToken` and
   `SignatureAlgorithm::EchoedChannelToken`, with `covers_the_body` as the axis that separates a body signature
   from a header token. The page was re-read the same day and its facts were unchanged, so this was a contract
   gap rather than a misreading. The *verification* half — JWKS fetching, certificate rotation, `aud`/`iss`/`exp`
   checking, and a stored-value comparison for the channel token — is **not** built and is recorded as
   Unresolved Question 9. *Blocks now:* nothing; the contract question is closed.
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
8. **Is Calendar's "use exponential backoff" for a `404` a real transient, or boilerplate?** The errors page
   lists it in the `404` row while naming two causes — *"has never existed"* and *"accessing a calendar that the
   user can not access"* — neither of which a retry repairs. So the sentence and the causes disagree, and which
   one describes observed behaviour is **not verifiable from the documentation**. *Impact:* the crate refuses
   the retry (`ADR-0082`); if a `404` is genuinely transient, refusing costs a failed sync that a single retry
   would have fixed. *Blocks:* nothing, because the refusal is the fail-closed direction and a full re-sync is
   the documented fallback — but it should be settled by the live smoke test rather than by argument.
9. **What verifies an `OidcIdToken` or `EchoedChannelToken` delivery, and where does the JWKS dependency live?**
   The contract can now **name** both mechanisms (`ADR-0099`), and one of them is **verified**: the echoed
   channel token is compared in constant time (`ADR-0101`, `verify_channel_token`), which **closes the
   constant-time-comparison half of this question**. An OIDC ID token still needs signature validation against
   Google's rotating certificates (JWKS fetching, key rotation, `aud`/`iss`/`exp` checking) — a network
   dependency inside a webhook path `P5-001` deliberately kept as a pure function of its arguments — and that
   half **remains open**. *Impact:* Gmail push cannot be declared **and acted on** until the JWT verifier is
   settled, and the same *shape* of question (how does a provider authenticate a push the connector cannot MAC?)
   is the one `P5-006` must ask of Microsoft Graph — this record makes **no claim** about Microsoft's mechanism,
   which is `P5-006`'s to establish from its own sources. *Blocks:* `P5-010`'s "webhook signature/replay tests"
   for the Gmail mechanism; nothing in `P5-005`'s declaration, which stays `Polling`.

## Verification Log

| Date | Check | Result |
| --- | --- | --- |
| 2026-09-27 | `https://developers.google.com/llms.txt` and `https://developers.google.com/gmail/api/llms.txt` fetched | **HTTP 404** for both. No `llms.txt` exists for Google Workspace; recorded as `not found` and the official guide pages used instead, per `external-research.md`. |
| 2026-09-27 | Gmail push guide fetched and read in full | `users.watch` request/response shapes; the immediate notification on a successful watch; the **7-day renewal bound** with daily recommended; the `PubsubMessage` envelope with `message.data` as **Base64URL-encoded JSON** decoding to `{"emailAddress","historyId"}`; the **1 event/second/user cap with excess dropped**; "notifications might be delayed or dropped"; the polling fallback. |
| 2026-09-27 | Gmail sync guide fetched | Full vs partial sync; `startHistoryId`; history available "at least one week"; **a `startHistoryId` out of range returns HTTP 404 and requires a full sync**. |
| 2026-09-27 | Gmail quota page fetched | The **May 2026 model change** and its grandfathering; 1,200,000/min/project, 6,000/min/user/project, 80,000,000/day/project; the **full per-method cost table**; 500 recipients/message; the daily threshold cannot be raised; per-user limits cannot be increased; a service account is one user for quota. **The batch ceiling is NOT on this page** — an earlier version of this row attributed "the batch ceiling of 50" to it, which the re-read below disproved. |
| 2026-09-30 | Gmail **batch** page fetched (a source the record had **never listed**) | The batch syntax (`multipart/mixed`, one part per call, the nested-request form); **a hard limit of 100 calls per batch** and a **recommendation of no more than 50** because "larger batch sizes are likely to trigger rate limiting"; **a batch counts as *n* requests against quota, not one**; "the server might perform your calls in any order"; a `Content-ID` on a part is echoed as `response-`-prefixed. **Corrected a record defect**: the 50 figure had been filed under the quota page, which does not state it, and the two facts had been merged into one "ceiling". Grounds `GMAIL_BATCH_HARD_LIMIT`/`GMAIL_BATCH_RECOMMENDED` and `ADR-0080`. |
| 2026-09-27 | Gmail quota page **re-fetched**, specifically for the per-method table and the unit | The page defines quota units as "an abstract unit of measurement representing Gmail resource usage" and publishes the per-method table — `messages.get` **20**, `messages.list` **5**, `history.list` **2**, `getProfile` **1**, `messages.send` **100**, `threads.get` **40**, `watch` **100**. **All recorded costs confirmed correct.** The re-read established the fact the declaration was missing: **the 1,200,000 and 6,000 ceilings are quota units, not requests** — so a request rate needs the per-call cost. Corrected in `ADR-0079`. The page still publishes **no Calendar cost**, so `calendar_events_read` declares `Unstated`. |
| 2026-09-27 | Gmail scopes page fetched | The non-sensitive/sensitive/restricted split, with **`gmail.readonly` and `gmail.metadata` both restricted** and `gmail.send` sensitive; the rule that storing or transmitting restricted-scope data requires a security assessment; the internal-app exemption. |
| 2026-09-27 | Gmail error page fetched | The 401/403/429/5xx taxonomy by `reason`; the four 403 reasons; the three distinct causes behind a 429; the exponential-backoff recipe with `max_backoff` 32–64 s; **"You can't assume that a 200 response means the email was successfully sent."** |
| 2026-09-27 | Gmail error page re-fetched, specifically for a request-id header | The page describes "two levels of error information: HTTP error codes and messages **in the header**; A JSON object in the response body". **No request-id header is named anywhere on it.** This **corrects** a limit recorded in `ADR-0075`/`TODO.md` that asserted Google "returns the identifier in a response **header**" — that was an assumption stated as a finding, and whether Google supplies one at all is **unverified** (`ADR-0076`). |
| 2026-09-27 | RFC 9110 §10.2.3 and §5.6.7 fetched from `https://www.rfc-editor.org/rfc/rfc9110.html` | `Retry-After = HTTP-date / delay-seconds`, `delay-seconds = 1*DIGIT`; a 503 "MAY send a `Retry-After` header field"; `HTTP-date` accepts all three formats and a recipient **MUST accept** them. Grounds `RetryAfter`/`parse_retry_after` and `ADR-0076`. |
| 2026-09-27 | Gmail scopes page **re-fetched** for the category tables | The three lists re-read verbatim: **non-sensitive** = `gmail.addons.current.action.compose`, `gmail.addons.current.message.action`, `gmail.labels`; **sensitive** = `gmail.addons.current.message.metadata`, `gmail.addons.current.message.readonly`, `gmail.send`; **restricted** = `mail.google.com/`, `gmail.readonly`, `gmail.compose`, `gmail.insert`, `gmail.modify`, `gmail.metadata`, `gmail.settings.basic`, `gmail.settings.sharing`. The page's own definitions and the rule "**If you store restricted scope data on servers (or transmit), then you must go through a security assessment**" confirmed word-for-word. **The recorded categories were correct**; now enforced as a dated table in `google::scopes` (`ADR-0078`). The page still lists **no category for a Calendar or OpenID scope**, so those remain unaccounted rather than assumed. |
| 2026-09-27 | Calendar push guide fetched | Channel creation with `id`/`type: web_hook`/`address`/`token`/`expiration`; the full `X-Goog-*` header table; **`X-Goog-Channel-Token` as the anti-spoofing control**; the **zero-length body**; the `sync` message that can arrive before the watch response; success codes `200/201/202/204/102`; 5xx retried; **"no automatic way to renew"** with an expected overlap; "Not 100% reliable. Expect a small percentage of messages to get dropped"; per-calendar vs per-user subscription granularity. |
| 2026-09-27 | Calendar sync guide fetched | `nextSyncToken`; incremental sync with `syncToken`; **HTTP 410 Gone for an invalidated token** requiring a full wipe; **HTTP 400 for disallowed query restrictions**; the pagination rule of repeating the exact same query with `pageToken`; `modifiedSince` recorded as deprecated in favour of sync tokens. |
| 2026-09-30 | Calendar **error** guide fetched (a source the record had not listed) | The two-level error structure and a JSON body per status. **The finding: HTTP `410 Gone` has THREE documented causes** — `fullSyncRequired` ("wipe the store and re-sync"), `updatedMinTooLongAgo` (same), and **`deleted` ("Resource has been deleted") whose action is "no further action is necessary"**. Also the `400` example body (`timeRangeEmpty`, "Because this is a permanent error, do not retry"), and that `rateLimitExceeded` "can return either 403 or 429 error codes—currently they are functionally similar". Grounds `CalendarGoneReason`/`calendar_signal` and `ADR-0081`, and **corrects** this record's claim that "Calendar's 410 has no such ambiguity". |
| 2026-09-27 | Pub/Sub push authentication page fetched | **JWT (RS256) in `Authorization: Bearer`**; the claim set (`aud`, `azp`, `email`, `sub`, `iss`, `exp`, `iat`); validation = signature + **email and audience claims matching the subscription configuration**; tokens "may be up to an hour old"; no body signature. This is the finding that `WebhookSupport::Push` cannot express. |
| 2026-09-27 | Gmail MCP reference fetched | `https://gmailmcp.googleapis.com/mcp/v1`, **Developer Preview**, ten tools with per-tool query costs; a Calendar MCP server linked from the Calendar navigation. |
| 2026-09-27 | `https://accounts.google.com/.well-known/openid-configuration` fetched | **Machine-readable**, so the endpoint values are the server's own published configuration rather than documentation examples: `authorization_endpoint`, `token_endpoint`, `revocation_endpoint`, `userinfo_endpoint`, `jwks_uri`; `code_challenge_methods_supported` = `plain`+`S256`; **`authorization_response_iss_parameter_supported: true`**; `token_endpoint_auth_methods_supported` **truncated in capture** and recorded as such. |
| 2026-09-27 | Google OAuth 2.0 for native apps fetched | Loopback is the **recommended** desktop method and custom schemes are **no longer supported**, so loopback is the only non-embedded option; `redirect_uri` must match an authorized URI **exactly**; **`client_secret` is Optional** on both exchanges; the token response's fields including `refresh_token` "always returned for installed applications" and `id_token` only with an identity scope; **refresh-token limits make older tokens stop working**; the `revocation_endpoint` and its HTTP 200/400 contract; **revocation removes grants for the whole project**; DPoP's key-storage obligation. |
| 2026-09-27 | Cross-check against `jarvis-connectors` | `AuthFlow::new` requires a **`https://` authorization endpoint** and a redirect URI for an OAuth method, so the discovery document's values are what make the flow constructible. `AuthorizationTransaction::begin` requires the listener's redirect to **match the flow's registration on host and path, ignoring the port** — so the registered form and the listening form are both needed, and the unverified question is what a human types into the console (Unresolved Question 7). |
| 2026-09-27 | Cross-check against `jarvis-connectors` (webhook and cursor) | `SignatureAlgorithm` has no OIDC/JWT variant and `SignatureScheme` requires a signed body, so **Finding 1 is a contract gap rather than a connector mistake**; `SyncCursorKind` has `MonotonicMarker` and `OpaqueToken`, so both cursor shapes are already representable and only the **staleness signal** (Finding 2) is new. |
| 2026-09-30 | Gmail error page **re-read against the Calendar errors page**, to compare the two status sets | The two pages are separate with separate summaries and **do not agree**: Gmail's guide has **no `410` subsection** (its sections are 400, 401, 403, 404, 429 and 5xx), while Calendar's documents `410` in detail; and on `404` Calendar suggests "use exponential backoff" where Gmail states no action. The consequence in code was that a Calendar `410` reached a caller as `unknown`/"reconcile", so `classify` now takes the API and carries a `(Calendar, 410)` arm (`ADR-0082`). Also confirmed the shared arms are shared **because the pages agree**: the Calendar page calls `rateLimitExceeded` "functionally similar" across `403` and `429`. |
| 2026-09-30 | Gmail **`Format` enum** page fetched (`reference/rest/v1/Format`, last updated **2026-03-24**) plus the `users.messages` resource and the `messages.get` method page | The per-format definitions verbatim: `minimal` = "only email message ID and labels"; `metadata` = "only email message ID, labels, and email headers"; `full` = the full parsed resource; `raw` = the base64url body. The resource lists nine top-level fields and the method page says only "the response body contains an instance of `Message`" — so **the format is what bounds the returned fields**, which the method page alone does not reveal. Consequence: `snippet` is not returned by `minimal` or `metadata`, so a tool offering those formats cannot declare it (`ADR-0083`). Also recorded that `full`/`raw` "cannot be used" with the `gmail.metadata` scope. |
| 2026-09-30 | Calendar **`events.list` reference** fetched (last updated **2026-07-29**) for the `syncToken` restrictions, and the **sync guide** re-read (last updated **2026-09-11**) | The `syncToken` parameter lists **eight** query parameters that "cannot be specified together with nextSyncToken": `iCalUID`, `orderBy`, `privateExtendedProperty`, `q`, `sharedExtendedProperty`, **`timeMin`**, **`timeMax`**, `updatedMin`. The sync guide adds the reason — a time range belongs to the initial full sync and each incremental sync repeats the initial filters — and the consequence ("The response code for list queries containing disallowed restrictions is `400`"). The connector's `calendar_events_read` offers **two** of the eight (`time_min`, `time_max`), so the pairing is now refused locally and by an `allOf`/`not` constraint (`ADR-0084`). Also confirmed from the same page: `maxResults` default 250, ceiling 2500; `nextPageToken` and `nextSyncToken` are mutually exclusive. |
| 2026-09-30 | Calendar **sync guide** re-read for the pagination rule (same fetch as the row above, this fact recorded separately because it points the **opposite way**) | A large incremental sync returns "a `pageToken` **instead of** a `syncToken`", and the guide instructs repeating "the exact same list query … (with the exact same `syncToken`)" and appending the page token, its own example being `…&syncToken=…&pageToken=…`. So `pageToken` is **not** disallowed with a sync token but **required with** it — the eight-parameter restriction must be applied per parameter and never by shape. The consequence for the connector was that `calendar_events_read` rendered `next_page_token` with **no `page_token` input**, so a large sync could not be walked to its cursor; the input is added and the input/output halves are asserted in step (`ADR-0085`). |
| 2026-09-30 | Audit of this record's own **input** facts against the connector's schemas — no page fetched, a cross-check of what is already recorded | The record establishes `timeMin`/`timeMax` as "datetime … **Must be an RFC3339 timestamp with mandatory time zone offset**" (from the `events.list` reference), so a time bound is a bounded *instant* and **not** a search string. That distinction was missing from the code: `time_min`/`time_max` were validated by the Gmail **search-query** validator, which bounded them at 512 characters and reported the failing argument as **`query`** — an argument `calendar_events_read` does not have. Google publishes **no length limit** for `timeMin`/`timeMax`, so the 64-character bound the fix introduces is a **JARVIS** figure and is recorded as one (`ADR-0086`). No source change; the connector's declared input bounds are now asserted equal to the constants that enforce them. |
| 2026-09-30 | Gmail **push guide** re-fetched (last updated **2026-09-15**) for the notification envelope and the renewal rule, and the **`users.watch` method reference** fetched (last updated **2026-04-15**) for the response type | The envelope verbatim: a `POST` whose body is `{ message: { data, messageId, publishTime }, subscription }` where **`message.data` is a Base64URL-encoded string** decoding to `{"emailAddress": …, "historyId": …}`. The renewal rule's two figures: **"at least once every 7 days"** (the bound) and **"We recommend calling `watch` once per day"** (the recommendation). And the response: `{ "historyId": string, "expiration": string (int64 format) }` where `expiration` is **"epoch millis"** — a **string** carrying **milliseconds**, both of which a naive parser gets wrong while still producing a valid-looking instant. Implemented as `google::watch`, with the unit pinned by a test against the reference's own example value (`ADR-0087`). Also confirmed from the same page: a successful `watch` **immediately sends a notification**. ⚠ **The clause that followed this row — "so the first delivery is not a change" — is a NON-SEQUITUR and was corrected on 2026-10-01 (`ADR-0104`): see the Gmail push guide re-read below.** The *cause* (an opening notification) is real; the *marker* is not, and the two are not the same thing. |
| 2026-09-30 | Cloud Pub/Sub **`PubsubMessage` reference** fetched (`docs.cloud.google.com/pubsub/docs/reference/rest/v1/PubsubMessage`, last updated **2026-05-14**) to check the envelope's field types, since the Gmail guide links to it | **The contradiction**: `data` is typed `string (bytes format)` and described as **"A base64-encoded string"**, while the Gmail push guide (one page over) calls the same field **"Base64URL"**. The two alphabets differ in `+`/`/` versus `-`/`_`, so the disagreement is invisible on any value containing neither — **including the guide's own example**, which was decoded and confirmed to yield `{"emailAddress": "user@example.com", "historyId": "1234567890"}`. A sweep of all 95 printable ASCII characters at all four base64 alignments found only **three** (`>`, `?`, `~`, after a one-character offset) that force the difference, so the connector decodes under **both** alphabets, URL-safe first, and reports which matched (`ADR-0088`). Also recorded: `messageId`, `publishTime` (RFC 3339) and `attributes`, none of which this path reads yet. |
| 2026-09-30 | Cloud Pub/Sub **push page** fetched (`docs.cloud.google.com/pubsub/docs/push`, last updated **2026-09-24**) for the envelope shape and the acknowledgement contract | **Corrects Finding 11's padding reading.** The page's minimum-value example of `message.data` is `SGVsbG8gQ2xvdWQgUHViL1N1YiEgSGVyZSBpcyBteSBtZXNzYWdlIQ==` — **padded**, decoding to `Hello Cloud Pub/Sub! Here is my message!` — so the field is padded standard base64 and a decoder refusing padding would refuse Google's own example. **At-least-once**: "A non-success response indicates that Pub/Sub must resend the messages" and a negative ack or an expired deadline causes a resend, so `messageId` is the deduplication key. **Both spellings** appear in the page's own examples (`messageId`/`message_id`, `publishTime`/`publish_time`), and `deliveryAttempt` is **top-level** beside `message`. **Acknowledgement is five codes** — `102`, `200`, `201`, `202`, `204`; "any other status code" is a negative acknowledgement, so a `203` or `206` requests redelivery. Also documented: **unwrapped** delivery (`payload-unwrapping`) has no `data` field, and **push backoff** (100 ms–60 s, global, triggered by negative acks) is independent of the retry policy. Grounds `PubsubDelivery`, `ACKNOWLEDGING_STATUSES` and `ADR-0089`. |
| 2026-09-30 | **Cross-check of this record's sensitivity facts against the connector's types** — no page fetched, an audit of what the record already establishes | The Gmail push guide says the payload decodes to *"the email address and the new mailbox history ID"*, so the record already establishes that a **person's address arrives over a public endpoint** — a sensitive value. Three types built on this record did not act on it: `PubsubNotification` (the address), `PubsubMessageBody` (`data`, whose bytes decode to the address — a field that *looks* like a harmless opaque blob) and `SyncCursor` (the token) all **derived `Debug`** and printed their sensitive field, while `AccessToken`, `FormRequest` and `VerifiedAccount` hand-write theirs to print a marker instead. A shape-based sweep found a **fourth**, `SyncCursorParts` — the parts struct with the token moved into it by `From<SyncCursor>`, so the crate's own advertised way to move a cursor printed the value past the redaction. `DiagnosticField::CursorObservedAt` had already written the rule — *"the token itself is never a field, because a cursor is provider-issued text that can address another account's data"* — and the types printed it anyway. All four now redact, each with the crate's marker-plus-control test (`ADR-0091`). |
| 2026-09-30 | Gmail **push guide** and **sync guide** re-fetched (push last updated **2026-09-15**, sync the same) for the **`watch` response's `historyId`** and what it anchors | The response's first field is described in the push guide as *"the current mailbox `historyId`"* with *"Your client receives notifications for all changes **after** that `historyId`"* — so it is a `startHistoryId`, not a position reached. And the guide's worked example uses **two different numbers**: *"Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can persist `9876543210` as the last known `historyId`"* — the response's id at one end, the post-sync position at the other. The sync guide confirms the position comes from the sync: a full sync stores *"the `historyId` of the most recent message"*. So the anchor the response supplies had **no reader** in this crate, and the two-number example is the fixture that can falsify their conflation. Both fields are now read together (`ADR-0092`). Also re-confirmed: history *"typically available for at least one week"*, and a `startHistoryId` outside it returns **HTTP 404** → full sync. |
| 2026-09-30 | `users.watch` **method reference** fetched (`developers.google.com/workspace/gmail/api/reference/rest/v1/users/watch`, last updated **2026-04-15**) for the **request** side of the watch — the body and its fields | The request is `POST …/users/me/watch` with a JSON body of `topicName` (required) plus optional `labelIds`, `labelFilterAction` and `labelFilterBehavior`. **Two fields produce a `200` rather than an error when misused**: `labelFilterAction` is *"deprecated because it caused incorrect behavior in some cases"* and is *"ignored"* when `labelFilterBehavior` is set; and `labelFilterBehavior` is the *"filtering behavior of `labelIds` list specified"*, so with no `labelIds` it governs nothing and the watch is registered unfiltered. The enum's two values are `include`/`exclude`. Also confirmed: `topicName` must be the fully qualified `projects/{project}/topics/{topic}` whose project *"must exactly match your Google developer project id"*; the endpoint is on the `gmail.googleapis.com` host **and** the four accepted scopes include `gmail.metadata`. The deprecated field is made unreachable and the pairing refused locally (`ADR-0093`). |
| 2026-09-30 | Cloud Pub/Sub **push page** re-fetched (last updated **2026-09-24**) for the **cost of a negative acknowledgement** | **Push backoff is subscription-global and cannot be disabled**: *"Push backoff applies to all the messages in a subscription (global)"*, *"Push backoff can't be turned on or off"*, and it ranges **100 ms to 60 s**, *"calculated based on the number of negative acknowledgments"*. Two triggers, one of which is not a response: a negative acknowledgement **or an expired acknowledgment deadline** — so a *slow* handler looks the same as a refusing one, and *"You can't modify the acknowledgment deadline of individual messages that you receive from push subscriptions."* The **retry policy is separate** and its delay **adds** to the backoff's. Also confirmed the delivery-rate window: a **slow-start** algorithm, and *"After 3,000 outstanding messages per region, the window increases linearly"* (decreases when a subscriber acknowledges < 99% of requests). So refusing one message is paid for by every account on the subscription, and the connector now bounds it by `deliveryAttempt` (`ADR-0094`). |
| 2026-09-30 | `users.stop` **method reference** fetched (`developers.google.com/workspace/gmail/api/reference/rest/v1/users/stop`, last updated **2026-04-15**) for the operation that ends a watch | *"Turn off push notification delivery for the given user mailbox"*, `POST …/users/me/stop`, **"The request body must be empty"**, and a success is *"an empty JSON object"*. Its four authorization scopes are the **same four `users.watch` requires** — `mail.google.com/`, `gmail.modify`, `gmail.readonly`, `gmail.metadata` — so it needs a **live grant**, while revocation (this record's other finding) removes exactly those scopes. Hence the forced order: revoking first makes the stop impossible, and with the watch still registered nothing ends the stream but the lease — up to seven days. The push guide supplies the timing for the successful case, *"All new notifications should stop within a few minutes"*, which has **no number in it**. Modelled as an ordered, policy-carrying plan (`ADR-0095`). |
| 2026-09-30 | `users.getProfile` **method reference** fetched (`developers.google.com/workspace/gmail/api/reference/rest/v1/users/getProfile`, last updated **2026-04-15**) for the account-identity operation | *"Gets the current user's Gmail profile"*, `GET …/users/me/profile`, **empty request body**, response `{ "emailAddress": string, "messagesTotal": integer, "threadsTotal": integer, "historyId": string }`. **Its accepted scopes are `mail.google.com/`, `gmail.modify`, `gmail.compose`, `gmail.readonly`, `gmail.metadata` — `openid` is NOT among them**, which corrects a comment in this repo that named `openid` as "the operation this scope exists for". So the Gmail read scope is what makes the profile readable, and `openid` is requested for the `id_token` the exchange receives (unverified). `historyId` is *"The ID of the mailbox's current history record"*, so this is a second way to obtain a sync anchor, of the same kind as the watch response's (Finding 13). Declared as `gmail_profile_read`, 1 quota unit (`ADR-0096`). |
| 2026-09-30 | **Audit of what the record already establishes about routing a delivery**, not a new page fetch — cross-checking Findings 1 and 12 against the connector's types | `emailAddress` is the **only** field in a push payload that names a mailbox (`messageId` is *"unrelated to Gmail messages"*; `historyId` is meaningful only within one mailbox), so attribution means matching that address against the addresses the connector holds. The channel cannot be authenticated (Finding 1), so the address is untrusted — and the consequence is bounded by two facts rather than by trust: a route selects **which mailbox to read** (the sync uses that account's own token), and the notified `historyId` is **not trusted as a position** (`history.list` runs from the stored cursor, so a forged id cannot cause a *missed* change). Byte-exact matching with a **reported-not-applied** case-only near-match, since Google publishes no canonicalisation rule for `emailAddress` (`ADR-0097`). |
| 2026-10-01 | **Audit of the record's identity facts against the connect-time step**, not a new page fetch — Findings 17 and 18 read together | `users.getProfile`'s `emailAddress` becomes the account's `provider_account_id`, which is the value a push delivery is matched against, so the connect step decides whether that lookup is single-valued. A second account for one address is refused, because two rows carrying one address make the router answer `Ambiguous` (unactionable without a person) and the duplicate is invisible — it looks like two mailboxes. Case-insensitive comparison **in this direction** (refuse to create) is the opposite of the router's (refuse to act), and both are the same restraint: neither acts on an uncertain case-match. The profile carries no display name, so the account stores `None` rather than the address as one (`ADR-0098`). |
| 2026-10-01 | **Cloud Pub/Sub push-authentication page** re-fetched (`docs.cloud.google.com/pubsub/docs/authenticate-push-subscriptions`, page footer now **2026-09-30**) for Finding 1's resolution | **The facts were unchanged, so Finding 1 was a contract gap rather than a misreading.** Re-confirmed: the JWT is *"an OpenIDConnect JWT"* sent in the **`Authorization` header** (`"Authorization" : "Bearer …"`), RS256 with a `kid`; the claims are `aud`, `azp`, `email`, `email_verified`, `sub`, `exp`, `iat`, `iss`; validation is *"Checking the token integrity by using signature validation"* **plus** *"Ensuring that the email and audience claims in the token match the values set in the push subscription configuration"*; *"The tokens attached to requests sent to push endpoints may be up to an hour old"*; and **the body is not signed** (there is no body signature to check). So the two facts the widened contract rests on — a header token, no body coverage — are both confirmed verbatim. Grounds `SignatureAlgorithm::OidcIdToken` and `ADR-0099`. |
| 2026-10-01 | **Calendar push guide** re-fetched (`developers.google.com/workspace/calendar/api/guides/push`, page footer **2026-09-11**, unchanged) for Finding 1's second mechanism | Re-confirmed `X-Goog-Channel-Token` is the anti-spoofing control and is **client-set then echoed**: *"an arbitrary string value to use as a channel token … you can use the token to verify that each incoming message is for a channel that your application created—to ensure that the notification is not being spoofed"*, echoed in the `X-Goog-Channel-Token` header. The delivery *"do[es] not include a message body"* / `Content-Length: 0`, so **a MAC over the body is not merely absent but impossible**, and the header is *"Sometimes present"* / *"Only present if defined"* — a token that may not be sent at all. Grounds `SignatureAlgorithm::EchoedChannelToken` and `ADR-0099`. |
| 2026-10-01 | **Cross-check of Finding 1's resolution against `jarvis-connectors`** — no page fetched, a re-read of the contract this record was said to be blocked on (`ADR-0099`) | The blocker's own words were "authenticated by an OIDC bearer JWT or an echoed channel token", and **neither covers the body while both authenticate a delivery** — so the missing thing was an axis, not a value. `SignatureAlgorithm` gains `OidcIdToken` and `EchoedChannelToken`; `covers_the_body`/`is_body_independent` name the axis (and are provably **not** `is_keyed_mac`: Ed25519 covers the body with a key, an echoed token needs a key and covers no body). A body-independent authenticator must declare `SignatureEncoding::Raw`, refused otherwise, because there is no signature whose bytes could be hex or base64. The manifest's push guard still refuses only `None`. **Verification is still unbuilt**, so the connector keeps `Polling` — for cardinality (one `WebhookSupport` value, two mechanisms) and the absent verifier, not because the mechanisms cannot be expressed. Question 1 marked resolved; the verification half recorded as Question 9. |
| 2026-10-01 | **Calendar push guide** re-read (page footer **2026-09-11**, unchanged) for the notification **message** rather than for Finding 1 | The header set, the **zero-length body** (`Content-Length: 0`), and the **`sync` message** verbatim: *"the Google Calendar API sends a `sync` message to indicate that notifications are starting"*, *"It's safe to ignore the `sync` notification"*, and *"Due to network timing issues, it's possible to receive the `sync` message even before you receive the `watch` method response."* The state table: `sync` delivered *"A new channel was successfully created"*; `exists` *"There was a change to a resource"*; `not_exists` listed but **not defined** for a caller. `X-Goog-Message-Number` *"is always 1 for sync messages"* **but** *"Message numbers increase for each subsequent message on the channel, but they're not sequential"* — so the number is **not** a discriminator. `X-Goog-Channel-Expiration` is *"expressed in human-readable format"* (e.g. `Tue, 19 Nov 2013 01:13:52 GMT`) — the **opposite** of the Gmail lease's epoch-millis string, so the two are read by different code. Implemented as `google::channel`, with the handshake predicate `is_sync` (`ADR-0100`). |
| 2026-10-01 | **Calendar push guide** re-read (same unchanged page) for the **channel token's** contract, plus an audit of the verifier against the crate's own constant-time routine | The token is *"an arbitrary string value"* set by the application, presented as the way *"to verify that each incoming message is for a channel that your application created"*, echoed in `X-Goog-Channel-Token`, and **optional** (*"Sometimes present"* / *"Only present if defined"*) — so a delivery without one is a documented shape for a channel registered without a token. `Maximum length: 256 characters` is the guide's figure, **recorded as a stated fact and deliberately not enforced** because `SecretValue::matches` already refuses a different-length candidate in constant time, so a bound check would decide nothing and would itself walk attacker input. Implemented as `verify_channel_token` returning a four-variant `ChannelTokenCheck` rather than a `bool` (`ADR-0101`), with the comparison reusing `SecretValue::matches` — the same constant-time routine the OAuth `state` uses (`ADR-0055`). |
| 2026-10-01 | **Audit of the record's channel facts against the routing step**, not a new page fetch — Findings 1, 12 and 18 read together for the second mechanism | A Calendar delivery names the **channel** and never the account — the account is whatever the `watch` call was authenticated as — so attribution is a join against what the connector registered, and the `id` the connector **chose** is the join key. That is the **opposite provenance** from the Gmail router's `emailAddress` (`ADR-0097`, a **provider** value), which is why the two want the same byte-exact strictness for opposite reasons: an untrusted provider string may only be compared as sent, and a connector-generated id is compared against its own record. The guide's own channel-creation example (*"a universally unique identifier (UUID) … You recommend using a universally unique identifier"*) is what makes a registration collision a recommendation-violation rather than a design case, and `Ambiguous` reports it without a pick. Implemented as `google::channel::{ChannelRegistration, ChannelRoute, route_channel}` (`ADR-0102`). |
| 2026-10-01 | **Composition of the Calendar push path**, not a new page fetch — the read, verify and route facts read together as one decision | The four pieces (read, verify, route, handshake rule) were each built and each correct alone, and nothing called them together — `ADR-0069`'s seam and `ADR-0098`'s "the join was the missing step", a third time. Composing them **forced a type change**: `ChannelRoute::Exact` carried only an `AccountReference`, but the token that *proves* the delivery lives on the registration, so routing without it would have meant a **second scan** to fetch the token — two lookups deciding one match. `Exact` now carries the whole `ChannelRegistration`, and `ingest_channel_delivery` returns a five-variant `ChannelIngest` (`Unreadable`/`Unroutable`/`Rejected`/`Handshake`/`Changed`) in which only `Changed` starts work, because `Ok(None)` could not tell a handshake from an unroutable delivery. Every outcome acknowledges (none is repaired by retrying, and a negative ack is subscription-global). Proved end to end against the two channel fixtures (`ADR-0103`). |
| 2026-10-01 | Gmail **push guide** re-fetched (`developers.google.com/workspace/gmail/api/guides/push`, page footer **2026-09-15**, unchanged) to build the Gmail ingest — **and it corrected a claim this record had already made** | Under *Watch response*: *"Additionally, a successful `watch` call immediately sends a notification to your Cloud Pub/Sub topic."* **And the notification it sends carries no state field** — the guide's only payload shape is `{"emailAddress": …, "historyId": …}`, the same one a real change produces, with no `sync`-equivalent. So **Gmail's opening notification is indistinguishable from a change**: the *cause* is real (an opening notification) but there is **no *marker***, and the row above's earlier "so the first delivery is not a change" was a **non-sequitur** — it asserted detectability from causation. **This is the same class as the Calendar handshake only in cause, not in what a consumer can do**: Calendar marks its opening message with `X-Goog-Resource-State: sync` and says it *"safe to ignore"*, Gmail marks nothing. So `GmailIngest` has **no `Handshake` outcome** (`ADR-0104`), and `channel.rs`'s claim that the Calendar rule was the "counterpart" of a Gmail rule is corrected in place. Also re-confirmed from the same page: *"Each Gmail user being watched has a maximum notification rate of one event per second. The service drops any user notifications exceeding that rate"*, and the at-least-once/`messageId`-is-the-dedupe-key rule the Pub/Sub pages establish. |
| 2026-10-01 | Gmail **errors page** re-fetched (`developers.google.com/workspace/gmail/api/guides/handle-errors`, page footer **2026-09-15**, unchanged) for the **remedy** of `authError`, to build the recovery decision | Under 401 `authError`: *"This error occurs when the access token you're using is either expired or invalid. **Missing authorization for the requested scopes can also cause this error.**"* And the fix, verbatim: *"To fix this error, **refresh the access token** using the long-lived refresh token. If you're using a client library, it automatically handles token refresh. **If this fails, direct the user through the OAuth flow**."* **Two consequences the connector's vocabulary did not carry.** (1) **The remedy is two steps and `RetryGuidance::Reauthenticate` names only the second** — so read literally it sends a user to a consent screen for an expired token a silent refresh repairs, and the *first* step was not a type anywhere. (2) **`authError` is two causes in one word** — an expired token (refreshing fixes it) *or* a scope the grant never had (only re-consenting does), and the refusal cannot tell them apart; **a refresh can, because it cannot grant a scope**. Implemented as `google::recovery` (`CallRecovery`/`RefreshRecovery`/`TokenState`, `recover_from_call`/`recover_from_refresh`), `ADR-0105`. |
| 2026-10-01 | Calendar **push guide** re-fetched (`…/calendar/api/guides/push`, footer **2026-09-11**, unchanged) for the **channel lease**, plus the **`events.watch` reference** (`…/calendar/v3/reference/events/watch`, footer **2026-05-12**) and the **`channels.stop` reference** (`…/calendar/v3/reference/channels/stop`, footer **2025-04-01**) for the two field types the guide states in prose | Under *Renew notification channels*, verbatim: *"Currently, there's no automatic way to renew a notification channel. When a channel is close to its expiration, you must replace it with a new one by calling the `watch` method. As always, you must use a unique value for the `id` property of the new channel. Note that there's likely to be an 'overlap' period of time when the two notification channels for the same resource are active."* — so renewal is a **replacement with a unique id and a published-but-unnumbered overlap**, not a refresh. And the **same expiry in two encodings**: the header is *"expressed in human-readable format"* (e.g. `Tue, 19 Nov 2013 01:13:52 GMT`) while *"The channel's expiration time … is included as a Unix timestamp (in milliseconds) in the information returned by the `watch` method."* The `events.watch` reference types the response `expiration` as **`long`** — *"Date and time of notification channel expiration, expressed as a Unix timestamp, in milliseconds. Optional."* — **which contradicts the guide's own request section**, where the same field is *"An `expiration` property string set to a Unix timestamp (in milliseconds)"*; the reference wins, and a string is refused by name. Also captured: `params.ttl` *"Default is **604800 seconds**"* (the same figure as Gmail's whole renewal **bound**, by a different mechanism), `resourceId` as *"a stable, version-independent identifier"* read because `channels.stop` *"requires that you provide at least the channel's `id` and the `resourceId`"*, and the stop **request** type of `expiration`/`token` as strings with no `expiration` on the response schema. Implemented as `parse_channel_watch_response` + `channel_lease` + `renewal_decision` + `ChannelRenewal`, `ADR-0106`. **This row also corrects Finding 10's unquoted claim** that Calendar's expiry is an RFC 3339 string — no page says that; it is epoch millis. |
| 2026-10-01 | Calendar **push guide** re-read (footer **2026-09-11**, unchanged) for the **`stop` permission rule**, plus its *Stop notifications* section for the call's arity | Verbatim: *"This method requires that you provide at least the channel's `id` and the `resourceId` properties… Note that if the Google Calendar API has several types of resources that have `watch` methods, there's only one `stop` method."* and *"Each notification channel is associated both with a particular user and a particular resource (or set of resources)."* So a channel is identified by a **pair** and **one account may need several calls** — the fact that makes Calendar's stop a different `TeardownStep` from `users.stop` rather than an instance of it. On permission, the two rules keyed on creation: *"If the channel was created by a regular user account, only the same user from the same client (as identified by the OAuth 2.0 client IDs from the auth tokens) who created the channel can stop the channel. If the channel was created by a service account, any user from the same client can stop the channel."* The discriminating fact is the **`client_id` inside the token**, which this crate exposes only as a rendered header value (`ADR-0061`) and which a JWT access token would carry as the **unverified** `aud`/`azp` claims (`ADR-0064`) — so the rule is recorded as a **limit** and a violation surfaces as the provider's `403`. Also re-confirmed from the same section: *"The `expiration` property controls when the notifications stop automatically"*, and the stop's own response is empty. Implemented as `calendar_channel_stop` + `TeardownStep::StopCalendarChannel` + `calendar_exposure`, `ADR-0107`. |
| 2026-10-01 | Gmail **`users.history.list` reference** re-fetched (`…/gmail/api/reference/rest/v1/users.history/list`, footer **2026-04-15**, unchanged) to check what the **response's `historyId`** says about storing it | **The field says nothing about it:** *"historyId | string | The ID of the mailbox's current history record."* The storing rule is stated **once**, in `startHistoryId`'s description: *"If you receive no nextPageToken in the response, there are no updates to retrieve and you can store the returned historyId for a future request."* So the page's id is the mailbox's position **at the moment that page was produced**, and a walk with more to come has not consumed the changes up to it. Also re-confirmed verbatim: *"History IDs increase chronologically but are not contiguous with random gaps in between valid IDs"*, *"Supplying an invalid or out of date startHistoryId typically returns an HTTP 404"*, *"A historyId is typically valid for at least a week, but in some rare circumstances may be valid for only a few hours"*, and *"If you receive an HTTP 404 error response, your application should perform a full sync"* — the facts behind the `404`-means-resync reading. Also noted: `historyId` is **not** marked optional in the response schema, so a `200` with no id is not a shape the provider documents — which is why `Unstated` refuses to infer "the mailbox is unchanged". Implemented as `HistoryPosition` + `of_page` + the qualified signal parameter, `ADR-0108`. |
| 2026-10-01 | Calendar **`events.list` reference** re-fetched (`.../calendar/v3/reference/events/list`, footer **2026-07-29**, unchanged) for the two continuation tokens the output schema declared with **no descriptions** | The mutual exclusion is stated in **each field's own description**, not in prose elsewhere: `nextPageToken` is *"Token used to access the next page of this result. **Omitted if no further results are available, in which case `nextSyncToken` is provided.**"* and `nextSyncToken` is *"Token used at a later point in time to retrieve only the entries that have changed since this result was returned. **Omitted if further results are available, in which case `nextPageToken` is provided.**"* So a page carries **at most one**. Also re-confirmed from the same page: `maxResults` *"By default the value is 250 events. The page size can never be larger than 2500 events."*; *"Incomplete pages can be detected by a non-empty nextPageToken field"*; and the `syncToken` description's list of parameters that cannot accompany it (`iCalUID orderBy privateExtendedProperty q sharedExtendedProperty timeMin timeMax updatedMin`), which is `ADR-0084`'s list. Implemented as `CalendarContinuation` + `of_page` + schema descriptions, `ADR-0109`. |
| 2026-10-01 | Gmail **push guide** re-fetched (`.../gmail/api/guides/push`, footer **2026-09-15**, unchanged) for the **two branches** a first sync has, and the `users.history.list` reference for what a `startHistoryId` of the anchor yields | The *Watch response* section names the anchor and then **forks**, verbatim: *"The response contains the current mailbox `historyId` for the user. **Your client receives notifications for all changes after that `historyId`.** If you need to process changes **before** this `historyId`, refer to Synchronize clients with Gmail."* So there are **two** documented first-sync branches � `history.list` from the anchor, or the mailbox's existing contents � and the connector could express only the second, because `resume_from` answered `FullSync` for every `Start` cursor while `WatchResponse::anchor` and `GmailProfile::history_id` both had readers and no consumer. Also re-confirmed: the worked example's two numbers (*"Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can persist `9876543210` as the last known `historyId`"*) and *"Additionally, a successful `watch` call immediately sends a notification to your Cloud Pub/Sub topic"* (the opening notification `ADR-0104` records as unmarked). The profile's `historyId` is the second anchor source and costs 1 quota unit rather than a message read. Implemented as `ResumePoint::FromAnchor` + `SyncOrigin` + `resume_anchored`/`resume_ignoring_anchor` + `AnchoredAccount`, `ADR-0110`. |
| 2026-10-01 | **Cross-check of `ADR-0107`'s recorded wiring gap against this crate's own types** — no page fetched, an audit of what the record already establishes about a watch's expiry | The gap `ADR-0107` named is closed by a **second Gmail function, not a parameter**: `parse_watch_response` already reads the `watch` response's `expiration` into a `UtcTimestamp` and `watch_lapse` turns it into a `WatchLapse`, so a caller **does** hold the instant `gmail_exposure` lacked, and `AlreadyEnded` becomes reachable for Gmail from the same `WatchLapse` a channel reaches it from. A **bound** ("at least once every 7 days") and a **lease** (this watch's `expiration`) are different inputs, so the bound-only figure is kept for the scheduler that holds no watch rather than replaced. Two guards falsified A-B-A: the `stop_succeeded` guard inverted, and the live arm's `for_seconds` replaced by `WATCH_RENEWAL_BOUND_SECONDS`. Also found and removed in the same slice: a **dangling intra-doc link to the `PushMechanism` enum `ADR-0107` removed**, and a **gap in this record's own Finding numbering** (findings ran `…20, 22, 23` because `ADR-0108` renamed the then-existing Finding 21 to 23, leaving no 21). Implemented as `gmail_watch_exposure`, `ADR-0111`. |
| 2026-10-01 | Calendar **`events.watch` reference** (`…/calendar/v3/reference/events/watch`, footer **2026-05-12**, unchanged) and **push guide** (*Make watch requests*, footer **2026-09-11**) read for the **request body** — the fields a channel *creation* carries, since the record had only the **response** shape | The reference gives the body as `{ id, token, type, address, params }` with `params.ttl` *"The time-to-live in seconds for the notification channel. **Default is 604800 seconds.**"*, and the guide states the **required** trio verbatim: `id` *"A UUID or similar unique string that identifies this channel… **Maximum length: 64 characters**"* (echoed as `X-Goog-Channel-Id`), `type` *"set to the value `web_hook`"*, and `address` *"the URL that listens and responds to notifications… **must use HTTPS**"* — *"If a channel has an expiration time, it's included as the value of the `X-Goog-Channel-Expiration` HTTP header (in human-readable format)"*. The **optional** pair: `token` *"an arbitrary string value to use as a channel token… **Maximum length: 256 characters**"* (*"use the token to verify that each incoming message is for a channel that your application created—to ensure that the notification is not being spoofed"*), and the guide warns *"Don't include sensitive data such as OAuth tokens."* So the **create** body carries a credential-adjacent `token` and a webhook `address`, which `JsonRequest::rendered_body`'s own doc (*"there is no credential here"*) had asserted could not happen — corrected by making the type's redaction body-dependent. The guide's **certificate** rule (*"only if there's a valid SSL certificate installed on your web server"*, invalid cases listed) is a **limit**: a chain is not observable at request-construction time. Implemented as `calendar_channel_watch` + `webhook_address`, `ADR-0112`. |
| 2026-10-01 | **Cross-check of the channel path against this crate's own types** — no page fetched, an audit of what the record already establishes about a channel's expiry and which value carries it | `parse_channel_watch_response` reads the response's `expiration` into `ChannelWatchResponse::expires_at`, whose doc says it *"can drive a renewal decision"*, and `renewal_decision` takes **the expiry as its only input** — but `ChannelRegistration`, the value that survives between the `watch` and the teardown, had **no field for it**, so the decision was reachable only from a test: the same "a value read and then dropped" shape `ADR-0107` found for `resourceId`, from the same call. The type's own doc counted *"four facts"* while **five** were in play, and the omitted one was the expiry. Closed by `expires_at` + `from_watch_response` (so the third provider fact travels with the two identifiers) + `renewal(now)` (which delegates, asserted equal so the bridge is not a second opinion). Three guards falsified A-B-A. Implemented as `ChannelRegistration::{expires_at, from_watch_response, renewal}`, `ADR-0113`. |

**No Google API was called, no credentials were used, no Cloud project was created, and no live test was run.**
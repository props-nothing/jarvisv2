---
integration: ntfy
status: implemented-minimal
last_verified: 2026-07-30
owners: []
selected_spec_version: "ntfy publish API as documented at docs.ntfy.sh (no version number is published for the HTTP API)"
selected_sdk: null
---

# ntfy (push notifications)

## Scope

One operation: publish a short plain-text notification to a topic, so the owner hears about an approval request or a finished
scheduled task while the console is closed (`ADR-0155`). Out of scope: subscribing, attachments, action buttons, scheduled delivery,
access tokens, templating, e-mail and phone-call delivery, and the ntfy server itself.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | https://docs.ntfy.sh/llms.txt returned 404: not found; docs discovered from https://docs.ntfy.sh/ | 2026-07-30 | discovery |
| API/specification | https://docs.ntfy.sh/publish/ (source: `docs/publish.md` in https://github.com/binwiederhier/ntfy, `main`) | 2026-07-30 | normative contract |
| OpenAPI/AsyncAPI/schema | not found | 2026-07-30 | none published |
| official SDK/source | none needed: one HTTP request | 2026-07-30 | |
| changelog/release notes | not read: the publish call used here (POST body, `Title`, `Priority`, `Tags`) is the oldest part of the API | 2026-07-30 | |
| security/privacy/limits | the "Limitations" and "Authentication" sections of the publish page | 2026-07-30 | risk and operations |

## Verified Contract

### Operations And Transport

- Publish by `PUT` or `POST` to `https://<server>/<topic>` with the message as the request body. Topics are created on first use.
- Headers used: `Title` (alias of `X-Title`), `Priority` (alias of `X-Priority`; 1 min to 5 max, 3 default), `Tags` (alias of `X-Tags`, comma-separated).
- The publish page notes UTF-8 in HTTP headers is not supported by every library, so JARVIS sends ASCII-only titles and tags.

### Authentication And Authorization

- No sign-up on the public server: **the topic is essentially a password**. Topic names are 1 to 64 characters of `[-_A-Za-z0-9]`.
- A protected topic on a server with access control takes `Authorization: Basic ...` or a Bearer access token. **Not built**: see Unresolved Questions.
- Basic auth is base64, not encryption, so a self-hosted server must use HTTPS.

### Limits And Failure Semantics

- Message length 4,096 bytes (longer is turned into an attachment); title 1 KB; all tags 512 bytes (HTTP 400 beyond).
- Default 60 requests per visitor, refilling one per 5 seconds; on ntfy.sh 250 messages per day. Repeated abuse can ban the IP.
- A failed publish is not retried by JARVIS: a lost push is acceptable, a flood is not.

### Data And Compliance

- The server and anyone who knows the topic see the message. On the public server that is a third party, so JARVIS puts **no answer text, no question text and no argument** in a message: only that something needs the owner or finished, and the tool's name.
- No ntfy account, token or key is stored by JARVIS.

### Versions And Deprecations

The publish API has no version number. The header aliases used are documented as current.

## JARVIS Mapping

- `daemon.push_topic` (unset means no push at all) and `daemon.push_server` (default `https://ntfy.sh`), both validated at configuration load: topic charset and length, and a server that is `https://`, or `http://` on this machine, with no login in the address.
- Sent by `apps/jarvisd/src/push.rs` over the existing `reqwest` client, with a short timeout, only when no console is open, so a person at the screen is not also pinged.
- Not a tool: the model cannot call it. It is the daemon telling its owner, the same as the desktop notification.

## Decisions

- Plain-text POST with headers, not the JSON publish form: fewer moving parts.
- Off by default; it is the one outbound path that carries anything about the owner's work, so it needs a deliberate topic.
- The content is deliberately content-free.

## Rejected Alternatives

- Pushing the answer text: leaks work product to a third party on a guessable-secret channel.
- E-mail through ntfy or the owner's Google account: needs the send scope and an approval, and is slower.
- Telegram/WhatsApp: needs an account, a bot token and a business relationship; ntfy needs none.

## Verification Plan

- Offline: topic and server validation tests; the request is built and its URL, headers and body checked against a local listener.
- Cheapest test that disproves the central assumption (a plain POST to a topic is accepted): `curl -d test https://ntfy.sh/<topic>` returns 200 with a JSON `id`; not run against the public server, because that would publish to a topic on a third party.
- Opt-in live smoke test: the owner sets a topic of their own and runs `jarvis push test`.

## Unresolved Questions

- Access tokens for a self-hosted server with access control (`Authorization: Bearer`): blocks push to protected topics only; a key-file setting like `search_api_key_ref` is the likely shape.
- Behaviour on a 429 from the public server: JARVIS drops the message; whether to back off is undecided.

## Verification Log

- 2026-07-30: publish page and limits read; `llms.txt` is absent.

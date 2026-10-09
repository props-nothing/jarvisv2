---
integration: ollama-web-search
status: implemented-contract-tested
last_verified: 2026-10-09
owners: []
selected_spec_version: "REST, undated; docs.ollama.com as of 2026-10-09"
selected_sdk: null
---

# Ollama web search

## Scope

One operation: `POST https://ollama.com/api/web_search`, a single-query web search returning snippets, used by the `jarvis.web.search`
tool. Out of scope: Ollama's companion web fetch API (JARVIS has its own guarded fetch, `web-fetch.md`), cloud model inference (already
used through the OpenAI-compatible endpoint), the Python and JavaScript SDK tool helpers, and any other search provider.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` | https://docs.ollama.com/llms.txt | 2026-10-09 | discovery; lists "Web search" and "Authentication" |
| Web search page | https://docs.ollama.com/capabilities/web-search.md | 2026-10-09 | the normative contract (request, response, auth) |
| Authentication | https://docs.ollama.com/api/authentication.md | 2026-10-09 | key creation, `Authorization: Bearer`, the local server needs none |
| OpenAPI | https://docs.ollama.com/openapi.yaml | 2026-10-09 | **does not contain `web_search`** (84 KB, no match); the page above is the only contract |
| Cloud usage | https://docs.ollama.com/api/cloud-usage.md | 2026-10-09 | confirms web search is metered as cloud usage; no per-request limit stated |
| SDK source | not read | | the REST page is complete for one operation |
| changelog | not found | | no dated changelog for this endpoint was located |

## Verified Contract

### Operations And Transport

`POST https://ollama.com/api/web_search`, JSON body `{ "query": string (required), "max_results": integer (optional, default 5, max 10) }`.
Success body `{ "results": [ { "title", "url", "content" } ] }`, where `content` is a relevant snippet. No streaming, pagination or
request id is documented.

### Authentication And Authorization

An Ollama API key (created at https://ollama.com/settings/keys with a free account) in `Authorization: Bearer <key>`. Keys do not expire
and are revoked in the same settings page. The local server at `localhost:11434` does **not** serve this endpoint: verified 2026-10-09, a
`POST /api/web_search` to the local server returns 404, so signing in to Ollama for cloud models does not give search; a key is needed.

### Limits And Failure Semantics

Not documented: rate limits, error bodies, status codes, query length, result size, retention. Observed (2026-10-09, live): an invalid
key is refused with a 401 or 403 (the adapter treats both as "key rejected"). Nothing else was observed because no valid key was
available to the author.

### Data And Compliance

The query text leaves the machine for ollama.com, together with the account the key belongs to. Retention and use of queries are not
stated on the pages read. Search is metered as cloud usage against the account's plan.

### Versions And Deprecations

No version is stated for the endpoint. Treat the shape as subject to change.

## JARVIS Mapping

Tool `jarvis.web.search`, scope `web.search`, effect `read_only`, risk 1, approval `Policy` (the same posture as `jarvis.web.fetch`: the
model chooses the query and a query carries data to its destination, so it is bounded to 400 characters and audited, and an owner who
wants it asked about sets `"jarvis.web.search" = "ask"`). The key is a file referenced by `daemon.search_api_key_ref` and written by
`jarvis keys set search`; it is never logged or returned. Results are fenced as untrusted data. Any non-success status is returned to the
model as a failed call with a fixed sentence (never the service's own text); redirects are not followed so the key cannot be carried to
another host. Without a key there is no search tool.

## Decisions

- Ollama search rather than scraping a search engine's HTML: it is documented, keyed and stable enough to build on, and the owner already
  uses Ollama. The adapter is the only place that knows the provider; another provider is another adapter behind the same tool id.
- Results are cut whole-result to fit one 4,000-character fence, so a long answer never splits a result or the fence.
- A key file that cannot be read or is malformed does not stop the daemon (search is an extra); it logs once and the tool is absent.

## Rejected Alternatives

- Scraping DuckDuckGo or Google HTML: no documented contract, terms and bot-blocking uncertain.
- Brave Search API: documented and good, but a second account for an owner who already has Ollama. A later adapter.
- Proxying through the local Ollama server: it does not serve the endpoint (404, above).

## Verification Plan

- offline contract tests against a local fixture (done: request shape and bearer header, fenced results, a result with no address dropped,
  401/429/500 and unreachable reported without the service's text, a malformed reply fails closed, long results cut, bad arguments never
  reach the network, the key never appears in `Debug`)
- live negative path (done 2026-10-09): `jarvis keys test search` with a deliberately invalid key reaches the real endpoint and is
  refused; the assistant reported "the search service rejected the key" and fell back to a page fetch
- live positive path (**not done**): needs a valid Ollama API key. Until one is used, the success shape is verified only against the
  documented example, so this record's status is `implemented-contract-tested`, not verified.

## Open Questions

| Question | Blocks |
| --- | --- |
| Real rate limits and error bodies | tuning retries; none are attempted today |
| Whether result `content` can be empty or very long | nothing: handled by dropping address-less results and clipping |
| Retention of queries | whether the default risk should be higher for a privacy-focused owner |
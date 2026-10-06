# ADR-0139: Settings are changed by the owner, through one validated path

Status: Accepted
Date: 2026-10-06

## Context

Changing a model, a key, a folder grant or the trust list meant editing `config.toml` by hand or re-running `init --force`,
which replaced the whole configuration. The console could not change anything. A Settings screen is the obvious fix, but
it adds an API that can widen what the assistant may do (folders, trust) and that accepts secrets, so it must not become a
second, weaker way to configure the daemon.

## Decision

1. **One code path.** `jarvis_storage::settings` holds the catalog of changeable settings, the conversion of a value for its
   kind, the validation, and the storage of keys. `jarvis config`, `jarvis keys` and the console's Settings routes are thin
   callers of it. A change is made on the stored document, checked by the **same parser the daemon starts with**
   (`Config::parse_with_environment`), and saved the way `init` saves it; a refused change leaves the file untouched.
2. **The owner only.** `GET/PUT/DELETE /api/v1/settings[...]`, `PUT/DELETE /api/v1/settings/keys/{model|voice}` and
   `POST /api/v1/restart` require the same bearer credential as the rest of the API (a test asserts 401 on every one). The
   assistant has no tool that reaches them: changing settings, like answering an approval, is an act of the person.
3. **Secrets are write-only.** A key is accepted once (a password field in the console; an environment variable, file or
   standard input in the CLI, never an argument), written to a private file in the configuration directory, and referenced
   by path. No response, listing, log line or error contains it (tests assert this for the listing, the save response and a
   refusal); only a state (`set`, `not set`, `FILE MISSING`, `EMPTY FILE`) is reported. The request body type has no
   `Debug`, so a log of the request cannot print it. The model key can be replaced but not removed.
4. **Applying is explicit.** Settings are read at startup, so a change says "restart to apply". The console's button calls
   `POST /api/v1/restart`, which starts `jarvis restart` (the program beside the daemon) detached; that stops the daemon
   gracefully and starts a new one. Nothing changes under a running daemon.
5. **Approvals stay light.** Nothing here adds a step to answering a yes or no (`ADR-0136`). Widening trust or folders from
   Settings is the owner's own decision, made with the credential they already hold.

## Consequences

- The file is rewritten from its parsed form, so hand-written comments are lost on the first change.
- A page script with the credential can change settings; that is the same authority it already has over approvals and runs,
  and the page's content policy (`ADR-0135`) is what keeps other scripts out.
- Tests: the storage module (one change keeps the rest, invalid changes leave the file byte-identical, keys never in the
  configuration or any listing), the routes (401 everywhere, write-only keys, invalid vs valid changes), and a live run in
  the console (key set through the password field and cleared, a refusal reason shown, restart bringing the voice up).
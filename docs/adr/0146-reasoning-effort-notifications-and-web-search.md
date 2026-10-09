# ADR-0146: A default reasoning effort, desktop notifications for scheduled results, and web search

Status: Accepted
Date: 2026-10-09

## Context

Three gaps stood between JARVIS and a useful daily assistant. A reasoning model's thinking time could not be chosen (only cut off after
the fact); a reminder that fired with no console open was seen nowhere; and the assistant could fetch a page but not look anything up.

## Decision

1. **`daemon.executor_reasoning_effort`** (`none`, `low`, `medium`, `high`; unset leaves it to the model) is the effort a request asks
   for when it does not choose its own. It lives in the model adapter (a default applied where the request is serialised), so the
   run loop is unchanged and the loop's own choice (dropping to `low` after an overrun, ADR-0142) still wins. It needs the live model;
   configuration refuses it otherwise, as for every setting with no consumer.
2. **Desktop notifications** (`daemon.notifications`, on unless turned off). When a scheduled run finishes and no console has asked for the
   schedules in the last 15 seconds, the result is shown with the system's own notifier. The text a model wrote is passed in environment
   variables (Windows, macOS) or after `--` (Linux), never inside a script or command line, and the Windows script uses single quotes only
   (a double quote inside a `-Command` argument was stripped by argument parsing and broke the toast, found by running it). Failure to
   notify is silent; a notification is never what a result depends on. The setting is read each time, so it applies at once.
3. **`jarvis.web.search`** over Ollama's documented `POST /api/web_search` (`docs/research/integrations/ollama-web-search.md`), opt-in by key
   (`daemon.search_api_key_ref`, `jarvis keys set|remove|test search`). Read-only, risk 1, approval by policy like a page fetch; query
   bounded and audited; results fenced as untrusted; failures reported to the model in fixed words; redirects refused. A bad key file does
   not stop the daemon.

## Consequences

A successful search was verified live afterwards with the owner's key (see the research record); before that the invalid-key path was exercised
and the assistant fell back to fetching a page. The toast was shown by running the exact script by hand on Windows; macOS and Linux notifiers are checked by
their command construction only. A daemon with several consoles open elsewhere counts any of them as "open".
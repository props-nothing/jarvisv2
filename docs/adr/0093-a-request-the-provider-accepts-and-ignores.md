# ADR-0093: A request the provider accepts and silently ignores

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract — the `users.watch` request that starts a push lease).
- **Relates to:** `ADR-0060` (a request type with no field a credential could go in), `ADR-0072` (a body is
  rendered once, at the layer that owns the encoding), `ADR-0084` (an argument pair the provider forbids — the
  *loud* sibling of this finding), `ADR-0086` (a refusal names the argument the caller sent), and `ADR-0092`
  (the watch **response**, whose two fields this request's response produces).

## Context

Every operation this crate builds is a `GET` with query parameters, and `HttpRequest` says so:

> "There is also no body: every declared operation is a `GET`, so a request type that could carry a body would
> be a shape nothing uses — and the first caller to put arguments in a body would be writing a request Google
> rejects for a `GET` rather than one this module refused."

`users.watch` is that first caller, and it is **not** a `GET`:

```
POST https://gmail.googleapis.com/gmail/v1/users/{userId}/watch

{ "labelIds": [ string ], "labelFilterAction": enum (LabelFilterAction),
  "labelFilterBehavior": enum (LabelFilterAction), "topicName": string }
```

So the crate needed a second request shape, and reading the reference closely produced **three** distinct
problems, only one of which is about the body.

**(a) The body.** The request takes JSON and authenticates by the caller's bearer header. `FormRequest` already
exists for a body-bearing `POST`, but its entire justification is the credential it holds — a token endpoint's
body carries the `code` and `refresh_token`, which is why that type has a hand-written redacting `Debug` and no
`Display`. A watch body carries a topic name and label ids. Reusing `FormRequest` would put a non-credential
body into the one type whose reason for existing is the credential it hides, and the redaction would then be
hiding nothing while reading as if it were.

**(b) A deprecated field that the provider does not reject.** The schema carries two spellings of the same
concept, and the reference says of the older one:

> "`labelFilterAction` (deprecated) … Filtering behavior of labelIds list specified. This field is deprecated
> because it caused incorrect behavior in some cases; use `labelFilterBehavior` instead."

and of the newer one:

> "`labelFilterBehavior` … This field replaces `labelFilterAction`; if set, `labelFilterAction` is ignored."

So `labelFilterAction` is not an error at the provider. With the new field present it is **ignored**; without
it, it causes "incorrect behavior in some cases". A caller — or a model reading a schema that advertises both —
can send it and receive a `200`.

**(c) A filter that governs nothing when sent alone.** The reference describes `labelIds` as the thing that
"dictates which labels are required for a push notification to be generated", and `labelFilterBehavior` as the
"filtering behavior of `labelIds` list specified". So `include`/`exclude` are **relative to a list**. Sent with
no list, they select nothing, and the provider does not report that: it registers a watch with no filter.

**(b) and (c) are the same defect class**, and it is the reason for this ADR: **a request the provider accepts
and silently ignores.** Every other wrong argument this crate refuses produces a `4xx` the caller sees, or is a
shape it can check (a malformed identifier, an oversized bound). These produce a `200` whose effect is not what
the caller asked for, and nothing downstream reports the difference:

- a connector that sent `labelFilterBehavior: include` with no labels receives **every** change while believing
  it receives some — a mailbox's whole activity where a subset was intended;
- a connector that sent `labelFilterAction` gets a watch whose meaning depends on whether another field
  happened to be present, and "incorrect behavior in some cases" is the provider's own description.

Neither is detectable after the fact, because the watch looks healthy — it has a `historyId` and an
`expiration` like any other — and the notifications that arrive are **valid notifications**. Only the set is
wrong.

## Decision

1. **A second request type, `WatchRequest`, because the credential boundary is what separates the shapes.**
   It holds a URL and an already-rendered JSON body and **no header map**, so no field could hold a bearer
   token — the `ADR-0060` property `HttpRequest` has and `FormRequest` does not need because it authenticates by
   its body. Its `content_type` is a constant, not a field, because it is a fact about the call rather than an
   argument to it.

2. **Its body is not redacted, and that is a decision with a reason.** A watch body holds a topic name in the
   caller's own Cloud project and Gmail's own label vocabulary (`INBOX`, `UNREAD`), so a diagnostic showing it
   discloses nothing the caller did not supply. The contrast with `FormRequest` is the point: redaction is
   applied to what needs it, and a type that redacted a topic name would make every watch diagnostic useless
   while protecting nothing.

3. **`LabelFilterBehavior` is a two-variant enum and produces exactly one field name.** The old spelling is
   therefore **unrepresentable** rather than merely discouraged — there is no parameter a caller could use to
   reach `labelFilterAction`, so it cannot be sent by accident, by a duplicated call, or by a "send both
   spellings to be safe" habit that a plain `Option<String>` field would invite. An absent parameter cannot be
   passed; a refused value can later be widened.

4. **A filter with no label list is refused, with `RequestError::Ignored`.** The variant is new because the
   failure is new: `Argument` and `DisallowedCombination` both describe requests the provider rejects, and this
   one the provider **accepts**. The message names the argument the caller sent and the remedy — send labels, or
   omit the filter — so the refusal is actionable rather than only correct.

5. **An empty label list is refused rather than read as "no filter".** After the emptiness check both render the
   same body, so accepting the empty one would equate a value a caller built by mistake — a loop over zero
   labels — with a deliberate choice. "No filter" is expressed by omitting the argument, which is the one
   rendering that cannot be produced by accident.

6. **The topic name is validated on the raw value, not the trimmed one**, and a test found why: trimming first
   and checking second **removes** a trailing `\n` before the control check looks for one, so
   `"projects/p/topics/t\n"` was silently accepted and sent as the clean string. Trimming is normalization and
   is right for the emptiness and length checks; refusing any control character the caller actually supplied is
   the stricter reading, and a topic name has no legitimate whitespace for it to reject. **The check that exists
   to catch a newline must run before the operation that deletes one.**

## Consequences

- **Two guards were falsified A-B-A**, both compiling: disabling the filter-without-a-list refusal, and
  disabling the empty-list refusal. Each was caught by exactly the test written for it.
- **The deprecated field's absence is asserted, not just its non-use.** The tests parse the rendered body and
  assert `labelFilterAction` is **absent** — in the unfiltered case *and* the filtered one, where sending it
  would look most plausible. A test that only checked `labelFilterBehavior` was present would pass if someone
  later added the old spelling beside it, which is exactly the state the reference describes as
  "incorrect behavior in some cases".
- **A generalisation worth keeping:** *the dangerous argument is the one the provider accepts.* A `400` is
  self-reporting; a `200` that ignored what you sent is not, and it surfaces as wrong behaviour days later with
  no error to trace. When a schema advertises two spellings of one concept, **the deprecated one belongs out of
  reach, not out of favour** — and when a modifier is relative to another argument, the pairing is a rule this
  layer must enforce, because the provider will not.
- **And a second, from the test that failed:** *a validation check and a normalization step on the same value
  must be ordered deliberately.* `trim()` then `is_control()` is not the same predicate as `is_control()` on the
  raw value, and the difference is invisible until a value is trimmed into validity. The failing test is the
  evidence; the fix moved the check, not the test.
- **A limit remains:** no request is sent, so whether Google accepts this body is established only by the live
  smoke test that does not exist. The topic name's **shape** is not validated — the reference requires the
  fully qualified `projects/{project}/topics/{topic}` and that the project "must exactly match your Google
  developer project id", neither of which is checked here: the first is a format this layer could check, and the
  second needs a project id the connector does not hold. Both are recorded rather than guessed.

## Alternatives considered

- **Give `HttpRequest` a body field.** Rejected: it would undo the property `ADR-0060` establishes — that type's
  whole reason for having no header and no credential surface is that every operation it builds is a `GET`. A
  body field would make it a general request type and the absence-of-credential argument would stop holding for
  it.
- **Reuse `FormRequest` for the watch.** Rejected: its justification is the credential its body carries, and
  its `Debug` redacts `rendered_body` for that reason. A watch body would make that redaction hide a topic name
  — a `Debug` that redacts something harmless is a diagnostic that misleads about what needed protecting.
- **Accept `labelFilterAction` as a deprecated alias and map it to the new field.** Rejected: it would send the
  new spelling and silently change what a caller who read the old schema got, and the reference warns the old
  field's *behaviour* was wrong in some cases — so mapping it would import a defect rather than fix one.
- **Accept a `filter` with no `label_ids` and send an unfiltered watch.** Rejected: that is the silent mistake
  the ADR is about. A caller that believes it filtered receives the whole mailbox.
- **Read an empty label list as no filter.** Rejected: it renders identically to the absent case, so a
  mistake-shaped value and a deliberate one would be indistinguishable.
- **Warn rather than refuse on the ignored pairing.** Rejected: this crate has no logging channel on the request
  path, and a warning is a message nobody reads at the layer where the request is already built — the refusal is
  the form the caller actually receives.
- **Validate the topic name's `projects/…/topics/…` shape.** Rejected *for now*, not as wrong: the check needs
  the caller's project id to be meaningful (the reference requires the project to match the one executing the
  watch), and a prefix check without it would refuse a correct name under a different project. Recorded as a
  limit.

## Conditions that would justify revisiting

- A second body-bearing operation is added (Calendar's `channels.watch`, or a Gmail `stop`), at which point the
  question of whether `WatchRequest` should generalise to a `JsonRequest` becomes live — and the answer should
  follow whether the new body carries a credential, which is the axis that separates this type from
  `FormRequest`.
- A live smoke test confirms whether Google *rejects* `labelFilterAction` in some configurations, which would
  move the deprecated field from "accepted and ignored" to "sometimes refused" and make the refusal here
  merely early rather than essential.
- The connector is given the caller's Cloud project id, at which point the topic name's project half can be
  checked against it and the limit above is removed.

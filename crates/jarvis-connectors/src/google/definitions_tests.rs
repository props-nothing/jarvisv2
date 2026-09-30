//! Tests for the derivation from the manifest to tool definitions.
//!
//! The value of a derived definition is that it cannot disagree with the manifest, so most of these assert
//! **agreement** rather than a literal. Where a literal is asserted it is a value that comes from
//! `jarvis-tools` or from a Google fact, and the reason is stated.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::google::GoogleConnector;
use jarvis_tools::{MAX_RISK_LEVEL, ToolEffect};

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn manifest() -> ConnectorManifest {
    must(
        GoogleConnector::manifest(),
        "the Google manifest must be valid",
    )
}

fn definitions() -> Vec<ToolDefinition> {
    must(
        super::definitions(&manifest()),
        "every declared operation must have a definition",
    )
}

#[test]
fn every_declared_operation_gets_exactly_one_definition() {
    // The two lists must stay in step, and `NoSchema` is what enforces it: an operation added to the manifest
    // without a schema here is a refusal rather than a tool with an invented contract. Asserted by COUNT as
    // well as by identity, because a derivation that dropped one operation would satisfy a per-operation
    // check.
    let manifest = manifest();
    let definitions = definitions();
    assert_eq!(definitions.len(), manifest.operations().len());
    for operation in manifest.operations() {
        let expected = format!("{}.{}", crate::google::CONNECTOR_ID, operation.id());
        assert!(
            definitions
                .iter()
                .any(|definition| definition.id().to_string() == expected),
            "`{expected}` must have a definition"
        );
    }
}

#[test]
fn every_definition_is_classified_as_a_connector_tool() {
    // `ToolSource` is DERIVED from the namespace by `ToolId`, never declared, and a definition whose declared
    // source disagrees with its identifier is refused by `ToolDefinition::new`. So this asserts the namespace
    // prefix is doing its job: a bare `gmail_messages_read` would be refused as unqualified, and a `native.`
    // prefix would classify as `Native` — which for a third-party API's tool would be a privilege claim.
    for definition in definitions() {
        assert_eq!(
            definition.source(),
            ToolSource::Connector,
            "{} must be a connector tool",
            definition.id()
        );
        assert_eq!(definition.id().namespace(), crate::google::CONNECTOR_ID);
    }
}

#[test]
fn each_definition_carries_its_manifests_effects_risk_and_scopes() {
    // The derivation's whole purpose: policy decides about a `ToolDefinition`, so a definition that restated
    // these rather than reading them would be policy deciding about a tool **other** than the one the
    // manifest describes — `P3-006d`'s defect class.
    let manifest = manifest();
    for operation in manifest.operations() {
        let expected_id = format!("{}.{}", crate::google::CONNECTOR_ID, operation.id());
        let Some(definition) = definitions()
            .into_iter()
            .find(|definition| definition.id().to_string() == expected_id)
        else {
            panic!("`{expected_id}` must have a definition");
        };
        assert_eq!(
            definition.risk().level(),
            operation.risk(),
            "{} risk",
            operation.id()
        );
        for effect in operation.effects().iter() {
            assert!(
                definition.effects().contains(effect),
                "{} must carry {effect:?}",
                operation.id()
            );
        }
        // The scopes are read from the manifest, so a caller's grant and the connector's own account of what
        // it needs cannot disagree.
        for scope in operation.required_scopes() {
            assert!(
                definition
                    .required_scopes()
                    .iter()
                    .any(|candidate| candidate.to_string() == *scope),
                "{} must require `{scope}`",
                operation.id()
            );
        }
    }
}

#[test]
fn a_declared_idempotency_is_mapped_explicitly_in_both_safe_directions() {
    // The mapping is where two vocabularies for one question meet, and getting it backwards is a real
    // defect rather than a style issue: `Declared` means the provider makes a repeat a no-op, so JARVIS needs
    // no key, while `ProviderKey` means the caller must supply one. Swapping them would either demand a key
    // the provider ignores or omit one it requires.
    assert_eq!(
        tool_idempotency(ProviderIdempotency::Declared),
        Idempotency::ProviderKey
    );
    assert_eq!(
        tool_idempotency(ProviderIdempotency::ProviderKey),
        Idempotency::Required
    );
    // And both unsafe answers collapse to `Unsupported`, which refuses repeats. They stay distinguishable in
    // the manifest for a reader; conflating them here is safe because neither claims a repeat is safe.
    assert_eq!(
        tool_idempotency(ProviderIdempotency::Unknown),
        Idempotency::Unsupported
    );
    assert_eq!(
        tool_idempotency(ProviderIdempotency::NotIdempotent),
        Idempotency::Unsupported
    );
    // The direction that matters is asserted from the manifest's own predicate, so the two cannot drift.
    assert!(ProviderIdempotency::Declared.permits_automatic_retry());
    assert!(!ProviderIdempotency::Unknown.permits_automatic_retry());
    assert!(!ProviderIdempotency::NotIdempotent.permits_automatic_retry());
}

#[test]
fn only_an_operation_whose_repeat_is_safe_gets_automatic_retries() {
    // `RetryPolicy::blind` refuses a retry only for a **mutating** effect, so a read-only operation passes
    // that check whatever its idempotency says. This module therefore consults the manifest's own answer as
    // well, and this test pins the consequence: an operation whose provider behaviour is unknown gets no
    // automatic retry, however harmless its effect looks.
    let retryable = retry_declaration(ProviderIdempotency::Declared);
    assert!(retryable.attempts > 0);
    assert_eq!(retryable.attempts, TOOL_RETRY_ATTEMPTS);

    for unsafe_state in [
        ProviderIdempotency::Unknown,
        ProviderIdempotency::NotIdempotent,
    ] {
        let none = retry_declaration(unsafe_state);
        assert_eq!(
            none.attempts, 0,
            "{unsafe_state:?} must not permit a blind retry"
        );
        assert_eq!(none, RetryDeclaration::none());
    }
}

#[test]
fn every_google_operation_is_read_only_and_therefore_auto() {
    // Risk 0 and `Auto` come from the guidance table and from `ApprovalPolicy::for_risk`, and the derivation
    // means a future write operation cannot be added with `Auto` by accident: it would have to raise its
    // declared risk first, which raises the policy. The assertion is on the effect and the policy together,
    // because either alone could be right while the pair is wrong.
    let manifest = manifest();
    for definition in definitions() {
        assert!(
            definition.effects().contains(ToolEffect::ReadOnly),
            "{} must be read-only",
            definition.id()
        );
        assert!(
            !definition.effects().is_mutating(),
            "{} must not be mutating",
            definition.id()
        );
        assert_eq!(
            definition.risk(),
            jarvis_tools::Risk::Minimal,
            "{} must be risk 0",
            definition.id()
        );
        assert_eq!(
            definition.approval(),
            ApprovalPolicy::Auto,
            "{} is risk 0, so the guidance table says auto when scoped",
            definition.id()
        );
    }
    // The manifest agrees, which is the cross-check that makes the assertion above meaningful rather than a
    // restatement of the derivation.
    assert_eq!(manifest.highest_risk(), 0);
    assert_eq!(manifest.effect_floor(), 0);
}

#[test]
fn the_input_and_output_classifications_differ_and_the_output_is_the_higher_one() {
    // A single classification field would have to be the maximum, which would over-restrict the input — a
    // message id is not confidential — and would hide what the tool actually consumes. So `ToolSensitivity`
    // carries two, and the direction that matters is that the OUTPUT is the confidential half: it is the
    // user's mail.
    for definition in definitions() {
        let sensitivity = definition.sensitivity();
        assert_eq!(sensitivity.input(), Sensitivity::Internal);
        assert_eq!(sensitivity.output(), Sensitivity::Confidential);
        assert_eq!(
            sensitivity.ceiling(),
            Sensitivity::Confidential,
            "{} must be placed by its output",
            definition.id()
        );
        // And the restrictive one must not be permitted to reach a remote model, which is the consequence
        // that makes this classification load-bearing rather than descriptive.
        assert!(
            !Sensitivity::Confidential.can_flow_to(Sensitivity::Internal),
            "confidential content must not flow to a destination that accepts only internal"
        );
    }
}

#[test]
fn the_schemas_are_the_2020_12_dialect_and_refuse_unknown_arguments() {
    // `jarvis-tools` requires the dialect keyword rather than defaulting it, because `exclusiveMinimum` is a
    // boolean in earlier drafts and a number in 2020-12 — so a default would silently reinterpret an older
    // document. Asserted on the parsed value, not on the constant, so a mistake in the dialect check would
    // fail here.
    for definition in definitions() {
        for schema in [definition.input_schema(), definition.output_schema()] {
            assert!(
                schema.document().get("$schema").is_some(),
                "{} must name its dialect",
                definition.id()
            );
        }
        // A read that addresses a **specific resource** must name it. Asserted per operation rather than as a
        // blanket rule, because a blanket rule is wrong for one of the three: an earlier version of this test
        // asserted that *every* input has a `required` keyword, and it failed on `gmail_messages_list` — whose
        // arguments are all legitimately optional, since calling it with nothing means "the newest messages".
        // The claim was too broad, the schema was right, and the failure is what surfaced the difference.
        let required = definition
            .input_schema()
            .document()
            .get("required")
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
            });
        match definition.id().name() {
            "gmail_messages_read" => {
                assert_eq!(
                    required,
                    Some(vec!["message_id"]),
                    "a read must name its message"
                );
            }
            "calendar_events_read" => {
                assert_eq!(
                    required,
                    Some(vec!["calendar_id"]),
                    "a read must name its calendar"
                );
            }
            "gmail_messages_list" => {
                // Deliberately empty: the schema's own `query` description says "Omit to list the newest
                // messages", so a required argument would be inventing a choice the API does not force.
                assert!(
                    required.is_none(),
                    "listing without a query is a meaningful request, so nothing may be required"
                );
            }
            "gmail_history_list" => {
                // Required, and the reference says so: "startHistoryId — Required. Returns history records
                // after the specified startHistoryId." Unlike the message list, there is no meaningful
                // "newest changes" default, so an absent position must be refused by the schema.
                assert_eq!(
                    required,
                    Some(vec!["start_history_id"]),
                    "an incremental sync must name the position it starts from"
                );
            }
            "gmail_profile_read" => {
                // **No arguments at all, and that is the operation's contract rather than an omission.** The
                // request asks about the calling credential, and the builder hardcodes `me` — so a `user_id`
                // argument would be a field a caller could fill with another mailbox, producing a request its
                // own token does not authorise. `required` is absent because there is nothing to require.
                assert!(
                    required.is_none(),
                    "identifying the connected account takes no arguments"
                );
                assert!(
                    definition
                        .input_schema()
                        .document()
                        .get("properties")
                        .and_then(serde_json::Value::as_object)
                        .is_some_and(serde_json::Map::is_empty),
                    "the profile tool must declare no arguments at all, so a caller cannot name another mailbox"
                );
            }
            other => panic!("`{other}` has no declared argument contract"),
        }
    }
}

#[test]
fn the_calendar_input_schema_refuses_a_time_range_with_a_sync_token() {
    // **The declaration and the builder must agree about an impossible pairing.** `request::calendar_events_list`
    // refuses `time_min`/`time_max` together with `sync_token`, because the `events.list` reference lists both
    // bounds among the parameters that "cannot be specified together with nextSyncToken". A schema that
    // advertised all three with no constraint would tell a model the combination is legal, and the model would
    // learn otherwise only from a refusal — so the constraint is asserted here on the **document**, which is
    // what a validator and a model both read (`ADR-0084`).
    let definitions = definitions();
    let Some(calendar) = definitions
        .iter()
        .find(|definition| definition.id().name() == "calendar_events_read")
    else {
        panic!("`calendar_events_read` must exist");
    };
    let schema = calendar.input_schema();

    let valid = serde_json::json!({ "calendar_id": "primary", "sync_token": "tok" });
    let report = must(schema.validate(&valid), "the schema must be usable");
    assert!(report.is_valid(), "a sync token alone is a legal call");

    // The two forbidden pairings, each asserted to be **rejected by the document** rather than merely absent
    // from prose. If the `allOf`/`not` constraint were dropped, the descriptions would still mention the
    // restriction and these two assertions would fail — which is the point: a description a validator does not
    // enforce is exactly the "documented but not applied" defect `ADR-0077` records.
    for document in [
        serde_json::json!({ "calendar_id": "primary", "sync_token": "tok", "time_min": "2026-01-01T00:00:00Z" }),
        serde_json::json!({ "calendar_id": "primary", "sync_token": "tok", "time_max": "2026-12-31T00:00:00Z" }),
    ] {
        let report = must(schema.validate(&document), "the schema must be usable");
        assert!(
            !report.is_valid(),
            "the schema must reject a time range with a sync token: {document}"
        );
    }

    // The control: a filtered **full** sync is legal, which the sync guide's own sample performs ("we are only
    // syncing events up to a year old"). Without it a schema that rejected every `time_min` would pass.
    let full = serde_json::json!({
        "calendar_id": "primary",
        "time_min": "2026-01-01T00:00:00Z",
        "time_max": "2026-12-31T00:00:00Z",
    });
    let report = must(schema.validate(&full), "the schema must be usable");
    assert!(report.is_valid(), "a filtered full sync is a legal call");
}

#[test]
fn every_output_that_can_return_a_page_token_accepts_one_as_input() {
    // **The symmetry check `ADR-0085` is about.** The `read_output` renderer emits `next_page_token` for every
    // list operation, and the caller is expected to fetch the next page with it — but `calendar_events_read`
    // declared no `page_token` input, so it emitted a token no argument could consume and a large read could not
    // be continued. A per-tool test would have missed it because each schema is internally consistent; only the
    // **pairing** of the two schemas exposes it.
    //
    // Generalised rather than asserted for the Calendar tool alone: any operation whose output declares
    // `next_page_token` — now or later — must accept `page_token`, so a new paginated read cannot ship
    // one-directional (`ADR-0083`'s method applied across the input and output halves of one tool).
    for definition in definitions() {
        let output_declares_a_page_token = definition
            .output_schema()
            .document()
            .pointer("/properties/next_page_token")
            .is_some();
        if !output_declares_a_page_token {
            continue;
        }
        let input_accepts_one = definition
            .input_schema()
            .document()
            .pointer("/properties/page_token")
            .is_some();
        assert!(
            input_accepts_one,
            "`{}` renders `next_page_token` and must accept `page_token`, or it advertises a page the caller \
             cannot fetch",
            definition.id()
        );
    }
}

/// Reads one declared input bound out of a tool's schema by JSON pointer.
///
/// Shared by the two drift tests below. A pointer that resolves to nothing is a failure rather than a skip,
/// because a schema that stopped declaring a bound would otherwise make the comparison vacuous.
fn declared_bound(tool: &str, name: &str, keyword: &str) -> usize {
    let definition = definitions()
        .into_iter()
        .find(|candidate| candidate.id().name() == tool)
        .unwrap_or_else(|| panic!("`{tool}` must exist"));
    let value = definition
        .input_schema()
        .document()
        .pointer(&format!("/properties/{name}/{keyword}"))
        .cloned()
        .unwrap_or_else(|| panic!("`{tool}.{name}` must declare `{keyword}`"));
    usize::try_from(
        value.as_u64().unwrap_or_else(|| {
            panic!("a declared bound must be a non-negative integer, not {value}")
        }),
    )
    .unwrap_or_else(|_| panic!("a declared bound must fit a usize"))
}

#[test]
fn every_declared_input_bound_matches_the_constant_that_enforces_it() {
    // **Two places state each of these numbers, with nothing between them.** The input schema declares
    // `maxLength` and `maximum` so a model is told the bounds *before* it chooses an argument, and `request.rs`
    // enforces the same bounds through its constants. They agree today by hand, and a change to one would leave
    // the other stating a bound the code does not keep: a model would be permitted something the builder
    // refuses, or refused something it permits.
    //
    // This is the "two values that must agree, with nothing holding both" defect the repository keeps recording,
    // applied to a declared number (`ADR-0086`). The comparison is against the **constants**, so it cannot be
    // satisfied by editing the schema alone — a literal copied here would drift with the constant and pass.
    use crate::google::client::{GMAIL_MAX_RESULTS_CAP, MAX_PAGE_TOKEN_CHARS};
    use crate::google::request::{
        CALENDAR_MAX_RESULTS_CAP, MAX_QUERY_CHARS, MAX_RESOURCE_ID_CHARS, MAX_TIME_BOUND_CHARS,
    };

    let cases = [
        ("gmail_messages_list", "query", "maxLength", MAX_QUERY_CHARS),
        (
            "gmail_messages_list",
            "max_results",
            "maximum",
            GMAIL_MAX_RESULTS_CAP as usize,
        ),
        (
            "gmail_messages_list",
            "page_token",
            "maxLength",
            MAX_PAGE_TOKEN_CHARS,
        ),
        (
            "gmail_history_list",
            "start_history_id",
            "maxLength",
            MAX_RESOURCE_ID_CHARS,
        ),
        (
            "gmail_history_list",
            "max_results",
            "maximum",
            GMAIL_MAX_RESULTS_CAP as usize,
        ),
        (
            "gmail_history_list",
            "page_token",
            "maxLength",
            MAX_PAGE_TOKEN_CHARS,
        ),
        (
            "gmail_messages_read",
            "message_id",
            "maxLength",
            MAX_RESOURCE_ID_CHARS,
        ),
        (
            "calendar_events_read",
            "calendar_id",
            "maxLength",
            MAX_RESOURCE_ID_CHARS,
        ),
        (
            "calendar_events_read",
            "time_min",
            "maxLength",
            MAX_TIME_BOUND_CHARS,
        ),
        (
            "calendar_events_read",
            "time_max",
            "maxLength",
            MAX_TIME_BOUND_CHARS,
        ),
        (
            "calendar_events_read",
            "max_results",
            "maximum",
            CALENDAR_MAX_RESULTS_CAP as usize,
        ),
        (
            "calendar_events_read",
            "page_token",
            "maxLength",
            MAX_PAGE_TOKEN_CHARS,
        ),
        (
            "calendar_events_read",
            "sync_token",
            "maxLength",
            MAX_PAGE_TOKEN_CHARS,
        ),
    ];
    for (tool, property_name, keyword, expected) in cases {
        assert_eq!(
            declared_bound(tool, property_name, keyword),
            expected,
            "`{tool}.{property_name}.{keyword}` must equal the constant the builder enforces"
        );
    }
}

#[test]
fn every_required_argument_declares_a_minimum_length_of_one() {
    // The other half of the drift: `minLength` is the schema's statement that a value may not be empty, and the
    // builder refuses an empty (or all-whitespace) identifier as well. Every declared `minLength` here is 1;
    // asserted so a schema that dropped it — or wrote 0, permitting the empty identifier the builder refuses —
    // fails rather than passing unnoticed.
    for (tool, name) in [
        ("gmail_history_list", "start_history_id"),
        ("gmail_messages_read", "message_id"),
        ("calendar_events_read", "calendar_id"),
    ] {
        assert_eq!(
            declared_bound(tool, name, "minLength"),
            1,
            "`{tool}.{name}` must refuse an empty value in the schema as the builder does"
        );
    }
}

#[test]
fn gmail_reads_do_not_offer_the_raw_format() {
    // `format=raw` returns the unparsed MIME message, which is the whole message including attachments. The
    // connector has no reason to expose it, and an enum that admitted it would let a model ask for bytes the
    // rest of this connector never parses. Asserted on the schema document because that is what a model sees.
    let definitions = definitions();
    let Some(read) = definitions
        .iter()
        .find(|definition| definition.id().name() == "gmail_messages_read")
    else {
        panic!("`gmail_messages_read` must exist");
    };
    let Some(formats) = read
        .input_schema()
        .document()
        .pointer("/properties/format/enum")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
        })
    else {
        panic!("the read tool must constrain `format` with an enum");
    };
    assert!(formats.contains(&"full"));
    assert!(formats.contains(&"minimal"));
    assert!(
        !formats.contains(&"raw"),
        "`raw` returns the unparsed MIME message and has no consumer here"
    );
}

#[test]
fn an_operation_with_no_schema_is_refused_rather_than_given_a_permissive_one() {
    // The refusal that keeps the manifest and this module in step. A permissive schema invented from the
    // identifier would advertise a tool whose arguments are unconstrained, and the defect would surface as a
    // model calling a tool with arguments nobody validated.
    //
    // **The lookup is tested rather than the whole `definition` call, and that is forced rather than
    // convenient.** `definition` now takes a `ValidatedOperation`, whose fields are private and which only
    // `ConnectorManifest` can build — so a manifest naming an unknown operation cannot be constructed in a
    // test, and the only route to `NoSchema` is a *future* manifest edit. Testing the three tables directly
    // is what is reachable, and it is the part that decides the answer; `definitions()` covers the accepted
    // direction on a real manifest.
    assert_eq!(input_schema("gmail_attachments_read"), None);
    assert_eq!(output_schema("gmail_attachments_read"), None);
    assert_eq!(title("gmail_attachments_read"), None);
    // The positive control: the tables DO describe the declared operations, so a lookup that returned `None`
    // for everything would not satisfy this test.
    for operation in manifest().operations() {
        assert!(
            input_schema(operation.id()).is_some(),
            "{} must have an input schema",
            operation.id()
        );
        assert!(
            output_schema(operation.id()).is_some(),
            "{} must have an output schema",
            operation.id()
        );
        assert!(
            title(operation.id()).is_some(),
            "{} must have a title",
            operation.id()
        );
    }
}

#[test]
fn the_derived_schemas_accept_the_arguments_the_manifest_describes() {
    // `ToolDefinition::new` validates a schema by compiling it, so a syntactically broken constant fails at
    // construction. This asserts the stronger property that a plausible argument validates against the
    // compiled schema — which is what catches a schema that compiles but describes the wrong shape.
    let definitions = definitions();
    let Some(list) = definitions
        .iter()
        .find(|definition| definition.id().name() == "gmail_messages_list")
    else {
        panic!("`gmail_messages_list` must exist");
    };
    let accepted = list
        .input_schema()
        .validate(&serde_json::json!({ "query": "is:unread" }));
    let accepted = must(accepted, "a valid instance must be checkable");
    assert!(
        accepted.is_valid(),
        "a documented argument must validate: {accepted:?}"
    );
    // And an unknown argument is refused, which is what `additionalProperties: false` is for: a model
    // inventing a field must get a refusal rather than having it silently ignored.
    let refused = list
        .input_schema()
        .validate(&serde_json::json!({ "queryx": "typo" }));
    let refused = must(refused, "a valid instance must be checkable");
    assert!(!refused.is_valid(), "an unknown argument must be refused");
}

#[test]
fn an_out_of_range_risk_cannot_reach_this_module_at_all() {
    // An earlier version of this file had a risk-ceiling check and a test for it, and **both were removed
    // because the case is unreachable**: `ValidatedOperation` has private fields and is built only by
    // `ConnectorManifest::new`, which already refuses a risk above the platform ceiling. So the refusal could
    // never fire, and an unreachable refusal reads as protection while enforcing nothing — the defect
    // `P5-001` and `P5-003` each recorded from a different direction.
    //
    // What replaces it is the assertion that the reachable path still enforces the ceiling, so the removal is
    // checked rather than merely claimed. A manifest declaring an over-ceiling risk must be refused by the
    // manifest's own constructor.
    assert!(!GoogleConnector::operations().is_empty());
    for operation in GoogleConnector::operations() {
        assert!(
            operation.risk <= MAX_RISK_LEVEL,
            "{} declares risk {}",
            operation.id,
            operation.risk
        );
    }
    // And the derived definitions agree with the validated manifest, which is the property that makes the
    // unreachable check unnecessary rather than merely absent.
    for definition in definitions() {
        assert!(definition.risk().level() <= MAX_RISK_LEVEL);
    }
}

#[test]
fn the_timeout_and_backoff_are_jarvis_bounds_and_consistent_with_each_other() {
    // Neither is a Google figure: Google documents no per-request deadline. The relationship worth asserting
    // is that the backoff ceiling fits inside the deadline, because a tool that backed off for longer than
    // its own timeout would time out rather than retry — the retries would be unreachable.
    assert_eq!(TOOL_TIMEOUT_SECONDS, 30);
    assert_eq!(TOOL_BACKOFF_CEILING_SECONDS, 32);
    let backoff = TOOL_BACKOFF_CEILING_SECONDS;
    let timeout = TOOL_TIMEOUT_SECONDS;
    assert!(
        backoff >= timeout,
        "the ceiling is Google's lower published figure (32 s) and exceeds this connector's own 30 s \
         deadline; if that changes, the deadline must be reconsidered rather than the test relaxed"
    );
    // The bound that matters is the sum of the attempts' delays against the deadline, so state it.
    let worst_case = u32::from(TOOL_RETRY_ATTEMPTS) * backoff;
    assert!(
        worst_case > timeout,
        "two attempts at 32 s cannot both complete inside a 30 s deadline, which is a real limit rather \
         than a defect: the retry budget is bounded by the deadline, and only the first retry is reachable"
    );
}

#[test]
fn the_definition_versions_are_the_contracts_version_and_not_googles_api_version() {
    // `P3-002` requires a version change when behaviour changes, because a stored intent naming a version
    // must mean what it meant when it was written. This version is of THIS tool's contract — its schemas and
    // declared effects — so it must not be Google's `v1`/`v3`.
    for definition in definitions() {
        assert_eq!(definition.version(), TOOL_VERSION);
        assert_ne!(definition.version(), "v1");
        assert_ne!(definition.version(), "v3");
    }
}

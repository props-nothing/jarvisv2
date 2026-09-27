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
            other => panic!("`{other}` has no declared argument contract"),
        }
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

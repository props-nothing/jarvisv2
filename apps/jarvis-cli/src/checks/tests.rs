use super::*;
use serde_json::json;

#[test]
fn a_model_list_is_read_in_both_the_openai_and_the_ollama_shape() {
    let openai = json!({ "data": [{ "id": "gpt-x" }, { "id": "glm-5.3:cloud" }] });
    assert!(lists_model(&openai, "glm-5.3:cloud"));
    assert!(!lists_model(&openai, "other"));

    let ollama = json!({ "models": [{ "name": "llama3.2:latest" }] });
    assert!(lists_model(&ollama, "llama3.2:latest"));
    assert!(
        !lists_model(&ollama, "llama3.2"),
        "a name must match exactly, not by prefix"
    );

    assert!(!lists_model(&json!({}), "x"));
    assert!(!lists_model(&json!("nonsense"), "x"));
}

/// Each status maps to a verdict and, when it is not fine, to the command that fixes it.
#[test]
fn a_models_status_says_what_is_wrong_and_how_to_fix_it() {
    assert_eq!(classify_models_status(200).0, Level::Ok);
    for rejected in [401, 403] {
        let (level, _, fix) = classify_models_status(rejected);
        assert_eq!(level, Level::Fail);
        assert_eq!(fix, Some("jarvis keys set model"));
    }
    assert_eq!(
        classify_models_status(404).0,
        Level::Warn,
        "a missing list is not proof the server is broken"
    );
    assert_eq!(classify_models_status(500).0, Level::Fail);
    assert!(classify_models_status(500).2.is_some());
}

#[test]
fn every_failing_verdict_carries_a_fix() {
    for status in [401, 403, 404, 500, 502] {
        let (level, _, fix) = classify_models_status(status);
        assert!(
            level == Level::Ok || fix.is_some(),
            "{status} must say what to do"
        );
    }
}

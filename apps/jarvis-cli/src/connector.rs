//! The `jarvis connector` verbs: scaffolding and the completion gate.
//!
//! # Why these are local and every other verb is not
//!
//! `jarvis memory` and `jarvis ask` talk to the daemon because the data lives there. This group does not, and
//! the reason is a property of the subject: a **connector manifest and a readiness review are repository
//! artifacts**. They are written by whoever is building the connector, reviewed in a pull request, and
//! versioned with the code. Putting them behind the daemon would mean starting a service to write a file that
//! the service then has to read back to answer a question about a file.
//!
//! So `connector new` writes two files into the working tree and `connector check` reads one. Neither needs a
//! profile, a socket, or a credential, which is also why they are the two verbs here that work with no daemon
//! running — the same property `jarvis doctor` has, and for the same reason (`doctor` must be able to explain
//! a broken install, so it cannot depend on the install being healthy).
//!
//! # What `check` reports and what it refuses to claim
//!
//! It reports **gaps by name** with each item's evidence strength, and it exits non-zero when any item is
//! missing. What it does **not** do is claim a connector is complete: five of the twelve items are derived
//! from the manifest and cannot be asserted falsely, but seven are the author's attestations and this program
//! can only check their *shape*. The output says so, per item, which is the whole point of carrying
//! [`EvidenceStrength`] through to the operator.

use std::path::{Path, PathBuf};

use jarvis_connectors::{
    Attestation, EvidencePath, LiveSmokeTest, MIN_SUPPORTED_JARVIS_VERSION, ReadinessError,
    ReadinessItem, ReadinessReview, Scaffold, ScaffoldError, ScaffoldRequest,
    proposed_research_path,
};

use crate::output::ExitStatus;

/// Runs one connector verb.
pub fn run(arguments: &[String]) -> ExitStatus {
    let json = crate::json_requested(arguments);
    match arguments.get(1).map(String::as_str) {
        Some("new") => new(arguments, json),
        Some("check") => check(arguments, json),
        Some("items") => items(json),
        Some(other) => {
            eprintln!("jarvis: unknown connector command {other:?}");
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
        None => {
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
    }
}

fn usage() -> &'static str {
    "usage: jarvis connector new <id> --display-name NAME --provider NAME --research-date YYYY-MM-DD \
     [--research-path PATH] [--min-jarvis VERSION] [--dir DIR]\n       \
     jarvis connector check <manifest.json> [--json]\n       \
     jarvis connector items [--json]"
}

/// `jarvis connector items` — the checklist itself.
///
/// The verb exists because the checklist is worth reading *before* someone has a manifest: it is the answer to
/// "what does finishing a connector involve", and a program that only told you after you had failed the gate
/// would be less useful than the Markdown file this replaces.
fn items(json: bool) -> ExitStatus {
    if json {
        let rows: Vec<serde_json::Value> = jarvis_connectors::ALL_ITEMS
            .iter()
            .map(|item| {
                serde_json::json!({
                    "item": item.as_str(),
                    "title": item.title(),
                    "evidence": item.evidence().as_str(),
                    "requires_an_assertion": item.evidence().requires_an_assertion(),
                    "webhook_only": !item.applies_to(&jarvis_connectors::WebhookSupport::Unsupported),
                })
            })
            .collect();
        match serde_json::to_string_pretty(&rows) {
            Ok(text) => println!("{text}"),
            Err(error) => return report_failure("items", &error.to_string()),
        }
        return ExitStatus::Ok;
    }
    println!("A connector is not complete until every applicable item is satisfied.");
    println!(
        "`derived` items are read from the manifest; `declared_*` items are the author's, so their"
    );
    println!("evidence is an assertion this tool can only shape-check.");
    println!();
    for item in jarvis_connectors::ALL_ITEMS {
        let webhook = if item.applies_to(&jarvis_connectors::WebhookSupport::Unsupported) {
            ""
        } else {
            " [push webhooks only]"
        };
        println!(
            "  {:<32} {:<17} {}{webhook}",
            item.as_str(),
            item.evidence().as_str(),
            item.title()
        );
    }
    ExitStatus::Ok
}

/// `jarvis connector new <id> ...`
fn new(arguments: &[String], json: bool) -> ExitStatus {
    let Some(connector_id) = arguments.get(2).filter(|value| !value.starts_with("--")) else {
        eprintln!("jarvis: `connector new` requires a connector identifier");
        eprintln!("{}", usage());
        return ExitStatus::Usage;
    };
    let display_name = match required_flag(arguments, "--display-name") {
        Ok(value) => value,
        Err(status) => return status,
    };
    let provider = match required_flag(arguments, "--provider") {
        Ok(value) => value,
        Err(status) => return status,
    };
    // The research date is **required**, and the reason is this module's own rule: the scaffold must not
    // invent the date anything was verified. Defaulting it to today would put a real date in `last_verified`
    // on a record whose every section is still blank — a stub that looks verified, which is precisely the
    // failure `external-research.md` exists to prevent. Requiring the flag makes the author state the date
    // they are verifying *by*, and a scaffold generated before any research is honest about having done none.
    let research_date = match required_flag(arguments, "--research-date") {
        Ok(value) => value,
        Err(status) => return status,
    };
    let research_path = match flag(arguments, "--research-path") {
        Ok(Some(value)) => value,
        Ok(None) => proposed_research_path(connector_id),
        Err(status) => return status,
    };
    let minimum_jarvis_version = match flag(arguments, "--min-jarvis") {
        Ok(Some(value)) => value,
        Ok(None) => MIN_SUPPORTED_JARVIS_VERSION.to_owned(),
        Err(status) => return status,
    };
    let directory = match flag(arguments, "--dir") {
        Ok(Some(value)) => PathBuf::from(value),
        Ok(None) => PathBuf::from("."),
        Err(status) => return status,
    };

    let request = match ScaffoldRequest::new(
        connector_id,
        display_name,
        provider,
        research_date,
        research_path,
        minimum_jarvis_version,
    ) {
        Ok(request) => request,
        Err(error) => return report_failure("new", &error.to_string()),
    };
    let scaffold = match Scaffold::new(request) {
        Ok(scaffold) => scaffold,
        Err(error) => return report_failure("new", &scaffold_error(&error)),
    };

    // The manifest goes beside the connector's source and the record into the research tree, which mirrors
    // where every existing record lives. Two paths, because a scaffold writing everything into one directory
    // would put a research record somewhere the integration index does not look.
    let manifest_path = directory.join(format!("{connector_id}.manifest.json"));
    let record_path = directory.join(scaffold.research_path());
    for path in [&manifest_path, &record_path] {
        if path.exists() {
            eprintln!("jarvis: refusing to overwrite {}", path.display());
            eprintln!("jarvis: pass a different --dir, or remove the file");
            return ExitStatus::Internal;
        }
    }
    if let Some(parent) = record_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return report_failure(
            "new",
            &format!("could not create {}: {error}", parent.display()),
        );
    }
    for (path, contents) in [
        (&manifest_path, scaffold.manifest_json()),
        (&record_path, scaffold.research_markdown()),
    ] {
        if let Err(error) = std::fs::write(path, contents) {
            return report_failure(
                "new",
                &format!("could not write {}: {error}", path.display()),
            );
        }
    }
    report_scaffold(&scaffold, connector_id, &manifest_path, &record_path, json)
}

/// Reports a written scaffold, in JSON or for a person.
///
/// Extracted from [`new`] because `clippy::too_many_lines` fired on that function, and the split is
/// principled rather than mechanical: everything before this point either validates an argument or touches the
/// filesystem, while everything here only renders what already happened. A rendering failure therefore cannot
/// be confused with a write failure.
fn report_scaffold(
    scaffold: &Scaffold,
    connector_id: &str,
    manifest_path: &Path,
    record_path: &Path,
    json: bool,
) -> ExitStatus {
    if json {
        let reply = serde_json::json!({
            "kind": "connector_scaffold",
            "id": connector_id,
            "manifest": manifest_path.display().to_string(),
            "research": record_path.display().to_string(),
            "outstanding": scaffold
                .outstanding_items()
                .iter()
                .map(|item| item.as_str())
                .collect::<Vec<_>>(),
        });
        return match serde_json::to_string_pretty(&reply) {
            Ok(text) => {
                println!("{text}");
                ExitStatus::Ok
            }
            Err(error) => report_failure("new", &error.to_string()),
        };
    }
    println!("wrote {}", manifest_path.display());
    println!("wrote {}", record_path.display());
    println!();
    println!(
        "The manifest does not validate yet, deliberately: it has no operations, no auth methods, and"
    );
    println!(
        "no documentation links, because all three are facts about the provider that only the research"
    );
    println!("can supply. `jarvis connector items` lists what finishing it involves.");
    ExitStatus::Ok
}

/// Reads a flag that has no default.
///
/// A required flag and an optional one want different messages: the optional case in [`flag`] reports a
/// missing *value*, and this reports a missing *flag* — and a caller who passed nothing at all needs the
/// second.
fn required_flag(arguments: &[String], name: &str) -> Result<String, ExitStatus> {
    if let Some(value) = flag(arguments, name)? {
        return Ok(value);
    }
    eprintln!("jarvis: `connector new` requires {name}");
    Err(ExitStatus::Usage)
}

/// `jarvis connector check <manifest.json>`
fn check(arguments: &[String], json: bool) -> ExitStatus {
    let Some(path) = arguments.get(2).filter(|value| !value.starts_with("--")) else {
        eprintln!("jarvis: `connector check` requires a manifest path");
        eprintln!("{}", usage());
        return ExitStatus::Usage;
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return report_failure("check", &format!("could not read {path}: {error}")),
    };
    let manifest: jarvis_connectors::ConnectorManifest = match serde_json::from_str(&text) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("jarvis: {path} is not a valid connector manifest: {error}");
            return ExitStatus::Internal;
        }
    };

    // The attestations come from a sidecar rather than from the manifest, because the manifest's own schema has
    // `deny_unknown_fields` and adding a readiness section would make a connector's *declarations* part of the
    // document an operator reads to decide whether to grant access. Evidence about a connector's tests is not
    // a fact about the connector's authority, so it does not belong in the manifest.
    let sidecar = sidecar_path(Path::new(path));
    let mut attestations = Vec::new();
    let mut live_smoke = None;
    if sidecar.exists() {
        match read_sidecar(&sidecar) {
            Ok((read, smoke)) => {
                attestations = read;
                live_smoke = smoke;
            }
            Err(message) => return report_failure("check", &message),
        }
    }

    let gaps = ReadinessReview::gaps(&manifest, &attestations, live_smoke.as_ref());
    let review = match ReadinessReview::evaluate(&manifest, &attestations, live_smoke.as_ref()) {
        Ok(review) => Some(review),
        Err(error) => {
            // A refusal here is an author's mistake in the attestation itself rather than a missing item, so it
            // is reported as a failure with the reason instead of as a gap.
            return report_failure("check", &readiness_error(&error));
        }
    };

    if json {
        let reply = serde_json::json!({
            "kind": "connector_readiness",
            "connector": manifest.id().as_str(),
            "version": manifest.version().as_str(),
            "sidecar": sidecar.display().to_string(),
            "sidecar_present": sidecar.exists(),
            "complete": gaps.is_empty(),
            "satisfied": review.as_ref().map(|review| {
                review.satisfied().iter().map(|assessment| serde_json::json!({
                    "item": assessment.item.as_str(),
                    "evidence": assessment.evidence.as_str(),
                    "detail": assessment.detail,
                    "declared": assessment.declared_by.is_some(),
                })).collect::<Vec<_>>()
            }).unwrap_or_default(),
            "gaps": gaps.iter().map(|gap| serde_json::json!({
                "item": gap.item.as_str(),
                "title": gap.title,
                "evidence": gap.evidence.as_str(),
            })).collect::<Vec<_>>(),
        });
        return match serde_json::to_string_pretty(&reply) {
            Ok(text) => {
                println!("{text}");
                ExitStatus::Ok
            }
            Err(error) => report_failure("check", &error.to_string()),
        };
    }
    render_readiness(review.as_ref(), &gaps, &sidecar);
    if gaps.is_empty() {
        ExitStatus::Ok
    } else {
        // NOT a failure: nothing is broken. A connector that is not finished yet is the normal state of work in
        // progress, and an exit code that read as an error would make a CI job fail for a connector somebody is
        // still writing. `DoctorWarnings` is the existing "degraded but not broken" status, which is what this
        // is.
        ExitStatus::DoctorWarnings
    }
}

/// Renders a readiness review for a person.
///
/// Extracted from [`check`] because `clippy::too_many_lines` fired on it, and the split is principled: every
/// branch above either reads a file, parses it, or evaluates the gate, while everything here only prints what
/// was already decided.
///
/// The `declared`/`derived` marker is the interesting part of the output. A gate that printed a uniform "ok"
/// would hide that seven of the twelve items rest on the author's word and three on the manifest itself, which
/// is the distinction the whole checklist is built around.
fn render_readiness(
    review: Option<&ReadinessReview>,
    gaps: &[jarvis_connectors::ReadinessGap],
    sidecar: &Path,
) {
    if let Some(review) = review {
        println!(
            "{} {} — {} item(s) satisfied",
            review.connector(),
            review.version().as_str(),
            review.satisfied().len()
        );
        for assessment in review.satisfied() {
            // Read from the recorded strength rather than inferred from "has no attestation": the conditional
            // item has no attestation and is not derived, so inferring would label a stated reason as a
            // manifest fact.
            println!(
                "  ok       {:<11} {} — {}",
                assessment.evidence.as_str(),
                assessment.item.as_str(),
                assessment.detail
            );
        }
    }
    if !sidecar.exists() {
        println!();
        println!("no readiness sidecar at {}", sidecar.display());
        println!(
            "without it, every declared item is a gap: this tool can read the manifest's own facts"
        );
        println!("but it cannot invent evidence of tests that were never reported.");
    }
    if gaps.is_empty() {
        println!();
        println!("every applicable item is satisfied.");
        return;
    }
    println!();
    println!("{} item(s) still outstanding:", gaps.len());
    for gap in gaps {
        println!("  missing  {:<16} {gap}", gap.evidence.as_str());
    }
}

/// The sidecar path for a manifest.
///
/// `vendor.manifest.json` becomes `vendor.readiness.json`, which is what `jarvis connector new` writes beside
/// its manifest. **This was wrong first, and running the verb is what showed it:** the obvious
/// `with_extension("readiness.json")` replaces only the LAST extension, so it produced
/// `vendor.manifest.readiness.json` — a path the scaffold does not write, so the documented workflow produced a
/// connector whose sidecar was never found, and every declared item silently became a gap. A unit test would
/// not have caught it, because both sides were "correct" against the same wrong assumption; the end-to-end run
/// did.
///
/// A manifest is recognised by its **`.json`** suffix, and everything before it (including a further
/// extension such as `.manifest`) is the connector's name. That rule is stated once here rather than at the
/// writer and the reader separately, because the two disagreeing is exactly the defect above.
fn sidecar_path(manifest: &Path) -> PathBuf {
    // Only `.json` is stripped, so `vendor.manifest.json` -> `vendor.manifest` -> `vendor.readiness.json`.
    let stem = manifest
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("connector");
    let stem = stem.strip_suffix(".manifest").unwrap_or(stem);
    manifest.with_file_name(format!("{stem}.readiness.json"))
}

/// Reads the readiness sidecar.
///
/// The schema is `{"attestations": [...], "live_smoke_test": {...}}`, and every value goes through the same
/// types `jarvis-connectors` uses, so a sidecar cannot express an item the checklist does not have, a path the
/// manifest's own research record would refuse, or a live-smoke-test refusal with no reason.
///
/// The live smoke test is a separate key rather than one of the attestations because its shape differs: it is
/// either `{"present": {"path": ..., "gate": ...}}` or `{"absent": {"reason": ...}}`, and neither is an
/// `Attestation`. Flattening it into the list would mean an item whose evidence is a path *or* a sentence, and
/// that union is what [`LiveSmokeTest`] already is.
type Sidecar = (Vec<(ReadinessItem, Attestation)>, Option<LiveSmokeTest>);

fn read_sidecar(path: &Path) -> Result<Sidecar, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let document: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not a readiness sidecar: {error}", path.display()))?;
    let rows = document
        .get("attestations")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut attestations = Vec::new();
    for row in rows {
        let code = row
            .get("item")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "an attestation is missing its `item`".to_owned())?;
        let item = jarvis_connectors::ALL_ITEMS
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == code)
            .ok_or_else(|| format!("`{code}` is not a readiness item"))?;
        let purpose = row
            .get("purpose")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("the `{code}` attestation needs a `purpose`"))?;
        let path_value = match row.get("path").and_then(serde_json::Value::as_str) {
            Some(value) => Some(
                EvidencePath::new(value)
                    .map_err(|error| format!("the `{code}` attestation's path: {error}"))?,
            ),
            None => None,
        };
        let test_count = row
            .get("test_count")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let covers_failure = row
            .get("covers_failure")
            .and_then(serde_json::Value::as_bool);
        let attestation = match (path_value, test_count, covers_failure) {
            (Some(path), None, None) => Attestation::artifact(purpose, path),
            (None, Some(count), None) => Attestation::suite(purpose, count),
            (None, Some(count), Some(covers)) => Attestation::onboarding(purpose, count, covers),
            (None, None, None) => Attestation::stated(purpose),
            _ => {
                return Err(format!(
                    "the `{code}` attestation mixes a path with a test count; an item is either an artifact \
                     or a suite"
                ));
            }
        };
        attestations.push((item, attestation));
    }
    let live_smoke = match document.get("live_smoke_test") {
        None | Some(serde_json::Value::Null) => None,
        Some(entry) => Some(read_live_smoke(entry)?),
    };
    Ok((attestations, live_smoke))
}

/// Reads the `live_smoke_test` entry.
fn read_live_smoke(entry: &serde_json::Value) -> Result<LiveSmokeTest, String> {
    if let Some(present) = entry.get("present") {
        let path = present
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "a live smoke test needs its `path`".to_owned())?;
        let gate = present
            .get("gate")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                "a live smoke test needs its `gate`, the condition that keeps it opt-in".to_owned()
            })?;
        let path = EvidencePath::new(path)
            .map_err(|error| format!("the live smoke test's path: {error}"))?;
        return LiveSmokeTest::present(path, gate).map_err(|error| error.to_string());
    }
    if let Some(absent) = entry.get("absent") {
        let reason = absent
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "an absent live smoke test needs its `reason`".to_owned())?;
        return LiveSmokeTest::absent(reason).map_err(|error| error.to_string());
    }
    Err("`live_smoke_test` must be either {\"present\": {...}} or {\"absent\": {...}}".to_owned())
}

/// Reads one `--flag value` pair.
fn flag(arguments: &[String], name: &str) -> Result<Option<String>, ExitStatus> {
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == name {
            let Some(value) = arguments.get(index + 1) else {
                eprintln!("jarvis: {name} requires a value");
                return Err(ExitStatus::Usage);
            };
            return Ok(Some(value.clone()));
        }
        index += 1;
    }
    Ok(None)
}

fn report_failure(verb: &str, message: &str) -> ExitStatus {
    eprintln!("jarvis: connector {verb} failed: {message}");
    ExitStatus::Internal
}

/// Renders a scaffold error, expanding the version message.
///
/// The generic `Display` for [`ScaffoldError`] is what a library caller wants; an operator gets the version
/// floor spelled out, because "the minimum JARVIS version is unusable" on its own does not tell them what to
/// type instead.
fn scaffold_error(error: &ScaffoldError) -> String {
    match error {
        ScaffoldError::Version { reason } => {
            format!("{reason} (the earliest supported version is {MIN_SUPPORTED_JARVIS_VERSION})")
        }
        other => other.to_string(),
    }
}

/// Renders a readiness error.
fn readiness_error(error: &ReadinessError) -> String {
    match error {
        ReadinessError::Path { value, reason } => {
            format!("the path `{value}` is unusable: {reason}")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::sidecar_path;
    use std::path::{Path, PathBuf};

    #[test]
    fn the_sidecar_sits_beside_the_manifest_the_scaffold_writes() {
        // The regression test for a real defect: `with_extension("readiness.json")` replaced only `.json`,
        // giving `vendor.manifest.readiness.json`, while the scaffold writes `vendor.readiness.json`. Every
        // declared item became a silent gap. Asserted for the exact name `connector new` produces, so the
        // writer and the reader cannot drift apart again.
        assert_eq!(
            sidecar_path(Path::new(".scratch/vendor.manifest.json")),
            PathBuf::from(".scratch/vendor.readiness.json")
        );
        // A manifest with no `.manifest` infix keeps its name, so a hand-named file works too.
        assert_eq!(
            sidecar_path(Path::new("vendor.json")),
            PathBuf::from("vendor.readiness.json")
        );
        // A path with no stem at all still produces something rather than panicking, because a caller that
        // passed a bare directory has already been told the path is unreadable.
        assert_eq!(
            sidecar_path(Path::new("/")),
            PathBuf::from("/connector.readiness.json")
        );
    }
}

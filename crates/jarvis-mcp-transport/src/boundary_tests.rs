//! **The crate's boundary invariant, asserted rather than documented.**
//!
//! `repository-layout.md` states it as *"no provider SDK type appears in this crate's public surface"* — and
//! **that sentence is not literally true, which is what writing this test revealed.**
//!
//! The invariant's *purpose* is real and worth enforcing: `AGENTS.md` requires that provider SDK types not
//! cross a JARVIS domain boundary, and an error type is a boundary. What the purpose actually forbids is the
//! SDK reaching a **domain or application** crate through this one. This crate is the transport, so it is where
//! the SDK legitimately lives, and a handful of SDK-typed utilities are visible from it:
//!
//! - `connect_over` — a test seam, documented as such since `P3-008e`.
//! - `modern_revision`, `sdk_default_revision`, `describe_negotiated` — the revision helpers in `revision.rs`,
//!   which exist to make the `LATEST = V_2025_11_25` trap executable.
//!
//! `P3-009b` added a **fifth** kind of leak — a public `sdk()` returning the server configuration and a public
//! `service()` returning the SDK's service — and that one had no comparable justification, so it was removed:
//! both are private, the SDK-typed handler methods are `pub(crate)`, and the public door is a JARVIS-owned
//! string (`served_protocol_version`) plus a posture record (`SdkSurfacePosture`).
//!
//! So the test below does what a documented invariant with an exception list should do: **a new name fails, and
//! every recorded exception must still match a real declaration** so the list cannot rot into an endorsement of
//! anything. The doc wording is corrected separately, because a claim that is false in a doc is worse than an
//! exception that is recorded in code.

#![cfg(test)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The SDK's crate name, as it appears in a path inside a signature.
const SDK_CRATE: &str = "rmcp";

/// Public declarations that name the SDK, each with the reason it is the way it is.
///
/// **The list is the honest form of the invariant rather than a loophole.** Writing the test revealed that the
/// documented claim was **already false** before `P3-009b`: `connect_over` and three revision helpers are public
/// with SDK types in their signatures. Recording one known exception is a stronger statement than a doc that
/// quietly says "none", and the accompanying test proves each entry still matches a real declaration.
///
/// A new leak fails the test; a **removal** also fails it, so deleting an exception is a change a reader must
/// make deliberately rather than a silent widening.
const RECORDED_EXCEPTIONS: &[KnownException] = &[
    KnownException {
        declaration: "pub async fn connect_over",
        reason: "a test seam: the negotiation tests drive the real SDK negotiation over an in-process duplex \
                 pair, and `P3-007` recorded that a fixture sharing the code's assumptions cannot find a \
                 revision defect. Gating its visibility behind an off-by-default feature is the clean end \
                 state and is recorded in `TODO.md`.",
    },
    KnownException {
        declaration: "pub fn modern_revision",
        reason: "the SDK's constant for the revision this crate targets, asserted against the JARVIS-owned \
                 string `MODERN_REVISION` so a pinned-SDK change is caught rather than absorbed. Used \
                 internally by the client and by tests; a caller outside the crate has `MODERN_REVISION`.",
    },
    KnownException {
        declaration: "pub fn sdk_default_revision",
        reason: "observational only, so a test can assert the pinned SDK's default is **not** the modern \
                 revision. This is the `LATEST = V_2025_11_25` trap as an executable fact; nothing \
                 negotiates with it.",
    },
    KnownException {
        declaration: "pub fn describe_negotiated",
        reason: "takes the SDK's type **deliberately**, so a caller cannot pass a bare string that disagrees \
                 with what was actually negotiated. Its own doc states that reason.",
    },
];

/// One recorded exception: the declaration as it appears, and why it is public anyway.
struct KnownException {
    /// The start of the declaration's first line, used to match it.
    declaration: &'static str,
    /// Why it is public.
    reason: &'static str,
}

/// The modules whose public surface is this crate's contract.
///
/// The module list is a directory rather than a literal list of files, so a module added next month is covered
/// without anyone remembering to add it here — which is the property the invariant needs, since the violation
/// was a *new* function in a *new* module.
fn source_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "the crate's sources must be findable at {}",
        root.display()
    );
    files
}

/// Recursively collects `.rs` files.
fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// Returns the lines of a file that are a **public item whose signature names an SDK type**.
///
/// A line is examined only when it begins a public declaration, and the declaration's own text plus a few
/// following lines is checked — because a return type and a generic list both commonly wrap.
///
/// # The two forms, and why the first version of this scan missed one
///
/// An SDK type can reach a signature as a **fully-qualified path** (`rmcp::transport::IntoTransport`) or as an
/// **imported name** (`StreamableHttpServerConfig`, brought in by a `use rmcp::…::{…}`). The first version of
/// this function looked only for the literal crate name, so it caught `connect_over` and **missed**
/// `pub fn sdk(&self) -> StreamableHttpServerConfig` — the very violation it was written to catch. A
/// falsification found that: the scan reported nothing while a public SDK-typed function was right there.
///
/// So the imported names are collected first and both forms are checked. `sdk_names` is what makes the
/// import form detectable at all.
fn offending_lines(source: &str) -> Vec<(usize, String)> {
    let names = sdk_names(source);
    let lines: Vec<&str> = source.lines().collect();
    let mut offenders = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("pub ") {
            continue;
        }
        // The declaration plus the next few lines, because a return type and a generic list both commonly wrap.
        let mut declaration = String::new();
        for followed in lines.iter().skip(index).take(6) {
            let _ = writeln!(declaration, "{followed}");
            // Stop at the end of the item's opening: the first `{`, `;`, or `=` at a line end.
            if followed.contains('{') || followed.trim_end().ends_with(';') {
                break;
            }
        }
        let names_it = declaration.contains(SDK_CRATE)
            || names
                .iter()
                .any(|name| mentions(&declaration, name.as_str()));
        if names_it {
            offenders.push((index + 1, trimmed.to_owned()));
        }
    }
    offenders
}

/// Returns whether `text` mentions `name` as a whole identifier.
///
/// A substring test would treat `Tool` as mentioned by `ToolSchema` and `Client` by `ClientConfig`, which would
/// make the scan report unrelated declarations. The boundary is that the character before must not be an
/// identifier character and the character after must not be one either.
fn mentions(text: &str, name: &str) -> bool {
    let mut offset = 0;
    while let Some(found) = text[offset..].find(name) {
        let start = offset + found;
        let end = start + name.len();
        let before_ok = text[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !(character.is_alphanumeric() || character == '_'));
        let after_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|character| !(character.is_alphanumeric() || character == '_'));
        if before_ok && after_ok {
            return true;
        }
        offset = end;
    }
    false
}

/// Collects the type names this file imports from the SDK.
///
/// Handles both `use rmcp::path::Name;` and `use rmcp::path::{A, B};` — the latter spanning several lines, which
/// is how this crate's imports are formatted. CamelCase identifiers are collected from the SDK path, because a
/// type is what can appear in a signature and this crate imports no SDK constants or functions into scope.
fn sdk_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_use = false;
    let mut statement = String::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("use rmcp") {
            in_use = true;
            statement.clear();
        }
        if !in_use {
            continue;
        }
        statement.push_str(trimmed);
        statement.push(' ');
        if trimmed.ends_with(';') {
            in_use = false;
            for candidate in identifier_like(&statement) {
                if !names.contains(&candidate) {
                    names.push(candidate);
                }
            }
        }
    }
    names
}

/// Returns the CamelCase identifiers in a `use` statement.
fn identifier_like(statement: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    for character in statement.chars() {
        if character.is_alphanumeric() || character == '_' {
            current.push(character);
        } else {
            push_candidate(&mut found, &current);
            current.clear();
        }
    }
    push_candidate(&mut found, &current);
    found
}

/// Records `candidate` when it looks like a type name rather than a path segment.
fn push_candidate(found: &mut Vec<String>, candidate: &str) {
    let starts_upper = candidate
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_uppercase());
    if starts_upper && candidate != SDK_CRATE && !found.contains(&candidate.to_owned()) {
        found.push(candidate.to_owned());
    }
}

/// `test`-gated code is exempt, because it is not part of the public surface.
///
/// A `#[cfg(test)]` module compiles against the SDK **inside** the crate without the SDK becoming this crate's
/// contract — which is the distinction `serving_transport_tests.rs` rests on. Exemption is applied by dropping
/// test-only regions before the scan, so a test module cannot accidentally hide a public item among its own.
fn without_test_only_regions(source: &str) -> String {
    let mut result = String::new();
    let mut depth: usize = 0;
    let mut skipping = false;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !skipping && trimmed.starts_with("#[cfg(test)]") {
            skipping = true;
            depth = 0;
            continue;
        }
        if skipping {
            depth = depth.saturating_add(line.matches('{').count());
            depth = depth.saturating_sub(line.matches('}').count());
            if line.contains('}') && depth == 0 {
                skipping = false;
            }
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// **No public declaration in any crate module names the SDK, except the recorded exceptions.**
///
/// Falsified by restoring what `P3-009b` originally wrote — `pub fn sdk(&self) -> StreamableHttpServerConfig`
/// and `pub fn service(&self) -> StreamableHttpService<..>` — which this test reports by line and text.
#[test]
fn no_sdk_type_appears_in_a_public_signature() {
    let mut offenders: Vec<String> = Vec::new();
    for file in source_files() {
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let scan = without_test_only_regions(&source);
        for (line, text) in offending_lines(&scan) {
            let excepted = RECORDED_EXCEPTIONS
                .iter()
                .any(|exception| text.starts_with(exception.declaration));
            if excepted {
                continue;
            }
            offenders.push(format!(
                "{}:{}: {text}",
                file.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&file)
                    .display(),
                line
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "a provider SDK type must not appear in this crate's public surface \
         (`repository-layout.md`, from `AGENTS.md`), but these declarations name `{SDK_CRATE}` and are not \
         recorded exceptions:\n{}",
        offenders.join("\n")
    );
}

/// Every recorded exception still matches a real declaration.
///
/// This is the test that keeps the list from rotting: if `connect_over` is renamed or made private, the
/// exception becomes a stale claim, and a stale exception list is how an exception becomes an endorsement of
/// anything.
#[test]
fn every_recorded_exception_still_matches_a_declaration() {
    let mut all = String::new();
    for file in source_files() {
        if let Ok(source) = std::fs::read_to_string(&file) {
            all.push_str(&without_test_only_regions(&source));
        }
    }
    for exception in RECORDED_EXCEPTIONS {
        assert!(
            all.contains(exception.declaration),
            "the recorded exception `{}` no longer matches a declaration, so the list is stale: {}",
            exception.declaration,
            exception.reason
        );
        assert!(
            !exception.reason.is_empty(),
            "an exception must state why it is public"
        );
    }
}

/// The scan works, so `no_sdk_type_appears_in_a_public_signature` is not passing vacuously.
///
/// Without this, an exemption rule that swallowed every file would make the invariant test green forever —
/// which is the failure mode of every source-reading test.
#[test]
fn the_scan_reports_a_public_declaration_that_names_the_sdk() {
    let source = "pub fn sdk(&self) -> rmcp::transport::StreamableHttpServerConfig {\n\
                  \x20   StreamableHttpServerConfig::default()\n\
                  }\n";
    let offenders = offending_lines(source);
    assert_eq!(offenders.len(), 1, "the scan must report the offender");
    assert!(offenders[0].1.contains("pub fn sdk"));
}

/// A private declaration naming the SDK is **not** reported, which is the whole point.
#[test]
fn the_scan_ignores_a_private_declaration() {
    let source = "fn sdk(&self) -> rmcp::transport::StreamableHttpServerConfig {\n\
                  \x20   StreamableHttpServerConfig::default()\n\
                  }\n";
    assert!(
        offending_lines(source).is_empty(),
        "a private function may name the SDK: the invariant is about the public surface"
    );
}

/// A `#[cfg(test)]` module is dropped before the scan, so crate-internal SDK use is not a violation.
#[test]
fn the_scan_drops_test_only_regions() {
    let source = "#[cfg(test)]\nmod tests {\n    pub fn f() -> rmcp::model::Tool {\n        todo!()\n    }\n}\n";
    let scanned = without_test_only_regions(source);
    assert!(
        !scanned.contains("rmcp"),
        "the test-only region must be dropped, got: {scanned}"
    );
    assert!(offending_lines(&scanned).is_empty());
}

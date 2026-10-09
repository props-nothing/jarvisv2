use super::*;

struct Scratch(PathBuf);

fn unique() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

impl Scratch {
    /// A throwaway profile with a valid configuration: a model, a key file and one granted folder.
    fn new() -> (Self, AppPaths) {
        let root = std::env::temp_dir().join(format!("jarvis-settings-{}", unique()));
        std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
        let paths = AppPaths::from_root(&root).unwrap_or_else(|error| panic!("{error}"));
        std::fs::create_dir_all(paths.config()).unwrap_or_else(|error| panic!("{error}"));
        let key = paths.config().join("model.key");
        std::fs::write(&key, "ollama").unwrap_or_else(|error| panic!("{error}"));
        let folder = root.join("notes");
        std::fs::create_dir_all(&folder).unwrap_or_else(|error| panic!("{error}"));
        let granted = strip_verbatim(
            std::fs::canonicalize(&folder).unwrap_or_else(|error| panic!("{error}")),
        );
        let document = format!(
            "schema_version = 1\n\n[profile]\nname = \"default\"\n\n[logging]\nlevel = \"info\"\n\n[daemon]\nshutdown_timeout_seconds = 30\nhttp_enabled = true\nexecutor_model = \"openai-compatible\"\nexecutor_model_name = \"m1\"\nexecutor_base_url = 'http://localhost:11434/v1'\nexecutor_api_key_ref = '{}'\ntool_workspace_roots = ['{}']\n",
            key.display(),
            granted.display()
        );
        std::fs::write(ConfigStore::from_paths(&paths).path(), document)
            .unwrap_or_else(|error| panic!("{error}"));
        (Self(root), paths)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

fn text(paths: &AppPaths) -> String {
    std::fs::read_to_string(ConfigStore::from_paths(paths).path()).unwrap_or_default()
}

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

/// **Changing one setting changes one setting.** The folder grant and the key file survive a model change, which
/// `init --force` would not have preserved.
#[test]
fn setting_one_value_keeps_everything_else() {
    let (_scratch, paths) = Scratch::new();
    let base = Config::parse_with_environment(&text(&paths), std::iter::empty::<(&str, &str)>());
    assert!(base.is_ok(), "the fixture itself must be valid: {base:?}");

    set(&paths, "executor_model_name", &words("glm-5.3:cloud"))
        .unwrap_or_else(|error| panic!("{error}"));

    let after = text(&paths);
    assert!(after.contains("glm-5.3:cloud"));
    assert!(
        after.contains("tool_workspace_roots"),
        "the folder grant must survive: {after}"
    );
    assert!(after.contains("executor_api_key_ref"));
    assert!(!after.contains("\"m1\""));
}

/// **A change the daemon would refuse is refused here, and the file is left exactly as it was.**
#[test]
fn an_invalid_change_is_refused_and_leaves_the_file_untouched() {
    let (_scratch, paths) = Scratch::new();
    let before = text(&paths);

    // A voice with no key is a setting with no consumer; the daemon's own parser says no.
    assert!(set(&paths, "speech_voice_id", &words("abc")).is_err());
    // A relative key file, a folder that does not exist, bad ports, an unknown setting.
    assert!(set(&paths, "speech_api_key_ref", &words("relative.key")).is_err());
    assert!(
        set(
            &paths,
            "tool_workspace_roots",
            &words("Z:/definitely/not/here")
        )
        .is_err()
    );
    assert!(set(&paths, "http_port", &words("99999")).is_err());
    assert!(set(&paths, "http_port", &words("0")).is_err());
    assert!(set(&paths, "nonsense", &words("x")).is_err());

    assert_eq!(
        text(&paths),
        before,
        "a refused change must not touch the stored document"
    );
}

/// An image without an interpreter is half a sandbox, so setting one half alone is refused by the daemon's parser.
#[test]
fn setting_half_of_a_pair_is_refused() {
    let (_scratch, paths) = Scratch::new();
    assert!(set(&paths, "code_sandbox_interpreter", &words("node -e")).is_err());
    assert!(set(&paths, "code_sandbox_image", &words("node:22-alpine")).is_err());
    assert!(!text(&paths).contains("code_sandbox"));
}

/// **A key goes to a private file and the configuration holds only the path.**
#[test]
fn a_key_is_written_to_a_file_and_never_into_the_configuration() {
    let (_scratch, paths) = Scratch::new();

    set_secret(&paths, SecretKind::Voice, "  sk_secret_0123456789  \n")
        .unwrap_or_else(|error| panic!("{error}"));

    let written = std::fs::read_to_string(paths.config().join("speech.key")).unwrap_or_default();
    assert_eq!(
        written, "sk_secret_0123456789",
        "trimmed, and stored in the key file"
    );
    let document = text(&paths);
    assert!(document.contains("speech_api_key_ref"));
    assert!(
        !document.contains("sk_secret_0123456789"),
        "the key must never be in the configuration"
    );
    assert_eq!(
        key_state(&paths, SecretKind::Voice).map(|state| state.state),
        Ok("set")
    );
    // And nothing a surface reads back contains it.
    let shown = format!("{:?}", list(&paths).unwrap_or_default());
    assert!(!shown.contains("sk_secret_0123456789"));

    remove_secret(&paths, SecretKind::Voice).unwrap_or_else(|error| panic!("{error}"));
    assert!(!text(&paths).contains("speech_api_key_ref"));
    assert!(!paths.config().join("speech.key").exists());
    assert!(
        remove_secret(&paths, SecretKind::Model).is_err(),
        "the model key can only be replaced"
    );
}

#[test]
fn malformed_keys_are_refused_without_echoing_them() {
    let (_scratch, paths) = Scratch::new();
    let before = text(&paths);
    let too_long = "k".repeat(300);
    for bad in ["", "two words", "tab\tinside", too_long.as_str()] {
        let error = set_secret(&paths, SecretKind::Voice, bad)
            .err()
            .unwrap_or_default();
        assert!(!error.is_empty(), "{bad:?} must be refused");
        assert!(
            !error.contains("two words"),
            "a refusal must not repeat the key"
        );
    }
    assert_eq!(text(&paths), before);
    assert!(!paths.config().join("speech.key").exists());
}

#[test]
fn the_list_reports_values_and_whether_key_files_exist() {
    let (_scratch, paths) = Scratch::new();
    let views = list(&paths).unwrap_or_else(|error| panic!("{error}"));
    let model = views
        .iter()
        .find(|view| view.key == "daemon.executor_model_name");
    assert_eq!(model.and_then(|view| view.value.as_deref()), Some("m1"));
    let key = views
        .iter()
        .find(|view| view.key == "daemon.executor_api_key_ref");
    assert_eq!(key.and_then(|view| view.file_state), Some("present"));
    let voice = views
        .iter()
        .find(|view| view.key == "daemon.speech_api_key_ref");
    assert_eq!(voice.and_then(|view| view.value.clone()), None);
}

/// **Related settings are applied together**: a code image alone is refused, but the image with its interpreter is one valid
/// change; and a batch with one bad part writes nothing.
#[test]
fn a_batch_applies_a_pair_together_and_is_all_or_nothing() {
    let (_scratch, paths) = Scratch::new();
    let before = text(&paths);

    let pair = [
        Change {
            key: "code_sandbox_image".to_owned(),
            values: Some(words("node:22-alpine")),
        },
        Change {
            key: "code_sandbox_interpreter".to_owned(),
            values: Some(words("node -e")),
        },
    ];
    apply(&paths, &pair).unwrap_or_else(|error| panic!("{error}"));
    let after = text(&paths);
    assert!(after.contains("node:22-alpine") && after.contains("code_sandbox_interpreter"));

    // Removing both together is fine too, where removing one alone is not.
    let off = [
        Change {
            key: "code_sandbox_image".to_owned(),
            values: None,
        },
        Change {
            key: "code_sandbox_interpreter".to_owned(),
            values: None,
        },
    ];
    apply(&paths, &off).unwrap_or_else(|error| panic!("{error}"));
    assert!(!text(&paths).contains("code_sandbox"));

    // One good part and one bad part: nothing is written.
    let mixed = [
        Change {
            key: "executor_model_name".to_owned(),
            values: Some(words("changed")),
        },
        Change {
            key: "http_port".to_owned(),
            values: Some(words("0")),
        },
    ];
    let snapshot = text(&paths);
    assert!(apply(&paths, &mixed).is_err());
    assert_eq!(
        text(&paths),
        snapshot,
        "a refused batch must leave the file as it was"
    );
    assert!(!before.is_empty());
}

#[test]
fn every_setting_has_a_tab_and_says_what_unset_means() {
    let (_scratch, paths) = Scratch::new();
    let views = list(&paths).unwrap_or_else(|error| panic!("{error}"));
    for view in &views {
        assert!(
            [
                "brain",
                "voice",
                "files",
                "permissions",
                "google",
                "advanced"
            ]
            .contains(&view.group),
            "{} has an unknown tab {}",
            view.key,
            view.group
        );
        assert!(
            !view.unset_means.is_empty(),
            "{} must explain what unset means",
            view.key
        );
    }
    let port = views.iter().find(|view| view.key == "daemon.http_port");
    assert_eq!(port.and_then(|view| view.default), Some("8765"));
}

/// **A tool has exactly one posture**: choosing one removes the others, and the file is left tidy.
#[test]
fn a_tool_has_one_posture_and_changing_it_removes_the_others() {
    let (_scratch, paths) = Scratch::new();
    let tool = "jarvis.files.edit";

    set_tool_posture(&paths, tool, Posture::Ask).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        tool_postures(&paths).unwrap_or_default().get(tool),
        Some(&"ask")
    );

    set_tool_posture(&paths, tool, Posture::Trusted).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        tool_postures(&paths).unwrap_or_default().get(tool),
        Some(&"trusted")
    );
    assert!(is_trusted(&paths, tool));
    assert!(
        !text(&paths).contains("[policy.approval]"),
        "the ask entry must be gone"
    );

    set_tool_posture(&paths, tool, Posture::Off).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        tool_postures(&paths).unwrap_or_default().get(tool),
        Some(&"off")
    );
    assert!(
        !is_trusted(&paths, tool),
        "an off tool must not stay trusted"
    );

    set_tool_posture(&paths, tool, Posture::Default).unwrap_or_else(|error| panic!("{error}"));
    assert!(tool_postures(&paths).unwrap_or_default().is_empty());
    let after = text(&paths);
    assert!(
        !after.contains("trust") && !after.contains("deny") && !after.contains("approval"),
        "back to default leaves no policy residue: {after}"
    );
}

#[test]
fn a_bad_tool_id_or_posture_is_refused() {
    let (_scratch, paths) = Scratch::new();
    let before = text(&paths);
    for bad in ["", "has space", "a/b", "x\"y", &"t".repeat(200)] {
        assert!(
            set_tool_posture(&paths, bad, Posture::Trusted).is_err(),
            "{bad:?}"
        );
    }
    assert!(Posture::parse("always").is_err());
    assert_eq!(text(&paths), before);
}

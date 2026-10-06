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
        let store = ConfigStore::from_paths(&paths);
        std::fs::write(store.path(), document).unwrap_or_else(|error| panic!("{error}"));
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
    assert!(
        base.is_ok(),
        "the fixture itself must be valid: {base:?}\n{}",
        text(&paths)
    );
    assert!(text(&paths).contains("tool_workspace_roots"));

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
    let (scratch, paths) = Scratch::new();
    let source = scratch.0.join("source.key");
    std::fs::write(&source, "  sk_secret_0123456789  \n").unwrap_or_else(|error| panic!("{error}"));
    let arguments = words(&format!("keys set voice --file {}", source.display()));

    set_key(&paths, &arguments, Which::Voice).unwrap_or_else(|error| panic!("{error}"));

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
    let table = current(&paths).unwrap_or_default();
    assert_eq!(key_state(&table, Which::Voice).0, "set");

    remove_key(&paths, Which::Voice).unwrap_or_else(|error| panic!("{error}"));
    assert!(!text(&paths).contains("speech_api_key_ref"));
    assert!(!paths.config().join("speech.key").exists());
    assert!(
        remove_key(&paths, Which::Model).is_err(),
        "the model key can only be replaced"
    );
}

#[test]
fn malformed_keys_are_refused_without_echoing_them() {
    let (scratch, _paths) = Scratch::new();
    let too_long = "k".repeat(300);
    for bad in ["", "two words", "tab\tinside", too_long.as_str()] {
        let source = scratch.0.join("bad.key");
        std::fs::write(&source, bad).unwrap_or_else(|error| panic!("{error}"));
        let arguments = words(&format!("keys set voice --file {}", source.display()));
        let error = read_secret(&arguments, Which::Voice)
            .err()
            .unwrap_or_default();
        assert!(!error.is_empty(), "{bad:?} must be refused");
        assert!(
            !error.contains("two words"),
            "a refusal must not repeat the key"
        );
    }
}

#[test]
fn flags_and_their_values_are_not_positional_words() {
    assert_eq!(
        positionals(&words(
            "keys set voice --from-env ELEVENLABS_API_KEY --root C:/x --json"
        )),
        words("keys set voice")
    );
}

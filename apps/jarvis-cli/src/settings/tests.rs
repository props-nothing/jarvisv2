use super::*;

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

#[test]
fn a_key_comes_from_a_file_and_a_bad_one_is_refused_without_echoing_it() {
    let directory = std::env::temp_dir().join(format!("jarvis-cli-keys-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap_or_else(|error| panic!("{error}"));
    let source = directory.join("k.txt");

    std::fs::write(&source, "  sk_good_0123456789 \n").unwrap_or_else(|error| panic!("{error}"));
    let good = words(&format!("keys set voice --file {}", source.display()));
    assert_eq!(
        read_secret(&good, SecretKind::Voice),
        Ok("sk_good_0123456789".to_owned())
    );

    std::fs::write(&source, "two words").unwrap_or_else(|error| panic!("{error}"));
    let error = read_secret(&good, SecretKind::Voice)
        .err()
        .unwrap_or_default();
    assert!(!error.is_empty() && !error.contains("two words"));

    jarvis_core::remove_scratch_dir(&directory);
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

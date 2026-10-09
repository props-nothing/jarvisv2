//! Desktop notifications, for a result that arrives while nobody is looking at the console.
//!
//! One job: show a short title and body with the operating system's own notifier. It uses only what each system ships
//! (`PowerShell` on Windows, `osascript` on macOS, `notify-send` on Linux), so there is no new dependency, and a system that has
//! none simply shows nothing: a notification is a convenience, never something a result depends on.
//!
//! # The text is never part of a command line or a script
//!
//! The title and body come from a model's answer, which may carry anything. They are passed in **environment variables** (Windows,
//! macOS) or as a separate argument after `--` (Linux), and the scripts read them from there, so no character of the text can
//! end a string or start a command. [`plan`] builds the invocation as data so that is testable on every platform.

use std::collections::BTreeMap;
use std::process::Stdio;

/// The longest title and body shown, in characters. A notification is a nudge; the full answer is in the console.
const MAX_TITLE_CHARS: usize = 80;
const MAX_BODY_CHARS: usize = 240;

const TITLE_VARIABLE: &str = "JARVIS_NOTE_TITLE";
const BODY_VARIABLE: &str = "JARVIS_NOTE_BODY";

/// A program, its arguments and the variables it is given.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invocation {
    /// The program to start.
    pub program: &'static str,
    /// Its arguments.
    pub arguments: Vec<String>,
    /// Extra environment variables.
    pub environment: BTreeMap<&'static str, String>,
}

/// The platforms a notification can be planned for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    /// Windows, through `PowerShell` and the system toast API.
    Windows,
    /// macOS, through `osascript`.
    MacOs,
    /// Linux and other Unix, through `notify-send`.
    Other,
}

impl Platform {
    /// The platform this build runs on.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

const WINDOWS_SCRIPT: &str = "\
$ErrorActionPreference = 'Stop'; \
[void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]; \
[void][Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime]; \
$title = [System.Security.SecurityElement]::Escape($env:JARVIS_NOTE_TITLE); \
$body = [System.Security.SecurityElement]::Escape($env:JARVIS_NOTE_BODY); \
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument; \
$xml.LoadXml('<toast><visual><binding template=''ToastGeneric''><text>' + $title + '</text><text>' + $body + '</text></binding></visual></toast>'); \
$app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe'; \
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show([Windows.UI.Notifications.ToastNotification]::new($xml))";

const MACOS_SCRIPT: &str = "display notification (system attribute \"JARVIS_NOTE_BODY\") with title (system attribute \"JARVIS_NOTE_TITLE\")";

/// Plans the notification for a platform, as data.
#[must_use]
pub fn plan(platform: Platform, title: &str, body: &str) -> Invocation {
    let title = clean(title, MAX_TITLE_CHARS);
    let body = clean(body, MAX_BODY_CHARS);
    match platform {
        Platform::Windows => Invocation {
            program: "powershell.exe",
            arguments: [
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                WINDOWS_SCRIPT,
            ]
            .map(str::to_owned)
            .to_vec(),
            environment: BTreeMap::from([(TITLE_VARIABLE, title), (BODY_VARIABLE, body)]),
        },
        Platform::MacOs => Invocation {
            program: "osascript",
            arguments: vec!["-e".to_owned(), MACOS_SCRIPT.to_owned()],
            environment: BTreeMap::from([(TITLE_VARIABLE, title), (BODY_VARIABLE, body)]),
        },
        Platform::Other => Invocation {
            program: "notify-send",
            arguments: vec!["--app-name=JARVIS".to_owned(), "--".to_owned(), title, body],
            environment: BTreeMap::new(),
        },
    }
}

/// One line of plain text no longer than `limit` characters: control characters become spaces and runs of them collapse.
fn clean(text: &str, limit: usize) -> String {
    let collapsed: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.chars().count() <= limit {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(limit.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}

/// Shows a notification, without waiting for it and without ever failing the caller.
pub fn show(title: &str, body: &str) {
    let invocation = plan(Platform::current(), title, body);
    let mut command = std::process::Command::new(invocation.program);
    command
        .args(&invocation.arguments)
        .envs(&invocation.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up for the PowerShell that shows the toast.
        command.creation_flags(0x0800_0000);
    }
    match command.spawn() {
        Ok(mut child) => {
            // Reaped on its own thread so a notifier that is slow to exit never blocks the scheduler or leaves a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => tracing::debug!(%error, "no desktop notifier is available"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOSTILE: &str =
        "it's \"quoted\"; $(calc) `whoami` <b>x</b> \u{1b}[31m\nsecond line\t&& rm -rf /";

    /// **No character of a notification's text reaches a script or a command line it could break out of.**
    ///
    /// The text is what a model wrote. On Windows and macOS it travels in environment variables and the script only names
    /// them; on Linux it is a plain argument after `--` to a program that takes no shell. Asserted for each platform by
    /// checking the script and arguments do not contain the text at all.
    #[test]
    fn the_text_travels_apart_from_the_script() {
        for platform in [Platform::Windows, Platform::MacOs] {
            let plan = plan(platform, HOSTILE, HOSTILE);
            for argument in &plan.arguments {
                assert!(
                    !argument.contains("whoami") && !argument.contains("rm -rf"),
                    "{platform:?}: {argument}"
                );
            }
            assert!(
                plan.environment[BODY_VARIABLE].contains("whoami"),
                "the text is in the variable"
            );
            assert!(
                !plan.environment[BODY_VARIABLE]
                    .chars()
                    .any(char::is_control),
                "control characters are removed from the text"
            );
        }
        let linux = plan(Platform::Other, HOSTILE, HOSTILE);
        assert_eq!(linux.program, "notify-send");
        let separator = linux.arguments.iter().position(|argument| argument == "--");
        assert_eq!(
            separator,
            Some(1),
            "the text follows `--`, so it can never be read as an option"
        );
        assert_eq!(linux.arguments.len(), 4);
    }

    #[test]
    fn the_text_is_one_short_line() {
        let long = "word ".repeat(200);
        let plan = plan(Platform::Other, "a\nb", &long);
        assert_eq!(plan.arguments[2], "a b");
        assert!(plan.arguments[3].chars().count() <= MAX_BODY_CHARS);
        assert!(plan.arguments[3].ends_with('\u{2026}'));
    }

    #[test]
    fn the_windows_script_escapes_the_text_as_xml_before_it_is_markup() {
        let plan = plan(Platform::Windows, "t", "b");
        let script = plan.arguments.last().unwrap_or_else(|| panic!("a script"));
        assert!(script.contains("SecurityElement]::Escape($env:JARVIS_NOTE_BODY)"));
        assert!(script.contains("SecurityElement]::Escape($env:JARVIS_NOTE_TITLE)"));
        // A double quote inside a `-Command` argument is stripped by Windows argument parsing and broke the XML once (found by
        // running the script), so the script uses single quotes only.
        assert!(!script.contains('"'), "{script}");
    }
}

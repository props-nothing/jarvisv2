//! The `jarvis project` verbs: what JARVIS is told about long-running work (`ADR-0151`).
//!
//! A project is a goal, the owner's standing guidance, a working folder and a journal. Runs that belong to a project (`jarvis ask --project
//! NAME`, `jarvis schedule add ... --project NAME`, or a console conversation started in it) are told all of that, so the owner writes it once
//! instead of pasting it into every objective. A project changes what a run is told, never what it may do: approvals, the permission
//! postures and the granted folders are exactly what they were.

use jarvis_protocol::{
    AddProjectNoteRequest, CreateProjectRequest, ProjectReply, UpdateProjectRequest,
};

use crate::api_client::ApiClient;
use crate::output::ExitStatus;
use crate::schedule::{print_json, report};

/// The usage text for `jarvis project`.
pub(crate) const fn project_usage() -> &'static str {
    "usage: jarvis project <add|list|show|set|pause|resume|done|note|remove> [...]\n       jarvis project add NAME [--goal TEXT] [--guidance TEXT | --guidance-file FILE] [--folder RELATIVE/PATH] [--daily-limit N]\n       jarvis project list [--json]\n       jarvis project show NAME [--json]\n       jarvis project set NAME [--name NEW] [--goal TEXT] [--guidance TEXT | --guidance-file FILE] [--folder PATH] [--daily-limit N] [--status active|paused|done]\n       jarvis project pause|resume|done NAME\n       jarvis project note NAME TEXT... [--kind progress|decision|blocker|next|result|owner]\n       jarvis project remove NAME"
}

/// Runs one project verb.
pub async fn run_project(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let verb = arguments.get(1).map(String::as_str);
    let flags = match Flags::parse(arguments.get(2..).unwrap_or_default()) {
        Ok(flags) => flags,
        Err(message) => {
            eprintln!("jarvis: {message}");
            eprintln!("{}", project_usage());
            return ExitStatus::Usage;
        }
    };
    match verb {
        Some("list") | None => list(client, flags.json).await,
        Some("add") => add(client, &flags).await,
        Some("show") => show(client, &flags).await,
        Some("set") => set(client, &flags).await,
        Some("pause") => set_status(client, &flags, "paused").await,
        Some("resume") => set_status(client, &flags, "active").await,
        Some("done") => set_status(client, &flags, "done").await,
        Some("note") => note(client, &flags).await,
        Some("remove") => remove(client, &flags).await,
        Some(other) => {
            eprintln!("jarvis: unknown project command {other:?}");
            eprintln!("{}", project_usage());
            ExitStatus::Usage
        }
    }
}

/// The words and flags after the verb.
#[derive(Debug, Default, PartialEq)]
struct Flags {
    words: Vec<String>,
    name: Option<String>,
    goal: Option<String>,
    guidance: Option<String>,
    folder: Option<String>,
    status: Option<String>,
    daily_limit: Option<u32>,
    kind: Option<String>,
    json: bool,
}

impl Flags {
    /// Splits the arguments after the verb. Flags may come anywhere, and a flag's value is never taken for a word.
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut flags = Self::default();
        let mut index = 0;
        while index < arguments.len() {
            let argument = arguments[index].as_str();
            if argument == "--json" {
                flags.json = true;
                index += 1;
                continue;
            }
            if !argument.starts_with("--") {
                flags.words.push(arguments[index].clone());
                index += 1;
                continue;
            }
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| format!("{argument} needs a value"))?
                .clone();
            match argument {
                "--name" => flags.name = Some(value),
                "--goal" => flags.goal = Some(value),
                "--guidance" => flags.guidance = Some(value),
                "--guidance-file" => {
                    let text = std::fs::read_to_string(&value)
                        .map_err(|error| format!("could not read {value}: {error}"))?;
                    flags.guidance = Some(text);
                }
                "--folder" => flags.folder = Some(value),
                "--status" => flags.status = Some(value),
                "--daily-limit" => {
                    let limit = value.parse::<u32>().map_err(|_| {
                        "--daily-limit needs a whole number (0 means no cap)".to_owned()
                    })?;
                    flags.daily_limit = Some(limit);
                }
                "--kind" => flags.kind = Some(value),
                // Handled before the verb is dispatched.
                "--root" => {}
                other => return Err(format!("unknown option {other}")),
            }
            index += 2;
        }
        Ok(flags)
    }

    fn target(&self) -> Result<&str, String> {
        self.words.first().map(String::as_str).ok_or_else(|| {
            "name the project, for example: jarvis project show Prospecting".to_owned()
        })
    }
}

/// Resolves what the person typed to a project: its name (ignoring case) or an identifier prefix that names exactly one.
async fn resolve(client: &ApiClient, typed: &str) -> Result<ProjectReply, ExitStatus> {
    let reply = client
        .list_projects()
        .await
        .map_err(|error| report(&error))?;
    let typed_lower = typed.to_lowercase();
    if let Some(found) = reply
        .projects
        .iter()
        .find(|project| project.name.to_lowercase() == typed_lower)
    {
        return Ok(found.clone());
    }
    let matches: Vec<&ProjectReply> = reply
        .projects
        .iter()
        .filter(|project| !typed.is_empty() && project.project_id.starts_with(typed))
        .collect();
    match matches.as_slice() {
        [only] => Ok((*only).clone()),
        [] => {
            eprintln!("jarvis: no project is named {typed:?} (see `jarvis project list`)");
            Err(ExitStatus::Rejected)
        }
        _ => {
            eprintln!("jarvis: {typed:?} matches more than one project; type more of it");
            Err(ExitStatus::Rejected)
        }
    }
}

fn usage_error(message: &str) -> ExitStatus {
    eprintln!("jarvis: {message}");
    eprintln!("{}", project_usage());
    ExitStatus::Usage
}

async fn add(client: &ApiClient, flags: &Flags) -> ExitStatus {
    let name = match flags.target() {
        Ok(name) => name.to_owned(),
        Err(message) => return usage_error(&message),
    };
    let request = CreateProjectRequest {
        name,
        goal: flags.goal.clone().unwrap_or_default(),
        guidance: flags.guidance.clone().unwrap_or_default(),
        folder: flags.folder.clone().unwrap_or_default(),
        daily_run_limit: flags.daily_limit.unwrap_or_default(),
    };
    match client.create_project(&request).await {
        Ok(project) => {
            println!("created project {} ({})", project.name, project.project_id);
            eprintln!(
                "jarvis: start work in it with `jarvis ask --project \"{}\" ...` or `jarvis schedule add ... --project \"{}\"`",
                project.name, project.name
            );
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

async fn list(client: &ApiClient, json: bool) -> ExitStatus {
    let reply = match client.list_projects().await {
        Ok(reply) => reply,
        Err(error) => return report(&error),
    };
    if json {
        return print_json(&reply);
    }
    if reply.projects.is_empty() {
        println!("no projects yet: create one with `jarvis project add NAME --goal \"...\"`");
        return ExitStatus::Ok;
    }
    println!("{} project(s)", reply.total);
    for project in &reply.projects {
        println!(
            "  {}  [{}]  {}",
            project.project_id, project.status, project.name
        );
        if !project.goal.is_empty() {
            println!("      goal: {}", first_line(&project.goal));
        }
    }
    ExitStatus::Ok
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

async fn show(client: &ApiClient, flags: &Flags) -> ExitStatus {
    let typed = match flags.target() {
        Ok(typed) => typed,
        Err(message) => return usage_error(&message),
    };
    let project = match resolve(client, typed).await {
        Ok(project) => project,
        Err(status) => return status,
    };
    let detail = match client.read_project(&project.project_id).await {
        Ok(detail) => detail,
        Err(error) => return report(&error),
    };
    if flags.json {
        return print_json(&detail);
    }
    let project = &detail.project;
    println!("{}  [{}]", project.name, project.status);
    println!("  id:       {}", project.project_id);
    if !project.folder.is_empty() {
        println!("  folder:   {}", project.folder);
    }
    if project.daily_run_limit > 0 {
        println!(
            "  cap:      {} scheduled run(s) per 24 hours ({} used)",
            project.daily_run_limit, project.runs_today
        );
    }
    if !project.goal.is_empty() {
        println!("  goal:     {}", project.goal);
    }
    if !project.guidance.is_empty() {
        println!("  guidance:");
        for line in project.guidance.lines() {
            println!("    {line}");
        }
    }
    if detail.notes.is_empty() {
        println!("  journal:  (no entries yet)");
    } else {
        println!("  journal:");
        for note in &detail.notes {
            let day = note.created_at.get(..10).unwrap_or("");
            println!("    {day} [{}] {}", note.kind, note.text.replace('\n', " "));
        }
    }
    ExitStatus::Ok
}

async fn apply(client: &ApiClient, typed: &str, request: UpdateProjectRequest) -> ExitStatus {
    let project = match resolve(client, typed).await {
        Ok(project) => project,
        Err(status) => return status,
    };
    match client.update_project(&project.project_id, &request).await {
        Ok(changed) => {
            println!("{}  [{}]", changed.name, changed.status);
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

async fn set(client: &ApiClient, flags: &Flags) -> ExitStatus {
    let typed = match flags.target() {
        Ok(typed) => typed,
        Err(message) => return usage_error(&message),
    };
    let request = UpdateProjectRequest {
        name: flags.name.clone(),
        goal: flags.goal.clone(),
        guidance: flags.guidance.clone(),
        folder: flags.folder.clone(),
        status: flags.status.clone(),
        daily_run_limit: flags.daily_limit,
    };
    if request == UpdateProjectRequest::default() {
        return usage_error("project set needs something to change, for example --goal \"...\"");
    }
    apply(client, typed, request).await
}

async fn set_status(client: &ApiClient, flags: &Flags, status: &str) -> ExitStatus {
    let typed = match flags.target() {
        Ok(typed) => typed,
        Err(message) => return usage_error(&message),
    };
    let request = UpdateProjectRequest {
        status: Some(status.to_owned()),
        ..UpdateProjectRequest::default()
    };
    apply(client, typed, request).await
}

async fn note(client: &ApiClient, flags: &Flags) -> ExitStatus {
    let typed = match flags.target() {
        Ok(typed) => typed,
        Err(message) => return usage_error(&message),
    };
    let text = flags.words.get(1..).unwrap_or_default().join(" ");
    if text.trim().is_empty() {
        return usage_error(
            "project note needs the note, for example: jarvis project note Prospecting start with the Dutch list",
        );
    }
    let project = match resolve(client, typed).await {
        Ok(project) => project,
        Err(status) => return status,
    };
    let request = AddProjectNoteRequest {
        text,
        kind: flags.kind.clone(),
    };
    match client.add_project_note(&project.project_id, &request).await {
        Ok(note) => {
            println!("noted in {} [{}]", project.name, note.kind);
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

async fn remove(client: &ApiClient, flags: &Flags) -> ExitStatus {
    let typed = match flags.target() {
        Ok(typed) => typed,
        Err(message) => return usage_error(&message),
    };
    let project = match resolve(client, typed).await {
        Ok(project) => project,
        Err(status) => return status,
    };
    match client.remove_project(&project.project_id).await {
        Ok(()) => {
            println!(
                "removed project {} (its conversations and schedules are kept)",
                project.name
            );
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn flags_are_taken_from_anywhere_and_their_values_are_not_words() {
        let flags = Flags::parse(&args("Prospecting --goal win --folder sales/nl --json"))
            .unwrap_or_default();
        assert_eq!(flags.words, ["Prospecting"]);
        assert_eq!(flags.goal.as_deref(), Some("win"));
        assert_eq!(flags.folder.as_deref(), Some("sales/nl"));
        assert!(flags.json);
    }

    #[test]
    fn a_note_keeps_its_words_after_the_project() {
        let flags = Flags::parse(&args("Prospecting start with the Dutch list --kind next"))
            .unwrap_or_default();
        assert_eq!(flags.target(), Ok("Prospecting"));
        assert_eq!(flags.words[1..].join(" "), "start with the Dutch list");
        assert_eq!(flags.kind.as_deref(), Some("next"));
    }

    #[test]
    fn a_missing_value_and_an_unknown_option_are_usage_errors() {
        assert!(Flags::parse(&args("P --goal")).is_err());
        assert!(Flags::parse(&args("P --frobnicate x")).is_err());
        assert!(Flags::default().target().is_err());
    }

    #[test]
    fn guidance_can_be_read_from_a_file_and_a_missing_file_is_named() {
        let path = std::env::temp_dir().join(format!(
            "jarvis-guidance-{}.txt",
            jarvis_core::scratch_tag()
        ));
        assert!(std::fs::write(&path, "Write in Dutch.\nBe brief.").is_ok());
        let flags = Flags::parse(&[
            "P".to_owned(),
            "--guidance-file".to_owned(),
            path.display().to_string(),
        ])
        .unwrap_or_default();
        assert_eq!(
            flags.guidance.as_deref(),
            Some("Write in Dutch.\nBe brief.")
        );
        let _ = std::fs::remove_file(&path);
        let missing = Flags::parse(&["--guidance-file".to_owned(), path.display().to_string()]);
        assert!(missing.is_err());
    }
}

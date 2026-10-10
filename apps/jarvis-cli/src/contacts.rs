//! The `jarvis contacts` verbs: the people and companies JARVIS is working with (`ADR-0155`).
//!
//! `export` writes a CSV to standard output, so the list can leave JARVIS as easily as a spreadsheet came in. `add` is the owner's own
//! write: unlike a model, the owner can move a contact out of `do_not_contact`.

use jarvis_protocol::{ContactReply, SaveContactRequest};

use crate::api_client::ApiClient;
use crate::output::ExitStatus;
use crate::schedule::{print_json, report};

const USAGE: &str = "usage: jarvis contacts <list|stats|add|remove|export> [...]\n       jarvis contacts list [--status STATUS] [--find TEXT] [--json]\n       jarvis contacts stats\n       jarvis contacts add COMPANY [--person NAME] [--role TEXT] [--email ADDRESS] [--phone NUMBER] [--status STATUS] [--notes TEXT] [--source TEXT]\n       jarvis contacts remove ID\n       jarvis contacts export > contacts.csv\n       statuses: new, contacted, replied, meeting, won, lost, do_not_contact";

/// One page is this many contacts; the CLI reads pages until the list ends.
const PAGE: u32 = 500;

/// Runs one contacts verb.
pub async fn run_contacts(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let rest = arguments.get(2..).unwrap_or_default();
    match arguments.get(1).map(String::as_str) {
        Some("list") | None => list(client, rest).await,
        Some("stats") => stats(client).await,
        Some("add") => add(client, rest).await,
        Some("remove") => remove(client, rest).await,
        Some("export") => export(client).await,
        Some(other) => {
            eprintln!("jarvis: unknown contacts command {other:?}");
            eprintln!("{USAGE}");
            ExitStatus::Usage
        }
    }
}

fn flag<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
}

/// The words that are not flags or flag values.
fn words(arguments: &[String]) -> Vec<&str> {
    let mut words = Vec::new();
    let mut skip = false;
    for argument in arguments {
        if skip {
            skip = false;
        } else if argument == "--json" {
        } else if argument.starts_with("--") {
            skip = true;
        } else {
            words.push(argument.as_str());
        }
    }
    words
}

async fn all(
    client: &ApiClient,
    status: Option<&str>,
) -> Result<(Vec<ContactReply>, Vec<jarvis_protocol::ContactCount>), ExitStatus> {
    let mut contacts = Vec::new();
    loop {
        let offset = u32::try_from(contacts.len()).unwrap_or(u32::MAX);
        let page = client
            .list_contacts(status, PAGE, offset)
            .await
            .map_err(|error| report(&error))?;
        let done = page.contacts.len() < PAGE as usize;
        contacts.extend(page.contacts);
        if done {
            return Ok((contacts, page.counts));
        }
    }
}

fn matches(contact: &ContactReply, text: &str) -> bool {
    let text = text.to_lowercase();
    [
        &contact.company,
        &contact.person,
        &contact.email,
        &contact.notes,
    ]
    .iter()
    .any(|field| field.to_lowercase().contains(&text))
}

async fn list(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let (mut contacts, _) = match all(client, flag(arguments, "--status")).await {
        Ok(found) => found,
        Err(status) => return status,
    };
    if let Some(text) = flag(arguments, "--find") {
        contacts.retain(|contact| matches(contact, text));
    }
    if arguments.iter().any(|argument| argument == "--json") {
        return print_json(&contacts);
    }
    if contacts.is_empty() {
        println!("no contacts");
        return ExitStatus::Ok;
    }
    println!("{} contact(s)", contacts.len());
    for contact in &contacts {
        let who = if contact.person.is_empty() {
            String::new()
        } else {
            format!(" / {}", contact.person)
        };
        let email = if contact.email.is_empty() {
            String::new()
        } else {
            format!("  <{}>", contact.email)
        };
        println!("  [{}] {}{who}{email}", contact.status, contact.company);
        if !contact.notes.is_empty() {
            println!("        {}", contact.notes.lines().next().unwrap_or(""));
        }
    }
    ExitStatus::Ok
}

async fn stats(client: &ApiClient) -> ExitStatus {
    match client.list_contacts(None, 1, 0).await {
        Ok(page) => {
            let total: u32 = page.counts.iter().map(|count| count.count).sum();
            println!("{total} contact(s)");
            for count in &page.counts {
                println!("  {:<15} {}", count.status, count.count);
            }
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

async fn add(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let named = words(arguments).join(" ");
    let company = (!named.is_empty()).then_some(named);
    let own = |name: &str| flag(arguments, name).map(str::to_owned);
    let request = SaveContactRequest {
        company,
        person: own("--person"),
        role: own("--role"),
        email: own("--email"),
        phone: own("--phone"),
        status: own("--status"),
        notes: own("--notes"),
        source: own("--source"),
    };
    if request == SaveContactRequest::default() {
        eprintln!("{USAGE}");
        return ExitStatus::Usage;
    }
    match client.save_contact(&request).await {
        Ok(contact) => {
            println!(
                "saved {} [{}] ({})",
                contact.company, contact.status, contact.contact_id
            );
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

async fn remove(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let Some(id) = words(arguments).first().map(|word| (*word).to_owned()) else {
        eprintln!("{USAGE}");
        return ExitStatus::Usage;
    };
    match client.remove_contact(&id).await {
        Ok(()) => {
            println!("removed");
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

async fn export(client: &ApiClient) -> ExitStatus {
    let (contacts, _) = match all(client, None).await {
        Ok(found) => found,
        Err(status) => return status,
    };
    print!("{}", to_csv(&contacts));
    ExitStatus::Ok
}

/// One CSV field, quoted when it needs to be. A leading `=`, `+`, `-` or `@` is defused so a spreadsheet does not run it as a formula.
fn csv_field(text: &str) -> String {
    let defused = if text.starts_with(['=', '+', '-', '@']) {
        format!("'{text}")
    } else {
        text.to_owned()
    };
    if defused.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", defused.replace('"', "\"\""))
    } else {
        defused
    }
}

/// The contacts as CSV with a header row.
pub(crate) fn to_csv(contacts: &[ContactReply]) -> String {
    let mut text = String::from("company,person,role,email,phone,status,notes,source,updated_at\n");
    for contact in contacts {
        let row = [
            &contact.company,
            &contact.person,
            &contact.role,
            &contact.email,
            &contact.phone,
            &contact.status,
            &contact.notes,
            &contact.source,
            &contact.updated_at,
        ]
        .map(|field| csv_field(field))
        .join(",");
        text.push_str(&row);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact(company: &str, notes: &str) -> ContactReply {
        ContactReply {
            contact_id: "id".to_owned(),
            company: company.to_owned(),
            person: String::new(),
            role: String::new(),
            email: "a@b.nl".to_owned(),
            phone: String::new(),
            status: "new".to_owned(),
            notes: notes.to_owned(),
            source: String::new(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn the_csv_quotes_what_needs_it_and_defuses_formulas() {
        let csv = to_csv(&[
            contact("Acme, BV", "said \"call back\""),
            contact("=HYPERLINK(1)", ""),
        ]);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines[0],
            "company,person,role,email,phone,status,notes,source,updated_at"
        );
        assert!(
            lines[1].starts_with("\"Acme, BV\",")
                && lines[1].contains("\"said \"\"call back\"\"\""),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].starts_with("'=HYPERLINK(1),"),
            "a spreadsheet must not run it: {}",
            lines[2]
        );
    }

    #[test]
    fn flags_and_words_are_told_apart() {
        let arguments: Vec<String> = "Acme BV --person Eva --email e@a.nl --json"
            .split(' ')
            .map(str::to_owned)
            .collect();
        assert_eq!(words(&arguments), ["Acme", "BV"]);
        assert_eq!(flag(&arguments, "--person"), Some("Eva"));
        assert_eq!(flag(&arguments, "--phone"), None);
    }

    #[test]
    fn a_search_text_matches_company_person_address_or_notes_without_regard_to_case() {
        let found = contact("Acme", "Asked for a QUOTE");
        assert!(matches(&found, "acme") && matches(&found, "quote") && matches(&found, "A@B"));
        assert!(!matches(&found, "beta"));
    }
}

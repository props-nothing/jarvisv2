//! Contacts: the people and companies JARVIS is working with, and where each stands (`ADR-0155`).
//!
//! A prospecting project that keeps its state in a spreadsheet loses it whenever the file is rewritten, and cannot answer "who have we
//! already contacted?". This is that state, as rows. Saving the same lead twice updates it (matched by address, else by company and
//! person), a status such as `do_not_contact` is a fact a later run can read, and nothing here is ever sent anywhere.

use jarvis_core::{ContactId, UtcTimestamp};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// The most contacts one workspace may hold.
pub const MAX_CONTACTS: i64 = 20_000;

/// The most a search returns.
pub const MAX_SEARCH_RESULTS: u32 = 200;

/// Where a contact stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactStatus {
    /// Known, not yet approached.
    New,
    /// Approached; no answer yet.
    Contacted,
    /// They answered.
    Replied,
    /// A meeting or call is booked or held.
    Meeting,
    /// A customer.
    Won,
    /// Not interested, or not a fit.
    Lost,
    /// Must not be approached again.
    DoNotContact,
}

impl ContactStatus {
    /// Every status, in the order a lead moves through them.
    pub const ALL: [Self; 7] = [
        Self::New,
        Self::Contacted,
        Self::Replied,
        Self::Meeting,
        Self::Won,
        Self::Lost,
        Self::DoNotContact,
    ];

    /// The stored and displayed name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Contacted => "contacted",
            Self::Replied => "replied",
            Self::Meeting => "meeting",
            Self::Won => "won",
            Self::Lost => "lost",
            Self::DoNotContact => "do_not_contact",
        }
    }

    /// Reads a stored or typed name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.as_str() == text)
    }
}

/// One stored contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredContact {
    /// The identifier.
    pub id: String,
    /// The project it was found for, if any.
    pub project_id: Option<String>,
    /// The company (required).
    pub company: String,
    /// The person, or empty.
    pub person: String,
    /// Their role, or empty.
    pub role: String,
    /// Their address, or empty.
    pub email: String,
    /// Their phone number, or empty.
    pub phone: String,
    /// Where they stand.
    pub status: ContactStatus,
    /// Free notes.
    pub notes: String,
    /// Where the lead came from.
    pub source: String,
    /// When it was first saved.
    pub created_at: String,
    /// When it last changed.
    pub updated_at: String,
}

/// What to save. An absent field is left as it is on an existing contact and empty on a new one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContactInput {
    /// The company; required for a new contact.
    pub company: Option<String>,
    /// The person.
    pub person: Option<String>,
    /// Their role.
    pub role: Option<String>,
    /// Their address.
    pub email: Option<String>,
    /// Their phone number.
    pub phone: Option<String>,
    /// Where they stand.
    pub status: Option<ContactStatus>,
    /// Free notes (they replace the old ones).
    pub notes: Option<String>,
    /// Where the lead came from.
    pub source: Option<String>,
}

macro_rules! columns {
    () => {
        "id, project_id, company, person, role, email, phone, status, notes, source, created_at, updated_at"
    };
}

fn invalid(field: &'static str) -> DatabaseError {
    DatabaseError::InvalidContact { field }
}

fn sqlite(operation: &'static str) -> impl FnOnce(sqlx::Error) -> DatabaseError {
    move |source| DatabaseError::Sqlite { operation, source }
}

/// Trims a field and checks its length and characters. Notes may span lines; nothing else may.
fn clean(
    value: Option<&String>,
    field: &'static str,
    max: usize,
    multiline: bool,
) -> Result<Option<String>, DatabaseError> {
    let Some(value) = value else { return Ok(None) };
    let value = value.trim();
    let bad_char = |character: char| {
        character.is_control() && !(multiline && (character == '\n' || character == '\t'))
    };
    if value.chars().count() > max || value.chars().any(bad_char) {
        return Err(invalid(field));
    }
    Ok(Some(value.to_owned()))
}

fn check(input: &ContactInput) -> Result<ContactInput, DatabaseError> {
    let email = clean(input.email.as_ref(), "email", 254, false)?;
    if email.as_deref().is_some_and(|email| {
        !email.is_empty() && (!email.contains('@') || email.contains(char::is_whitespace))
    }) {
        return Err(invalid("email"));
    }
    Ok(ContactInput {
        company: clean(input.company.as_ref(), "company", 200, false)?,
        person: clean(input.person.as_ref(), "person", 200, false)?,
        role: clean(input.role.as_ref(), "role", 200, false)?,
        email,
        phone: clean(input.phone.as_ref(), "phone", 40, false)?,
        status: input.status,
        notes: clean(input.notes.as_ref(), "notes", 2000, true)?,
        source: clean(input.source.as_ref(), "source", 300, false)?,
    })
}

fn decode(row: &sqlx::sqlite::SqliteRow) -> Result<StoredContact, DatabaseError> {
    let text = |name: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(name)
            .map_err(sqlite("decode a contact"))
    };
    Ok(StoredContact {
        id: text("id")?,
        project_id: row
            .try_get::<Option<String>, _>("project_id")
            .map_err(sqlite("decode a contact"))?,
        company: text("company")?,
        person: text("person")?,
        role: text("role")?,
        email: text("email")?,
        phone: text("phone")?,
        status: ContactStatus::parse(&text("status")?).ok_or(
            DatabaseError::StoredScheduleInvalid {
                field: "contact status",
            },
        )?,
        notes: text("notes")?,
        source: text("source")?,
        created_at: text("created_at")?,
        updated_at: text("updated_at")?,
    })
}

/// Finds the contact an input names: by address, else by company and person when the stored one has no address.
async fn find_existing(
    database: &SqliteDatabase,
    workspace_id: &str,
    input: &ContactInput,
) -> Result<Option<StoredContact>, DatabaseError> {
    let by_email = input.email.as_deref().filter(|email| !email.is_empty());
    let row = if let Some(email) = by_email {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM contacts WHERE workspace_id = ?1 AND lower(email) = lower(?2)"
        ))
        .bind(workspace_id)
        .bind(email)
        .fetch_optional(database.pool())
        .await
        .map_err(sqlite("find a contact by address"))?
    } else {
        None
    };
    let row = match (row, input.company.as_deref()) {
        (Some(row), _) => Some(row),
        (None, Some(company)) if !company.is_empty() => sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM contacts WHERE workspace_id = ?1 AND lower(company) = lower(?2) \
              AND lower(person) = lower(?3) AND email = ''"
        ))
        .bind(workspace_id)
        .bind(company)
        .bind(input.person.as_deref().unwrap_or(""))
        .fetch_optional(database.pool())
        .await
        .map_err(sqlite("find a contact by company"))?,
        (None, _) => None,
    };
    row.as_ref().map(decode).transpose()
}

/// Saves a contact: updates the one the input names, or creates a new one. Returns it and whether it was created.
///
/// # Errors
///
/// `owner` says whether the owner is asking. Anyone else (a model) may not move a contact out of `do_not_contact`: that is the owner's
/// word, and a model that was told to find leads must not be able to undo it.
///
/// # Errors
///
/// Returns [`DatabaseError::InvalidContact`] for a bad field, a new contact with no company, or a status change the caller may not
/// make, [`DatabaseError::ContactLimit`] when the workspace is full, and [`DatabaseError::Sqlite`] when the write fails.
pub async fn save_contact(
    database: &SqliteDatabase,
    workspace_id: &str,
    project_id: Option<&str>,
    input: &ContactInput,
    owner: bool,
    now: UtcTimestamp,
) -> Result<(StoredContact, bool), DatabaseError> {
    let input = check(input)?;
    let stamp = now.to_string();
    if let Some(existing) = find_existing(database, workspace_id, &input).await? {
        if !owner
            && existing.status == ContactStatus::DoNotContact
            && input
                .status
                .is_some_and(|status| status != ContactStatus::DoNotContact)
        {
            return Err(invalid("status"));
        }
        let pick = |new: &Option<String>, old: &str| new.clone().unwrap_or_else(|| old.to_owned());
        let status = input.status.unwrap_or(existing.status);
        let row = sqlx::query(concat!(
            "UPDATE contacts SET company = ?2, person = ?3, role = ?4, email = ?5, phone = ?6, status = ?7, notes = ?8, \
             source = ?9, project_id = COALESCE(project_id, ?10), updated_at = ?11 WHERE id = ?1 RETURNING ",
            columns!()
        ))
        .bind(&existing.id)
        .bind(pick(&input.company, &existing.company))
        .bind(pick(&input.person, &existing.person))
        .bind(pick(&input.role, &existing.role))
        .bind(pick(&input.email, &existing.email))
        .bind(pick(&input.phone, &existing.phone))
        .bind(status.as_str())
        .bind(pick(&input.notes, &existing.notes))
        .bind(pick(&input.source, &existing.source))
        .bind(project_id)
        .bind(&stamp)
        .fetch_one(database.pool())
        .await
        .map_err(sqlite("update a contact"))?;
        return Ok((decode(&row)?, false));
    }
    let company = input
        .company
        .as_deref()
        .filter(|company| !company.is_empty())
        .ok_or_else(|| invalid("company"))?;
    let held: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contacts WHERE workspace_id = ?1")
        .bind(workspace_id)
        .fetch_one(database.pool())
        .await
        .map_err(sqlite("count contacts"))?;
    if held >= MAX_CONTACTS {
        return Err(DatabaseError::ContactLimit);
    }
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let row = sqlx::query(concat!(
        "INSERT INTO contacts (id, workspace_id, project_id, company, person, role, email, phone, status, notes, source, \
         created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12) RETURNING ",
        columns!()
    ))
    .bind(ContactId::new().to_string())
    .bind(workspace_id)
    .bind(project_id)
    .bind(company)
    .bind(text(&input.person))
    .bind(text(&input.role))
    .bind(text(&input.email))
    .bind(text(&input.phone))
    .bind(input.status.unwrap_or(ContactStatus::New).as_str())
    .bind(text(&input.notes))
    .bind(text(&input.source))
    .bind(&stamp)
    .fetch_one(database.pool())
    .await
    .map_err(sqlite("create a contact"))?;
    Ok((decode(&row)?, true))
}

/// Escapes `%`, `_` and the escape character itself, so a search text is matched literally.
fn like_pattern(text: &str) -> String {
    let mut pattern = String::with_capacity(text.len() + 2);
    pattern.push('%');
    for character in text.chars() {
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

/// Contacts whose company, person, address or notes contain `query` (any, when absent) and that have `status` (any, when absent),
/// by company, at most `limit` (never more than [`MAX_SEARCH_RESULTS`]).
///
/// # Errors
///
/// Returns [`DatabaseError`] when the read fails.
pub async fn search_contacts(
    database: &SqliteDatabase,
    workspace_id: &str,
    query: Option<&str>,
    status: Option<ContactStatus>,
    limit: u32,
) -> Result<Vec<StoredContact>, DatabaseError> {
    let pattern = query
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| like_pattern(&text.to_lowercase()));
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM contacts WHERE workspace_id = ?1 AND (?2 IS NULL OR status = ?2) AND (?3 IS NULL OR \
         lower(company) LIKE ?3 ESCAPE '\\' OR lower(person) LIKE ?3 ESCAPE '\\' OR lower(email) LIKE ?3 ESCAPE '\\' \
         OR lower(notes) LIKE ?3 ESCAPE '\\') \
         ORDER BY company COLLATE NOCASE, person COLLATE NOCASE, id LIMIT ?4"
    ))
    .bind(workspace_id)
    .bind(status.map(ContactStatus::as_str))
    .bind(pattern)
    .bind(i64::from(limit.clamp(1, MAX_SEARCH_RESULTS)))
    .fetch_all(database.pool())
    .await
    .map_err(sqlite("search contacts"))?;
    rows.iter().map(decode).collect()
}

/// One page of contacts with `status` (any, when absent), by company: `limit` of them after skipping `offset`. For the owner's own
/// views and exports, so it is not capped at [`MAX_SEARCH_RESULTS`].
///
/// # Errors
///
/// Returns [`DatabaseError`] when the read fails.
pub async fn list_contacts(
    database: &SqliteDatabase,
    workspace_id: &str,
    status: Option<ContactStatus>,
    limit: u32,
    offset: u32,
) -> Result<Vec<StoredContact>, DatabaseError> {
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM contacts WHERE workspace_id = ?1 AND (?2 IS NULL OR status = ?2) \
         ORDER BY company COLLATE NOCASE, person COLLATE NOCASE, id LIMIT ?3 OFFSET ?4"
    ))
    .bind(workspace_id)
    .bind(status.map(ContactStatus::as_str))
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(database.pool())
    .await
    .map_err(sqlite("list contacts"))?;
    rows.iter().map(decode).collect()
}

/// How many contacts stand at each status, for every status (zero included).
///
/// # Errors
///
/// Returns [`DatabaseError`] when the read fails.
pub async fn contact_stats(
    database: &SqliteDatabase,
    workspace_id: &str,
) -> Result<Vec<(ContactStatus, u32)>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT status, COUNT(*) AS held FROM contacts WHERE workspace_id = ?1 GROUP BY status",
    )
    .bind(workspace_id)
    .fetch_all(database.pool())
    .await
    .map_err(sqlite("count contacts by status"))?;
    let mut counts: Vec<(ContactStatus, u32)> = ContactStatus::ALL
        .into_iter()
        .map(|status| (status, 0))
        .collect();
    for row in &rows {
        let name: String = row
            .try_get("status")
            .map_err(sqlite("decode a contact count"))?;
        let held: i64 = row
            .try_get("held")
            .map_err(sqlite("decode a contact count"))?;
        if let Some(status) = ContactStatus::parse(&name)
            && let Some(entry) = counts.iter_mut().find(|(known, _)| *known == status)
        {
            entry.1 = u32::try_from(held).unwrap_or(u32::MAX);
        }
    }
    Ok(counts)
}

/// Deletes a contact by identifier, returning whether one was removed.
///
/// # Errors
///
/// Returns [`DatabaseError`] when the write fails.
pub async fn delete_contact(
    database: &SqliteDatabase,
    workspace_id: &str,
    id: &str,
) -> Result<bool, DatabaseError> {
    let done = sqlx::query("DELETE FROM contacts WHERE workspace_id = ?1 AND id = ?2")
        .bind(workspace_id)
        .bind(id)
        .execute(database.pool())
        .await
        .map_err(sqlite("delete a contact"))?;
    Ok(done.rows_affected() > 0)
}

#[cfg(test)]
#[path = "contact_tests.rs"]
mod tests;

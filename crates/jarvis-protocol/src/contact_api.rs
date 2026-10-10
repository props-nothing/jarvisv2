//! Request and reply bodies for the contact list (`ADR-0155`).

use serde::{Deserialize, Serialize};

/// One contact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContactReply {
    /// The identifier, for `jarvis contacts remove`.
    pub contact_id: String,
    /// The company.
    pub company: String,
    /// The person, or empty.
    pub person: String,
    /// Their role, or empty.
    pub role: String,
    /// Their address, or empty.
    pub email: String,
    /// Their phone number, or empty.
    pub phone: String,
    /// `new`, `contacted`, `replied`, `meeting`, `won`, `lost` or `do_not_contact`.
    pub status: String,
    /// Free notes.
    pub notes: String,
    /// Where the lead came from.
    pub source: String,
    /// When it last changed.
    pub updated_at: String,
}

/// How many contacts stand at one status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContactCount {
    /// The status.
    pub status: String,
    /// How many.
    pub count: u32,
}

/// Reply of `GET /api/v1/contacts`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContactListReply {
    /// The contacts of this page.
    pub contacts: Vec<ContactReply>,
    /// Every contact of the workspace, by status.
    pub counts: Vec<ContactCount>,
}

/// Request body for `POST /api/v1/contacts`: saves a contact the way the owner would, which includes lifting a do-not-contact.
/// An absent field is left as it is.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SaveContactRequest {
    /// The company; required for a new contact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    /// The person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// Their role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Their address; matches an existing contact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Their phone number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    /// Where they stand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Free notes (they replace the old ones).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Where the lead came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

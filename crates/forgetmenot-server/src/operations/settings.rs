//! The store's behaviour settings, read and written like any other document.
//!
//! A change is a commit on `config.yml`, so it is reviewable, revertible and can
//! be made on a branch with everything else that belongs to the same change. The
//! version a write is made from is the file's blob id, exactly as for a memory,
//! so two people changing settings at once cannot silently overwrite each other.

use git2::Oid;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::service::{WriteError, is_valid_message_title};
use crate::store::branch::BranchName;
use crate::store::catalog::Catalog;
use crate::store::settings::{self, KEYS, SETTINGS_PATH, SettingKey, SettingProblem};

use super::{
    CurrentDocument, OperationError, UNREADABLE_BASE_VERSION, ValidationMessage, WriteOutcome,
    commit_to, target_catalog, write_error, write_outcome,
};

/// The settings as the API and the settings tools report them.
#[derive(Clone, Debug, Serialize)]
pub struct SettingsDoc {
    /// Every key with the value in force, which is the file's over the defaults.
    pub settings: serde_json::Map<String, serde_json::Value>,
    /// The version of `config.yml`, null when the store has no such file, which
    /// is what a write of the first setting sends back as `base_version`.
    pub version: Option<String>,
    /// What each key means and takes, so that a caller can offer the keys
    /// without knowing them.
    pub schema: Vec<SettingSchemaRow>,
}

/// One key of the settings, as the schema describes it.
#[derive(Clone, Debug, Serialize)]
pub struct SettingSchemaRow {
    pub key: String,
    #[serde(rename = "type")]
    pub value_type: String,
    /// The value in force when no file sets this key.
    pub default: serde_json::Value,
    pub description: String,
}

/// A change to one setting.
#[derive(Clone, Debug, Deserialize)]
pub struct SettingsWriteRequest {
    /// The new value, of the type the schema gives for the key.
    pub value: serde_json::Value,
    /// The version the caller read; the current one when absent.
    #[serde(default)]
    pub base_version: Option<String>,
    pub author: String,
    /// The commit's title line.
    pub message: String,
}

/// The settings of one revision of the store.
pub fn settings_doc(catalog: &Catalog) -> SettingsDoc {
    let defaults = settings::Settings::default();
    SettingsDoc {
        settings: catalog.settings().as_json(),
        version: catalog.settings_version().map(|oid| oid.to_string()),
        schema: KEYS
            .into_iter()
            .map(|key| SettingSchemaRow {
                key: key.as_str().to_string(),
                value_type: key.value_type().as_str().to_string(),
                default: defaults.value(key),
                description: key.description().to_string(),
            })
            .collect(),
    }
}

/// The settings in force, on `main` or on one branch.
pub async fn settings_get(
    state: &AppState,
    branch: Option<&BranchName>,
) -> Result<SettingsDoc, OperationError> {
    let catalog = target_catalog(state, branch).await?;
    Ok(settings_doc(&catalog))
}

/// Set one setting, as one commit that writes the whole file.
///
/// The file is written from the entries it already has with this key changed, so
/// a setting somebody else wrote survives a change to another one, and the file
/// is created when the store has never had one. The result is checked as a whole
/// before it is committed: a store whose settings this server cannot read is
/// refused at the write rather than discovered at the next event.
pub async fn settings_set(
    state: &AppState,
    key: &str,
    request: &SettingsWriteRequest,
    branch: Option<&BranchName>,
) -> Result<WriteOutcome, OperationError> {
    let Some(key) = SettingKey::parse(key) else {
        return Err(OperationError::invalid(
            SETTINGS_PATH,
            &format!("`{key}` is not a setting; the settings are {}", key_list()),
        ));
    };
    let catalog = target_catalog(state, branch).await?;
    let current = catalog.settings_version();
    let expected = match request.base_version.as_deref() {
        None => current,
        Some(text) => match Oid::from_str(text) {
            Ok(version) if Some(version) == current => current,
            Ok(_) => return Err(settings_conflict(&catalog)),
            Err(_) => {
                return Err(OperationError::invalid(
                    SETTINGS_PATH,
                    UNREADABLE_BASE_VERSION,
                ));
            }
        },
    };

    let mut entries = catalog.settings_entries().clone();
    entries.insert(
        yaml_serde::Value::String(key.as_str().to_string()),
        settings::yaml_of_json(&request.value),
    );
    let text = settings::render(&entries)
        .map_err(|error| OperationError::invalid(SETTINGS_PATH, &error.to_string()))?;

    let mut errors = problems_of(&text);
    if !is_valid_message_title(&request.message) {
        errors.push(ValidationMessage {
            path: SETTINGS_PATH.to_string(),
            message: WriteError::BadMessage.to_string(),
        });
    }
    if !errors.is_empty() {
        return Err(OperationError::Invalid { errors });
    }

    let outcome = commit_to(
        state,
        branch,
        &request.author,
        &request.message,
        &format!("setting: {key}\nauthor: {}", request.author),
        vec![(SETTINGS_PATH.to_string(), Some(text.into_bytes()))],
        vec![(SETTINGS_PATH.to_string(), expected)],
    )
    .await;

    match outcome {
        Ok(outcome) => Ok(write_outcome(&outcome, SETTINGS_PATH)),
        Err(WriteError::VersionMismatch { .. }) => {
            // The file moved between the version check above and the commit, so
            // the answer is the settings as the store has them now.
            let catalog = target_catalog(state, branch).await?;
            Err(settings_conflict(&catalog))
        }
        Err(error) => Err(write_error(SETTINGS_PATH, error)),
    }
}

/// Everything wrong with the file a write would leave behind, one message per
/// key. A file this server cannot read at all is one message about the file.
fn problems_of(text: &str) -> Vec<ValidationMessage> {
    let message = |message: String| ValidationMessage {
        path: SETTINGS_PATH.to_string(),
        message,
    };
    match settings::parse(text.as_bytes()) {
        Err(error) => vec![message(error.to_string())],
        Ok((_, problems)) => problems
            .iter()
            .map(|problem| match problem {
                SettingProblem::UnknownKey { key } => message(format!(
                    "`{key}` is not a setting; the settings are {}",
                    key_list()
                )),
                SettingProblem::WrongType { key, mismatch } => message(format!(
                    "`{key}` takes {}, not {}",
                    mismatch.expected, mismatch.found
                )),
            })
            .collect(),
    }
}

/// The settings a caller may name, for the refusal that says what was expected.
fn key_list() -> String {
    KEYS.into_iter()
        .map(|key| key.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The conflict answer: the settings as the store has them, so that the caller
/// can write again from the version that is there.
fn settings_conflict(catalog: &Catalog) -> OperationError {
    OperationError::Conflict {
        what: SETTINGS_PATH.to_string(),
        current: CurrentDocument::Settings(Box::new(settings_doc(catalog))),
    }
}

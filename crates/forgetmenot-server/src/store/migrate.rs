//! Bringing a store written by an earlier version of this server up to the
//! shape this one writes.
//!
//! One step is here. Earlier versions marked a memory with `metadata.kind`,
//! one of two words, where this one writes `metadata.pinned`; and two settings
//! had other names ([`settings::LEGACY_NAMES`]). The reader still takes both
//! earlier forms, so a store works unmigrated; this step rewrites the files
//! that carry them, as one commit, the first time the server opens the store.
//! A store that carries neither is left as it is, so the step runs once.
//!
//! This module, [`memory::LEGACY_KIND_KEY`] and [`settings::LEGACY_NAMES`] are
//! for one release: once every store has been opened by it, they go.

use git2::Oid;

use super::MemoryId;
use super::git::{FileWrites, GitError, GitRepo};
use super::memory::MemoryDocument;
use super::settings::{self, SETTINGS_PATH};

/// The author the migration's commit carries.
const AUTHOR: &str = "forgetmenot";

/// The title of the migration's commit.
const COMMIT_TITLE: &str = "Write memories with the pinned flag, settings by their new names";

const COMMIT_BODY: &str = "Written by the server when it opened a store from an earlier version. \
A memory's `kind` key becomes `pinned: true` or is dropped, and the settings \
`interrupt_on_critical` and `deliver_knowledge_index` take the names \
`pinned_interrupt_execution` and `deliver_descriptions`. Nothing a session is \
delivered changes.";

/// Rewrite every file at the head of the store that is in an earlier shape, as
/// one commit, and answer with that commit; `None` when no file is.
///
/// A file this server cannot parse is left alone: it is reported by the load
/// that follows, as it was before.
pub fn to_current(repository: &GitRepo) -> Result<Option<Oid>, GitError> {
    let head = repository.head_oid()?;
    let mut files: FileWrites = Vec::new();
    for entry in repository.read_tree(head)? {
        let rewritten = if entry.path == SETTINGS_PATH {
            settings_by_new_names(&entry.bytes)
        } else {
            memory_with_pinned_flag(&entry.path, &entry.bytes)
        };
        if let Some(text) = rewritten {
            files.push((entry.path, Some(text.into_bytes())));
        }
    }
    if files.is_empty() {
        return Ok(None);
    }
    let outcome = repository.commit_files(AUTHOR, COMMIT_TITLE, COMMIT_BODY, files, Some(head))?;
    Ok(Some(outcome.commit_oid))
}

/// The text of a memory file that carries the earlier key, written the way this
/// server writes it; `None` for a file that is not a memory, does not parse or
/// does not carry the key.
fn memory_with_pinned_flag(path: &str, bytes: &[u8]) -> Option<String> {
    let id = MemoryId::from_repository_path(path)?;
    let document = MemoryDocument::parse(id, bytes).ok()?;
    if !document.has_legacy_kind() {
        return None;
    }
    document.render().ok()
}

/// The text of a settings file that names a setting by an earlier name, with
/// each such entry under its new name and in its old place; `None` for a file
/// that does not parse or names none.
///
/// An entry whose new name the file already carries is dropped: the new name
/// is the one the reader takes.
fn settings_by_new_names(bytes: &[u8]) -> Option<String> {
    let (file, _) = settings::parse(bytes).ok()?;
    let renamed_to = |name: &yaml_serde::Value| {
        let name = name.as_str()?;
        settings::LEGACY_NAMES
            .into_iter()
            .find(|(legacy, _)| *legacy == name)
            .map(|(_, key)| key.as_str())
    };
    if !file.entries.keys().any(|name| renamed_to(name).is_some()) {
        return None;
    }
    let mut entries = yaml_serde::Mapping::new();
    for (name, value) in &file.entries {
        match renamed_to(name) {
            None => {
                entries.insert(name.clone(), value.clone());
            }
            Some(new_name) => {
                let new_name = yaml_serde::Value::String(new_name.to_string());
                if !file.entries.contains_key(&new_name) {
                    entries.insert(new_name, value.clone());
                }
            }
        }
    }
    settings::render(&entries).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestStore;

    /// A memory file as an earlier version wrote it, with `kind` as given.
    fn earlier_memory(name: &str, kind: &str) -> (String, Option<Vec<u8>>) {
        let text = format!(
            "---\nname: {name}\ndescription: about {name}\n\
             metadata:\n  kind: {kind}\n  scope: global\n---\nThe body of {name}.\n"
        );
        (format!("memories/{name}.md"), Some(text.into_bytes()))
    }

    fn text_at_head(store: &TestStore, path: &str) -> String {
        let repository = store.repository();
        let head = repository.head_oid().expect("the store has a head");
        let bytes = repository
            .blob_at(head, path)
            .expect("the tree is readable")
            .expect("the file is in the store");
        String::from_utf8(bytes).expect("the file is text")
    }

    /// Detects a memory that changes how it is delivered in the migration: the
    /// one that was shown in full must stay pinned, the other must not become
    /// pinned, and neither file may keep the earlier key. Detects too a
    /// migration that commits again on a store already migrated, which would
    /// add a commit at every start.
    #[test]
    fn earlier_kinds_become_the_pinned_flag_once() {
        let store = TestStore::with(vec![
            earlier_memory("shown-in-full", "critical"),
            earlier_memory("shown-as-a-line", "knowledge"),
        ]);

        let commit = to_current(store.repository()).expect("the migration runs");
        assert!(
            commit.is_some(),
            "a store in the earlier shape is rewritten"
        );

        let full = text_at_head(&store, "memories/shown-in-full.md");
        assert!(
            full.contains("  pinned: true\n") && !full.contains("kind:"),
            "the memory shown in full is pinned and carries no earlier key, got {full:?}"
        );
        let line = text_at_head(&store, "memories/shown-as-a-line.md");
        assert!(
            !line.contains("pinned") && !line.contains("kind:"),
            "the other memory carries neither key, got {line:?}"
        );
        assert!(
            full.ends_with("The body of shown-in-full.\n"),
            "the body is unchanged, got {full:?}"
        );

        let catalog = store.catalog();
        let pinned = |id: &str| {
            catalog
                .memory(&MemoryId::new(id))
                .expect("the memory is in the catalog")
                .pinned()
        };
        assert!(pinned("shown-in-full") && !pinned("shown-as-a-line"));

        let again = to_current(store.repository()).expect("the migration runs again");
        assert_eq!(again, None, "a migrated store is left as it is");
    }

    /// Detects a setting lost or reset by the rename: the value the store set
    /// under the earlier name must be in force under the new one, and a key
    /// that was not renamed must keep its place and value.
    #[test]
    fn settings_keep_their_values_under_their_new_names() {
        let store = TestStore::with(vec![(
            SETTINGS_PATH.to_string(),
            Some(
                b"interrupt_on_critical: false\nreminder_tokens: 5000\ndeliver_knowledge_index: false\n"
                    .to_vec(),
            ),
        )]);

        to_current(store.repository())
            .expect("the migration runs")
            .expect("a settings file with earlier names is rewritten");

        assert_eq!(
            text_at_head(&store, SETTINGS_PATH),
            "pinned_interrupt_execution: false\nreminder_tokens: 5000\ndeliver_descriptions: false\n"
        );
        let catalog = store.catalog();
        let settings = catalog.settings();
        assert!(!settings.pinned_interrupt_execution && !settings.deliver_descriptions);
        assert_eq!(settings.reminder_tokens, Some(5000));
    }
}

//! The store fixture the unit tests of several modules build their catalogs on.
//!
//! A catalog is read from a git repository, so a test that needs one commits
//! files into a temporary store and loads it. That is the same store the server
//! reads, which is what makes a catalog built here the catalog the decision
//! under test is made against; only the commit is shared, so each test still
//! says which files it needs and nothing else.

use tempfile::TempDir;

use crate::store::catalog::Catalog;
use crate::store::git::GitRepo;

/// A store in a temporary directory, which is removed when the store is
/// dropped.
///
/// The directory is held by the store rather than handed back beside the
/// catalog, so that a test cannot read a catalog out of a repository that has
/// already been deleted.
pub(crate) struct TestStore {
    _directory: TempDir,
    repository: GitRepo,
}

impl TestStore {
    /// A store whose first commit writes `files`.
    pub(crate) fn with(files: Vec<(String, Option<Vec<u8>>)>) -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        let repository = GitRepo::open_or_init(directory.path()).expect("the store opens");
        let store = Self {
            _directory: directory,
            repository,
        };
        store.commit(files);
        store
    }

    /// Write `files` as one more commit, which is how a test asks what an edit
    /// to the store changes.
    pub(crate) fn commit(&self, files: Vec<(String, Option<Vec<u8>>)>) {
        self.repository
            .commit_files("test", "write the store", "", files, None)
            .expect("the store's files are committed");
    }

    /// The catalog of the store as it stands now.
    pub(crate) fn catalog(&self) -> Catalog {
        Catalog::load(&self.repository).expect("the catalog is built")
    }
}

/// The catalog of a store whose only commit writes `files`.
///
/// The store is returned with it because dropping the store deletes the
/// repository the catalog was read from.
pub(crate) fn catalog_of(files: Vec<(String, Option<Vec<u8>>)>) -> (TestStore, Catalog) {
    let store = TestStore::with(files);
    let catalog = store.catalog();
    (store, catalog)
}

/// One memory file: `kind` is `critical` or `knowledge`, and `scope` is the
/// scope id it belongs to.
pub(crate) fn memory_file(
    name: &str,
    kind: &str,
    scope: &str,
    description: &str,
    body: &str,
) -> (String, Option<Vec<u8>>) {
    let text = format!(
        "---\nname: {name}\ndescription: {description}\n\
         metadata:\n  kind: {kind}\n  scope: {scope}\n---\n{body}"
    );
    (format!("memories/{name}.md"), Some(text.into_bytes()))
}

/// One scope file: `id` and whatever else the scope says, as the YAML lines
/// that follow it.
pub(crate) fn scope_file(id: &str, rest: &str) -> (String, Option<Vec<u8>>) {
    (
        format!("scopes/{id}.yaml"),
        Some(format!("id: {id}\n{rest}").into_bytes()),
    )
}

/// The store's settings file, with the keys the test names.
pub(crate) fn settings_file(yaml: &str) -> (String, Option<Vec<u8>>) {
    (
        crate::store::settings::SETTINGS_PATH.to_string(),
        Some(yaml.as_bytes().to_vec()),
    )
}

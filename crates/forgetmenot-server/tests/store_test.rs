//! Tests of the store: its git operations, the catalog it parses and the
//! validation rules `forgetmenot check` reports.

mod common;

use std::collections::BTreeSet;
use std::process::{Command, Output};

use forgetmenot_server::store::git::GitError;
use forgetmenot_server::store::memory::MemoryDocument;
use forgetmenot_server::store::validate::{
    Candidate, ValidationError, ValidationReport, ValidationWarning, WriteMode, validate,
    validate_write,
};
use forgetmenot_server::store::{MemoryId, ScopeId};

use common::{TempStore, binary_path, example_store, example_store_path, store_with};

/// A memory file from its frontmatter lines and its body.
fn memory_file(frontmatter: &str, body: &str) -> Vec<u8> {
    format!("---\n{frontmatter}---\n{body}").into_bytes()
}

/// A memory file with the two required keys and the given `metadata` lines.
fn memory_with_metadata(name: &str, metadata_lines: &[&str], body: &str) -> Vec<u8> {
    let mut frontmatter = format!("name: {name}\ndescription: the index entry for {name}\n");
    if !metadata_lines.is_empty() {
        frontmatter.push_str("metadata:\n");
        for line in metadata_lines {
            frontmatter.push_str("  ");
            frontmatter.push_str(line);
            frontmatter.push('\n');
        }
    }
    memory_file(&frontmatter, body)
}

/// A valid critical memory in the global scope.
fn global_memory(name: &str, body: &str) -> Vec<u8> {
    memory_with_metadata(
        name,
        &["kind: critical", "scopes: [global]", "source: user"],
        body,
    )
}

/// A valid knowledge memory in one session's silo.
fn session_memory(name: &str, session_key: &str, body: &str) -> Vec<u8> {
    memory_with_metadata(
        name,
        &[
            "kind: knowledge",
            &format!("scopes: ['session:{session_key}']"),
            "source: assistant",
        ],
        body,
    )
}

/// Fails unless `report` holds an error that `predicate` accepts.
fn assert_reports(
    report: &ValidationReport,
    described: &str,
    predicate: impl Fn(&ValidationError) -> bool,
) {
    assert!(
        report.errors().iter().any(predicate),
        "no {described} was reported; the report holds: {:#?}",
        report.errors()
    );
}

/// Fails unless `report` holds a warning that `predicate` accepts.
fn assert_warns(
    report: &ValidationReport,
    described: &str,
    predicate: impl Fn(&ValidationWarning) -> bool,
) {
    assert!(
        report.warnings().iter().any(predicate),
        "no {described} was warned about; the report holds: {:#?}",
        report.warnings()
    );
}

// ---------------------------------------------------------------- git commits

const MEMORY_WITH_UNKNOWN_KEYS: &str = concat!(
    "---\n",
    "name: bench-power\n",
    "description: Cut bench power at the wall before rewiring\n",
    "review-by: 2026-12-01\n",
    "metadata:\n",
    "  kind: critical\n",
    "  scopes:\n",
    "  - global\n",
    "  source: user\n",
    "  type: feedback\n",
    "  strength: hard\n",
    "---\n",
    "# Cut bench power before rewiring\n",
    "\n",
    "Switch the supply off at the wall.\n",
    "\n",
    "```text\n",
    "   indented   spacing   kept\n",
    "```\n",
    "\n",
    "a trailing line with two spaces  \n",
);

/// Detects lossy re-serialization, which would rewrite parts of a file its
/// author never touched: a dropped `metadata` block, or a body whose blank
/// lines, indentation or trailing spaces were normalised.
#[test]
fn a_frontmatter_round_trip_preserves_unknown_keys_and_the_body_bytes() {
    let store = store_with(&[(
        "memories/bench-power.md",
        MEMORY_WITH_UNKNOWN_KEYS.as_bytes(),
    )]);
    let catalog = store.catalog();
    let id = MemoryId::new("bench-power");
    let document = &catalog
        .memory(&id)
        .expect("the memory is in the catalog")
        .document;

    let (_, original_body) = MEMORY_WITH_UNKNOWN_KEYS
        .split_once("\n---\n")
        .expect("the fixture has frontmatter");
    assert_eq!(
        document.body, original_body,
        "the body changed when it was read"
    );

    let rendered = document.render().expect("the memory renders");
    let reparsed =
        MemoryDocument::parse(id, rendered.as_bytes()).expect("the rendered memory parses");
    assert_eq!(
        reparsed.body, original_body,
        "the body changed when it was written"
    );
    assert_eq!(
        reparsed.frontmatter, document.frontmatter,
        "the frontmatter changed when it was written"
    );
    assert!(
        reparsed.frontmatter.extra.contains_key("review-by"),
        "the unknown top-level key was dropped: {rendered}"
    );
    assert!(
        reparsed.frontmatter.metadata.extra.contains_key("strength"),
        "the unknown metadata key was dropped: {rendered}"
    );
    assert_eq!(
        rendered, MEMORY_WITH_UNKNOWN_KEYS,
        "the file did not come back byte for byte"
    );
}

/// Detects a lost update: a commit made by hand between the server reading the
/// store and writing it must make the write fail, not overwrite it.
#[test]
fn a_stale_expected_head_is_refused_and_leaves_the_branch_where_it_was() {
    let store = store_with(&[(
        "memories/bench-power.md",
        &global_memory("bench-power", "# One\n\nOne.\n"),
    )]);
    let stale_head = store.repository().head_oid().expect("a head");
    store.commit(
        "a commit made by someone else",
        vec![(
            "memories/reading-list.md".to_string(),
            Some(global_memory("reading-list", "# Two\n\nTwo.\n")),
        )],
    );
    let current_head = store.repository().head_oid().expect("a head");

    let error = store
        .repository()
        .commit_files(
            "test",
            "a write from a stale read",
            "",
            vec![(
                "memories/bench-power.md".to_string(),
                Some(global_memory(
                    "bench-power",
                    "# Overwritten\n\nOverwritten.\n",
                )),
            )],
            Some(stale_head),
        )
        .expect_err("a write from a stale head must be refused");

    match error {
        GitError::HeadMoved { current } => {
            assert_eq!(current, current_head, "HeadMoved named the wrong head")
        }
        other => panic!("expected HeadMoved, got {other}"),
    }
    assert_eq!(
        store.repository().head_oid().expect("a head"),
        current_head,
        "the branch moved even though the write was refused"
    );
}

/// Detects an author or a commit title that never reaches the commit, which
/// would leave the history unable to say who changed a memory or why.
#[test]
fn a_commit_records_the_given_author_and_title() {
    let store = store_with(&[]);
    let head = store.repository().head_oid().expect("a head");
    store
        .repository()
        .commit_files(
            "alpha/session-1",
            "record the bench rule",
            "memory: bench-power",
            vec![(
                "memories/bench-power.md".to_string(),
                Some(global_memory("bench-power", "# One\n\nOne.\n")),
            )],
            Some(head),
        )
        .expect("the commit succeeds");

    let log = store
        .repository()
        .log_for_path("memories/bench-power.md")
        .expect("the log reads");
    assert_eq!(
        log.len(),
        1,
        "the file should have exactly one commit: {log:#?}"
    );
    assert_eq!(
        log[0].author, "alpha/session-1",
        "the author was not recorded"
    );
    assert_eq!(
        log[0].title, "record the bench rule",
        "the title was not recorded"
    );
}

/// Detects a commit that moves the branch without the file actually landing in
/// the new tree, or a reported blob id that is not the one committed.
#[test]
fn a_commit_with_a_matching_expected_head_contains_the_written_file() {
    let store = store_with(&[]);
    let head = store.repository().head_oid().expect("a head");
    let content = global_memory("bench-power", "# One\n\nOne.\n");
    let outcome = store
        .repository()
        .commit_files(
            "test",
            "record the bench rule",
            "",
            vec![("memories/bench-power.md".to_string(), Some(content.clone()))],
            Some(head),
        )
        .expect("the commit succeeds");

    assert_eq!(
        store.repository().head_oid().expect("a head"),
        outcome.commit_oid,
        "the branch does not point at the new commit"
    );
    let tree = store
        .repository()
        .read_tree(outcome.commit_oid)
        .expect("the tree reads");
    let entry = tree
        .iter()
        .find(|entry| entry.path == "memories/bench-power.md")
        .expect("the written file is in the new tree");
    assert_eq!(
        entry.bytes, content,
        "the committed bytes differ from the written ones"
    );
    assert_eq!(
        outcome.blob_oids.get("memories/bench-power.md"),
        Some(&entry.blob_oid),
        "the reported blob id is not the committed one"
    );
}

/// Detects a commit that overwrites an edit someone made in the working tree
/// and never committed, which would lose work recorded nowhere else.
#[test]
fn an_uncommitted_working_tree_change_blocks_a_commit_to_the_same_path() {
    let store = store_with(&[(
        "memories/bench-power.md",
        &global_memory("bench-power", "# One\n\nOne.\n"),
    )]);
    let head = store.repository().head_oid().expect("a head");
    let on_disk = store.path().join("memories/bench-power.md");
    std::fs::write(&on_disk, "edited by hand\n").expect("the working tree is writable");

    let error = store
        .repository()
        .commit_files(
            "test",
            "a write over a hand edit",
            "",
            vec![(
                "memories/bench-power.md".to_string(),
                Some(global_memory("bench-power", "# Server\n\nServer.\n")),
            )],
            Some(head),
        )
        .expect_err("a write over a hand edit must be refused");

    match error {
        GitError::DirtyWorkingTree { path } => {
            assert_eq!(path, "memories/bench-power.md", "the wrong path was named")
        }
        other => panic!("expected DirtyWorkingTree, got {other}"),
    }
    assert_eq!(
        std::fs::read_to_string(&on_disk).expect("the file is readable"),
        "edited by hand\n",
        "the hand edit was overwritten"
    );
    assert_eq!(
        store.repository().head_oid().expect("a head"),
        head,
        "a commit was made even though the write was refused"
    );
}

// ---------------------------------------------------------------- git history

const FIRST_VERSION: &str = "# Cut bench power\n\nSwitch the supply off at the wall.\n";
const SECOND_VERSION: &str = "# Cut bench power\n\nSwitch the supply off at the wall.\nConfirm with the meter that the rail reads zero.\n";

/// A store in which the bench memory was created, an unrelated memory was
/// committed, and then the bench memory was changed.
fn store_with_history() -> TempStore {
    let store = store_with(&[(
        "memories/bench-power.md",
        &global_memory("bench-power", FIRST_VERSION),
    )]);
    store.commit(
        "add an unrelated memory",
        vec![(
            "memories/reading-list.md".to_string(),
            Some(global_memory("reading-list", "# Reading\n\nReading.\n")),
        )],
    );
    store.commit(
        "tighten the bench rule",
        vec![(
            "memories/bench-power.md".to_string(),
            Some(global_memory("bench-power", SECOND_VERSION)),
        )],
    );
    store
}

/// Detects a history filtered wrongly: a log that lists every commit in the
/// store, or one that hides the commit that created the file, would make the
/// history page describe the wrong changes.
#[test]
fn log_for_path_lists_only_the_commits_that_touched_the_path_newest_first() {
    let store = store_with_history();
    let titles: Vec<String> = store
        .repository()
        .log_for_path("memories/bench-power.md")
        .expect("the log reads")
        .into_iter()
        .map(|commit| commit.title)
        .collect();
    assert_eq!(
        titles,
        vec![
            "tighten the bench rule".to_string(),
            "build the fixture store".to_string()
        ],
        "the log is not the two commits that touched the path, newest first"
    );
}

/// Detects a history view that reads the current file instead of the file as it
/// was, which would make every past version look like the present one.
#[test]
fn blob_at_returns_the_content_the_path_had_at_an_older_commit() {
    let store = store_with_history();
    let log = store
        .repository()
        .log_for_path("memories/bench-power.md")
        .expect("the log reads");
    let oldest = log.last().expect("the file has a first commit").oid;
    let bytes = store
        .repository()
        .blob_at(oldest, "memories/bench-power.md")
        .expect("the blob reads")
        .expect("the file exists at its first commit");
    assert_eq!(
        bytes,
        global_memory("bench-power", FIRST_VERSION),
        "the older commit did not return the older content"
    );
}

/// Detects a diff taken against the wrong revision, or one not limited to the
/// path asked for.
#[test]
fn diff_for_path_marks_the_changed_line_as_added() {
    let store = store_with_history();
    let newest = store
        .repository()
        .log_for_path("memories/bench-power.md")
        .expect("the log reads")
        .first()
        .expect("the file has a latest commit")
        .oid;
    let diff = store
        .repository()
        .diff_for_path(newest, "memories/bench-power.md")
        .expect("the diff reads");
    assert!(
        diff.contains("+Confirm with the meter that the rail reads zero."),
        "the added line is not marked as added:\n{diff}"
    );
    assert!(
        !diff.contains("reading-list"),
        "the diff is not limited to the path asked for:\n{diff}"
    );
}

/// Detects a diff of the commit that created a file failing or coming back
/// empty: the history page shows the first commit of every memory, and its
/// parent has no such file to diff against.
#[test]
fn diff_for_path_shows_a_newly_created_file_as_entirely_added() {
    let store = store_with_history();
    let oldest = store
        .repository()
        .log_for_path("memories/bench-power.md")
        .expect("the log reads")
        .last()
        .expect("the file has a first commit")
        .oid;
    let diff = store
        .repository()
        .diff_for_path(oldest, "memories/bench-power.md")
        .expect("the diff reads");
    assert!(
        diff.contains("+# Cut bench power"),
        "the created file's body is not shown as added:\n{diff}"
    );
    assert!(
        diff.contains("+name: bench-power"),
        "the created file's frontmatter is not shown as added:\n{diff}"
    );
}

// ------------------------------------------------------------------ catalogue

/// Detects an implies closure that only follows one step, which would leave a
/// scope's own implied scopes off when it is turned on.
#[test]
fn the_implies_closure_is_transitive() {
    let store = store_with(&[
        ("scopes/a.yaml", b"id: a\nimplies: [b]\n"),
        ("scopes/b.yaml", b"id: b\nimplies: [c]\n"),
        ("scopes/c.yaml", b"id: c\n"),
    ]);
    let closed = store
        .catalog()
        .closure(&BTreeSet::from([ScopeId::new("a")]));
    assert!(
        closed.contains(&ScopeId::new("c")),
        "c is two implies steps from a but is not in the closure: {closed:?}"
    );
}

/// Detects a closure walk that revisits scopes: a store whose scopes imply each
/// other in a circle would hang or exhaust the stack, and one broken store must
/// not take the server down.
#[test]
fn an_implies_cycle_terminates_and_keeps_both_scopes() {
    let store = store_with(&[
        ("scopes/a.yaml", b"id: a\nimplies: [b]\n"),
        ("scopes/b.yaml", b"id: b\nimplies: [a]\n"),
    ]);
    let closed = store
        .catalog()
        .closure(&BTreeSet::from([ScopeId::new("a")]));
    assert_eq!(
        closed,
        BTreeSet::from([ScopeId::new("a"), ScopeId::new("b")]),
        "a cycle did not close to both of its scopes"
    );
}

/// A store with one critical and one knowledge memory in the global scope, plus
/// one memory in a scope of its own.
fn store_for_due() -> TempStore {
    store_with(&[
        ("scopes/widgets.yaml", b"id: widgets\n"),
        (
            "memories/b-critical.md",
            &memory_with_metadata(
                "b-critical",
                &["kind: critical", "scopes: [global]"],
                "# B\n\nB.\n",
            ),
        ),
        (
            "memories/a-knowledge.md",
            &memory_with_metadata(
                "a-knowledge",
                &["kind: knowledge", "scopes: [global]"],
                "# A\n\nA.\n",
            ),
        ),
        (
            "memories/d-widgets.md",
            &memory_with_metadata(
                "d-widgets",
                &["kind: critical", "scopes: [widgets]"],
                "# D\n\nD.\n",
            ),
        ),
    ])
}

fn due_ids(store: &TempStore, active: &[ScopeId]) -> Vec<String> {
    store
        .catalog()
        .due(&active.iter().cloned().collect())
        .into_iter()
        .map(|memory| memory.id.as_str().to_string())
        .collect()
}

/// Detects an order that puts index lines before the memories delivered in
/// full, which would bury the critical text the reader has to see.
#[test]
fn due_lists_critical_memories_before_knowledge_memories() {
    let store = store_for_due();
    assert_eq!(
        due_ids(&store, &[ScopeId::global()]),
        vec!["b-critical".to_string(), "a-knowledge".to_string()],
        "critical memories do not come first"
    );
}

/// Detects a selection that ignores scopes and delivers the whole store to
/// every context.
#[test]
fn due_omits_memories_whose_scopes_are_not_active() {
    let store = store_for_due();
    let due = due_ids(&store, &[ScopeId::global()]);
    assert!(
        !due.contains(&"d-widgets".to_string()),
        "a memory of an inactive scope is due: {due:?}"
    );
    assert!(
        due_ids(&store, &[ScopeId::new("widgets")]).contains(&"d-widgets".to_string()),
        "a memory of an active scope is not due"
    );
}

// ----------------------------------------------------------------- validation

/// Detects the whole rule set firing on a store that follows every rule, which
/// would make the review page and `check` useless.
///
/// The counts also guard the fixture: a copy that silently produced an empty
/// store would otherwise make an empty report meaningless.
#[test]
fn the_example_store_has_no_validation_errors() {
    let store = example_store();
    let catalog = store.catalog();
    assert_eq!(
        catalog.scopes().len(),
        count_example_files("scopes", ".yaml"),
        "the example store's scopes did not all load"
    );
    assert_eq!(
        catalog.memories().len(),
        count_example_files("memories", ".md"),
        "the example store's memories did not all load"
    );
    let report = validate(&catalog);
    assert!(
        report.is_clean(),
        "the example store reports problems: {:#?} {:#?}",
        report.errors(),
        report.warnings()
    );
}

/// Detects a memory pointing at a scope that does not exist, which would never
/// be delivered and would look like a working memory in the index.
#[test]
fn an_unknown_scope_in_a_memory_is_reported() {
    let store = store_with(&[(
        "memories/bench-power.md",
        &memory_with_metadata(
            "bench-power",
            &["scopes: [no-such-scope]"],
            "# One\n\nOne.\n",
        ),
    )]);
    assert_reports(
        &validate(&store.catalog()),
        "unknown scope",
        |error| matches!(error, ValidationError::UnknownScope { scope, .. } if scope.as_str() == "no-such-scope"),
    );
}

/// Detects an implicit scope being treated as unknown, which would report an
/// error on every memory in `global` or in a session.
#[test]
fn an_implicit_scope_is_not_reported_as_unknown() {
    let store = store_with(&[
        (
            "memories/bench-power.md",
            &memory_with_metadata(
                "bench-power",
                &["kind: critical", "scopes: [global, 'machine:alpha']"],
                "# One\n\nOne.\n",
            ),
        ),
        (
            "memories/sessions/alpha/session-1/notes.md",
            &session_memory("notes", "alpha/session-1", "# Notes\n\nNotes.\n"),
        ),
    ]);
    let report = validate(&store.catalog());
    assert!(
        report.is_clean(),
        "implicit scopes were reported: {:#?}",
        report.errors()
    );
}

/// Detects an `implies` target that does not exist, which would silently drop a
/// scope its author expected to be turned on with this one.
#[test]
fn an_unknown_implies_target_is_reported() {
    let store = store_with(&[(
        "scopes/widgets.yaml",
        b"id: widgets\nimplies: [no-such-scope]\n",
    )]);
    assert_reports(
        &validate(&store.catalog()),
        "unknown implies target",
        |error| matches!(error, ValidationError::UnknownImpliesTarget { target, .. } if target.as_str() == "no-such-scope"),
    );
}

/// Detects a trigger pattern that does not compile being accepted, which would
/// leave a scope that can never turn on and no sign of why.
#[test]
fn a_trigger_pattern_that_does_not_compile_is_reported_with_its_error() {
    let store = store_with(&[(
        "scopes/widgets.yaml",
        b"id: widgets\ntriggers:\n  - on: user_message\n    pattern: '[unclosed'\n",
    )]);
    let catalog = store.catalog();
    assert_reports(&validate(&catalog), "invalid trigger pattern", |error| {
        matches!(
            error,
            ValidationError::InvalidTriggerPattern { pattern, message, .. }
                if pattern == "[unclosed" && !message.is_empty()
        )
    });
    assert!(
        catalog.triggers().is_empty(),
        "a pattern that does not compile was left in the trigger index"
    );
}

/// Detects a machine qualifier accepted on a field where it means nothing: a
/// message is the same text on every machine, so a qualifier there would make a
/// trigger that never fires for anyone else and look deliberate.
#[test]
fn a_machine_qualifier_outside_working_directory_is_reported() {
    let store = store_with(&[(
        "scopes/widgets.yaml",
        b"id: widgets\ntriggers:\n  - on: user_message\n    pattern: widget\n    machine: alpha\n",
    )]);
    assert_reports(
        &validate(&store.catalog()),
        "misplaced machine qualifier",
        |error| matches!(error, ValidationError::MisplacedMachineQualifier { machine, .. } if machine == "alpha"),
    );
}

/// Detects a machine qualifier refused on an `any` trigger: the texts an `any`
/// trigger matches include the working directory and the paths the tools are
/// called with, so restricting one to a machine is as meaningful there as on a
/// directory trigger, and refusing it would make such a trigger unwritable.
#[test]
fn a_machine_qualifier_on_an_any_trigger_is_accepted() {
    let store = store_with(&[(
        "scopes/workshop.yaml",
        b"id: workshop\ntriggers:\n  - pattern: '/workshop(/|$)'\n    machine: alpha\n",
    )]);
    let report = validate(&store.catalog());
    assert!(
        report.is_clean(),
        "a machine-qualified any trigger was reported: {:#?}",
        report.errors()
    );
}

/// Detects a scope whose declared id differs from its file name, which would
/// make every reference to it resolve to one or the other unpredictably.
#[test]
fn a_scope_id_that_differs_from_its_file_stem_is_reported() {
    let store = store_with(&[("scopes/widgets.yaml", b"id: gadgets\n")]);
    assert_reports(
        &validate(&store.catalog()),
        "scope id mismatch",
        |error| matches!(error, ValidationError::ScopeIdMismatch { declared, expected, .. } if declared.as_str() == "gadgets" && expected.as_str() == "widgets"),
    );
}

/// Detects an id outside the allowed pattern, which could collide with the
/// implicit `machine:` and `session:` forms or with a file name on disk.
#[test]
fn a_scope_id_that_breaks_the_id_pattern_is_reported() {
    let store = store_with(&[("scopes/Widgets.yaml", b"id: Widgets\n")]);
    assert_reports(
        &validate(&store.catalog()),
        "invalid scope id",
        |error| matches!(error, ValidationError::InvalidScopeId { id, .. } if id == "Widgets"),
    );
}

/// Detects a `name` key that disagrees with the file it is in, which would make
/// the id a link resolves by disagree with the name the file claims.
#[test]
fn a_memory_name_that_differs_from_its_path_is_reported() {
    let store = store_with(&[(
        "memories/bench-power.md",
        &global_memory("gadget-power", "# One\n\nOne.\n"),
    )]);
    assert_reports(
        &validate(&store.catalog()),
        "memory name mismatch",
        |error| matches!(error, ValidationError::MemoryNameMismatch { declared, expected, .. } if declared == "gadget-power" && expected == "bench-power"),
    );
}

/// Detects a create that silently replaces the memory already at that id,
/// losing its content in one commit.
///
/// Two paths in one git tree cannot produce the same id, so the write path is
/// the only interface at which this failure can be seen.
#[test]
fn creating_a_memory_whose_id_already_exists_is_reported() {
    let store = store_with(&[(
        "memories/bench-power.md",
        &global_memory("bench-power", "# One\n\nOne.\n"),
    )]);
    let catalog = store.catalog();
    let document = MemoryDocument::parse(
        MemoryId::new("bench-power"),
        &global_memory("bench-power", "# Another\n\nAnother.\n"),
    )
    .expect("the candidate parses");
    let candidate = Candidate::Memory {
        document: &document,
    };

    assert_reports(
        &validate_write(&catalog, candidate, WriteMode::Create),
        "duplicate memory id",
        |error| matches!(error, ValidationError::DuplicateMemoryId { id, .. } if id.as_str() == "bench-power"),
    );
    assert!(
        !validate_write(&catalog, candidate, WriteMode::Update).has_errors(),
        "replacing the memory at its own id was reported as a duplicate"
    );
}

/// Detects a link to a memory that does not exist, which renders as a dead
/// link in the frontend and tells the model to fetch something absent.
#[test]
fn a_link_to_a_missing_memory_is_reported() {
    let store = store_with(&[(
        "memories/reading-list.md",
        &global_memory("reading-list", "# Reading\n\nSee [[no-such-memory]].\n"),
    )]);
    assert_reports(
        &validate(&store.catalog()),
        "missing link target",
        |error| matches!(error, ValidationError::MissingLinkTarget { target, .. } if target == "no-such-memory"),
    );
}

/// Detects a link from outside a session silo into it. A session's notes are
/// delivered only to that session, so a link from a shared memory would be dead
/// for every reader except one.
#[test]
fn a_link_from_outside_a_session_silo_into_it_is_reported() {
    let store = store_with(&[
        (
            "memories/reading-list.md",
            &global_memory(
                "reading-list",
                "# Reading\n\nSee [[sessions/alpha/session-1/notes]].\n",
            ),
        ),
        (
            "memories/sessions/alpha/session-1/notes.md",
            &session_memory("notes", "alpha/session-1", "# Notes\n\nNotes.\n"),
        ),
    ]);
    assert_reports(
        &validate(&store.catalog()),
        "link crossing a session silo",
        |error| matches!(error, ValidationError::LinkCrossesSessionSilo { silo, .. } if silo == "sessions/alpha/session-1"),
    );
}

/// Detects a link between two memories in the same silo being reported, which
/// would make a session unable to reference its own notes.
#[test]
fn a_link_inside_one_session_silo_is_not_reported() {
    let store = store_with(&[
        (
            "memories/sessions/alpha/session-1/notes.md",
            &session_memory(
                "notes",
                "alpha/session-1",
                "# Notes\n\nSee [[measurements]].\n",
            ),
        ),
        (
            "memories/sessions/alpha/session-1/measurements.md",
            &session_memory(
                "measurements",
                "alpha/session-1",
                "# Measurements\n\nMeasurements.\n",
            ),
        ),
    ]);
    let report = validate(&store.catalog());
    assert!(
        report.is_clean(),
        "a link inside one silo was reported: {:#?}",
        report.errors()
    );
}

/// Detects a session memory carrying scopes beyond its own session, which would
/// deliver a silo's notes to contexts that cannot resolve the links in them.
#[test]
fn a_session_memory_with_scopes_beyond_its_session_is_reported() {
    let store = store_with(&[(
        "memories/sessions/alpha/session-1/notes.md",
        &memory_with_metadata(
            "notes",
            &["scopes: ['session:alpha/session-1', global]"],
            "# Notes\n\nNotes.\n",
        ),
    )]);
    assert_reports(
        &validate(&store.catalog()),
        "session memory scopes",
        |error| matches!(error, ValidationError::SessionMemoryScopes { expected, .. } if expected.as_str() == "session:alpha/session-1"),
    );
}

/// Detects one malformed file taking the whole store down: the rest of the
/// store must still load, and the broken file must be named.
#[test]
fn frontmatter_that_is_not_valid_yaml_is_reported_and_the_rest_still_loads() {
    let store = store_with(&[
        (
            "memories/broken.md",
            b"---\nname: broken\nmetadata: [global\n---\n# Broken\n\nBroken.\n",
        ),
        (
            "memories/bench-power.md",
            &global_memory("bench-power", "# One\n\nOne.\n"),
        ),
    ]);
    let catalog = store.catalog();
    assert_reports(
        &validate(&catalog),
        "parse failure",
        |error| matches!(error, ValidationError::ParseFailure { path, .. } if path == "memories/broken.md"),
    );
    assert!(
        catalog.memory(&MemoryId::new("bench-power")).is_some(),
        "a valid memory was dropped because another file was broken"
    );
}

/// Detects a file without frontmatter taking the store down. Claude Code's own
/// memory directory holds an index file that has none, and a store made of such
/// a directory has to load: the file is not a memory, which is a warning about
/// that file, not an error about the store.
#[test]
fn a_file_without_frontmatter_is_skipped_with_a_warning_and_the_rest_still_loads() {
    let store = store_with(&[
        (
            "memories/MEMORY.md",
            b"# Memory index\n\nA plain list with no frontmatter.\n",
        ),
        (
            "memories/bench-power.md",
            &global_memory("bench-power", "# One\n\nOne.\n"),
        ),
    ]);
    let catalog = store.catalog();
    let report = validate(&catalog);

    assert!(
        !report.has_errors(),
        "a file without frontmatter made the store invalid: {:#?}",
        report.errors()
    );
    assert_warns(
        &report,
        "skipped file",
        |warning| matches!(warning, ValidationWarning::NotAMemoryFile { path } if path == "memories/MEMORY.md"),
    );
    assert!(
        catalog.memory(&MemoryId::new("MEMORY")).is_none(),
        "a file without frontmatter was loaded as a memory"
    );
    assert!(
        catalog.memory(&MemoryId::new("bench-power")).is_some(),
        "a valid memory was dropped because another file had no frontmatter"
    );
}

/// Detects a memory with no `description` passing unremarked: its index entry is
/// empty, so the model is offered a memory with nothing said about it.
#[test]
fn a_memory_without_a_description_is_warned_about_but_still_loads() {
    let store = store_with(&[(
        "memories/bench-power.md",
        &memory_file("name: bench-power\n", "# One\n\nOne.\n"),
    )]);
    let catalog = store.catalog();
    let report = validate(&catalog);

    assert!(
        !report.has_errors(),
        "a missing description made the store invalid: {:#?}",
        report.errors()
    );
    assert_warns(
        &report,
        "missing description",
        |warning| matches!(warning, ValidationWarning::MissingDescription { memory, .. } if memory.as_str() == "bench-power"),
    );
    assert!(
        catalog.memory(&MemoryId::new("bench-power")).is_some(),
        "a memory without a description was dropped"
    );
}

// -------------------------------------------------------------- check command

fn run_check(store_path: &std::path::Path) -> Output {
    Command::new(binary_path())
        .arg("check")
        .arg("--store")
        .arg(store_path)
        .output()
        .expect("the forgetmenot binary runs")
}

/// Files of one kind in the example store, counted from the directory rather
/// than written down, so that adding an example does not break this test.
fn count_example_files(directory: &str, suffix: &str) -> usize {
    fn walk(path: &std::path::Path, suffix: &str, count: &mut usize) {
        for entry in std::fs::read_dir(path).expect("the example directory reads") {
            let entry = entry.expect("a readable entry").path();
            if entry.is_dir() {
                walk(&entry, suffix, count);
            } else if entry.to_string_lossy().ends_with(suffix) {
                *count += 1;
            }
        }
    }
    let mut count = 0;
    walk(&example_store_path().join(directory), suffix, &mut count);
    count
}

/// Detects a `check` that fails on a valid store, or one that reports nothing
/// about what it found, which would make it useless as a gate.
#[test]
fn check_exits_zero_and_reports_counts_on_the_example_store() {
    let store = example_store();
    let output = run_check(store.path());
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "check failed on the example store\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (label, expected) in [
        ("scopes", count_example_files("scopes", ".yaml")),
        ("memories", count_example_files("memories", ".md")),
    ] {
        assert!(
            stdout.contains(&format!("{label}: {expected}")),
            "check did not report {expected} {label}:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("triggers: "),
        "check did not report a trigger count:\n{stdout}"
    );
}

/// Detects a `check` that passes a settings file this server cannot act on: a
/// key it does not have and a value of the wrong type are both a behaviour
/// somebody asked for and is not getting, and `check` is the gate that says so
/// before the store is deployed.
#[test]
fn check_exits_one_and_names_the_setting_a_store_gets_wrong() {
    for (settings, expected) in [
        ("remind_tokens: 1000\n", "remind_tokens"),
        (
            "interrupt_on_critical: sometimes\n",
            "interrupt_on_critical",
        ),
    ] {
        let store = store_with(&[("config.yml", settings.as_bytes())]);
        let output = run_check(store.path());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.status.code(),
            Some(1),
            "check did not exit 1 on {settings:?}:\n{stdout}"
        );
        assert!(
            stdout.contains("config.yml") && stdout.contains(expected),
            "check did not name the file and the setting for {settings:?}:\n{stdout}"
        );
    }
}

/// Detects a `check` that exits zero on a broken store, or one that reports the
/// problem without saying which file to open.
#[test]
fn check_exits_one_and_names_the_file_holding_a_bad_regex() {
    let store = store_with(&[(
        "scopes/widgets.yaml",
        b"id: widgets\ntriggers:\n  - on: user_message\n    pattern: '[unclosed'\n",
    )]);
    let output = run_check(store.path());
    assert_eq!(
        output.status.code(),
        Some(1),
        "check did not exit 1 on a broken store"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("scopes/widgets.yaml"),
        "check did not name the broken file:\n{stdout}"
    );
}

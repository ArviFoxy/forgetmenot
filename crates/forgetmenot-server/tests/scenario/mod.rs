//! The scenario library: Claude Code driving a real server over time.
//!
//! A scenario is an ordinary `#[test]`. It builds a [`World`] (a server on a
//! store from a builder), takes a [`Claude`] on a machine, starts [`Session`]s
//! on it, and asserts on the [`Answer`]s and on what the model can actually
//! read. The simulator sends exactly the payloads Claude Code 2.1.270 sends, in
//! its order, and does what Claude Code does with each answer; the payload
//! shapes are held to the recordings under `tests/fixtures/hooks/` by the tests
//! in [`payloads`].
//!
//! What belongs here is what only emerges when a whole session runs: event order
//! and harness semantics, cross-context effects, persistence across a restart,
//! what the model was actually shown. A rule that a single event shows is tested
//! where that event is, not here.
//!
//! A test file using this module declares `mod common;` beside it, the way it
//! would to use the shared server helpers: everything here reads the server
//! through those.
#![allow(dead_code, unused_imports)]

pub mod answer;
pub mod claude;
pub mod mcp;
pub mod payloads;
pub mod session;
pub mod store;
pub mod world;

pub use answer::Answer;
pub use claude::Claude;
pub use mcp::{Branch, Mcp, McpOutcome};
pub use session::{Compaction, Session, Subagent, ToolCall, bash, read};
pub use store::{Field, MemoryBuilder, ScopeBuilder, Store, StoreBuilder};
pub use world::{SettingsBuilder, World, WorldBuilder};

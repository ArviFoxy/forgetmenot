//! forgetmenot server library.
//!
//! The store is a git repository: scope and memory files are the source of
//! truth and every write is a commit, so a person editing by hand and the
//! server editing over HTTP use the same history.
//!
//! Around it sit the service layer ([`service`], [`context`], [`stats`]), the
//! operations built on it ([`operations`]), and the adapters that use those: the
//! hook endpoint in [`hook`], the JSON API in [`api`], and the MCP tools in
//! [`mcp`].

pub mod api;
pub mod app;
pub mod clock;
pub mod config;
pub mod context;
pub mod hook;
pub mod mcp;
pub mod operations;
pub mod render;
pub mod service;
pub mod stats;
pub mod store;
pub mod triggers;

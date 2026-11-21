//! Wysebee Agent for Mac, bar the window.
//!
//! A small app in the menu bar that pairs with Wysebee Cloud and runs the "Run local
//! program" tools one agent's settings hold, and the scripts its skills declare, after the
//! person allowed each on this Mac — and the folder tools, over the folders the person
//! picked here.
//! It runs no model. Wysebee's agent-link protocol is the contract.
//!
//! - [`programs`]: reading a program, filling its blanks, finding and running it.
//! - [`skills`]: reading a skill's `SKILL.md` and file list, and putting a skill in place.
//! - [`folders`]: the person's local folders, and the four folder tools over them.
//! - [`approvals`]: the state file and what the person allowed.
//! - [`secrets`]: the link's secret, in the Keychain.
//! - [`cloud`]: which Wysebee Cloud, and the rule its address must pass.
//! - [`link`]: the protocol's HTTP calls.
//! - [`agent`]: the [`Agent`] the shell drives: pairing, the poll loop, the calls.
//!
//! Nothing here knows of Tauri. **Every folder, the secret store, the cloud address and
//! the `PATH` searched are handed in** ([`agent::Options`]), so the tests use temporary
//! folders, a secret store in memory and a fake cloud on 127.0.0.1.

pub mod agent;
pub mod android;
pub mod approvals;
pub mod cloud;
pub mod dev;
pub mod folders;
pub mod guard;
pub mod ios;
pub mod link;
pub mod programs;
pub mod screen;
pub mod secrets;
pub mod skills;

pub use agent::{Agent, Options, Status, Step, Timing};

/// The app's version: the workspace `Cargo.toml`'s.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

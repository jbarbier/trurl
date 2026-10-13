//! Trurl's core: everything that is not UI.
//!
//! The desktop app, the terminal app and the dev bridge all drive [`app::App`]
//! and render the [`events::Event`]s it emits. See `docs/ARCHITECTURE.md`.

pub mod api;
pub mod app;
pub mod chat;
pub mod downloads;
pub mod errors;
pub mod events;
pub mod gguf;
pub mod hardware;
pub mod hub;
pub mod licenses;
pub mod models;
pub mod net;
pub mod paths;
pub mod pins;
pub mod server;
pub mod settings;
pub mod sse;
pub mod think;
pub mod update;

pub use errors::{ErrorCode, UserError};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

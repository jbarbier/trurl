//! Trurl's core: everything that is not UI.
//!
//! The desktop app, the terminal app and the dev bridge all drive [`app::App`]
//! and render the [`events::Event`]s it emits. See `docs/ARCHITECTURE.md`.

pub mod app;
pub mod chat;
pub mod downloads;
pub mod events;
pub mod errors;
pub mod gguf;
pub mod hardware;
pub mod hub;
pub mod models;
pub mod paths;
pub mod pins;
pub mod server;
pub mod settings;
pub mod sse;
pub mod think;

pub use errors::{ErrorCode, UserError};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

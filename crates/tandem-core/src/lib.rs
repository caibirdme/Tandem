//! Application service shared by the desktop shell and development HTTP server.

mod activities;
mod agent;
mod app;
mod dispatch;
mod error;
mod events;
pub mod model;
mod projects;
mod providers;
mod session_models;
mod session_reasoning;
mod sessions;
mod skills;
mod store;
mod titles;
mod turns;
mod vault;

pub use app::{App, AppOptions};
pub use dispatch::{CreateSessionRequest, Request};
pub use error::{Error, ErrorCode, ErrorResponse, EventDecodeError, Result};
pub use store::StorageError;
pub use vault::{LocalVault, MemoryVault, SecretError, SecretStore};

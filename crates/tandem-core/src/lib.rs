//! Application service shared by the desktop shell and development HTTP server.

mod agent;
mod app;
mod dispatch;
mod error;
mod events;
pub mod model;
mod projects;
mod providers;
mod session_models;
mod sessions;
mod store;
mod titles;
mod turns;
mod vault;

pub use app::{App, AppOptions};
pub use dispatch::Request;
pub use error::{Error, ErrorCode, ErrorResponse, EventDecodeError, Result};
pub use store::StorageError;
pub use vault::{LocalVault, MemoryVault, SecretError, SecretStore};

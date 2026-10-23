//! Application service shared by the desktop shell and development HTTP server.

mod activities;
mod agent;
mod app;
mod dispatch;
mod error;
mod events;
pub mod model;
mod model_discovery;
mod projects;
mod providers;
mod reasoning;
mod reviews;
mod session_models;
mod session_reasoning;
mod sessions;
mod skills;
mod store;
mod subagents;
mod submissions;
mod telemetry;
mod titles;
mod turns;
pub mod usage;
mod vault;

pub use app::{App, AppOptions};
pub use dispatch::{CreateSessionRequest, Request, SendMessageRequest};
pub use error::{Error, ErrorCode, ErrorResponse, EventDecodeError, Result};
pub use model_discovery::ModelDiscoveryError;
pub use store::StorageError;
pub use vault::{LocalVault, MemoryVault, SecretError, SecretStore};

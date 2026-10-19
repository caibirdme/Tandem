//! Application service shared by the desktop shell and development HTTP server.

mod agent;
mod app;
mod dispatch;
mod events;
pub mod model;
mod projects;
mod providers;
mod session_models;
mod sessions;
mod storage_paths;
mod store;
mod titles;
mod turns;
mod vault;

pub use app::{App, AppOptions};
pub use dispatch::Request;
pub use vault::{LocalVault, MemoryVault, SecretStore};

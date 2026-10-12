//! JobHunt Cloud: the local product, hosted.
//!
//! This crate adds only what a hosted service needs around the shared
//! application (`jobhunt-app`): configuration from the environment
//! ([`config`]), authentication ([`auth`]), the HTTP API and hosted MCP
//! ([`api`], [`mcp`]), scheduled workers ([`worker`]), usage events
//! ([`usage`]), request tracing ([`observability`]) and the process entry
//! points ([`server`]). Discovery, verification, eligibility, ranking,
//! taste, profile rules and the evidence policy are the domain crates',
//! reached through the same use cases the CLI and the local MCP server
//! call; storage is the Postgres backend of `jobhunt-storage`.
//!
//! ```text
//! CLI ───────────┐
//! Local MCP ─────┤
//!                ├── jobhunt-app ── domain services
//! HTTP API ──────┤        │
//! Hosted MCP ────┤   Store (repository traits)
//! Workers ───────┘     /            \
//!                  SQLite         Postgres
//! ```

pub mod api;
pub mod auth;
pub mod client;
pub mod config;
pub mod digest;
pub mod email;
pub mod mcp;
pub mod notify;
pub mod observability;
pub mod server;
pub mod usage;
pub mod worker;

pub use config::{CloudConfig, Role};
pub use server::CloudError;

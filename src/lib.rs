//! Unforged for Rust: check signatures with Nib and documents with Loupe, through the API or
//! entirely on your own machines with the offline binaries. The same code runs either way.
//!
//! ```no_run
//! use unforged::{nib::VerifyOptions, Unforged};
//!
//! # async fn run() -> unforged::Result<()> {
//! let client = Unforged::new("unf_...")?;
//! let result = client
//!     .nib()
//!     .verify("application.pdf", VerifyOptions::subject("cust-88").reference("APP-1042"))
//!     .await?;
//! println!("{:?}", result.decision);
//! # Ok(())
//! # }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod client;
mod days;
mod error;
mod files;
mod http;
mod json;
mod model;
mod offline;
mod page;
mod webhook_endpoints;

pub mod account;
#[cfg(feature = "blocking")]
pub mod blocking;
pub mod folio;
pub mod licensing;
pub mod loupe;
pub mod nib;
pub mod sig;
pub mod webhooks;

use std::path::PathBuf;

pub use client::{Builder, Mode, Unforged};
pub use error::{ApiError, ApiErrorKind, Error, Result};
pub use files::{File, Target};
pub use model::Secret;
pub use offline::{Licence, ReferenceSource};
pub use page::Page;
pub use webhook_endpoints::{NibWebhooks, Webhook, WebhookDelivery, Webhooks};

/// A decision policy: one of your saved policies by name (online), or a policy in full.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Policy {
    /// One of your saved policies (online).
    Named(String),
    /// A policy JSON file.
    File(PathBuf),
    /// A policy as JSON.
    Json(serde_json::Value),
}

impl From<&str> for Policy {
    fn from(name: &str) -> Self {
        Policy::Named(name.into())
    }
}

impl From<String> for Policy {
    fn from(name: String) -> Self {
        Policy::Named(name)
    }
}

impl From<PathBuf> for Policy {
    fn from(path: PathBuf) -> Self {
        Policy::File(path)
    }
}

impl From<&std::path::Path> for Policy {
    fn from(path: &std::path::Path) -> Self {
        Policy::File(path.to_path_buf())
    }
}

impl From<serde_json::Value> for Policy {
    fn from(v: serde_json::Value) -> Self {
        Policy::Json(v)
    }
}

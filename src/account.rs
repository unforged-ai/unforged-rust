//! Your organisation's API keys and audit log, and the API's own status (online only).
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::error::Result;
use crate::files::seg;
use crate::http::{Request, Transport};
use crate::model::{decode, list, Secret};
use crate::page::Page;

/// An API key.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub display_prefix: String,
    pub permissions: Vec<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub owner: Option<Value>,
    /// The key itself: only when it's issued.
    pub key: Secret,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ApiKey {
    pub fn active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// One entry of the audit log.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AuditEntry {
    pub id: i64,
    pub action: String,
    pub target: Option<String>,
    pub detail: Value,
    pub at: String,
    pub actor: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The versions the API is running.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Versions {
    pub model: String,
    pub api_version: String,
    pub region: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Keys, the audit log and the API's status.
#[derive(Debug, Clone, Copy)]
pub struct Account<'a>(pub(crate) &'a Transport);

impl Account<'_> {
    /// Your organisation's keys (or a child organisation's).
    pub async fn keys(&self, org_id: Option<&str>) -> Result<Vec<ApiKey>> {
        let v = self
            .0
            .json(Request::get("/keys").opt("org_id", org_id))
            .await?;
        list(&v, "api_keys")
    }

    /// Issue a key with some of your own permissions; its `key` is shown only now.
    pub async fn create_key(
        &self,
        name: &str,
        permissions: &[&str],
        org_id: Option<&str>,
    ) -> Result<ApiKey> {
        let mut body = json!({ "name": name, "permissions": permissions });
        if let Some(o) = org_id {
            body["org_id"] = o.into();
        }
        decode(self.0.json(Request::post("/keys").json(body)).await?)
    }

    pub async fn revoke_key(&self, key_id: &str) -> Result<()> {
        let r = Request::new(
            reqwest::Method::DELETE,
            format!("/keys/{}", seg(key_id, "key id")?),
        );
        self.0.send(r, &[]).await.map(drop)
    }

    /// One page of the audit log, newest first; `action` is an exact action or a prefix ending
    /// in `*`.
    pub async fn audit(
        &self,
        action: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Page<AuditEntry>> {
        let r = Request::get("/audit")
            .opt("action", action)
            .opt("cursor", cursor)
            .opt("limit", limit);
        let v = self.0.json(r).await?;
        Ok(Page {
            items: list(&v, "entries")?,
            next_cursor: crate::json::s(&v, "next_cursor"),
        })
    }

    /// Whether the API is serving requests.
    pub async fn health(&self) -> Result<bool> {
        let v = self.0.json(Request::get("/health")).await?;
        Ok(crate::json::s(&v, "status").as_deref() == Some("ok"))
    }

    /// `ready`, or `degraded` (checks still complete), or `unavailable`.
    pub async fn ready(&self) -> Result<String> {
        let v = self.0.send(Request::get("/ready"), &[503]).await?.json()?;
        Ok(crate::json::s(&v, "status").unwrap_or_else(|| "unavailable".into()))
    }

    pub async fn versions(&self) -> Result<Versions> {
        decode(self.0.json(Request::get("/version")).await?)
    }
}

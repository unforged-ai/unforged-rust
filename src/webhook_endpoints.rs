use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::error::Result;
use crate::files::check_id;
use crate::http::{Request, Transport};
use crate::model::{decode, list, Secret};
use crate::page::Page;

/// An endpoint we send events to.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Webhook {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub created_at: String,
    /// The signing secret: only when the endpoint is created or its secret rolled.
    pub secret: Secret,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One attempt to deliver an event.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct WebhookDelivery {
    pub id: String,
    pub event_id: String,
    pub event_type: String,
    pub attempt: u32,
    pub status: String,
    pub response_status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    pub created_at: String,
    pub delivered_at: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl WebhookDelivery {
    pub fn succeeded(&self) -> bool {
        self.status == "succeeded"
    }
}

/// Where we send one product's events.
#[derive(Debug, Clone, Copy)]
pub struct Webhooks<'a> {
    http: &'a Transport,
    base: &'static str,
}

impl<'a> Webhooks<'a> {
    pub(crate) fn new(http: &'a Transport, base: &'static str) -> Self {
        Self { http, base }
    }

    pub async fn list(&self) -> Result<Vec<Webhook>> {
        let v = self.http.json(Request::get(self.base)).await?;
        list(&v, "webhooks")
    }

    /// Add an endpoint; we send it a test event first, and its `secret` is shown only now.
    pub async fn create(&self, url: &str, events: &[&str]) -> Result<Webhook> {
        let r = Request::post(self.base).json(json!({ "url": url, "events": events }));
        decode(self.http.json(r).await?)
    }

    pub async fn delete(&self, webhook_id: &str) -> Result<()> {
        let r = Request::new(
            reqwest::Method::DELETE,
            format!("{}/{}", self.base, check_id(webhook_id, "webhook id")?),
        );
        self.http.send(r, &[]).await.map(drop)
    }

    pub(crate) fn path(&self, webhook_id: &str, rest: &str) -> Result<String> {
        Ok(format!(
            "{}/{}{rest}",
            self.base,
            check_id(webhook_id, "webhook id")?
        ))
    }

    pub(crate) fn http(&self) -> &'a Transport {
        self.http
    }
}

/// Nib's endpoints, which also keep their delivery history.
#[derive(Debug, Clone, Copy)]
pub struct NibWebhooks<'a>(pub(crate) Webhooks<'a>);

impl<'a> std::ops::Deref for NibWebhooks<'a> {
    type Target = Webhooks<'a>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl NibWebhooks<'_> {
    /// Replace the signing secret; the new one is shown only now.
    pub async fn roll_secret(&self, webhook_id: &str) -> Result<Webhook> {
        let r = Request::post(self.0.path(webhook_id, "/secret/roll")?);
        decode(self.0.http().json(r).await?)
    }

    /// One page of delivery attempts from the last 30 days, newest first.
    pub async fn deliveries(
        &self,
        webhook_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Page<WebhookDelivery>> {
        let r = Request::get(self.0.path(webhook_id, "/deliveries")?)
            .opt("cursor", cursor)
            .opt("limit", limit);
        let v = self.0.http().json(r).await?;
        Ok(Page {
            items: list(&v, "deliveries")?,
            next_cursor: crate::json::s(&v, "next_cursor"),
        })
    }

    /// Send a delivery's event again.
    pub async fn redeliver(&self, webhook_id: &str, delivery_id: &str) -> Result<()> {
        let rest = format!(
            "/deliveries/{}/redeliver",
            check_id(delivery_id, "delivery id")?
        );
        let r = Request::post(self.0.path(webhook_id, &rest)?);
        self.0.http().send(r, &[]).await.map(drop)
    }
}

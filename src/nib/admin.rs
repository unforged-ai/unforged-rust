use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::error::Result;
use crate::files::{check_id, seg, File};
use crate::http::{multipart, Part, Request, Transport};
use crate::model::{decode, list, Secret};

/// A short-lived token that lets your own app run one check for one person.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Session {
    pub id: String,
    pub token: Secret,
    pub subject_reference: String,
    pub expires_at: String,
    pub max_checks: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A saved decision policy.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SavedPolicy {
    pub name: String,
    pub version: u32,
    pub id: String,
    pub auto_approve_enabled: bool,
    pub auto_threshold: f64,
    pub approve_threshold: f64,
    pub fail_threshold: f64,
    pub quality: Option<Value>,
    pub created_at: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The thresholds of a policy to save.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolicySettings {
    pub approve_threshold: f64,
    pub fail_threshold: f64,
    /// The bar for `AUTO_APPROVE` (the approve threshold unless set).
    pub auto_threshold: Option<f64>,
    pub auto_approve: bool,
    pub quality: Option<BTreeMap<String, f64>>,
}

/// A form template, so a check can look at one named box on a form.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Template {
    pub template_id: String,
    pub name: String,
    pub boxes: Vec<Value>,
    pub blank_type: String,
    pub created_at: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// An access key for dropping documents into your own bucket for checking.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct BucketKey {
    pub id: Option<String>,
    pub access_key_id: String,
    pub status: String,
    pub created_at: Option<String>,
    pub reissue_required: bool,
    /// Shown only when the key is issued.
    #[serde(rename = "secret_access_key")]
    pub secret: Secret,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for BucketKey {
    fn default() -> Self {
        Self {
            id: None,
            access_key_id: String::new(),
            status: "active".into(),
            created_at: None,
            reissue_required: false,
            secret: Secret::default(),
            extra: Map::new(),
        }
    }
}

/// How long finished checks are kept.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct HistorySettings {
    /// Days, `"30m"` or `"indefinite"`.
    pub history_retention: Value,
    pub default: bool,
    pub audit_log_retention_days: Option<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Whether Nib learns references from confirmed signatures.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct LearningSettings {
    /// `off`, `confirmed_only` or `confirmed_and_auto`.
    pub reference_learning: String,
    pub learn_min_band_lo: f64,
    pub learn_min_probability: f64,
    pub max_learned: u32,
    pub terms_accepted: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Short-lived tokens for your own app.
#[derive(Debug, Clone, Copy)]
pub struct Sessions<'a>(pub(crate) &'a Transport);

impl Sessions<'_> {
    /// A token for one check against `subject_reference`, valid for up to 15 minutes.
    pub async fn create(&self, subject_reference: &str, ttl_seconds: u32) -> Result<Session> {
        let r = Request::post("/nib/sessions").json(json!({
            "subject_reference": check_id(subject_reference, "subject")?,
            "scope": ["check"],
            "ttl_seconds": ttl_seconds,
        }));
        decode(self.0.json(r).await?)
    }

    /// End a session now; revoking one that has already ended is fine.
    pub async fn revoke(&self, session_id: &str) -> Result<()> {
        let r = Request::new(
            reqwest::Method::DELETE,
            format!("/nib/sessions/{}", seg(session_id, "session id")?),
        );
        self.0.send(r, &[]).await.map(drop)
    }
}

/// Your saved decision policies, which online checks name.
#[derive(Debug, Clone, Copy)]
pub struct Policies<'a>(pub(crate) &'a Transport);

impl Policies<'_> {
    pub async fn list(&self) -> Result<Vec<SavedPolicy>> {
        list(
            &self.0.json(Request::get("/nib/policies")).await?,
            "policies",
        )
    }

    pub async fn get(&self, name: &str, version: Option<u32>) -> Result<SavedPolicy> {
        let r = Request::get(format!("/nib/policies/{}", seg(name, "policy name")?))
            .opt("version", version);
        decode(self.0.json(r).await?)
    }

    /// Save a new version of a policy (an unchanged one comes back as it was).
    pub async fn save(&self, name: &str, p: &PolicySettings) -> Result<SavedPolicy> {
        let r = Request::new(
            reqwest::Method::PUT,
            format!("/nib/policies/{}", seg(name, "policy name")?),
        )
        .json(json!({
            "auto_approve_enabled": p.auto_approve,
            "auto_threshold": p.auto_threshold.unwrap_or(p.approve_threshold),
            "approve_threshold": p.approve_threshold,
            "fail_threshold": p.fail_threshold,
            "quality": p.quality,
        }));
        decode(self.0.json(r).await?)
    }
}

/// Form templates.
#[derive(Debug, Clone, Copy)]
pub struct Templates<'a>(pub(crate) &'a Transport);

impl Templates<'_> {
    pub async fn list(&self) -> Result<Vec<Template>> {
        list(
            &self.0.json(Request::get("/nib/templates")).await?,
            "templates",
        )
    }

    pub async fn get(&self, template_id: &str) -> Result<Template> {
        let r = Request::get(format!(
            "/nib/templates/{}",
            seg(template_id, "template id")?
        ));
        decode(self.0.json(r).await?)
    }

    /// Upload a blank form (PNG or PDF) with its boxes, each `(name, [x, y, w, h])`.
    pub async fn create(
        &self,
        template_id: &str,
        blank: impl Into<File>,
        boxes: &[(&str, [i64; 4])],
        name: Option<&str>,
    ) -> Result<Template> {
        let mut spec = json!({
            "template_id": check_id(template_id, "template id")?,
            "boxes": boxes.iter().map(|(n, b)| json!({"name": n, "x": b[0], "y": b[1], "w": b[2], "h": b[3]})).collect::<Vec<_>>(),
        });
        if let Some(n) = name {
            spec["name"] = n.into();
        }
        let (filename, data) = blank.into().read().await?;
        let filename =
            filename.unwrap_or_else(|| format!("blank{}", crate::files::extension(&data)));
        let (ct, body) = multipart(vec![
            Part::json("template", &spec),
            Part::file("blank", filename, data),
        ]);
        decode(
            self.0
                .json(Request::post("/nib/templates").raw(ct, body))
                .await?,
        )
    }

    pub async fn delete(&self, template_id: &str) -> Result<()> {
        let r = Request::new(
            reqwest::Method::DELETE,
            format!("/nib/templates/{}", seg(template_id, "template id")?),
        );
        self.0.send(r, &[]).await.map(drop)
    }
}

/// Access keys for your own bucket.
#[derive(Debug, Clone, Copy)]
pub struct BucketKeys<'a>(pub(crate) &'a Transport);

impl BucketKeys<'_> {
    pub async fn list(&self) -> Result<Vec<BucketKey>> {
        list(
            &self.0.json(Request::get("/nib/bucket-keys")).await?,
            "bucket_keys",
        )
    }

    /// Issue a key, its secret shown once; `None` the first time, while access is set up (ask
    /// again in a minute).
    pub async fn create(&self) -> Result<Option<BucketKey>> {
        let resp = self
            .0
            .send(Request::post("/nib/bucket-keys").json(json!({})), &[])
            .await?;
        if resp.status == 202 {
            return Ok(None);
        }
        decode(resp.json()?).map(Some)
    }

    pub async fn revoke(&self, key_id: &str) -> Result<()> {
        let r = Request::new(
            reqwest::Method::DELETE,
            format!("/nib/bucket-keys/{}", seg(key_id, "key id")?),
        );
        self.0.send(r, &[]).await.map(drop)
    }
}

/// How long check history is kept, and whether Nib learns from confirmed signatures.
#[derive(Debug, Clone, Copy)]
pub struct Settings<'a>(pub(crate) &'a Transport);

impl Settings<'_> {
    pub async fn history(&self) -> Result<HistorySettings> {
        decode(self.0.json(Request::get("/nib/settings/retention")).await?)
    }

    /// Keep finished checks for `keep`: a number of days (1 to 3650), `"30m"` or `"indefinite"`.
    pub async fn set_history(&self, keep: impl Into<Value>) -> Result<HistorySettings> {
        let r = Request::new(reqwest::Method::PUT, "/nib/settings/retention")
            .json(json!({ "history_retention": keep.into() }));
        decode(self.0.json(r).await?)
    }

    pub async fn learning(&self) -> Result<LearningSettings> {
        decode(self.0.json(Request::get("/nib/settings/learning")).await?)
    }

    /// `off`, `confirmed_only` or `confirmed_and_auto`, with the bar a signature must clear.
    pub async fn set_learning(
        &self,
        reference_learning: &str,
        learn_min_band_lo: f64,
        learn_min_probability: Option<f64>,
        max_learned: u32,
    ) -> Result<LearningSettings> {
        let mut body = json!({
            "reference_learning": reference_learning,
            "learn_min_band_lo": learn_min_band_lo,
            "max_learned": max_learned,
        });
        if let Some(p) = learn_min_probability {
            body["learn_min_probability"] = p.into();
        }
        let r = Request::new(reqwest::Method::PUT, "/nib/settings/learning").json(body);
        decode(self.0.json(r).await?)
    }
}

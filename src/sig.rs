//! Sig: send documents to be signed, follow them, and download the signed copies (online only).
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::files::{seg, File};
use crate::http::{multipart, Part, Request, Transport};
use crate::model::{amount, decode, list};
use crate::page::Page;
use crate::webhook_endpoints::Webhooks;

/// A document in a package.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SigDocument {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub bytes: u64,
    pub sha256: String,
    pub pages: Option<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// How a signer's signature compared with their references, when the package was checked with
/// Nib.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SigNibCheck {
    pub outcome: String,
    pub tries: u32,
    pub decision: Option<String>,
    pub probability: Option<f64>,
    pub reference: Option<String>,
    pub desk_item: Option<String>,
}

/// Someone who signs a package.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SigSigner {
    pub id: String,
    pub name: String,
    pub email: String,
    pub order: u32,
    pub status: String,
    pub mobile: Option<String>,
    pub role: Option<String>,
    pub sent_at: Option<String>,
    pub viewed_at: Option<String>,
    pub signed_at: Option<String>,
    pub declined_at: Option<String>,
    pub decline_reason: Option<String>,
    pub nib_reference: Option<String>,
    pub nib: Option<SigNibCheck>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SigSigner {
    pub fn signed(&self) -> bool {
        self.status == "signed"
    }
}

/// Something that happened to a package.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SigEvent {
    pub id: String,
    pub kind: String,
    pub at: String,
    pub signer_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Documents and the people who sign them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SigPackage {
    pub id: String,
    pub title: String,
    pub status: String,
    pub expires_at: String,
    pub created_at: String,
    pub documents: Vec<SigDocument>,
    pub signers: Vec<SigSigner>,
    pub reference: Option<String>,
    pub message: Option<String>,
    pub sent_at: Option<String>,
    pub completed_at: Option<String>,
    pub voided_at: Option<String>,
    pub void_reason: Option<String>,
    pub events: Vec<SigEvent>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SigPackage {
    pub fn completed(&self) -> bool {
        self.status == "completed"
    }
}

/// A package as the list shows it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SigPackageSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub signers: u32,
    pub signed: u32,
    pub expires_at: String,
    pub created_at: String,
    pub reference: Option<String>,
    pub completed_at: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Whether a signed copy has changed since it was signed.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SigCopyCheck {
    pub intact: bool,
    pub sha256: String,
    pub sealed_by: Option<String>,
    pub time_stamped_at: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Where signed copies go.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SigSettings {
    pub file_to_folio: bool,
    pub check_with_nib: bool,
    pub folio_folder: String,
    pub folio_class: Option<String>,
    pub folio_sender_chooses: bool,
    pub updated_at: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Documents signed and the charge so far.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SigUsage {
    pub period_start: String,
    pub period_end: String,
    pub packages_sent: u64,
    pub packages_completed: u64,
    pub documents_signed: u64,
    #[serde(deserialize_with = "amount")]
    pub unit_price: String,
    #[serde(deserialize_with = "amount")]
    pub estimated_charge: String,
    pub currency: String,
    pub pricing: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Someone to sign.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Signer {
    pub name: String,
    pub email: String,
    pub mobile: Option<String>,
    /// Signing order, from 1; people with the same number sign at the same time.
    pub order: Option<u32>,
    pub role: Option<String>,
    /// Their reference in Nib, to compare their signature with.
    pub nib_reference: Option<String>,
}

impl Signer {
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
            ..Self::default()
        }
    }

    pub fn order(mut self, order: u32) -> Self {
        self.order = Some(order);
        self
    }

    pub fn mobile(mut self, mobile: impl Into<String>) -> Self {
        self.mobile = Some(mobile.into());
        self
    }

    pub fn role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    pub fn nib_reference(mut self, reference: impl Into<String>) -> Self {
        self.nib_reference = Some(reference.into());
        self
    }

    fn to_json(&self) -> Result<Value> {
        if self.name.trim().is_empty() || self.email.trim().is_empty() {
            return Err(Error::invalid("each signer needs a name and an email"));
        }
        let mut v = json!({ "name": self.name, "email": self.email });
        for (k, x) in [
            ("mobile", &self.mobile),
            ("role", &self.role),
            ("nib_reference", &self.nib_reference),
        ] {
            if let Some(x) = x {
                v[k] = x.clone().into();
            }
        }
        if let Some(o) = self.order {
            v["order"] = o.into();
        }
        Ok(v)
    }
}

/// How a package is made.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageOptions {
    pub title: Option<String>,
    pub message: Option<String>,
    pub reference: Option<String>,
    pub require_location: Option<bool>,
    pub require_camera: Option<bool>,
    pub file_to_folio: Option<bool>,
    pub check_with_nib: Option<bool>,
    pub folio_folder: Option<String>,
    pub folio_class: Option<String>,
    pub expires_in_days: Option<u32>,
    /// Where people sign, when you place it yourself.
    pub fields: Option<Vec<Value>>,
    /// Send it now (the default), or keep it as a draft.
    pub send: bool,
    pub idempotency_key: Option<String>,
}

impl Default for PackageOptions {
    fn default() -> Self {
        Self {
            title: None,
            message: None,
            reference: None,
            require_location: None,
            require_camera: None,
            file_to_folio: None,
            check_with_nib: None,
            folio_folder: None,
            folio_class: None,
            expires_in_days: None,
            fields: None,
            send: true,
            idempotency_key: None,
        }
    }
}

/// Changes to Sig's settings; anything left `None` stays as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsChange {
    pub file_to_folio: Option<bool>,
    pub check_with_nib: Option<bool>,
    pub folio_folder: Option<String>,
    pub folio_class: Option<String>,
    pub folio_sender_chooses: Option<bool>,
}

fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("tif" | "tiff") => "image/tiff",
        _ => "application/octet-stream",
    }
}

/// Sig through the API.
#[derive(Debug, Clone, Copy)]
pub struct Sig<'a>(pub(crate) &'a Transport);

fn pid(package_id: &str) -> Result<String> {
    seg(package_id, "package id")
}

impl<'a> Sig<'a> {
    /// Where Sig tells you a signer viewed, signed or declined, and when signed copies are ready.
    pub fn webhooks(&self) -> Webhooks<'a> {
        Webhooks::new(self.0, "/sig/webhooks")
    }

    /// Upload documents and the people who sign them; with `send: false` it stays a draft.
    pub async fn create_package(
        &self,
        files: Vec<File>,
        signers: &[Signer],
        o: PackageOptions,
    ) -> Result<SigPackage> {
        if files.is_empty() {
            return Err(Error::invalid("a package needs at least one document"));
        }
        if signers.is_empty() {
            return Err(Error::invalid("a package needs at least one signer"));
        }
        let mut req = Map::new();
        req.insert(
            "signers".into(),
            signers
                .iter()
                .map(Signer::to_json)
                .collect::<Result<Vec<_>>>()?
                .into(),
        );
        for (k, v) in [
            ("title", o.title),
            ("message", o.message),
            ("reference", o.reference),
            ("folio_folder", o.folio_folder),
            ("folio_class", o.folio_class),
        ] {
            if let Some(v) = v {
                req.insert(k.into(), v.into());
            }
        }
        for (k, v) in [
            ("require_location", o.require_location),
            ("require_camera", o.require_camera),
            ("file_to_folio", o.file_to_folio),
            ("check_with_nib", o.check_with_nib),
        ] {
            if let Some(v) = v {
                req.insert(k.into(), v.into());
            }
        }
        if let Some(d) = o.expires_in_days {
            req.insert("expires_in_days".into(), d.into());
        }
        if let Some(f) = o.fields {
            req.insert("fields".into(), f.into());
        }
        req.insert("send".into(), o.send.into());
        let mut parts = vec![Part::json("package", &Value::Object(req))];
        for f in files {
            if matches!(f, File::Url(_) | File::PartName(_)) {
                return Err(Error::invalid(
                    "Sig takes the documents themselves, not a URL",
                ));
            }
            let (name, data) = f.read().await?;
            let name = name.ok_or_else(|| {
                Error::invalid(
                    "give bytes a file name (File::named) so the signer sees what the document is called",
                )
            })?;
            parts.push(Part {
                name: "file".into(),
                content_type: content_type(&name).into(),
                filename: Some(name),
                data,
            });
        }
        let (ct, body) = multipart(parts);
        let mut r = Request::post("/sig/packages").raw(ct, body);
        if let Some(k) = o.idempotency_key {
            r = r.idempotency_key(k);
        }
        decode(self.0.json(r).await?)
    }

    /// One page of packages, newest first.
    pub async fn list_packages(
        &self,
        status: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Page<SigPackageSummary>> {
        let r = Request::get("/sig/packages")
            .opt("status", status)
            .opt("cursor", cursor)
            .opt("limit", limit);
        let v = self.0.json(r).await?;
        Ok(Page {
            items: list(&v, "packages")?,
            next_cursor: crate::json::s(&v, "next_cursor"),
        })
    }

    pub async fn get_package(&self, package_id: &str) -> Result<SigPackage> {
        decode(
            self.0
                .json(Request::get(format!("/sig/packages/{}", pid(package_id)?)))
                .await?,
        )
    }

    pub async fn send_package(&self, package_id: &str) -> Result<SigPackage> {
        let r = Request::post(format!("/sig/packages/{}/send", pid(package_id)?));
        decode(self.0.json(r).await?)
    }

    /// Email everyone who still has to sign.
    pub async fn remind(&self, package_id: &str) -> Result<SigPackage> {
        let r = Request::post(format!("/sig/packages/{}/remind", pid(package_id)?));
        decode(self.0.json(r).await?)
    }

    pub async fn void_package(&self, package_id: &str, reason: Option<&str>) -> Result<SigPackage> {
        let body = match reason {
            Some(r) => json!({ "reason": r }),
            None => json!({}),
        };
        let r = Request::post(format!("/sig/packages/{}/void", pid(package_id)?)).json(body);
        decode(self.0.json(r).await?)
    }

    /// Replace where people sign on a draft.
    pub async fn set_fields(&self, package_id: &str, fields: Vec<Value>) -> Result<SigPackage> {
        let r = Request::new(
            reqwest::Method::PUT,
            format!("/sig/packages/{}/fields", pid(package_id)?),
        )
        .json(json!({ "fields": fields }));
        decode(self.0.json(r).await?)
    }

    /// Correct a signer's name, email or mobile.
    pub async fn correct_signer(
        &self,
        package_id: &str,
        signer_id: &str,
        name: Option<&str>,
        email: Option<&str>,
        mobile: Option<&str>,
    ) -> Result<SigPackage> {
        let mut body = Map::new();
        for (k, v) in [("name", name), ("email", email), ("mobile", mobile)] {
            if let Some(v) = v {
                body.insert(k.into(), v.into());
            }
        }
        if body.is_empty() {
            return Err(Error::invalid("pass a name, email or mobile to correct"));
        }
        let r = Request::new(
            reqwest::Method::PATCH,
            format!(
                "/sig/packages/{}/signers/{}",
                pid(package_id)?,
                seg(signer_id, "signer id")?
            ),
        )
        .json(Value::Object(body));
        decode(self.0.json(r).await?)
    }

    async fn pdf(&self, path: String) -> Result<Vec<u8>> {
        self.0
            .bytes(Request::get(path).accept("application/pdf"))
            .await
    }

    /// The document as it was uploaded.
    pub async fn download_document(&self, package_id: &str, document_id: &str) -> Result<Vec<u8>> {
        self.pdf(format!(
            "/sig/packages/{}/documents/{}/content",
            pid(package_id)?,
            seg(document_id, "document id")?
        ))
        .await
    }

    /// The signed copy, once everyone has signed.
    pub async fn download_signed(&self, package_id: &str, document_id: &str) -> Result<Vec<u8>> {
        self.pdf(format!(
            "/sig/packages/{}/documents/{}/signed",
            pid(package_id)?,
            seg(document_id, "document id")?
        ))
        .await
    }

    pub async fn download_certificate(&self, package_id: &str) -> Result<Vec<u8>> {
        self.pdf(format!("/sig/packages/{}/certificate", pid(package_id)?))
            .await
    }

    /// Whether a signed copy has changed since it was signed.
    pub async fn check_copy(&self, pdf: impl Into<File>) -> Result<SigCopyCheck> {
        let (_, data) = pdf.into().read().await?;
        let r = Request::post("/sig/check").raw("application/pdf".into(), data);
        decode(self.0.json(r).await?)
    }

    pub async fn settings(&self) -> Result<SigSettings> {
        decode(self.0.json(Request::get("/sig/settings")).await?)
    }

    /// Change some settings; anything left `None` stays as it is.
    pub async fn set_settings(&self, c: SettingsChange) -> Result<SigSettings> {
        let now = self.settings().await?;
        let mut body = json!({
            "file_to_folio": c.file_to_folio.unwrap_or(now.file_to_folio),
            "check_with_nib": c.check_with_nib.unwrap_or(now.check_with_nib),
            "folio_folder": c.folio_folder.unwrap_or(now.folio_folder),
            "folio_sender_chooses": c.folio_sender_chooses.unwrap_or(now.folio_sender_chooses),
        });
        if let Some(cls) = c.folio_class.or(now.folio_class).filter(|c| !c.is_empty()) {
            body["folio_class"] = cls.into();
        }
        let r = Request::new(reqwest::Method::PUT, "/sig/settings").json(body);
        decode(self.0.json(r).await?)
    }

    /// Documents signed and the charge so far, for `current` or a month like `2026-10`.
    pub async fn usage(&self, period: &str) -> Result<SigUsage> {
        decode(
            self.0
                .json(Request::get("/sig/usage").param("period", period))
                .await?,
        )
    }
}

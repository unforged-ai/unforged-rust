//! Folio: file documents, keep them for as long as they must be kept, and read them back with a
//! reason where your organisation asks for one (online only).
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::files::{seg, File};
use crate::http::{Request, Response, Transport};
use crate::model::{decode, list};
use crate::page::Page;
use crate::webhook_endpoints::Webhooks;

const MAX_DIRECT_BYTES: usize = 50 * 1024 * 1024;

/// What a filed document comes back with.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Receipt {
    pub record_id: String,
    pub sha256: String,
    pub size: Option<u64>,
    #[serde(alias = "class")]
    pub class_id: Option<String>,
    pub lock: Option<String>,
    pub earliest_delete: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A record as a listing or a search shows it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RecordSummary {
    pub id: String,
    pub folder: String,
    pub name: String,
    pub size: u64,
    #[serde(alias = "class")]
    pub class_id: String,
    pub lock: String,
    pub earliest_delete: Option<String>,
    pub filed_at_ms: Option<i64>,
    pub archived: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl RecordSummary {
    /// The folder and name together.
    pub fn path(&self) -> String {
        join(&self.folder, &self.name)
    }
}

/// One record in full: where it is, how long it's kept, and its receipt.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Record {
    pub id: String,
    pub folder: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    #[serde(alias = "class")]
    pub class_id: String,
    pub class_version: Option<u32>,
    pub lock: String,
    pub earliest_delete: Option<String>,
    pub state: String,
    pub matter: Option<String>,
    pub subject: Option<String>,
    pub filed_at_ms: Option<i64>,
    pub nib_verdict: Option<String>,
    pub attachments: Vec<Value>,
    pub receipt: Value,
    pub storage: Value,
    pub filed_by: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Record {
    pub fn path(&self) -> String {
        join(&self.folder, &self.name)
    }

    /// False while the document is archived and needs restoring first.
    pub fn readable_now(&self) -> bool {
        self.storage
            .get("readable_now")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

/// A document waiting to be filed.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct QueuedFiling {
    pub id: String,
    pub state: String,
    pub folder: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub record_id: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl QueuedFiling {
    /// Filed, refused or failed: no longer waiting.
    pub fn done(&self) -> bool {
        self.state != "queued"
    }
}

/// An open reason for looking at records.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Reason {
    pub id: String,
    pub code: String,
    pub text: Option<String>,
    pub matter: Option<String>,
    pub context: Value,
    pub opened_at_ms: Option<i64>,
    pub expires_at_ms: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Records due to end their retention soon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RetentionDue {
    pub next_7_days: u64,
    pub next_30_days: u64,
    pub past_end: u64,
}

/// What needs looking at, as counts.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Attention {
    pub records_held: u64,
    pub pending_approvals: u64,
    pub my_pending_requests: u64,
    pub active_holds: u64,
    pub retention_due: RetentionDue,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Whether Folio is ready for you, and where your account stands.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Standing {
    pub state: String,
    pub overdue_since: Option<String>,
    pub suspends_on: Option<String>,
    pub setup: Option<String>,
    pub grant: Option<Value>,
    pub grant_requested: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Standing {
    pub fn ready(&self) -> bool {
        matches!(self.setup.as_deref(), None | Some("ready"))
    }
}

/// A legal hold.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Hold {
    pub id: String,
    pub scope: Value,
    pub reason_code: String,
    pub reason_text: Option<String>,
    pub placed_at_ms: Option<i64>,
    pub released_at_ms: Option<i64>,
    pub records_covered: Option<u64>,
    pub placed_by: Option<String>,
    pub released_by: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Hold {
    pub fn active(&self) -> bool {
        self.released_at_ms.is_none()
    }
}

/// A change that waits for a second person.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Approval {
    pub id: String,
    pub action: String,
    pub state: String,
    pub target: Value,
    pub request_code: Option<String>,
    pub request_text: Option<String>,
    pub requested_at_ms: Option<i64>,
    pub expires_at_ms: Option<i64>,
    pub decided_at_ms: Option<i64>,
    pub result: Value,
    pub mine: Option<bool>,
    pub requested_by: Option<String>,
    pub approved_by: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Approval {
    pub fn pending(&self) -> bool {
        self.state == "pending"
    }

    pub fn approved(&self) -> bool {
        self.state == "done"
    }
}

/// A change made straight away, or one waiting for a colleague's approval.
#[derive(Debug, Clone, PartialEq)]
pub enum Change<T> {
    Done(Box<T>),
    Pending(Box<Approval>),
}

/// A record's new end date.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RetentionChange {
    pub record_id: String,
    pub earliest_delete: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A record moved from one folder to another.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Move {
    pub record_id: String,
    #[serde(rename = "from")]
    pub from_folder: String,
    #[serde(rename = "to")]
    pub to_folder: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// How long a kind of record is kept, and what happens at the end.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RetentionClass {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub period: Value,
    pub start: String,
    pub lock: String,
    pub profile: String,
    pub end_action: Value,
    pub version: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What applying a class's latest version to its older records would change.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ExtensionPreview {
    pub class_id: String,
    pub to_version: u32,
    pub records: u64,
    pub bytes: u64,
    pub earliest_new_end: Option<String>,
    pub latest_new_end: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One ledger entry, carrying the hash of the entry before it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct LedgerEvent {
    pub seq: u64,
    pub at_ms: Option<i64>,
    pub action: String,
    pub target: Value,
    pub reason_code: Option<String>,
    pub reason_text: Option<String>,
    pub via: Option<String>,
    pub hash: String,
    pub prev: Option<String>,
    pub head: Option<Value>,
    pub actor: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A public key that signed your ledger's heads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct HeadKey {
    pub key_id: String,
    pub public_key: String,
    pub region: String,
}

/// One page of the ledger, newest first, with the keys that signed its heads.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct LedgerPage {
    #[serde(rename = "entries")]
    pub events: Vec<LedgerEvent>,
    pub head_keys: Vec<HeadKey>,
    pub next_before: Option<i64>,
}

/// Events on one day.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ActivityDay {
    pub day: String,
    pub count: u64,
}

/// One person's or one key's events per day, and their first and last.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Activity {
    pub days: Vec<ActivityDay>,
    pub total: u64,
    pub first_at_ms: Option<i64>,
    pub last_at_ms: Option<i64>,
}

/// Part of a snippet, and whether it's what matched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SnippetPart {
    pub text: String,
    pub hit: bool,
}

/// Text around a match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Snippet {
    pub page: u32,
    pub parts: Vec<SnippetPart>,
}

impl Snippet {
    pub fn text(&self) -> String {
        self.parts.iter().map(|p| p.text.as_str()).collect()
    }

    /// The snippet with each match wrapped, as `the [lease] ends`.
    pub fn marked(&self, before: &str, after: &str) -> String {
        self.parts
            .iter()
            .map(|p| {
                if p.hit {
                    format!("{before}{}{after}", p.text)
                } else {
                    p.text.clone()
                }
            })
            .collect()
    }
}

/// A record that matched, with its pages and snippets in a content search.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SearchHit {
    pub pages: Vec<u32>,
    pub snippets: Option<Vec<Snippet>>,
    pub low_confidence: bool,
    #[serde(flatten)]
    pub record: RecordSummary,
}

/// What a search found; `not_searchable` counts records whose text isn't read yet.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SearchResult {
    pub scope: String,
    #[serde(rename = "records")]
    pub hits: Vec<SearchHit>,
    pub matched_records: Option<u64>,
    pub matched_pages: Option<u64>,
    pub not_searchable: Option<u64>,
    pub low_confidence: Option<u64>,
    pub snippets: Option<bool>,
}

impl IntoIterator for SearchResult {
    type Item = SearchHit;
    type IntoIter = std::vec::IntoIter<SearchHit>;

    fn into_iter(self) -> Self::IntoIter {
        self.hits.into_iter()
    }
}

/// One of a subject's records, and whether it could be erased now.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ErasureItem {
    pub record_id: String,
    pub standing: String,
    pub earliest_delete: Option<String>,
    pub explanation: Option<String>,
    pub holds: Vec<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What erasing a subject's records would and wouldn't touch, and why.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ErasureAssessment {
    pub subject: String,
    pub items: Vec<ErasureItem>,
}

/// A folder.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Folder {
    pub path: String,
    pub matter: Option<String>,
    pub subject: Option<String>,
    pub protected: bool,
    pub created_at_ms: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A fresh check of the stored bytes against the receipt.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Integrity {
    pub record_id: String,
    pub result: String,
    pub expected: String,
    pub found: Option<String>,
    pub checked_at_ms: Option<i64>,
    pub note: Option<String>,
}

impl Integrity {
    pub fn ok(&self) -> bool {
        self.result == "ok"
    }
}

/// Everything that has happened to a record since it was filed, with a fresh integrity check.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CustodyReport {
    pub record_id: String,
    pub folder: String,
    pub name: String,
    pub sha256: String,
    pub events: Vec<LedgerEvent>,
    pub integrity: Integrity,
    pub note: String,
}

/// Why a record is being opened: an open [`Reason`], or one of your organisation's codes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AccessReason {
    Open(String),
    Code { code: String, text: Option<String> },
}

impl AccessReason {
    /// A reason opened with `reasons().open(..)`.
    pub fn open(reason: &Reason) -> Self {
        AccessReason::Open(reason.id.clone())
    }

    /// One of your organisation's reason codes.
    pub fn code(code: impl Into<String>) -> Self {
        AccessReason::Code {
            code: code.into(),
            text: None,
        }
    }

    /// A few words to go with the code, in plain ASCII on one line.
    pub fn text(self, text: impl Into<String>) -> Self {
        match self {
            AccessReason::Code { code, .. } => AccessReason::Code {
                code,
                text: Some(text.into()),
            },
            other => other,
        }
    }

    fn apply(&self, r: Request) -> Result<Request> {
        Ok(match self {
            AccessReason::Open(id) => r.header("unforged-reason-id", id.clone()),
            AccessReason::Code { code, text } => {
                let r = r.header("unforged-reason-code", code.clone());
                match text.as_deref().filter(|t| !t.is_empty()) {
                    None => r,
                    Some(t) if t.is_ascii() && !t.contains(['\r', '\n']) => {
                        r.header("unforged-reason", t)
                    }
                    Some(_) => {
                        return Err(Error::invalid(
                            "the reason's text travels in a header, so keep it to plain ASCII on one line; for anything longer, open a reason with reasons().open() and pass it",
                        ))
                    }
                }
            }
        })
    }
}

impl From<&Reason> for AccessReason {
    fn from(r: &Reason) -> Self {
        AccessReason::open(r)
    }
}

/// Why a change is being made: one of your organisation's reason codes, with optional words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Why {
    pub code: String,
    pub text: Option<String>,
}

impl Why {
    pub fn code(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            text: None,
        }
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    fn fields(&self) -> Result<Map<String, Value>> {
        if self.code.is_empty() {
            return Err(Error::invalid(
                "say why, with one of your organisation's reason codes",
            ));
        }
        let mut m = Map::new();
        m.insert("code".into(), json!(self.code));
        m.insert("text".into(), json!(self.text));
        Ok(m)
    }
}

/// What a hold or a reason covers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scope {
    Record(String),
    Folder(String),
    Matter(String),
    Subject(String),
    Records(Vec<String>),
}

impl Scope {
    fn to_json(&self) -> Value {
        let (kind, value) = match self {
            Scope::Record(v) => ("record", json!(v)),
            Scope::Folder(v) => ("folder", json!(v)),
            Scope::Matter(v) => ("matter", json!(v)),
            Scope::Subject(v) => ("subject", json!(v)),
            Scope::Records(v) => ("records", json!(v)),
        };
        json!({ "kind": kind, "value": value })
    }
}

/// Where a document is filed and what's kept with it.
#[derive(Debug, Clone, Default)]
pub struct FilingOptions {
    pub name: Option<String>,
    pub folder: String,
    pub retention_class: Option<String>,
    pub matter: Option<String>,
    pub subject: Option<String>,
    pub source_system: Option<String>,
    pub source_ref: Option<String>,
    pub original_date: Option<String>,
    pub idempotency_key: Option<String>,
}

impl FilingOptions {
    pub fn folder(folder: impl Into<String>) -> Self {
        Self {
            folder: folder.into(),
            ..Self::default()
        }
    }

    /// What to call the file; its own file name by default.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn retention_class(mut self, class_id: impl Into<String>) -> Self {
        self.retention_class = Some(class_id.into());
        self
    }

    pub fn matter(mut self, matter: impl Into<String>) -> Self {
        self.matter = Some(matter.into());
        self
    }

    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// The system the document came from, and its reference there.
    pub fn source(mut self, system: impl Into<String>, reference: Option<String>) -> Self {
        self.source_system = Some(system.into());
        self.source_ref = reference;
        self
    }

    /// The document's own date, like `2024-03-31` (with a source).
    pub fn original_date(mut self, day: impl Into<String>) -> Self {
        self.original_date = Some(day.into());
        self
    }

    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    fn request(&self, path: &str, own_name: Option<String>, data: Vec<u8>) -> Result<Request> {
        let name = self
            .name
            .clone()
            .or(own_name)
            .ok_or_else(|| Error::invalid("give the file a name to be filed under"))?;
        if (self.source_ref.is_some() || self.original_date.is_some())
            && self.source_system.is_none()
        {
            return Err(Error::invalid(
                "a source reference and original date go with a source system",
            ));
        }
        let original_date = match &self.original_date {
            Some(d) => Some(day(d, "the original date")?),
            None => None,
        };
        let mut r = Request::post(path)
            .param("folder", &self.folder)
            .param("name", name)
            .opt("class", self.retention_class.as_ref())
            .opt("matter", self.matter.as_ref())
            .opt("subject", self.subject.as_ref())
            .opt("source_system", self.source_system.as_ref())
            .opt("source_ref", self.source_ref.as_ref())
            .opt("original_date", original_date)
            .raw("application/octet-stream".into(), data);
        if let Some(k) = &self.idempotency_key {
            r = r.idempotency_key(k);
        }
        Ok(r)
    }
}

fn day<'a>(s: &'a str, what: &str) -> Result<&'a str> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().ok();
    let ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && num(5..7).is_some_and(|m| (1..=12).contains(&m))
        && num(8..10).is_some_and(|d| (1..=31).contains(&d));
    if ok {
        Ok(s)
    } else {
        Err(Error::invalid(format!(
            "{what} must be a date, like 2031-03-31"
        )))
    }
}

fn delete(path: String) -> Request {
    Request::new(Method::DELETE, path)
}

fn rid(record_id: &str) -> Result<String> {
    Ok(format!("/folio/records/{}", seg(record_id, "record id")?))
}

/// Folio through the API.
#[derive(Debug, Clone, Copy)]
pub struct Folio<'a>(pub(crate) &'a Transport);

impl<'a> Folio<'a> {
    pub fn records(&self) -> Records<'a> {
        Records(self.0)
    }

    /// Queued filing, for big batches.
    pub fn filings(&self) -> Filings<'a> {
        Filings(self.0)
    }

    /// Reasons for opening records, given once for a folder, matter, subject or set of records.
    pub fn reasons(&self) -> Reasons<'a> {
        Reasons(self.0)
    }

    /// Legal holds: nothing in a hold's scope is deleted until it's released.
    pub fn holds(&self) -> Holds<'a> {
        Holds(self.0)
    }

    /// Changes waiting for a second person.
    pub fn approvals(&self) -> Approvals<'a> {
        Approvals(self.0)
    }

    pub fn classes(&self) -> Classes<'a> {
        Classes(self.0)
    }

    pub fn folders(&self) -> Folders<'a> {
        Folders(self.0)
    }

    /// Every filing, view, change and refusal, chained by hash.
    pub fn ledger(&self) -> Ledger<'a> {
        Ledger(self.0)
    }

    /// Requests to erase a person's records.
    pub fn erasure(&self) -> Erasure<'a> {
        Erasure(self.0)
    }

    pub fn webhooks(&self) -> Webhooks<'a> {
        Webhooks::new(self.0, "/folio/webhooks")
    }

    /// Find records by name, or with `content` set by the words inside them.
    pub async fn search(&self, q: &str, o: SearchOptions) -> Result<SearchResult> {
        let narrows = o.matter.is_some()
            || o.subject.is_some()
            || o.retention_class.is_some()
            || o.lock.is_some()
            || o.since_ms.is_some()
            || o.until_ms.is_some()
            || o.held.is_some();
        if !o.content && narrows {
            return Err(Error::invalid(
                "matter, subject, class, lock, dates and held narrow a search inside documents; set content with them",
            ));
        }
        let r = Request::get("/folio/search")
            .param("q", q)
            .param("scope", if o.content { "content" } else { "names" })
            .param("folder", &o.folder)
            .opt("matter", o.matter)
            .opt("subject", o.subject)
            .opt("class", o.retention_class)
            .opt("lock", o.lock)
            .opt("from", o.since_ms)
            .opt("to", o.until_ms)
            .opt("held", o.held)
            .opt("limit", o.limit);
        decode(self.0.json(r).await?)
    }

    /// What needs looking at, as counts.
    pub async fn attention(&self) -> Result<Attention> {
        decode(self.0.json(Request::get("/folio/attention")).await?)
    }

    /// Whether Folio is ready for you, and where your account stands.
    pub async fn standing(&self) -> Result<Standing> {
        decode(self.0.json(Request::get("/folio/standing")).await?)
    }
}

/// What to search for, and where.
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Search the words inside documents, not only their names.
    pub content: bool,
    pub folder: String,
    pub matter: Option<String>,
    pub subject: Option<String>,
    pub retention_class: Option<String>,
    pub lock: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub held: Option<bool>,
    pub limit: Option<u32>,
}

impl SearchOptions {
    pub fn names() -> Self {
        Self::default()
    }

    pub fn content() -> Self {
        Self {
            content: true,
            ..Self::default()
        }
    }

    pub fn folder(mut self, folder: impl Into<String>) -> Self {
        self.folder = folder.into();
        self
    }
}

/// How to read a record back.
#[derive(Debug, Clone, Default)]
pub struct DownloadOptions {
    pub reason: Option<AccessReason>,
    /// The first byte and, optionally, the last.
    pub range: Option<(u64, Option<u64>)>,
}

impl DownloadOptions {
    pub fn reason(reason: impl Into<AccessReason>) -> Self {
        Self {
            reason: Some(reason.into()),
            range: None,
        }
    }
}

/// Filing documents, reading them back, and the changes a record allows.
#[derive(Debug, Clone, Copy)]
pub struct Records<'a>(&'a Transport);

impl Records<'_> {
    /// File a document and return its receipt once it's safely kept.
    pub async fn file(&self, file: impl Into<File>, o: FilingOptions) -> Result<Receipt> {
        let (own_name, data) = file.into().read().await?;
        if data.len() > MAX_DIRECT_BYTES {
            return Err(Error::invalid(
                "this file is over 50 MB, the most we take in one request; get in touch and we'll set you up for larger files",
            ));
        }
        decode(
            self.0
                .json(o.request("/folio/records", own_name, data)?)
                .await?,
        )
    }

    /// File the document from a signature check, with the check's result kept alongside it.
    pub async fn file_from_check(
        &self,
        job_id: &str,
        item_index: u32,
        o: FilingOptions,
    ) -> Result<Receipt> {
        if job_id.is_empty() {
            return Err(Error::invalid("pass the check's job id"));
        }
        let mut body = Map::new();
        body.insert("job_id".into(), json!(job_id));
        body.insert("item_index".into(), json!(item_index));
        if !o.folder.is_empty() {
            body.insert("folder".into(), json!(o.folder));
        }
        for (k, v) in [
            ("name", &o.name),
            ("class", &o.retention_class),
            ("matter", &o.matter),
            ("subject", &o.subject),
        ] {
            if let Some(v) = v {
                body.insert(k.into(), json!(v));
            }
        }
        let mut r = Request::post("/folio/records/from-check").json(Value::Object(body));
        if let Some(k) = &o.idempotency_key {
            r = r.idempotency_key(k);
        }
        decode(self.0.json(r).await?)
    }

    /// One page of the records in a folder (and the folders below it, with `recursive`).
    pub async fn list(
        &self,
        folder: &str,
        recursive: bool,
        after: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Page<RecordSummary>> {
        let r = Request::get("/folio/records")
            .param("folder", folder)
            .param("recursive", recursive)
            .opt("after", after)
            .opt("limit", limit);
        let v = self.0.json(r).await?;
        Ok(Page {
            items: list(&v, "records")?,
            next_cursor: crate::json::s(&v, "next_after"),
        })
    }

    /// A record's details and receipt, with a reason where opening it needs one.
    pub async fn get(&self, record_id: &str, reason: Option<AccessReason>) -> Result<Record> {
        let mut r = Request::get(rid(record_id)?);
        if let Some(why) = &reason {
            r = why.apply(r)?;
        }
        decode(self.0.json(r).await?)
    }

    async fn content(&self, record_id: &str, o: &DownloadOptions) -> Result<Response> {
        let mut r =
            Request::get(format!("{}/content", rid(record_id)?)).accept("application/octet-stream");
        if let Some(why) = &o.reason {
            r = why.apply(r)?;
        }
        if let Some((first, last)) = o.range {
            let last = last.map(|l| l.to_string()).unwrap_or_default();
            r = r.header("range", format!("bytes={first}-{last}"));
        }
        let resp = self.0.send(r, &[]).await?;
        verify(&resp, record_id)?;
        Ok(resp)
    }

    /// The document's bytes, checked against its receipt.
    pub async fn download(&self, record_id: &str, o: DownloadOptions) -> Result<Vec<u8>> {
        Ok(self.content(record_id, &o).await?.body)
    }

    /// Save the document to `to` (a file, or a folder to keep its own name in), checked against
    /// its receipt first. Returns where it was saved.
    pub async fn download_to(
        &self,
        record_id: &str,
        to: impl AsRef<Path>,
        o: DownloadOptions,
    ) -> Result<PathBuf> {
        let resp = self.content(record_id, &o).await?;
        let to = to.as_ref();
        let target = if to.is_dir() {
            let name = resp
                .headers
                .get("content-disposition")
                .and_then(|h| h.to_str().ok())
                .and_then(filename)
                .unwrap_or_else(|| record_id.to_string());
            to.join(name)
        } else {
            to.to_path_buf()
        };
        let dir = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let body = resp.body;
        let saved = target.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            use std::io::Write;
            let mut tmp = tempfile::Builder::new()
                .prefix(".unforged-")
                .tempfile_in(&dir)?;
            tmp.write_all(&body)?;
            tmp.persist(&saved).map_err(|e| Error::Io(e.error))?;
            Ok(())
        })
        .await
        .map_err(|_| Error::Io(std::io::Error::other("saving the download stopped")))??;
        Ok(target)
    }

    /// Move a record to another folder; leaving a protected folder waits for approval.
    pub async fn move_to(&self, record_id: &str, folder: &str, why: Why) -> Result<Change<Move>> {
        let mut body = why.fields()?;
        body.insert("folder".into(), json!(folder));
        let r = Request::post(format!("{}/move", rid(record_id)?)).json(Value::Object(body));
        change(self.0.send(r, &[]).await?)
    }

    /// Keep a record until at least `until` (like `2031-03-31`); retention only ever moves later.
    pub async fn extend_retention(
        &self,
        record_id: &str,
        until: &str,
        why: Why,
    ) -> Result<RetentionChange> {
        let mut body = why.fields()?;
        body.insert("earliest_delete".into(), json!(day(until, "until")?));
        let r = Request::post(format!("{}/retention", rid(record_id)?)).json(Value::Object(body));
        decode(self.0.json(r).await?)
    }

    /// Ask to delete a record before its retention ends; a colleague must approve.
    pub async fn request_early_delete(&self, record_id: &str, why: Why) -> Result<Approval> {
        let r = Request::post(format!("{}/early-delete", rid(record_id)?))
            .json(Value::Object(why.fields()?));
        decode(self.0.json(r).await?)
    }

    /// Everything that has happened to the record since it was filed, with a fresh integrity
    /// check.
    pub async fn custody(&self, record_id: &str) -> Result<CustodyReport> {
        let r = Request::get(format!("{}/custody", rid(record_id)?));
        decode(self.0.json(r).await?)
    }

    /// Read the stored document now and compare it with its receipt.
    pub async fn check_integrity(&self, record_id: &str) -> Result<Integrity> {
        let v = self
            .0
            .json(Request::get(format!("{}/integrity", rid(record_id)?)))
            .await?;
        let mut i: Integrity = decode(v.get("integrity").cloned().unwrap_or(json!({})))?;
        i.note = crate::json::s(&v, "note");
        Ok(i)
    }
}

fn change<T: serde::de::DeserializeOwned>(resp: Response) -> Result<Change<T>> {
    let v = resp.json()?;
    Ok(if resp.status == 202 {
        Change::Pending(Box::new(decode(v)?))
    } else {
        Change::Done(Box::new(decode(v)?))
    })
}

fn verify(resp: &Response, record_id: &str) -> Result<()> {
    if resp.status != 200 {
        return Ok(());
    }
    let digest = resp
        .headers
        .get("digest")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let want = match digest.split_once('=') {
        Some((alg, v)) if alg.eq_ignore_ascii_case("sha-256") => v.trim().to_ascii_lowercase(),
        _ => return Ok(()),
    };
    let got: String = Sha256::digest(&resp.body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !want.is_empty() && got != want {
        return Err(Error::Integrity {
            message: format!(
                "the copy of {record_id} we received doesn't match its receipt, so it wasn't kept. Please try again, and if it happens twice, let us know"
            ),
            record_id: record_id.to_string(),
        });
    }
    Ok(())
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn filename(disposition: &str) -> Option<String> {
    let mut plain = None;
    let mut extended = None;
    for part in disposition.split(';').map(str::trim) {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        match k.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                let v = v.trim();
                let encoded = v
                    .get(..7)
                    .filter(|p| p.eq_ignore_ascii_case("utf-8''"))
                    .map(|_| &v[7..]);
                extended = encoded.and_then(percent_decode);
            }
            "filename" => {
                let v = v.trim();
                let v = v
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(v);
                plain = Some(v.replace("\\\"", "\""));
            }
            _ => {}
        }
    }
    let name = extended.or(plain)?.replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or("").trim().to_string();
    if name.is_empty() || name == "." || name == ".." {
        None
    } else {
        Some(name)
    }
}

/// Queued filing, for big batches.
#[derive(Debug, Clone, Copy)]
pub struct Filings<'a>(&'a Transport);

impl Filings<'_> {
    /// Hand over a document to be filed shortly.
    pub async fn queue(&self, file: impl Into<File>, o: FilingOptions) -> Result<QueuedFiling> {
        let (own_name, data) = file.into().read().await?;
        decode(
            self.0
                .json(o.request("/folio/filings", own_name, data)?)
                .await?,
        )
    }

    pub async fn get(&self, filing_id: &str) -> Result<QueuedFiling> {
        let r = Request::get(format!("/folio/filings/{}", seg(filing_id, "filing id")?));
        decode(self.0.json(r).await?)
    }

    /// Wait until a queued filing has been filed, refused or failed.
    pub async fn wait(&self, filing_id: &str, timeout: Option<Duration>) -> Result<QueuedFiling> {
        let timeout = timeout.unwrap_or(Duration::from_secs(300));
        let deadline = Instant::now() + timeout;
        let mut delay = self.0.poll_interval();
        loop {
            let f = self.get(filing_id).await?;
            if f.done() {
                return Ok(f);
            }
            if Instant::now() + delay > deadline {
                return Err(Error::CheckTimeout {
                    message: format!(
                        "{} is still waiting to be filed after {}s; it carries on, so check on it again later",
                        f.name,
                        timeout.as_secs()
                    ),
                    job_id: Some(filing_id.to_string()),
                });
            }
            tokio::time::sleep(delay).await;
            delay = (delay.mul_f64(1.5)).min(Duration::from_secs(5));
        }
    }

    /// Wait for several queued filings, each one's outcome in the same order.
    pub async fn wait_all(
        &self,
        filing_ids: &[&str],
        timeout: Option<Duration>,
    ) -> Result<Vec<QueuedFiling>> {
        let deadline = Instant::now() + timeout.unwrap_or(Duration::from_secs(600));
        let mut out = Vec::with_capacity(filing_ids.len());
        for id in filing_ids {
            let left = deadline.saturating_duration_since(Instant::now());
            out.push(self.wait(id, Some(left)).await?);
        }
        Ok(out)
    }
}

/// Reasons for opening records.
#[derive(Debug, Clone, Copy)]
pub struct Reasons<'a>(&'a Transport);

impl Reasons<'_> {
    /// Open a reason for a folder, matter, subject or set of records; pass it to reads with
    /// [`AccessReason::open`], then end it.
    pub async fn open(
        &self,
        code: &str,
        scope: Scope,
        text: Option<&str>,
        seconds: Option<u32>,
    ) -> Result<Reason> {
        if matches!(scope, Scope::Record(_)) {
            return Err(Error::invalid(
                "a reason covers a folder, matter, subject or set of records; for one record, use Scope::Records",
            ));
        }
        let mut body = json!({ "code": code, "text": text, "context": scope.to_json() });
        if let Some(s) = seconds {
            body["seconds"] = json!(s);
        }
        decode(
            self.0
                .json(Request::post("/folio/reasons").json(body))
                .await?,
        )
    }

    /// End a reason now.
    pub async fn end(&self, reason_id: &str) -> Result<()> {
        let r = delete(format!("/folio/reasons/{}", seg(reason_id, "reason")?));
        self.0.send(r, &[]).await.map(drop)
    }
}

/// Legal holds.
#[derive(Debug, Clone, Copy)]
pub struct Holds<'a>(&'a Transport);

impl Holds<'_> {
    /// Place a hold on one record, a folder (and everything below it), a matter or a subject.
    pub async fn place(&self, scope: Scope, why: Why) -> Result<Hold> {
        if matches!(scope, Scope::Records(_)) {
            return Err(Error::invalid(
                "a hold covers one record, a folder, a matter or a subject",
            ));
        }
        let mut body = why.fields()?;
        body.insert("scope".into(), scope.to_json());
        let r = Request::post("/folio/holds").json(Value::Object(body));
        decode(self.0.json(r).await?)
    }

    /// Holds, newest first.
    pub async fn list(&self, include_released: bool) -> Result<Vec<Hold>> {
        let r = Request::get("/folio/holds").param("include_released", include_released);
        list(&self.0.json(r).await?, "holds")
    }

    /// Ask to release a hold; it stays until a colleague approves.
    pub async fn request_release(&self, hold_id: &str, why: Why) -> Result<Approval> {
        let r = Request::post(format!("/folio/holds/{}/release", seg(hold_id, "hold")?))
            .json(Value::Object(why.fields()?));
        decode(self.0.json(r).await?)
    }
}

/// Changes waiting for a second person.
#[derive(Debug, Clone, Copy)]
pub struct Approvals<'a>(&'a Transport);

impl Approvals<'_> {
    /// One page of requests, newest first, by `whose` (`mine`, `others`, `all`) and `state`.
    pub async fn list(
        &self,
        whose: &str,
        state: &str,
        before: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Page<Approval>> {
        let r = Request::get("/folio/approvals")
            .param("whose", whose)
            .param("state", state)
            .opt("before", before)
            .opt("limit", limit);
        let v = self.0.json(r).await?;
        Ok(Page {
            items: list(&v, "approvals")?,
            next_cursor: crate::json::s(&v, "next_before"),
        })
    }

    /// Approve a request; what it asked for is carried out straight away.
    pub async fn approve(&self, approval_id: &str, why: Why) -> Result<Approval> {
        self.decide(approval_id, "approve", why).await
    }

    /// Refuse a request; nothing it asked for happens.
    pub async fn refuse(&self, approval_id: &str, why: Why) -> Result<Approval> {
        self.decide(approval_id, "refuse", why).await
    }

    async fn decide(&self, approval_id: &str, verb: &str, why: Why) -> Result<Approval> {
        let r = Request::post(format!(
            "/folio/approvals/{}/{verb}",
            seg(approval_id, "approval")?
        ))
        .json(Value::Object(why.fields()?));
        decode(self.0.json(r).await?)
    }
}

/// A retention class to create, or a new version of one.
#[derive(Debug, Clone)]
pub struct ClassSettings {
    pub name: String,
    pub period: String,
    pub lock: String,
    pub start: String,
    pub profile: String,
    pub end_action: Option<Value>,
    pub notice_days: Option<u32>,
    pub description: Option<String>,
}

impl ClassSettings {
    /// `period` like `7 years`, `18 months`, `90 days` or `no end`.
    pub fn new(name: impl Into<String>, period: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            period: period.into(),
            lock: "retained".into(),
            start: "filing".into(),
            profile: "reference".into(),
            end_action: None,
            notice_days: None,
            description: None,
        }
    }

    pub fn locked(mut self) -> Self {
        self.lock = "locked".into();
        self
    }

    /// Delete at the end after giving this many days' notice, instead of asking first.
    pub fn delete_after_notice(mut self, days: u32) -> Self {
        self.notice_days = Some(days);
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

fn period(p: &str) -> Result<Value> {
    let p = p.trim().to_ascii_lowercase().replace('_', " ");
    if p == "no end" {
        return Ok(json!({ "unit": "no_end" }));
    }
    let bad = || {
        Error::invalid(format!(
            "period {p:?} should look like '7 years', '18 months', '90 days' or 'no end'"
        ))
    };
    let split = p.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let n: u32 = p[..split].parse().map_err(|_| bad())?;
    let unit = p[split..].trim();
    let unit = unit.strip_suffix('s').unwrap_or(unit);
    if !matches!(unit, "day" | "month" | "year") {
        return Err(bad());
    }
    Ok(json!({ "unit": format!("{unit}s"), "n": n }))
}

/// Retention classes.
#[derive(Debug, Clone, Copy)]
pub struct Classes<'a>(&'a Transport);

impl Classes<'_> {
    /// Every class, at its current version.
    pub async fn list(&self) -> Result<Vec<RetentionClass>> {
        list(
            &self.0.json(Request::get("/folio/classes")).await?,
            "classes",
        )
    }

    pub async fn get(&self, class_id: &str) -> Result<RetentionClass> {
        self.list()
            .await?
            .into_iter()
            .find(|c| c.id == class_id)
            .ok_or_else(|| {
                Error::Api(Box::new(crate::error::ApiError::local(
                    crate::ApiErrorKind::NotFound,
                    "not_found",
                    format!("there's no retention class called {class_id:?}"),
                )))
            })
    }

    /// Create a class or a new version; one that protects less waits for approval.
    pub async fn save(
        &self,
        class_id: &str,
        s: ClassSettings,
        why: Why,
    ) -> Result<Change<RetentionClass>> {
        let w = why.fields()?;
        let end_action = s.end_action.unwrap_or_else(|| match s.notice_days {
            None => json!({ "action": "ask_first" }),
            Some(d) => json!({ "action": "delete_after_notice", "notice_days": d }),
        });
        let mut body = json!({
            "id": class_id,
            "name": s.name,
            "period": period(&s.period)?,
            "start": s.start,
            "lock": s.lock,
            "profile": s.profile,
            "end_action": end_action,
            "reason_code": w["code"],
            "reason_text": w["text"],
        });
        if let Some(d) = s.description {
            body["description"] = json!(d);
        }
        change(
            self.0
                .send(Request::post("/folio/classes").json(body), &[])
                .await?,
        )
    }

    /// What applying the class's latest version to its older records would change.
    pub async fn preview_extension(&self, class_id: &str) -> Result<ExtensionPreview> {
        let r = Request::get(format!(
            "/folio/classes/{}/extension",
            seg(class_id, "class id")?
        ));
        decode(self.0.json(r).await?)
    }

    /// Ask to apply the class's latest version to the records filed under older ones.
    pub async fn request_extension(&self, class_id: &str, why: Why) -> Result<Approval> {
        let r = Request::post(format!(
            "/folio/classes/{}/extend",
            seg(class_id, "class id")?
        ))
        .json(Value::Object(why.fields()?));
        decode(self.0.json(r).await?)
    }
}

/// Folders.
#[derive(Debug, Clone, Copy)]
pub struct Folders<'a>(&'a Transport);

impl Folders<'_> {
    /// One page of the folders directly inside `parent` (`""` for the top level).
    pub async fn list(
        &self,
        parent: &str,
        after: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Page<Folder>> {
        let r = Request::get("/folio/folders")
            .param("parent", parent)
            .opt("after", after)
            .opt("limit", limit);
        let v = self.0.json(r).await?;
        Ok(Page {
            items: list(&v, "folders")?,
            next_cursor: crate::json::s(&v, "next_after"),
        })
    }

    /// Create a folder (and any missing folders above it).
    pub async fn create(
        &self,
        path: &str,
        matter: Option<&str>,
        subject: Option<&str>,
    ) -> Result<Folder> {
        let mut body = json!({ "path": path });
        if let Some(m) = matter {
            body["matter"] = json!(m);
        }
        if let Some(s) = subject {
            body["subject"] = json!(s);
        }
        decode(
            self.0
                .json(Request::post("/folio/folders").json(body))
                .await?,
        )
    }
}

/// Which ledger events to read.
#[derive(Debug, Clone, Default)]
pub struct LedgerFilters {
    pub actor: Option<String>,
    pub action: Option<String>,
    pub target: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub before: Option<i64>,
    pub limit: Option<u32>,
}

/// The ledger.
#[derive(Debug, Clone, Copy)]
pub struct Ledger<'a>(&'a Transport);

impl Ledger<'_> {
    /// One page, newest first; pass `next_before` back as `before` for the next.
    pub async fn page(&self, f: LedgerFilters) -> Result<LedgerPage> {
        let r = Request::get("/folio/ledger")
            .opt("actor", f.actor)
            .opt("action", f.action)
            .opt("target", f.target)
            .opt("from", f.since_ms)
            .opt("to", f.until_ms)
            .opt("before", f.before)
            .opt("limit", f.limit);
        decode(self.0.json(r).await?)
    }

    /// One person's or one key's events per day over the last `days` days.
    pub async fn activity(&self, actor: &str, days: u32) -> Result<Activity> {
        let r = Request::get("/folio/ledger/activity")
            .param("actor", actor)
            .param("days", days);
        decode(self.0.json(r).await?)
    }
}

/// Requests to erase a person's records.
#[derive(Debug, Clone, Copy)]
pub struct Erasure<'a>(&'a Transport);

impl Erasure<'_> {
    /// The subject's records, and for each whether it could be erased now and why not.
    pub async fn assess(&self, subject: &str) -> Result<ErasureAssessment> {
        let r = Request::get("/folio/erasure").param("subject", subject);
        decode(self.0.json(r).await?)
    }

    /// Ask to erase the subject's records; once a colleague approves, the approval's `result`
    /// says what was erased and what was kept.
    pub async fn request(
        &self,
        subject: &str,
        include_retained: bool,
        why: Why,
    ) -> Result<Approval> {
        let mut body = why.fields()?;
        body.insert("subject".into(), json!(subject));
        body.insert("include_retained".into(), json!(include_retained));
        let r = Request::post("/folio/erasure").json(Value::Object(body));
        decode(self.0.json(r).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_and_days() {
        assert_eq!(period("7 years").unwrap(), json!({"unit": "years", "n": 7}));
        assert_eq!(
            period("1 month").unwrap(),
            json!({"unit": "months", "n": 1})
        );
        assert_eq!(period("No_End").unwrap(), json!({"unit": "no_end"}));
        assert!(period("seven years").is_err());
        assert!(period("3 weeks").is_err());
        assert!(day("2031-03-31", "until").is_ok());
        assert!(day("2031-13-01", "until").is_err());
        assert!(day("31/03/2031", "until").is_err());
    }

    #[test]
    fn file_names_from_disposition() {
        assert_eq!(
            filename("attachment; filename=\"lease.pdf\"").as_deref(),
            Some("lease.pdf")
        );
        assert_eq!(
            filename("attachment; filename=x.pdf; filename*=UTF-8''caf%C3%A9.pdf").as_deref(),
            Some("café.pdf")
        );
        assert_eq!(
            filename("attachment; filename=\"../../etc/passwd\"").as_deref(),
            Some("passwd")
        );
        assert_eq!(filename("attachment; filename=\"..\""), None);
        assert_eq!(filename("inline"), None);
    }

    #[test]
    fn reason_text_must_fit_a_header() {
        let r = Request::get("/x");
        assert!(AccessReason::code("audit")
            .text("line one\nline two")
            .apply(r.clone())
            .is_err());
        let ok = AccessReason::code("audit")
            .text("quarterly")
            .apply(r)
            .unwrap();
        assert_eq!(ok.headers.len(), 2);
    }
}

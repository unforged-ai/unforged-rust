use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde_json::Value;

use crate::json::{arr, b, bbox, f, i, obj, s, st, strings, u};

/// The decision a check reached. Nib and Loupe share these.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Decision {
    AutoApprove,
    Approve,
    Flag,
    Failed,
    Unreadable,
    NoSignature,
    /// A decision this version of the SDK doesn't know yet.
    Other(String),
}

impl Decision {
    pub fn parse(s: &str) -> Self {
        match s {
            "AUTO_APPROVE" => Decision::AutoApprove,
            "APPROVE" => Decision::Approve,
            "FLAG" => Decision::Flag,
            "FAILED" => Decision::Failed,
            "UNREADABLE" => Decision::Unreadable,
            "NO_SIGNATURE" => Decision::NoSignature,
            other => Decision::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Decision::AutoApprove => "AUTO_APPROVE",
            Decision::Approve => "APPROVE",
            Decision::Flag => "FLAG",
            Decision::Failed => "FAILED",
            Decision::Unreadable => "UNREADABLE",
            Decision::NoSignature => "NO_SIGNATURE",
            Decision::Other(s) => s,
        }
    }

    /// `AUTO_APPROVE` or `APPROVE`.
    pub fn is_approved(&self) -> bool {
        matches!(self, Decision::AutoApprove | Decision::Approve)
    }

    pub(crate) fn of(v: &Value, k: &str) -> Option<Self> {
        v.get(k).and_then(Value::as_str).map(Decision::parse)
    }
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How readable the signature image was.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quality {
    pub ok: bool,
    pub issues: Vec<String>,
    pub effective_dpi: Option<f64>,
    pub sharpness: Option<f64>,
    pub contrast: Option<f64>,
}

impl Quality {
    fn from_json(v: &Value) -> Self {
        Self {
            ok: b(v, "ok"),
            issues: strings(v, "issues"),
            effective_dpi: f(v, "effective_dpi"),
            sharpness: f(v, "sharpness"),
            contrast: f(v, "contrast"),
        }
    }
}

/// How well the signature matched one reference.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceScore {
    /// The reference: its id online, its file offline.
    pub reference: Option<String>,
    pub probability: f64,
}

/// One signature found on the document.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub index: u32,
    pub page: u32,
    /// x, y, width, height in pixels.
    pub bbox: [i64; 4],
    pub decision: Decision,
    pub match_probability: Option<f64>,
    pub band: Option<(f64, f64)>,
    pub per_reference: Vec<ReferenceScore>,
    pub flags: Vec<String>,
    pub reasons: Vec<String>,
    pub best_match: bool,
    pub quality: Quality,
    pub located_by: Option<String>,
    /// Where its PNG was saved, when you asked for crops.
    pub crop: Option<PathBuf>,
    pub raw: Value,
}

impl Signature {
    pub(crate) fn from_json(v: &Value, ref_names: &[Option<String>]) -> Self {
        let per_reference = arr(v, "per_reference")
            .iter()
            .enumerate()
            .filter_map(|(n, s)| match s {
                Value::Object(_) => Some(ReferenceScore {
                    reference: crate::json::s(s, "ref"),
                    probability: f(s, "probability")?,
                }),
                _ => Some(ReferenceScore {
                    reference: ref_names.get(n).cloned().flatten(),
                    probability: s.as_f64()?,
                }),
            })
            .collect();
        let band = v
            .get("band")
            .and_then(Value::as_array)
            .and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)));
        Self {
            index: u(v, "index").unwrap_or(0) as u32,
            page: u(v, "page").unwrap_or(1) as u32,
            bbox: bbox(v.get("bbox").or_else(|| v.get("box"))),
            decision: Decision::of(v, "decision").unwrap_or(Decision::Failed),
            match_probability: f(v, "match_probability"),
            band,
            per_reference,
            flags: strings(v, "flags"),
            reasons: strings(v, "reasons"),
            best_match: b(v, "best_match"),
            quality: Quality::from_json(&obj(v, "quality")),
            located_by: s(v, "located_by"),
            crop: s(v, "crop").map(PathBuf::from),
            raw: v.clone(),
        }
    }
}

/// Why an item couldn't be checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemError {
    pub kind: String,
    pub detail: String,
}

/// One document's result.
#[derive(Debug, Clone, PartialEq)]
pub struct Verification {
    pub decision: Option<Decision>,
    pub signatures: Vec<Signature>,
    pub input_type: Option<String>,
    pub pages: Option<u32>,
    pub notes: Vec<String>,
    pub model: Option<String>,
    pub policy: Option<String>,
    pub status: String,
    pub error: Option<ItemError>,
    pub id: Option<String>,
    pub job_id: Option<String>,
    pub item_index: Option<u32>,
    pub subject_reference: Option<String>,
    pub reference: Option<String>,
    pub process: Option<String>,
    pub metadata: BTreeMap<String, String>,
    /// Offline: who the copy that checked it is licensed to.
    pub licensed_to: Option<String>,
    /// Offline: checked under a trial licence.
    pub trial: bool,
    /// Offline: the licence's own notices, such as a term ending soon.
    pub notices: Vec<String>,
    pub raw: Value,
}

fn metadata(v: &Value) -> BTreeMap<String, String> {
    v.get("metadata")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

impl Verification {
    /// The decision is `AUTO_APPROVE` or `APPROVE`.
    pub fn approved(&self) -> bool {
        self.decision.as_ref().is_some_and(Decision::is_approved)
    }

    /// The signature the decision is based on.
    pub fn best_match(&self) -> Option<&Signature> {
        self.signatures
            .iter()
            .find(|s| s.best_match)
            .or_else(|| self.signatures.first())
    }

    pub(crate) fn from_hosted(v: &Value, job_id: Option<&str>) -> Self {
        let versions = obj(v, "versions");
        let error = v.get("error").filter(|e| e.is_object()).map(|e| ItemError {
            kind: st(e, "type"),
            detail: st(e, "detail"),
        });
        Self {
            decision: Decision::of(v, "decision"),
            signatures: arr(v, "signatures")
                .iter()
                .map(|s| Signature::from_json(s, &[]))
                .collect(),
            input_type: s(v, "input_type"),
            pages: u(v, "pages").map(|p| p as u32),
            notes: strings(v, "notes"),
            model: s(&versions, "model"),
            policy: s(&versions, "policy"),
            status: s(v, "status").unwrap_or_else(|| "completed".into()),
            error,
            id: s(v, "id"),
            job_id: job_id.map(str::to_owned).or_else(|| s(v, "job_id")),
            item_index: u(v, "item_index").map(|n| n as u32),
            subject_reference: s(v, "subject_reference"),
            reference: s(v, "reference"),
            process: s(v, "process"),
            metadata: metadata(v),
            licensed_to: None,
            trial: false,
            notices: Vec::new(),
            raw: v.clone(),
        }
    }

    pub(crate) fn from_offline(v: &Value, notices: Vec<String>) -> Self {
        let refs: Vec<Option<String>> = arr(v, "references").iter().map(|r| s(r, "file")).collect();
        Self {
            decision: Decision::of(v, "decision"),
            signatures: arr(v, "signatures")
                .iter()
                .map(|s| Signature::from_json(s, &refs))
                .collect(),
            input_type: s(v, "input_type"),
            pages: u(v, "pages").map(|p| p as u32),
            notes: strings(v, "notes"),
            model: s(v, "model"),
            policy: s(v, "policy"),
            status: "completed".into(),
            error: None,
            id: None,
            job_id: None,
            item_index: None,
            subject_reference: None,
            reference: None,
            process: None,
            metadata: BTreeMap::new(),
            licensed_to: s(v, "licensed_to"),
            trial: b(v, "trial"),
            notices,
            raw: v.clone(),
        }
    }
}

/// A check job.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub job_id: String,
    pub status: String,
    pub results: Vec<Verification>,
    pub items: u64,
    pub source: Option<String>,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub summary: Option<Value>,
    pub next_cursor: Option<String>,
    pub raw: Value,
}

impl Job {
    /// Finished: completed, failed or rejected.
    pub fn done(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "rejected")
    }

    pub(crate) fn from_json(v: &Value) -> Self {
        let job_id = st(v, "job_id");
        Self {
            results: arr(v, "results")
                .iter()
                .map(|r| Verification::from_hosted(r, Some(&job_id)))
                .collect(),
            status: st(v, "status"),
            items: u(v, "items").or_else(|| u(v, "total")).unwrap_or(0),
            source: s(v, "source"),
            created_at: s(v, "created_at"),
            started_at: s(v, "started_at"),
            completed_at: s(v, "completed_at"),
            summary: v.get("summary").filter(|s| !s.is_null()).cloned(),
            next_cursor: s(v, "next_cursor"),
            raw: v.clone(),
            job_id,
        }
    }
}

/// A checked item as the checks list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedItem {
    pub job_id: String,
    pub index: u32,
    pub decision: Option<Decision>,
    pub match_probability: Option<f64>,
    pub subject_reference: Option<String>,
    pub reference: Option<String>,
    pub process: Option<String>,
    pub file: Option<String>,
    pub input_type: Option<String>,
    pub created_at: String,
    pub raw: Value,
}

impl CheckedItem {
    pub(crate) fn from_json(v: &Value) -> Self {
        Self {
            job_id: st(v, "job_id"),
            index: u(v, "index").unwrap_or(0) as u32,
            decision: Decision::of(v, "decision"),
            match_probability: f(v, "match_probability"),
            subject_reference: s(v, "subject_reference"),
            reference: s(v, "reference"),
            process: s(v, "process"),
            file: s(v, "file"),
            input_type: s(v, "input_type"),
            created_at: st(v, "created_at"),
            raw: v.clone(),
        }
    }
}

/// A reference signature enrolled for a subject.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub ref_id: String,
    pub source: String,
    pub reference: Option<String>,
    pub anchor: bool,
    pub created_at: Option<String>,
    pub raw: Value,
}

/// A person you check signatures for, and their references.
#[derive(Debug, Clone, PartialEq)]
pub struct Subject {
    pub subject_reference: String,
    pub references: Vec<Reference>,
    pub created_at: Option<String>,
    pub checks_total: Option<u64>,
    pub raw: Value,
}

impl Subject {
    pub(crate) fn from_json(v: &Value) -> Self {
        Self {
            subject_reference: st(v, "subject_reference"),
            references: arr(v, "references")
                .iter()
                .map(|r| Reference {
                    ref_id: st(r, "ref_id"),
                    source: s(r, "source").unwrap_or_else(|| "unverified".into()),
                    reference: s(r, "ref"),
                    anchor: r.get("anchor").and_then(Value::as_bool).unwrap_or(true),
                    created_at: s(r, "created_at"),
                    raw: r.clone(),
                })
                .collect(),
            created_at: s(v, "created_at"),
            checks_total: u(&obj(v, "checks"), "total"),
            raw: v.clone(),
        }
    }
}

/// A subject as the subjects list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct SubjectSummary {
    pub subject_reference: String,
    pub reference_count: u64,
    pub created_at: Option<String>,
    pub last_checked_at: Option<String>,
    pub raw: Value,
}

impl SubjectSummary {
    pub(crate) fn from_json(v: &Value) -> Self {
        Self {
            subject_reference: st(v, "subject_reference"),
            reference_count: u(&obj(v, "references"), "total").unwrap_or(0),
            created_at: s(v, "created_at"),
            last_checked_at: s(v, "last_checked_at"),
            raw: v.clone(),
        }
    }
}

/// One reference an enrolment took or turned down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledReference {
    pub ref_id: String,
    pub status: String,
    pub source: String,
    pub reference: Option<String>,
    pub reason: Option<String>,
    pub detail: Option<String>,
}

/// What an enrolment did.
#[derive(Debug, Clone, PartialEq)]
pub struct Enrolment {
    pub subject_reference: String,
    pub references: Vec<EnrolledReference>,
    pub raw: Value,
}

impl Enrolment {
    /// The references that were turned down.
    pub fn rejected(&self) -> impl Iterator<Item = &EnrolledReference> {
        self.references.iter().filter(|r| r.status == "rejected")
    }

    pub(crate) fn from_json(v: &Value) -> Self {
        Self {
            subject_reference: st(v, "subject_reference"),
            references: arr(v, "references")
                .iter()
                .map(|r| EnrolledReference {
                    ref_id: st(r, "ref_id"),
                    status: st(r, "status"),
                    source: s(r, "source").unwrap_or_else(|| "unverified".into()),
                    reference: s(r, "ref"),
                    reason: s(r, "reason"),
                    detail: s(r, "detail"),
                })
                .collect(),
            raw: v.clone(),
        }
    }
}

/// A signature confirmed as genuine.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirmation {
    pub job_id: String,
    pub item_index: u32,
    pub confirmed_at: String,
    pub learning: String,
    pub raw: Value,
}

impl Confirmation {
    pub(crate) fn from_json(v: &Value) -> Self {
        Self {
            job_id: st(v, "job_id"),
            item_index: u(v, "item_index").unwrap_or(0) as u32,
            confirmed_at: st(v, "confirmed_at"),
            learning: st(v, "learning"),
            raw: v.clone(),
        }
    }
}

/// This month's checks and the estimated charge.
#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub period_start: String,
    pub period_end: String,
    pub checks: u64,
    pub billable_checks: u64,
    pub decisions: BTreeMap<String, u64>,
    pub unit_price: String,
    pub estimated_charge: String,
    pub currency: String,
    pub raw: Value,
}

fn text(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

impl Usage {
    pub(crate) fn from_json(v: &Value) -> Self {
        Self {
            period_start: st(v, "period_start"),
            period_end: st(v, "period_end"),
            checks: u(v, "checks").unwrap_or(0),
            billable_checks: u(v, "billable_checks").unwrap_or(0),
            decisions: v
                .get("decisions")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, n)| Some((k.clone(), n.as_u64()?)))
                        .collect()
                })
                .unwrap_or_default(),
            unit_price: text(v, "unit_price"),
            estimated_charge: text(v, "estimated_charge"),
            currency: st(v, "currency"),
            raw: v.clone(),
        }
    }
}

/// A signer match's answer: who signed which documents.
#[derive(Debug, Clone, PartialEq)]
pub struct SignerMatch {
    pub status: String,
    pub people: Vec<Value>,
    pub unmatched: Vec<Value>,
    pub ambiguous: Vec<Value>,
    pub documents: Vec<Value>,
    pub job_id: Option<String>,
    pub item_index: Option<u32>,
    pub raw: Value,
}

impl SignerMatch {
    pub(crate) fn from_result(v: &Verification) -> Self {
        let m = obj(&v.raw, "signer_match");
        Self {
            status: s(&m, "status").unwrap_or_else(|| v.status.clone()),
            people: arr(&m, "people").to_vec(),
            unmatched: arr(&m, "unmatched").to_vec(),
            ambiguous: arr(&m, "ambiguous").to_vec(),
            documents: arr(&m, "documents").to_vec(),
            job_id: v.job_id.clone(),
            item_index: v.item_index,
            raw: v.raw.clone(),
        }
    }
}

/// A signature found by `extract_signatures`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedSignature {
    pub page: u32,
    pub bbox: [i64; 4],
    pub score: f64,
    /// The PNG it was saved as.
    pub crop: PathBuf,
}

/// The signatures found in one file (offline `extract_signatures`).
#[derive(Debug, Clone, PartialEq)]
pub struct Extraction {
    pub file: String,
    pub status: String,
    pub input_type: String,
    pub pages: u32,
    pub notes: Vec<String>,
    pub signatures: Vec<ExtractedSignature>,
    pub raw: Value,
}

impl Extraction {
    pub(crate) fn from_json(v: &Value, out: &std::path::Path) -> Self {
        Self {
            file: st(v, "file"),
            status: st(v, "status"),
            input_type: st(v, "input_type"),
            pages: u(v, "pages").unwrap_or(0) as u32,
            notes: strings(v, "notes"),
            signatures: arr(v, "signatures")
                .iter()
                .map(|sg| ExtractedSignature {
                    page: u(sg, "page").unwrap_or(1) as u32,
                    bbox: bbox(sg.get("box").or_else(|| sg.get("bbox"))),
                    score: f(sg, "score").unwrap_or(0.0),
                    crop: out.join(st(sg, "crop")),
                })
                .collect(),
            raw: v.clone(),
        }
    }
}

/// Who an offline copy is licensed to, what it can do, and until when.
#[derive(Debug, Clone, PartialEq)]
pub struct OfflineLicence {
    pub id: String,
    /// `licence`, `trial` or `short-lived`.
    pub kind: String,
    pub issued_to: String,
    /// The person a short-lived licence is for.
    pub issued_for: Option<String>,
    pub issued_by: String,
    pub issued_on: Option<String>,
    pub valid_from: String,
    pub valid_until: String,
    /// The last day it works, including the time it allows for renewing.
    pub works_until: String,
    /// What it allows: `verify`, `extract`, `loupe`.
    pub includes: Vec<String>,
    /// `active`, `renew_soon`, `grace`, `trial`, `short_lived`, or why it can't be used.
    pub state: String,
    pub days_left: Option<i64>,
    pub notices: Vec<String>,
    pub raw: Value,
}

impl OfflineLicence {
    /// A trial licence.
    pub fn trial(&self) -> bool {
        self.kind == "trial"
    }

    pub(crate) fn from_json(v: Value, notices: Vec<String>) -> Self {
        Self {
            id: st(&v, "id"),
            kind: st(&v, "kind"),
            issued_to: st(&v, "issued_to"),
            issued_for: s(&v, "for"),
            issued_by: st(&v, "issued_by"),
            issued_on: s(&v, "issued_on"),
            valid_from: st(&v, "valid_from"),
            valid_until: st(&v, "valid_until"),
            works_until: st(&v, "works_until"),
            includes: strings(&v, "includes"),
            state: st(&v, "state"),
            days_left: i(&v, "days_left"),
            notices,
            raw: v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn offline_scores_take_the_reference_files() {
        let v = json!({
            "decision": "APPROVE", "model": "nib-2.6", "policy": "assist", "pages": 1,
            "input_type": "pdf", "licensed_to": "Acme", "trial": true,
            "references": [{"file": "card.png"}, {"file": "passport.pdf"}],
            "signatures": [{"index": 0, "page": 1, "box": [1, 2, 3, 4], "decision": "APPROVE",
                "match_probability": 0.94, "band": [0.9, 0.97], "per_reference": [0.95, 0.9],
                "best_match": true, "quality": {"ok": true, "issues": []}}]
        });
        let r = Verification::from_offline(&v, vec!["renew soon".into()]);
        assert!(r.approved());
        assert!(r.trial);
        let best = r.best_match().unwrap();
        assert_eq!(best.bbox, [1, 2, 3, 4]);
        assert_eq!(best.band, Some((0.9, 0.97)));
        assert_eq!(
            best.per_reference[1].reference.as_deref(),
            Some("passport.pdf")
        );
        assert_eq!(r.notices, vec!["renew soon"]);
    }

    #[test]
    fn hosted_results_read_their_versions() {
        let v = json!({
            "status": "failed", "error": {"type": "unreadable", "detail": "not a document"},
            "versions": {"model": "m", "policy": "p"}, "item_index": 2,
            "metadata": {"branch": "leeds"}
        });
        let r = Verification::from_hosted(&v, Some("job_1"));
        assert_eq!(r.job_id.as_deref(), Some("job_1"));
        assert_eq!(r.model.as_deref(), Some("m"));
        assert_eq!(r.error.as_ref().unwrap().kind, "unreadable");
        assert_eq!(r.metadata["branch"], "leeds");
        assert!(!r.approved());
    }

    #[test]
    fn unknown_decisions_keep_their_name() {
        assert_eq!(Decision::parse("REVIEW").as_str(), "REVIEW");
        assert_eq!(Decision::parse("FLAG"), Decision::Flag);
    }
}

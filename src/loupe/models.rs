use serde_json::Value;

use crate::json::{arr, b, f, obj, s, st, strings, u};
use crate::nib::Decision;

/// A finding: something about the documents that suggests they aren't what they seem.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub detector: String,
    pub code: String,
    pub strength: f64,
    pub reason: String,
    /// Where it is: document, page, box and the value read there.
    pub evidence: Vec<Value>,
}

impl Signal {
    fn from_json(v: &Value) -> Self {
        Self {
            detector: st(v, "detector"),
            code: st(v, "code"),
            strength: f(v, "strength").unwrap_or(0.0),
            reason: st(v, "reason"),
            evidence: arr(v, "evidence").to_vec(),
        }
    }
}

/// Something that was checked and held.
#[derive(Debug, Clone, PartialEq)]
pub struct Assurance {
    pub detector: String,
    pub code: String,
    pub reason: String,
    pub evidence: Vec<Value>,
}

impl Assurance {
    fn from_json(v: &Value) -> Self {
        Self {
            detector: st(v, "detector"),
            code: st(v, "code"),
            reason: st(v, "reason"),
            evidence: arr(v, "evidence").to_vec(),
        }
    }
}

/// One document's result.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentResult {
    pub name: String,
    pub pages: u32,
    /// `bank_statement`, `payslip`, `invoice` and so on, when it was recognised.
    pub document_type: Option<String>,
    pub verdict: Option<Decision>,
    pub probability: f64,
    pub top_reasons: Vec<String>,
    /// `trusted`, `likely_genuine`, `unclear`, `doubtful` or `likely_altered`.
    pub rating: Option<String>,
    pub trusted_because: Vec<Assurance>,
    pub not_checked: Vec<String>,
    pub signals: Vec<Signal>,
    pub raw: Value,
}

impl DocumentResult {
    pub(crate) fn from_json(v: &Value) -> Self {
        Self {
            name: st(v, "name"),
            pages: u(v, "pages").unwrap_or(0) as u32,
            document_type: s(v, "document_type"),
            verdict: Decision::of(v, "verdict"),
            probability: f(v, "probability").unwrap_or(0.0),
            top_reasons: strings(v, "top_reasons"),
            rating: s(v, "rating"),
            trusted_because: arr(v, "trusted_because").iter().map(Assurance::from_json).collect(),
            not_checked: strings(v, "not_checked"),
            signals: arr(v, "signals").iter().map(Signal::from_json).collect(),
            raw: v.clone(),
        }
    }
}

/// What a check found, for the documents sent together.
#[derive(Debug, Clone, PartialEq)]
pub struct LoupeResult {
    pub verdict: Option<Decision>,
    pub probability: f64,
    pub top_reasons: Vec<String>,
    pub rating: Option<String>,
    pub trusted_because: Vec<Assurance>,
    pub documents: Vec<DocumentResult>,
    /// Findings that involve more than one document.
    pub signals: Vec<Signal>,
    /// The engine, detector and policy versions that gave the verdict.
    pub versions: Value,
    pub raw: Value,
}

impl LoupeResult {
    /// The verdict is `AUTO_APPROVE` or `APPROVE`.
    pub fn approved(&self) -> bool {
        self.verdict.as_ref().is_some_and(Decision::is_approved)
    }

    pub(crate) fn from_json(v: &Value) -> Self {
        Self {
            verdict: Decision::of(v, "verdict"),
            probability: f(v, "probability").unwrap_or(0.0),
            top_reasons: strings(v, "top_reasons"),
            rating: s(v, "rating"),
            trusted_because: arr(v, "trusted_because").iter().map(Assurance::from_json).collect(),
            documents: arr(v, "documents").iter().map(DocumentResult::from_json).collect(),
            signals: arr(v, "signals").iter().map(Signal::from_json).collect(),
            versions: obj(v, "versions"),
            raw: v.clone(),
        }
    }

    /// One document's result as a check of its own.
    pub(crate) fn from_document(v: &Value) -> Self {
        let doc = DocumentResult::from_json(v);
        Self {
            verdict: doc.verdict.clone(),
            probability: doc.probability,
            top_reasons: doc.top_reasons.clone(),
            rating: doc.rating.clone(),
            trusted_because: doc.trusted_because.clone(),
            signals: Vec::new(),
            versions: obj(v, "versions"),
            raw: v.clone(),
            documents: vec![doc],
        }
    }
}

/// A document sent with a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedDocument {
    pub index: u32,
    pub name: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// What the person who looked at the documents decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    /// `genuine`, `fraud` or `inconclusive`.
    pub outcome: String,
    pub note: Option<String>,
    pub by: String,
    pub at: String,
}

/// A check through the API.
#[derive(Debug, Clone, PartialEq)]
pub struct LoupeCheck {
    pub id: String,
    /// `queued`, `running`, `completed` or `failed`.
    pub status: String,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub documents_deleted_at: Option<String>,
    pub documents: Vec<CheckedDocument>,
    pub result: Option<LoupeResult>,
    pub error: Option<String>,
    pub review: Option<Review>,
    pub assigned_to: Option<String>,
    pub assigned_at: Option<String>,
    pub raw: Value,
}

impl LoupeCheck {
    /// Finished: completed or failed.
    pub fn done(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed")
    }

    pub(crate) fn from_json(v: &Value) -> Self {
        Self {
            id: st(v, "id"),
            status: st(v, "status"),
            created_at: st(v, "created_at"),
            completed_at: s(v, "completed_at"),
            documents_deleted_at: s(v, "documents_deleted_at"),
            documents: arr(v, "documents")
                .iter()
                .map(|d| CheckedDocument {
                    index: u(d, "index").unwrap_or(0) as u32,
                    name: st(d, "name"),
                    sha256: st(d, "sha256"),
                    size_bytes: u(d, "size_bytes").unwrap_or(0),
                })
                .collect(),
            result: v
                .get("result")
                .filter(|r| r.is_object())
                .map(LoupeResult::from_json),
            error: s(v, "error"),
            review: v.get("review").filter(|r| r.is_object()).map(|r| Review {
                outcome: st(r, "outcome"),
                note: s(r, "note"),
                by: st(r, "by"),
                at: st(r, "at"),
            }),
            assigned_to: s(v, "assigned_to"),
            assigned_at: s(v, "assigned_at"),
            raw: v.clone(),
        }
    }
}

/// A finished check's result, the same online and offline.
#[derive(Debug, Clone, PartialEq)]
pub struct LoupeReport {
    /// The check's id (online only).
    pub check_id: Option<String>,
    pub result: LoupeResult,
    /// Offline: who the copy that checked it is licensed to.
    pub licensed_to: Option<String>,
    /// Offline: checked under a trial licence.
    pub trial: bool,
    /// Offline: the documents' text was read.
    pub text_read: bool,
    /// Offline: registers were asked about the numbers and addresses found.
    pub register_lookups: bool,
    /// Offline: the licence's own notices, such as a term ending soon.
    pub notices: Vec<String>,
}

impl LoupeReport {
    pub(crate) fn from_offline(v: &Value, notices: Vec<String>) -> Self {
        let result = if s(v, "kind").as_deref() == Some("document") {
            LoupeResult::from_document(v)
        } else {
            LoupeResult::from_json(v)
        };
        Self {
            check_id: None,
            result,
            licensed_to: s(v, "licensed_to"),
            trial: b(v, "trial"),
            text_read: b(v, "text_read"),
            register_lookups: b(v, "register_lookups"),
            notices,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn one_document_offline_reads_as_a_submission_of_one() {
        let v = json!({
            "kind": "document", "licensed_to": "Acme", "text_read": true, "register_lookups": false,
            "name": "statement.pdf", "pages": 3, "document_type": "bank_statement",
            "verdict": "FLAG", "probability": 0.81, "top_reasons": ["The balance doesn't add up"],
            "rating": "doubtful", "trusted_because": [], "not_checked": [],
            "signals": [{"detector": "arithmetic", "code": "balance_mismatch", "strength": 0.8,
                "reason": "The balance doesn't add up", "evidence": [{"page": 2}]}],
            "versions": {"engine": "1.7.0"}
        });
        let r = LoupeReport::from_offline(&v, vec![]);
        assert_eq!(r.result.verdict, Some(Decision::Flag));
        assert_eq!(r.result.documents.len(), 1);
        assert_eq!(r.result.documents[0].signals[0].code, "balance_mismatch");
        assert_eq!(r.result.versions["engine"], "1.7.0");
        assert!(r.text_read);
        assert!(!r.result.approved());
    }

    #[test]
    fn checks_read_their_review() {
        let c = LoupeCheck::from_json(&json!({
            "id": "lck_1", "status": "completed", "created_at": "2026-10-31T10:00:00Z",
            "result": {"verdict": "APPROVE", "probability": 0.1, "documents": []},
            "review": {"outcome": "genuine", "note": "Matches the bank's copy", "by": "Sam", "at": "x"}
        }));
        assert!(c.done());
        assert!(c.result.unwrap().approved());
        assert_eq!(c.review.unwrap().outcome, "genuine");
    }
}

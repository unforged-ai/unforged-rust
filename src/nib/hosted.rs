use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::models::{
    CheckedItem, Confirmation, Enrolment, Job, SignerMatch, Subject, SubjectSummary, Usage,
    Verification,
};
use super::VerifyOptions;
use crate::error::{ApiError, ApiErrorKind, Error, Result};
use crate::files::{check_id, expand, File, Target};
use crate::http::{multipart, Part, Request, Transport};
use crate::page::Page;
use crate::Policy;

const DEFAULT_WAIT: Duration = Duration::from_secs(300);
const PAGE: u32 = 500;

/// Files for one request, each under a part name.
#[derive(Default)]
struct Parts {
    parts: Vec<Part>,
    count: HashMap<&'static str, usize>,
}

impl Parts {
    async fn add(&mut self, f: &File, hint: &'static str) -> Result<Value> {
        match f {
            File::PartName(n) => Ok(Value::String(n.clone())),
            File::Url(u) => Ok(json!({ "url": u })),
            _ => {
                let (filename, data) = f.read().await?;
                let n = self.count.entry(hint).or_insert(0);
                *n += 1;
                let name = if *n == 1 {
                    hint.to_string()
                } else {
                    format!("{hint}-{n}")
                };
                self.parts.push(Part::file(
                    &name,
                    filename.unwrap_or_else(|| name.clone()),
                    data,
                ));
                Ok(Value::String(name))
            }
        }
    }
}

/// One document to check in a job.
#[derive(Debug, Clone, Default)]
pub struct CheckItem {
    pub file: Option<File>,
    pub subject_reference: Option<String>,
    pub references: Vec<File>,
    pub target: Option<Target>,
    pub template_id: Option<String>,
    pub reference: Option<String>,
    pub process: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

impl CheckItem {
    /// A document, to compare with a subject's references.
    pub fn new(file: impl Into<File>, subject: impl Into<String>) -> Self {
        Self {
            file: Some(file.into()),
            subject_reference: Some(subject.into()),
            ..Self::default()
        }
    }

    /// A document, to compare with these reference files.
    pub fn with_references<I, F>(file: impl Into<File>, references: I) -> Self
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        Self {
            file: Some(file.into()),
            references: references.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Your own reference for this document.
    pub fn reference(mut self, reference: impl Into<String>) -> Self {
        self.reference = Some(reference.into());
        self
    }

    pub fn target(mut self, target: Target) -> Self {
        self.target = Some(target);
        self
    }

    pub fn process(mut self, process: impl Into<String>) -> Self {
        self.process = Some(process.into());
        self
    }

    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    async fn prepare(&self, parts: &mut Parts) -> Result<Value> {
        let mut out = Map::new();
        if let Some(f) = &self.file {
            out.insert("file".into(), parts.add(f, "questioned").await?);
        }
        if let Some(s) = &self.subject_reference {
            out.insert("subject_reference".into(), check_id(s, "subject")?.into());
        }
        if !self.references.is_empty() {
            let mut refs = Vec::new();
            for f in expand(self.references.clone())? {
                refs.push(parts.add(&f, "reference").await?);
            }
            out.insert("references".into(), refs.into());
        }
        if let Some(t) = &self.target {
            out.insert("target".into(), t.to_json());
        }
        for (k, v) in [
            ("template_id", &self.template_id),
            ("reference", &self.reference),
            ("process", &self.process),
        ] {
            if let Some(v) = v {
                out.insert(k.into(), v.clone().into());
            }
        }
        if !self.metadata.is_empty() {
            out.insert("metadata".into(), json!(self.metadata));
        }
        Ok(Value::Object(out))
    }
}

/// How a job is made.
#[derive(Debug, Clone, Default)]
pub struct CreateOptions {
    /// One of your saved policies, by name.
    pub policy: Option<String>,
    pub process: Option<String>,
    pub metadata: BTreeMap<String, String>,
    /// Your own id for the job, which also makes sending it twice safe.
    pub job_id: Option<String>,
    /// Wait for the job to finish (up to `timeout`).
    pub wait: bool,
    pub timeout: Option<Duration>,
}

/// Which jobs to list.
#[derive(Debug, Clone, Default)]
pub struct JobFilters {
    pub status: Option<String>,
    pub source: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// Which checked items to list (and count, for `summary`).
#[derive(Debug, Clone, Default)]
pub struct ItemFilters {
    pub decision: Option<String>,
    pub subject_reference: Option<String>,
    pub reference: Option<String>,
    pub process: Option<String>,
    pub input_type: Option<String>,
    pub q: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

impl ItemFilters {
    fn apply(&self, r: Request) -> Request {
        r.opt("decision", self.decision.as_ref())
            .opt("subject_reference", self.subject_reference.as_ref())
            .opt("reference", self.reference.as_ref())
            .opt("process", self.process.as_ref())
            .opt("input_type", self.input_type.as_ref())
            .opt("q", self.q.as_ref())
            .opt("since", self.since.as_ref())
            .opt("until", self.until.as_ref())
    }
}

/// Check jobs (online only).
#[derive(Debug, Clone, Copy)]
pub struct Checks<'a> {
    http: &'a Transport,
}

impl<'a> Checks<'a> {
    pub(crate) fn new(http: &'a Transport) -> Self {
        Self { http }
    }

    /// Check one document or a batch.
    pub async fn create(&self, items: Vec<CheckItem>, options: CreateOptions) -> Result<Job> {
        let mut prepared = Vec::with_capacity(items.len());
        let mut parts = Parts::default();
        for it in &items {
            prepared.push(it.prepare(&mut parts).await?);
        }
        self.create_prepared(prepared, parts, options).await
    }

    async fn create_prepared(
        &self,
        prepared: Vec<Value>,
        parts: Parts,
        options: CreateOptions,
    ) -> Result<Job> {
        if prepared.is_empty() {
            return Err(Error::invalid("there are no items to check"));
        }
        let count = prepared.len() as u64;
        let job_id = match &options.job_id {
            Some(j) => check_id(j, "job id")?.to_string(),
            None => uuid::Uuid::new_v4().to_string(),
        };
        let mut req = Map::new();
        req.insert("items".into(), prepared.into());
        if let Some(p) = &options.policy {
            req.insert("policy".into(), p.clone().into());
        }
        if let Some(p) = &options.process {
            req.insert("process".into(), p.clone().into());
        }
        if !options.metadata.is_empty() {
            req.insert("metadata".into(), json!(options.metadata));
        }
        req.insert("job_id".into(), job_id.clone().into());
        let req = Value::Object(req);
        let mut r = Request::post("/nib/checks").idempotency_key(job_id.clone());
        r = if parts.parts.is_empty() {
            r.json(req)
        } else {
            let mut all = vec![Part::json("request", &req)];
            all.extend(parts.parts);
            let (ct, body) = multipart(all);
            r.raw(ct, body)
        };
        let job_id = match self.http.send(r, &[]).await {
            Ok(resp) if resp.status == 200 => {
                let result = Verification::from_hosted(&resp.json()?, Some(&job_id));
                return Ok(Job {
                    job_id,
                    status: "completed".into(),
                    results: vec![result],
                    items: 1,
                    source: None,
                    created_at: None,
                    started_at: None,
                    completed_at: None,
                    summary: None,
                    next_cursor: None,
                    raw: Value::Null,
                });
            }
            Ok(resp) => resp
                .json()?
                .get("job_id")
                .and_then(Value::as_str)
                .map_or(job_id, str::to_owned),
            Err(Error::Api(e)) => match e.job_id() {
                Some(j) => j.to_string(),
                None => return Err(Error::Api(e)),
            },
            Err(e) => return Err(e),
        };
        if options.wait {
            return self
                .wait(&job_id, options.timeout.unwrap_or(DEFAULT_WAIT))
                .await;
        }
        Ok(Job {
            job_id,
            status: "queued".into(),
            results: Vec::new(),
            items: count,
            source: None,
            created_at: None,
            started_at: None,
            completed_at: None,
            summary: None,
            next_cursor: None,
            raw: Value::Null,
        })
    }

    /// A job and one page of its results.
    pub async fn get(
        &self,
        job_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        decision: Option<&str>,
    ) -> Result<Job> {
        let r = Request::get(format!("/nib/checks/{}", check_id(job_id, "job id")?))
            .opt("cursor", cursor)
            .opt("limit", limit)
            .opt("decision", decision);
        Ok(Job::from_json(&self.http.json(r).await?))
    }

    /// Every result of a job.
    pub async fn results(&self, job_id: &str, decision: Option<&str>) -> Result<Vec<Verification>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let job = self
                .get(job_id, cursor.as_deref(), Some(PAGE), decision)
                .await?;
            all.extend(job.results);
            match job.next_cursor {
                Some(c) => cursor = Some(c),
                None => return Ok(all),
            }
        }
    }

    /// Wait for a job to finish, then return it with all its results.
    pub async fn wait(&self, job_id: &str, timeout: Duration) -> Result<Job> {
        let deadline = Instant::now() + timeout;
        let mut delay = self.http.poll_interval();
        loop {
            let mut job = self.get(job_id, None, Some(PAGE), None).await?;
            if job.done() {
                let mut cursor = job.next_cursor.take();
                while let Some(c) = cursor {
                    let page = self.get(job_id, Some(&c), Some(PAGE), None).await?;
                    job.results.extend(page.results);
                    cursor = page.next_cursor;
                }
                return Ok(job);
            }
            if Instant::now() + delay > deadline {
                return Err(Error::CheckTimeout {
                    message: format!(
                        "check {job_id} is still {} after {timeout:?}; it carries on, so wait for it again later",
                        job.status
                    ),
                    job_id: Some(job_id.to_string()),
                });
            }
            tokio::time::sleep(delay).await;
            delay = (delay.mul_f64(1.5)).min(Duration::from_secs(5));
        }
    }

    /// One page of jobs, newest first (without their results).
    pub async fn list(&self, filters: &JobFilters) -> Result<Page<Job>> {
        let r = Request::get("/nib/checks")
            .param("view", "jobs")
            .opt("status", filters.status.as_ref())
            .opt("source", filters.source.as_ref())
            .opt("since", filters.since.as_ref())
            .opt("until", filters.until.as_ref())
            .opt("cursor", filters.cursor.as_ref())
            .opt("limit", filters.limit);
        let v = self.http.json(r).await?;
        Ok(Page::from_json(&v, "jobs", Job::from_json))
    }

    /// One page of checked items across jobs, newest first.
    pub async fn items(&self, filters: &ItemFilters) -> Result<Page<CheckedItem>> {
        let r = filters
            .apply(Request::get("/nib/checks").param("view", "items"))
            .opt("cursor", filters.cursor.as_ref())
            .opt("limit", filters.limit);
        let v = self.http.json(r).await?;
        Ok(Page::from_json(&v, "items", CheckedItem::from_json))
    }

    /// Counts per decision, with the same filters as `items`.
    pub async fn summary(&self, filters: &ItemFilters) -> Result<Value> {
        let r = filters.apply(Request::get("/nib/checks").param("view", "summary"));
        self.http.json(r).await
    }

    /// Confirm an item's signature as genuine (it may become a reference).
    pub async fn confirm(&self, job_id: &str, index: u32) -> Result<Confirmation> {
        let r = Request::post(format!(
            "/nib/checks/{}/items/{index}/confirm",
            check_id(job_id, "job id")?
        ));
        Ok(Confirmation::from_json(&self.http.json(r).await?))
    }

    /// The questioned signature, cropped, as PNG bytes.
    pub async fn image(&self, job_id: &str, index: u32, signature: Option<u32>) -> Result<Vec<u8>> {
        let r = Request::get(format!(
            "/nib/checks/{}/items/{index}/image",
            check_id(job_id, "job id")?
        ))
        .opt("signature", signature)
        .accept("image/png");
        self.http.bytes(r).await
    }

    /// A signer match's `n`-th specimen signature, as PNG bytes.
    pub async fn specimen_image(&self, job_id: &str, index: u32, n: u32) -> Result<Vec<u8>> {
        let r = Request::get(format!(
            "/nib/checks/{}/items/{index}/specimens/{n}/image",
            check_id(job_id, "job id")?
        ))
        .accept("image/png");
        self.http.bytes(r).await
    }
}

/// The people you check, and their reference signatures (online only).
#[derive(Debug, Clone, Copy)]
pub struct Subjects<'a> {
    http: &'a Transport,
}

impl<'a> Subjects<'a> {
    pub(crate) fn new(http: &'a Transport) -> Self {
        Self { http }
    }

    /// One page of subjects; `q` finds them by reference.
    pub async fn list(
        &self,
        q: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Page<SubjectSummary>> {
        let r = Request::get("/nib/subjects")
            .opt("q", q)
            .opt("cursor", cursor)
            .opt("limit", limit);
        let v = self.http.json(r).await?;
        Ok(Page::from_json(&v, "subjects", SubjectSummary::from_json))
    }

    pub async fn get(&self, subject_reference: &str) -> Result<Subject> {
        let r = Request::get(format!(
            "/nib/subjects/{}",
            check_id(subject_reference, "subject")?
        ));
        Ok(Subject::from_json(&self.http.json(r).await?))
    }

    /// Add reference signatures, named after their files, creating the subject if it's new.
    /// `source` says where they came from, such as `signature_card` or `unverified`.
    pub async fn enrol<I, F>(
        &self,
        subject_reference: &str,
        references: I,
        source: &str,
    ) -> Result<Enrolment>
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        let files: Vec<File> = references.into_iter().map(Into::into).collect();
        self.enrol_named(subject_reference, name_references(files), source)
            .await
    }

    /// Add reference signatures under ids you choose.
    pub async fn enrol_named(
        &self,
        subject_reference: &str,
        references: Vec<(String, File)>,
        source: &str,
    ) -> Result<Enrolment> {
        let path = format!(
            "/nib/subjects/{}/references",
            check_id(subject_reference, "subject")?
        );
        let mut results: Vec<Value> = Vec::new();
        let mut raw = json!({ "subject_reference": subject_reference });
        let (by_url, local): (Vec<_>, Vec<_>) = references
            .into_iter()
            .partition(|(_, f)| matches!(f, File::Url(_)));
        if !local.is_empty() {
            let mut parts = Vec::new();
            for (rid, f) in &local {
                check_id(rid, "reference id")?;
                let (filename, data) = f.read().await?;
                parts.push(Part::file(
                    rid,
                    filename.unwrap_or_else(|| rid.clone()),
                    data,
                ));
                parts.push(Part::text(&format!("source.{rid}"), source));
            }
            let (ct, body) = multipart(parts);
            raw = self
                .http
                .json(Request::new(reqwest::Method::PUT, path.clone()).raw(ct, body))
                .await?;
            results.extend(crate::json::arr(&raw, "references").iter().cloned());
        }
        if !by_url.is_empty() {
            let mut refs = Vec::new();
            for (rid, f) in &by_url {
                check_id(rid, "reference id")?;
                if let File::Url(u) = f {
                    refs.push(json!({ "ref_id": rid, "url": u, "source": source }));
                }
            }
            raw = self
                .http
                .json(Request::new(reqwest::Method::PUT, path).json(json!({ "references": refs })))
                .await?;
            results.extend(crate::json::arr(&raw, "references").iter().cloned());
        }
        if let Value::Object(m) = &mut raw {
            m.insert("references".into(), results.into());
        }
        Ok(Enrolment::from_json(&raw))
    }

    /// Permanently delete a subject and its references.
    pub async fn delete(&self, subject_reference: &str) -> Result<()> {
        let r = Request::new(
            reqwest::Method::DELETE,
            format!("/nib/subjects/{}", check_id(subject_reference, "subject")?),
        );
        self.http.send(r, &[]).await.map(drop)
    }

    /// Permanently delete one reference.
    pub async fn delete_reference(&self, subject_reference: &str, ref_id: &str) -> Result<()> {
        let r = Request::new(
            reqwest::Method::DELETE,
            format!(
                "/nib/subjects/{}/references/{}",
                check_id(subject_reference, "subject")?,
                check_id(ref_id, "reference id")?
            ),
        );
        self.http.send(r, &[]).await.map(drop)
    }

    /// A reference's thumbnail, as PNG bytes.
    pub async fn reference_image(&self, subject_reference: &str, ref_id: &str) -> Result<Vec<u8>> {
        let r = Request::get(format!(
            "/nib/subjects/{}/references/{}/image",
            check_id(subject_reference, "subject")?,
            check_id(ref_id, "reference id")?
        ))
        .accept("image/png");
        self.http.bytes(r).await
    }

    /// Remove learned references from `since` (RFC 3339).
    pub async fn unlearn_since(&self, subject_reference: &str, since: &str) -> Result<Vec<String>> {
        self.unlearn(subject_reference, json!({ "since": since }))
            .await
    }

    /// Remove the reference learned from one check (`job/index`).
    pub async fn unlearn_check(&self, subject_reference: &str, check: &str) -> Result<Vec<String>> {
        self.unlearn(subject_reference, json!({ "check": check }))
            .await
    }

    async fn unlearn(&self, subject_reference: &str, body: Value) -> Result<Vec<String>> {
        let r = Request::post(format!(
            "/nib/subjects/{}/references/unlearn",
            check_id(subject_reference, "subject")?
        ))
        .json(body);
        Ok(crate::json::strings(&self.http.json(r).await?, "unlearned"))
    }
}

/// Reference ids from the files' names, kept unique.
fn name_references(files: Vec<File>) -> Vec<(String, File)> {
    let mut seen = HashSet::new();
    files
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            let stem = f
                .local_path()
                .and_then(|p| p.file_stem())
                .map(|s| {
                    s.to_string_lossy()
                        .chars()
                        .map(|c| {
                            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                                c
                            } else {
                                '-'
                            }
                        })
                        .take(120)
                        .collect::<String>()
                })
                .filter(|s| !s.is_empty());
            let mut rid = stem.unwrap_or_else(|| format!("ref-{}", i + 1));
            if seen.contains(&rid) || rid.starts_with("learned-") || rid == "." || rid == ".." {
                rid = format!("{rid}-{}", i + 1);
            }
            seen.insert(rid.clone());
            (rid, f)
        })
        .collect()
}

fn one(job: Job) -> Result<Verification> {
    let job_id = job.job_id.clone();
    let Some(v) = job.results.into_iter().next() else {
        return Err(Error::Api(Box::new(ApiError::local(
            ApiErrorKind::Other,
            &job.status,
            format!("check {job_id} finished without a result"),
        ))));
    };
    if v.status == "failed" {
        let (kind, detail) = v
            .error
            .map(|e| (e.kind, e.detail))
            .unwrap_or_else(|| ("failed".into(), "the check failed".into()));
        if kind == "no_usable_references" {
            return Err(Error::no_references(detail));
        }
        return Err(Error::CheckFailed {
            kind,
            detail,
            job_id: Some(job_id),
        });
    }
    Ok(v)
}

pub(super) async fn verify(http: &Transport, file: File, o: VerifyOptions) -> Result<Verification> {
    let policy = match o.policy {
        None => None,
        Some(Policy::Named(n)) => Some(n),
        Some(_) => {
            return Err(Error::invalid(
                "online, the policy is one of your saved policies, by name",
            ))
        }
    };
    let item = CheckItem {
        file: Some(file.clone()),
        subject_reference: o.subject,
        references: o.references,
        target: o.target,
        template_id: o.template_id,
        reference: o.reference,
        process: o.process,
        metadata: o.metadata,
    };
    let checks = Checks::new(http);
    let job = checks
        .create(
            vec![item],
            CreateOptions {
                policy,
                wait: true,
                timeout: o.timeout,
                ..CreateOptions::default()
            },
        )
        .await?;
    let mut v = one(job)?;
    if let Some(dir) = o.save_crops {
        save_crops(&checks, &mut v, &file, dir).await?;
    }
    Ok(v)
}

async fn save_crops(
    checks: &Checks<'_>,
    v: &mut Verification,
    file: &File,
    dir: PathBuf,
) -> Result<()> {
    if v.signatures.is_empty() {
        return Ok(());
    }
    tokio::fs::create_dir_all(&dir).await?;
    let stem = file
        .local_path()
        .and_then(|p| p.file_stem())
        .map_or_else(|| "signature".into(), |s| s.to_string_lossy().into_owned());
    let job_id = v.job_id.clone().unwrap_or_default();
    let index = v.item_index.unwrap_or(0);
    for s in &mut v.signatures {
        let png = checks.image(&job_id, index, Some(s.index)).await?;
        let path = dir.join(format!("{stem}-p{}-{}.png", s.page, s.index + 1));
        tokio::fs::write(&path, png).await?;
        s.crop = Some(path);
    }
    Ok(())
}

pub(super) async fn signer_match<P, S, D>(
    http: &Transport,
    people: P,
    documents: D,
    timeout: Option<Duration>,
) -> Result<SignerMatch>
where
    P: IntoIterator<Item = (String, S)>,
    S: IntoIterator<Item = File>,
    D: IntoIterator<Item = File>,
{
    let mut parts = Parts::default();
    let mut ps = Vec::new();
    for (label, specimens) in people {
        let mut specs = Vec::new();
        for f in specimens {
            specs.push(parts.add(&f, "specimen").await?);
        }
        ps.push(json!({ "label": label, "specimens": specs }));
    }
    let mut docs = Vec::new();
    for f in documents {
        docs.push(parts.add(&f, "document").await?);
    }
    let item = json!({ "kind": "signer_match", "people": ps, "documents": docs });
    let job = Checks::new(http)
        .create_prepared(
            vec![item],
            parts,
            CreateOptions {
                wait: true,
                timeout: Some(timeout.unwrap_or(Duration::from_secs(600))),
                ..CreateOptions::default()
            },
        )
        .await?;
    Ok(SignerMatch::from_result(&one(job)?))
}

pub(super) async fn usage(http: &Transport) -> Result<Usage> {
    Ok(Usage::from_json(
        &http.json(Request::get("/nib/usage")).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_are_named_after_their_files() {
        let named = name_references(vec![
            File::path("cards/Jane Card.png"),
            File::path("other/Jane Card.jpg"),
            File::bytes(b"x".to_vec()),
            File::path("learned-1.png"),
        ]);
        let ids: Vec<&str> = named.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(ids, ["Jane-Card", "Jane-Card-2", "ref-3", "learned-1-4"]);
    }
}

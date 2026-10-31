//! Loupe: check that documents haven't been altered or made up, through the API or offline.
mod models;

use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub use models::{
    Assurance, CheckedDocument, DocumentResult, LoupeCheck, LoupeReport, LoupeResult, Review,
    Signal,
};

use crate::client::{Backend, OfflineSettings, Unforged};
use crate::error::{Error, Result};
use crate::files::{check_id, File};
use crate::http::{multipart, Part, Request, Transport};
use crate::nib::OfflineLicence;
use crate::offline::{on_disk, path_arg, write_private};
use crate::page::Page;
use crate::Policy;

const DEFAULT_WAIT: Duration = Duration::from_secs(300);

/// How to check documents.
#[derive(Debug, Clone, Default)]
pub struct LoupeOptions {
    /// A saved policy by name, or a policy in full (a JSON value or file).
    pub policy: Option<Policy>,
    /// Online: the Loupe version to check with, when it should be another than your
    /// organisation's.
    pub loupe_version: Option<String>,
    /// Offline: look up VAT, company and firm numbers and web addresses with their registers,
    /// through Unforged. Only those numbers and addresses are sent, never the document.
    pub lookups: bool,
    /// How long to wait for the answer.
    pub timeout: Option<Duration>,
}

impl LoupeOptions {
    pub fn policy(mut self, policy: impl Into<Policy>) -> Self {
        self.policy = Some(policy.into());
        self
    }

    pub fn loupe_version(mut self, version: impl Into<String>) -> Self {
        self.loupe_version = Some(version.into());
        self
    }

    pub fn lookups(mut self, on: bool) -> Self {
        self.lookups = on;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// What someone decided about a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Genuine,
    Fraud,
    Inconclusive,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Genuine => "genuine",
            Outcome::Fraud => "fraud",
            Outcome::Inconclusive => "inconclusive",
        }
    }
}

/// Which checks to list.
#[derive(Debug, Clone, Default)]
pub struct LoupeFilters {
    /// `waiting` (still need someone to decide) or `decided`.
    pub queue: Option<String>,
    /// With `queue`: `me`, `nobody` or one member.
    pub assigned_to: Option<String>,
    /// A document name to look for.
    pub q: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

/// Loupe, through the API or offline: the same calls either way.
#[derive(Debug, Clone, Copy)]
pub struct Loupe<'a> {
    client: &'a Unforged,
}

impl<'a> Loupe<'a> {
    pub(crate) fn new(client: &'a Unforged) -> Self {
        Self { client }
    }

    /// Check documents sent together (a ZIP or a folder counts as several), and wait for what
    /// the check found.
    pub async fn check<I, F>(&self, files: I, options: LoupeOptions) -> Result<LoupeReport>
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        let files: Vec<File> = files.into_iter().map(Into::into).collect();
        if files.is_empty() {
            return Err(Error::invalid("there are no documents to check"));
        }
        match &*self.client.backend {
            Backend::Hosted(t) => {
                let checks = LoupeChecks { http: t };
                let timeout = options.timeout.unwrap_or(DEFAULT_WAIT);
                let created = checks.create(files, &options).await?;
                let done = checks.wait(&created.id, timeout).await?;
                if done.status == "failed" {
                    return Err(Error::CheckFailed {
                        kind: "failed".into(),
                        detail: done
                            .error
                            .unwrap_or_else(|| "the documents couldn't be checked".into()),
                        job_id: Some(done.id),
                    });
                }
                let result = done.result.ok_or_else(|| {
                    Error::Decode(format!("check {} finished without a result", done.id))
                })?;
                Ok(LoupeReport {
                    check_id: Some(done.id),
                    result,
                    licensed_to: None,
                    trial: false,
                    text_read: true,
                    register_lookups: true,
                    notices: Vec::new(),
                })
            }
            Backend::Offline(o) => check_offline(o, files, options).await,
        }
    }

    /// Checks through the API: queue, history, evidence packs and decisions (online only).
    pub fn checks(&self) -> Result<LoupeChecks<'a>> {
        Ok(LoupeChecks {
            http: self.client.hosted(
                "offline, check answers straight away: there are no checks to list, review or assign",
            )?,
        })
    }

    /// Who this offline copy is licensed to, what it can do, and until when (offline only).
    pub async fn licence(&self) -> Result<OfflineLicence> {
        match &*self.client.backend {
            Backend::Offline(o) => o.loupe()?.licence().await,
            Backend::Hosted(_) => Err(Error::not_supported(
                "licence is for the offline binary",
            )),
        }
    }
}

/// Loupe checks through the API.
#[derive(Debug, Clone, Copy)]
pub struct LoupeChecks<'a> {
    http: &'a Transport,
}

impl LoupeChecks<'_> {
    /// Send documents to be checked, without waiting.
    pub async fn create(&self, files: Vec<File>, options: &LoupeOptions) -> Result<LoupeCheck> {
        let mut parts = Vec::new();
        for f in crate::files::expand(files)? {
            match f {
                File::Url(_) | File::PartName(_) => {
                    return Err(Error::invalid(
                        "Loupe checks the documents you send: pass files or bytes",
                    ))
                }
                f => {
                    let (name, data) = f.read().await?;
                    let name = name.unwrap_or_else(|| {
                        format!("document-{}{}", parts.len() + 1, crate::files::extension(&data))
                    });
                    parts.push(Part::file("file", name, data));
                }
            }
        }
        if let Some(p) = &options.policy {
            let v = match p {
                Policy::Named(n) => json!({ "name": n }),
                Policy::Json(v) => v.clone(),
                Policy::File(path) => {
                    let data = tokio::fs::read(path).await?;
                    serde_json::from_slice(&data).map_err(|e| {
                        Error::invalid(format!("{} isn't a policy: {e}", path.display()))
                    })?
                }
            };
            parts.push(Part::json("policy", &v));
        }
        if let Some(v) = &options.loupe_version {
            parts.push(Part::text("loupe_version", v));
        }
        let (ct, body) = multipart(parts);
        let r = Request::post("/loupe/checks").raw(ct, body);
        Ok(LoupeCheck::from_json(&self.http.json(r).await?))
    }

    pub async fn get(&self, check_id: &str) -> Result<LoupeCheck> {
        let r = Request::get(format!("/loupe/checks/{}", check_id_ok(check_id)?));
        Ok(LoupeCheck::from_json(&self.http.json(r).await?))
    }

    /// Wait for a check to finish.
    pub async fn wait(&self, check_id: &str, timeout: Duration) -> Result<LoupeCheck> {
        let deadline = Instant::now() + timeout;
        let mut delay = self.http.poll_interval();
        loop {
            let c = self.get(check_id).await?;
            if c.done() {
                return Ok(c);
            }
            if Instant::now() + delay > deadline {
                return Err(Error::CheckTimeout {
                    message: format!(
                        "check {check_id} is still {} after {timeout:?}; it carries on, so wait for it again later",
                        c.status
                    ),
                    job_id: Some(check_id.to_string()),
                });
            }
            tokio::time::sleep(delay).await;
            delay = delay.mul_f64(1.5).min(Duration::from_secs(5));
        }
    }

    /// One page of checks, newest first (or by queue).
    pub async fn list(&self, filters: &LoupeFilters) -> Result<Page<LoupeCheck>> {
        let r = Request::get("/loupe/checks")
            .opt("queue", filters.queue.as_ref())
            .opt("assigned_to", filters.assigned_to.as_ref())
            .opt("q", filters.q.as_ref())
            .opt("cursor", filters.cursor.as_ref())
            .opt("limit", filters.limit);
        let v = self.http.json(r).await?;
        Ok(Page::from_json(&v, "checks", LoupeCheck::from_json))
    }

    /// The check as a PDF to keep with your case.
    pub async fn evidence(&self, check_id: &str) -> Result<Vec<u8>> {
        let r = Request::get(format!("/loupe/checks/{}/evidence", check_id_ok(check_id)?))
            .accept("application/pdf");
        self.http.bytes(r).await
    }

    /// Record what the person who looked at the documents decided, and why.
    pub async fn review(&self, check_id: &str, outcome: Outcome, note: &str) -> Result<LoupeCheck> {
        let r = Request::post(format!("/loupe/checks/{}/review", check_id_ok(check_id)?))
            .json(json!({ "outcome": outcome.as_str(), "note": note }));
        Ok(LoupeCheck::from_json(&self.http.json(r).await?))
    }

    /// Give the check to `me` or a member, or take it off whoever has it with `None`.
    pub async fn assign(&self, check_id: &str, to: Option<&str>) -> Result<LoupeCheck> {
        let r = Request::post(format!("/loupe/checks/{}/assign", check_id_ok(check_id)?))
            .json(json!({ "to": to }));
        Ok(LoupeCheck::from_json(&self.http.json(r).await?))
    }
}

fn check_id_ok(id: &str) -> Result<&str> {
    check_id(id, "check id")
}

async fn check_offline(o: &OfflineSettings, files: Vec<File>, opts: LoupeOptions) -> Result<LoupeReport> {
    if opts.loupe_version.is_some() {
        return Err(Error::not_supported(
            "offline, checks use the version of unforged-loupe you run",
        ));
    }
    let runner = o.loupe()?;
    let scratch = runner.scratch()?;
    let tmp = scratch.path();
    let mut args = vec!["check".to_string()];
    for (i, f) in files.iter().enumerate() {
        let p = match f {
            File::Path(p) if p.is_dir() => p.clone(),
            File::Bytes { name: Some(n), data } => {
                let safe: String = n
                    .chars()
                    .map(|c| if c == '/' || c == '\\' { '-' } else { c })
                    .collect();
                let dir = tmp.join(format!("{}", i + 1));
                tokio::fs::create_dir(&dir).await?;
                let path = dir.join(safe);
                write_private(&path, data).await?;
                path
            }
            f => on_disk(f, tmp, &format!("document-{}", i + 1)).await?,
        };
        args.push(path_arg(&p));
    }
    if let Some(p) = opts.policy {
        let path = match p {
            Policy::File(p) => p,
            Policy::Json(v) => {
                let path = tmp.join("policy.json");
                write_private(&path, v.to_string().as_bytes()).await?;
                path
            }
            Policy::Named(_) => {
                return Err(Error::invalid(
                    "offline, the policy is a JSON file or value; saved policies live in your Unforged account",
                ))
            }
        };
        args.push("--policy".into());
        args.push(path_arg(&path));
    }
    if opts.lookups {
        args.push("--lookups".into());
    }
    let out = runner.run(args, opts.timeout, &[0]).await?;
    drop(scratch);
    let v: Value = serde_json::from_str(out.stdout.trim()).map_err(|_| Error::Offline {
        message: "the offline binary's answer wasn't JSON".into(),
        exit_code: Some(0),
        stderr: String::new(),
    })?;
    Ok(LoupeReport::from_offline(&v, out.notices))
}

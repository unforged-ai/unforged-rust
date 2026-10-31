//! The same client for code without an async runtime (the `blocking` feature). Each call waits
//! for its answer. Don't use it from inside an async runtime: use [`crate::Unforged`] there.
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::files::File;
use crate::loupe::{LoupeCheck, LoupeFilters, LoupeOptions, LoupeReport, Outcome};
use crate::nib::{
    CheckItem, CheckedItem, Confirmation, CreateOptions, Enrolment, ExtractOptions, Extraction,
    ItemFilters, Job, JobFilters, OfflineLicence, SignerMatch, Subject, SubjectSummary, Usage,
    Verification, VerifyOptions,
};
use crate::page::Page;
use crate::Mode;

/// One client for Unforged that waits for each answer.
#[derive(Clone)]
pub struct Unforged {
    inner: crate::Unforged,
    rt: Arc<tokio::runtime::Runtime>,
}

impl fmt::Debug for Unforged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("blocking::Unforged")
            .field(&self.inner)
            .finish()
    }
}

impl Unforged {
    /// Wraps a client set up with [`crate::Unforged::builder`].
    pub fn from_client(inner: crate::Unforged) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::Io)?;
        Ok(Self {
            inner,
            rt: Arc::new(rt),
        })
    }

    /// A client for the API with this key.
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        Self::from_client(crate::Unforged::new(api_key)?)
    }

    /// A client set up entirely from the environment.
    pub fn from_env() -> Result<Self> {
        Self::from_client(crate::Unforged::from_env()?)
    }

    /// An offline client, with the binaries and licence from the environment.
    pub fn offline() -> Result<Self> {
        Self::from_client(crate::Unforged::offline()?)
    }

    pub fn mode(&self) -> Mode {
        self.inner.mode()
    }

    pub fn base_url(&self) -> Option<String> {
        self.inner.base_url()
    }

    pub fn nib(&self) -> Nib<'_> {
        Nib { c: self }
    }

    pub fn loupe(&self) -> Loupe<'_> {
        Loupe { c: self }
    }
}

/// Nib, waiting for each answer.
#[derive(Debug, Clone, Copy)]
pub struct Nib<'a> {
    c: &'a Unforged,
}

impl<'a> Nib<'a> {
    pub fn verify(&self, file: impl Into<File>, options: VerifyOptions) -> Result<Verification> {
        self.c.rt.block_on(self.c.inner.nib().verify(file, options))
    }

    pub fn enrol<I, F>(&self, subject: &str, references: I, source: &str) -> Result<Enrolment>
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        self.c
            .rt
            .block_on(self.c.inner.nib().enrol(subject, references, source))
    }

    pub fn checks(&self) -> Result<Checks<'a>> {
        self.c.inner.nib().checks()?;
        Ok(Checks { c: self.c })
    }

    pub fn subjects(&self) -> Result<Subjects<'a>> {
        self.c.inner.nib().subjects()?;
        Ok(Subjects { c: self.c })
    }

    pub fn signer_match<P, S, D>(
        &self,
        people: P,
        documents: D,
        timeout: Option<Duration>,
    ) -> Result<SignerMatch>
    where
        P: IntoIterator<Item = (String, S)>,
        S: IntoIterator<Item = File>,
        D: IntoIterator<Item = File>,
    {
        self.c
            .rt
            .block_on(self.c.inner.nib().signer_match(people, documents, timeout))
    }

    pub fn usage(&self) -> Result<Usage> {
        self.c.rt.block_on(self.c.inner.nib().usage())
    }

    pub fn extract_signatures<I, F>(
        &self,
        files: I,
        out: impl Into<PathBuf>,
        options: ExtractOptions,
    ) -> Result<Vec<Extraction>>
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        self.c
            .rt
            .block_on(self.c.inner.nib().extract_signatures(files, out, options))
    }

    pub fn licence(&self) -> Result<OfflineLicence> {
        self.c.rt.block_on(self.c.inner.nib().licence())
    }
}

/// Runs `f` against the async API's `Checks`, waiting for the answer.
macro_rules! on {
    ($c:expr, $part:ident, $sub:ident, |$x:ident| $body:expr) => {{
        let api = $c.inner.$part();
        let $x = api.$sub()?;
        $c.rt.block_on($body)
    }};
}

/// Check jobs (online only), waiting for each answer.
#[derive(Debug, Clone, Copy)]
pub struct Checks<'a> {
    c: &'a Unforged,
}

impl Checks<'_> {
    pub fn create(&self, items: Vec<CheckItem>, options: CreateOptions) -> Result<Job> {
        on!(self.c, nib, checks, |x| x.create(items, options))
    }

    pub fn get(
        &self,
        job_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        decision: Option<&str>,
    ) -> Result<Job> {
        on!(self.c, nib, checks, |x| x
            .get(job_id, cursor, limit, decision))
    }

    pub fn results(&self, job_id: &str, decision: Option<&str>) -> Result<Vec<Verification>> {
        on!(self.c, nib, checks, |x| x.results(job_id, decision))
    }

    pub fn wait(&self, job_id: &str, timeout: Duration) -> Result<Job> {
        on!(self.c, nib, checks, |x| x.wait(job_id, timeout))
    }

    pub fn list(&self, filters: &JobFilters) -> Result<Page<Job>> {
        on!(self.c, nib, checks, |x| x.list(filters))
    }

    pub fn items(&self, filters: &ItemFilters) -> Result<Page<CheckedItem>> {
        on!(self.c, nib, checks, |x| x.items(filters))
    }

    pub fn summary(&self, filters: &ItemFilters) -> Result<Value> {
        on!(self.c, nib, checks, |x| x.summary(filters))
    }

    pub fn confirm(&self, job_id: &str, index: u32) -> Result<Confirmation> {
        on!(self.c, nib, checks, |x| x.confirm(job_id, index))
    }

    pub fn image(&self, job_id: &str, index: u32, signature: Option<u32>) -> Result<Vec<u8>> {
        on!(self.c, nib, checks, |x| x.image(job_id, index, signature))
    }

    pub fn specimen_image(&self, job_id: &str, index: u32, n: u32) -> Result<Vec<u8>> {
        on!(self.c, nib, checks, |x| x.specimen_image(job_id, index, n))
    }
}

/// Subjects and their references (online only), waiting for each answer.
#[derive(Debug, Clone, Copy)]
pub struct Subjects<'a> {
    c: &'a Unforged,
}

impl Subjects<'_> {
    pub fn list(
        &self,
        q: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Page<SubjectSummary>> {
        on!(self.c, nib, subjects, |x| x.list(q, cursor, limit))
    }

    pub fn get(&self, subject_reference: &str) -> Result<Subject> {
        on!(self.c, nib, subjects, |x| x.get(subject_reference))
    }

    pub fn enrol<I, F>(
        &self,
        subject_reference: &str,
        references: I,
        source: &str,
    ) -> Result<Enrolment>
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        on!(self.c, nib, subjects, |x| x.enrol(
            subject_reference,
            references,
            source
        ))
    }

    pub fn enrol_named(
        &self,
        subject_reference: &str,
        references: Vec<(String, File)>,
        source: &str,
    ) -> Result<Enrolment> {
        on!(self.c, nib, subjects, |x| x.enrol_named(
            subject_reference,
            references,
            source
        ))
    }

    pub fn delete(&self, subject_reference: &str) -> Result<()> {
        on!(self.c, nib, subjects, |x| x.delete(subject_reference))
    }

    pub fn delete_reference(&self, subject_reference: &str, ref_id: &str) -> Result<()> {
        on!(self.c, nib, subjects, |x| x
            .delete_reference(subject_reference, ref_id))
    }

    pub fn reference_image(&self, subject_reference: &str, ref_id: &str) -> Result<Vec<u8>> {
        on!(self.c, nib, subjects, |x| x
            .reference_image(subject_reference, ref_id))
    }

    pub fn unlearn_since(&self, subject_reference: &str, since: &str) -> Result<Vec<String>> {
        on!(self.c, nib, subjects, |x| x
            .unlearn_since(subject_reference, since))
    }

    pub fn unlearn_check(&self, subject_reference: &str, check: &str) -> Result<Vec<String>> {
        on!(self.c, nib, subjects, |x| x
            .unlearn_check(subject_reference, check))
    }
}

/// Loupe, waiting for each answer.
#[derive(Debug, Clone, Copy)]
pub struct Loupe<'a> {
    c: &'a Unforged,
}

impl<'a> Loupe<'a> {
    pub fn check<I, F>(&self, files: I, options: LoupeOptions) -> Result<LoupeReport>
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        self.c
            .rt
            .block_on(self.c.inner.loupe().check(files, options))
    }

    pub fn checks(&self) -> Result<LoupeChecks<'a>> {
        self.c.inner.loupe().checks()?;
        Ok(LoupeChecks { c: self.c })
    }

    pub fn licence(&self) -> Result<OfflineLicence> {
        self.c.rt.block_on(self.c.inner.loupe().licence())
    }
}

/// Loupe checks through the API, waiting for each answer.
#[derive(Debug, Clone, Copy)]
pub struct LoupeChecks<'a> {
    c: &'a Unforged,
}

impl LoupeChecks<'_> {
    pub fn create(&self, files: Vec<File>, options: &LoupeOptions) -> Result<LoupeCheck> {
        on!(self.c, loupe, checks, |x| x.create(files, options))
    }

    pub fn get(&self, check_id: &str) -> Result<LoupeCheck> {
        on!(self.c, loupe, checks, |x| x.get(check_id))
    }

    pub fn wait(&self, check_id: &str, timeout: Duration) -> Result<LoupeCheck> {
        on!(self.c, loupe, checks, |x| x.wait(check_id, timeout))
    }

    pub fn list(&self, filters: &LoupeFilters) -> Result<Page<LoupeCheck>> {
        on!(self.c, loupe, checks, |x| x.list(filters))
    }

    pub fn evidence(&self, check_id: &str) -> Result<Vec<u8>> {
        on!(self.c, loupe, checks, |x| x.evidence(check_id))
    }

    pub fn review(&self, check_id: &str, outcome: Outcome, note: &str) -> Result<LoupeCheck> {
        on!(self.c, loupe, checks, |x| x.review(check_id, outcome, note))
    }

    pub fn assign(&self, check_id: &str, to: Option<&str>) -> Result<LoupeCheck> {
        on!(self.c, loupe, checks, |x| x.assign(check_id, to))
    }
}

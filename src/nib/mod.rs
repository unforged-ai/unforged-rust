//! Nib: check handwritten signatures, through the API or offline.
mod hosted;
mod models;
mod offline;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

pub use hosted::{CheckItem, Checks, CreateOptions, ItemFilters, JobFilters, Subjects};
pub use models::{
    CheckedItem, Confirmation, Decision, Enrolment, EnrolledReference, ExtractedSignature,
    Extraction, ItemError, Job, OfflineLicence, Quality, Reference, ReferenceScore, Signature,
    SignerMatch, Subject, SubjectSummary, Usage, Verification,
};

use crate::client::{Backend, Unforged};
use crate::error::{Error, Result};
use crate::files::{File, Target};
use crate::Policy;

/// What to check a document against, and how.
#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    pub subject: Option<String>,
    pub references: Vec<File>,
    pub target: Option<Target>,
    pub template_id: Option<String>,
    pub policy: Option<Policy>,
    pub reference: Option<String>,
    pub process: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub save_crops: Option<PathBuf>,
    pub timeout: Option<Duration>,
}

impl VerifyOptions {
    /// Against an enrolled person's references (offline: those your reference source finds).
    pub fn subject(subject: impl Into<String>) -> Self {
        Self {
            subject: Some(subject.into()),
            ..Self::default()
        }
    }

    /// Against these reference files (a folder stands for the files inside it).
    pub fn references<I, F>(files: I) -> Self
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        Self {
            references: files.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Adds reference files to compare with as well.
    pub fn with_references<I, F>(mut self, files: I) -> Self
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        self.references.extend(files.into_iter().map(Into::into));
        self
    }

    /// One place on a form to look at.
    pub fn target(mut self, target: Target) -> Self {
        self.target = Some(target);
        self
    }

    /// Use only this form template, instead of matching one.
    pub fn template_id(mut self, id: impl Into<String>) -> Self {
        self.template_id = Some(id.into());
        self
    }

    /// Online, one of your saved policies by name; offline, a policy file or JSON.
    pub fn policy(mut self, policy: impl Into<Policy>) -> Self {
        self.policy = Some(policy.into());
        self
    }

    /// Your own reference for the check, such as an application number.
    pub fn reference(mut self, reference: impl Into<String>) -> Self {
        self.reference = Some(reference.into());
        self
    }

    /// The process the check belongs to, such as `loan-application`.
    pub fn process(mut self, process: impl Into<String>) -> Self {
        self.process = Some(process.into());
        self
    }

    /// A value kept with the check.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Save each signature found as a PNG in this folder.
    pub fn save_crops(mut self, dir: impl Into<PathBuf>) -> Self {
        self.save_crops = Some(dir.into());
        self
    }

    /// How long to wait for the answer.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Where offline `extract_signatures` looks.
#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    pub target: Option<Target>,
    pub template_id: Option<String>,
    pub timeout: Option<Duration>,
}

/// Nib, through the API or offline: the same calls either way.
#[derive(Debug, Clone, Copy)]
pub struct Nib<'a> {
    client: &'a Unforged,
}

const ONLINE_ONLY_SUBJECTS: &str = "offline, references stay in your own systems and the SDK never stores them: pass them with VerifyOptions::references, or give the client a reference source";

impl<'a> Nib<'a> {
    pub(crate) fn new(client: &'a Unforged) -> Self {
        Self { client }
    }

    /// Check the signature on a document, and wait for the answer.
    pub async fn verify(&self, file: impl Into<File>, options: VerifyOptions) -> Result<Verification> {
        if options.subject.is_none() && options.references.is_empty() {
            return Err(Error::invalid(
                "say what to compare with: a subject or reference files",
            ));
        }
        let file = file.into();
        match &*self.client.backend {
            Backend::Hosted(t) => hosted::verify(t, file, options).await,
            Backend::Offline(o) => offline::verify(o, file, options).await,
        }
    }

    /// Add reference signatures for a subject, named after their files (online only).
    pub async fn enrol<I, F>(&self, subject: &str, references: I, source: &str) -> Result<Enrolment>
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        self.subjects()?.enrol(subject, references, source).await
    }

    /// Check jobs: batches, history and confirmations (online only).
    pub fn checks(&self) -> Result<Checks<'a>> {
        Ok(Checks::new(self.client.hosted(
            "offline, verify answers straight away: there are no check jobs to list or read",
        )?))
    }

    /// The people you check and their references (online only).
    pub fn subjects(&self) -> Result<Subjects<'a>> {
        Ok(Subjects::new(self.client.hosted(ONLINE_ONLY_SUBJECTS)?))
    }

    /// Did these people sign these documents? `people` maps each person's label to their
    /// specimen signatures (online only).
    pub async fn signer_match<P, S, D>(
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
        let t = self
            .client
            .hosted("signer match isn't in the offline binary yet")?;
        hosted::signer_match(t, people, documents, timeout).await
    }

    /// This month's checks and the estimated charge (online only).
    pub async fn usage(&self) -> Result<Usage> {
        let t = self
            .client
            .hosted("offline checks aren't metered, so there's no usage to show")?;
        hosted::usage(t).await
    }

    /// Find every signature in the files and save each as a PNG in `out` (offline only).
    pub async fn extract_signatures<I, F>(
        &self,
        files: I,
        out: impl Into<PathBuf>,
        options: ExtractOptions,
    ) -> Result<Vec<Extraction>>
    where
        I: IntoIterator<Item = F>,
        F: Into<File>,
    {
        match &*self.client.backend {
            Backend::Offline(o) => {
                let files = files.into_iter().map(Into::into).collect();
                offline::extract(o, files, out.into(), options).await
            }
            Backend::Hosted(_) => Err(Error::not_supported(
                "extract_signatures runs offline; online, verify with save_crops saves each signature",
            )),
        }
    }

    /// Who this offline copy is licensed to, what it can do, and until when (offline only).
    pub async fn licence(&self) -> Result<OfflineLicence> {
        match &*self.client.backend {
            Backend::Offline(o) => o.nib()?.licence().await,
            Backend::Hosted(_) => Err(Error::not_supported(
                "licence is for the offline binary",
            )),
        }
    }
}

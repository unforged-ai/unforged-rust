//! Licences for the offline binaries: your main licence, and the short-lived licences you give
//! your own clients, from your workspace (online) or from your licence server (offline).
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::client::{Backend, Unforged};
use crate::error::{Error, Result};
use crate::files::seg;
use crate::http::{Request, Transport};
use crate::model::{decode, list, Secret};
use crate::offline::{path_arg, Licence, Runner};

/// What a short-lived licence lets its holder run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Include {
    /// Nib: checking signatures and finding them.
    Nib,
    /// Nib's signature checks only.
    Verify,
    /// Nib's signature finding only.
    Extract,
    /// Loupe's document checks.
    Loupe,
}

impl Include {
    fn names(self) -> &'static [&'static str] {
        match self {
            Include::Nib => &["verify", "extract"],
            Include::Verify => &["verify"],
            Include::Extract => &["extract"],
            Include::Loupe => &["loupe"],
        }
    }
}

/// A short-lived licence to give: who it's for, from when, for how many days, and what it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLicence {
    pub for_name: String,
    /// The first day, `YYYY-MM-DD` (today when not given).
    pub from: Option<String>,
    /// Days it runs, counting the first and last (30, or the longest you may give if less).
    pub days: Option<u32>,
    /// What it runs (everything you may give when empty).
    pub includes: Vec<Include>,
}

impl NewLicence {
    /// A licence for this person or business; their licence shows the name.
    pub fn new(for_name: impl Into<String>) -> Self {
        Self {
            for_name: for_name.into(),
            from: None,
            days: None,
            includes: Vec::new(),
        }
    }

    pub fn from(mut self, day: impl Into<String>) -> Self {
        self.from = Some(day.into());
        self
    }

    pub fn days(mut self, days: u32) -> Self {
        self.days = Some(days);
        self
    }

    pub fn includes(mut self, what: impl IntoIterator<Item = Include>) -> Self {
        self.includes.extend(what);
        self
    }

    fn names(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for n in self.includes.iter().flat_map(|i| i.names()) {
            if !out.contains(n) {
                out.push(n);
            }
        }
        out
    }

    fn checked_from(&self) -> Result<Option<i64>> {
        match self.from.as_deref() {
            None => Ok(None),
            Some(d) => crate::days::parse(d).map(Some).ok_or_else(|| {
                Error::invalid(format!("give the first day as YYYY-MM-DD, not {d:?}"))
            }),
        }
    }

    fn check(&self) -> Result<()> {
        if self.for_name.trim().is_empty() {
            return Err(Error::invalid("say who the licence is for"));
        }
        if self.days == Some(0) {
            return Err(Error::invalid("a licence runs for at least one day"));
        }
        self.checked_from().map(drop)
    }
}

/// A short-lived licence you gave.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ShortLivedLicence {
    pub id: String,
    /// Who it's for, as their licence shows it.
    #[serde(rename = "for")]
    pub for_name: String,
    /// `verify`, `extract`, `loupe`.
    pub includes: Vec<String>,
    pub valid_from: String,
    /// The last day it works.
    pub valid_until: String,
    /// `scheduled`, `active`, `ended` or `revoked`.
    pub status: String,
    /// The licence text to give them: when it's given, and from [`Licensing::licence`].
    #[serde(alias = "licence")]
    pub token: Secret,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ShortLivedLicence {
    fn with_status(mut self) -> Self {
        if self.status.is_empty() {
            let today = crate::days::iso(crate::days::today());
            self.status = if today < self.valid_from {
                "scheduled"
            } else if today > self.valid_until {
                "ended"
            } else {
                "active"
            }
            .into();
        }
        self
    }
}

/// What you may give your own clients.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ShortLivedTerms {
    /// Most that can be in use at once.
    pub at_once: u32,
    /// The longest one may run, counting its first and last day.
    pub longest_days: u32,
    pub includes: Vec<String>,
    /// The last day one may run to.
    pub until: String,
    /// How many count towards `at_once` today.
    pub counted: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Your organisation's main licence for the offline binaries.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MainLicence {
    pub id: String,
    /// `licence` or `trial`.
    pub kind: String,
    pub issued_to: String,
    pub issued_by: String,
    pub issued_on: String,
    pub valid_from: String,
    pub valid_until: String,
    /// The last day it works offline, including the time it allows for renewing.
    pub works_until: String,
    /// `verify`, `extract`, `loupe`.
    pub includes: Vec<String>,
    /// Your licence includes the licence server.
    pub licence_server: bool,
    /// What you may give your own clients; `None` when your plan doesn't include it.
    pub short_lived: Option<ShortLivedTerms>,
    /// The licence text: from [`Licensing::master`] only.
    pub token: Secret,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A link that lets a client download the offline binaries for their licence.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DownloadLink {
    pub id: String,
    pub licence_id: String,
    #[serde(rename = "for")]
    pub for_name: String,
    pub version: String,
    pub platform: String,
    pub expires_at: String,
    /// The page your client opens.
    pub url: Option<String>,
    /// The link's secret, shown only now.
    pub token: Secret,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One file of a release.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ReleaseFile {
    pub name: String,
    /// `nib` or `loupe`; `None` for `SHA256SUMS`.
    pub program: Option<String>,
    pub platform: Option<String>,
    pub size: u64,
    pub sha256: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One version of the offline binaries.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Release {
    pub version: String,
    pub published_at: Option<String>,
    pub notes: Option<String>,
    pub platforms: Vec<String>,
    pub files: Vec<ReleaseFile>,
    /// The platforms the licence server is built for, when your licence includes it.
    pub licence_server: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The versions you can download.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Releases {
    /// Your main licence still works; downloads stop while it doesn't.
    pub open: bool,
    /// The newest version, until someone in your organisation downloads it.
    pub new_version: Option<String>,
    pub releases: Vec<Release>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A short-lived link to one release file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ReleaseFileLink {
    pub version: String,
    pub file_name: String,
    pub expires_in_seconds: u64,
    pub url: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What your licence server has loaded, and what it can give today.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ServerStatus {
    /// It can give licences now.
    pub ready: bool,
    pub customer: Option<String>,
    pub label: Option<String>,
    pub main_licence_until: Option<String>,
    pub approval_until: Option<String>,
    /// The last day a licence given today may run to.
    pub last_day: Option<String>,
    pub longest_days: Option<u32>,
    /// Why it can't give licences, when it can't.
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What a licence is, and whether it works on this computer today.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct LicenceTest {
    /// It works here today.
    pub works: bool,
    /// `works`, or why it doesn't.
    pub state: String,
    /// `licence`, `trial`, `short-lived`, or the licence server's own approval.
    pub is: String,
    /// `unforged`, `workspace` or `licence_server`.
    pub made_by: String,
    pub id: String,
    #[serde(rename = "for")]
    pub for_name: Option<String>,
    pub issued_to: String,
    pub valid_from: String,
    pub valid_until: String,
    pub works_until: String,
    pub includes: Vec<String>,
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Licences, from your workspace (online) or your licence server (offline).
#[derive(Debug, Clone, Copy)]
pub struct Licensing<'a>(pub(crate) &'a Unforged);

impl Licensing<'_> {
    fn online(&self, what: &str) -> Result<&Transport> {
        self.0.hosted(what)
    }

    fn server(&self) -> Result<Runner> {
        match &*self.0.backend {
            Backend::Offline(o) => o.licence_server(),
            Backend::Hosted(_) => Err(Error::not_supported(
                "your licence server runs on your own machines: use an offline client to reach it",
            )),
        }
    }

    /// Gives a short-lived licence: online from your workspace, offline from your licence server.
    pub async fn give(&self, new: NewLicence) -> Result<ShortLivedLicence> {
        new.check()?;
        match &*self.0.backend {
            Backend::Hosted(t) => give_online(t, &new).await,
            Backend::Offline(_) => give_offline(&self.server()?, &new).await,
        }
    }

    /// Every short-lived licence given (offline: those your licence server gave, without their
    /// text).
    pub async fn licences(&self) -> Result<Vec<ShortLivedLicence>> {
        match &*self.0.backend {
            Backend::Hosted(t) => {
                let v = t.json(Request::get("/licensing/licences")).await?;
                list(&v, "licences")
            }
            Backend::Offline(_) => {
                let out = run(&self.server()?, vec!["list".into(), "--json".into()], &[0]).await?;
                let all: Vec<ShortLivedLicence> = decode(parse_json(&out)?)?;
                Ok(all
                    .into_iter()
                    .map(ShortLivedLicence::with_status)
                    .collect())
            }
        }
    }

    /// Your licence server: what it has loaded and what it can give today (offline only).
    pub async fn status(&self) -> Result<ServerStatus> {
        let out = run(
            &self.server()?,
            vec!["status".into(), "--json".into()],
            &[0, 3],
        )
        .await?;
        match parse_json(&out) {
            Ok(v) => decode(v),
            // nothing set up yet: it says so in a sentence rather than JSON
            Err(Error::Licence { message }) => Ok(ServerStatus {
                message: Some(message),
                ..ServerStatus::default()
            }),
            Err(e) => Err(e),
        }
    }

    /// Loads a file into your licence server: its pass (`server.lic`) or your main licence
    /// (offline only). A server given both through the client needs nothing loaded.
    pub async fn load(&self, file: impl AsRef<std::path::Path>) -> Result<()> {
        let args = vec!["load".to_string(), path_arg(file.as_ref())];
        run(&self.server()?, args, &[0]).await.map(drop)
    }

    /// Tests any Unforged licence: what it is, and whether it works on this computer today. It
    /// runs your licence server's own test, so it needs an offline client.
    pub async fn test(&self, licence: Licence) -> Result<LicenceTest> {
        let mut runner = self.server()?;
        let mut args = vec!["test".to_string()];
        match licence {
            Licence::File(p) => args.push(path_arg(&p)),
            text => runner.licence = Some(text),
        }
        args.push("--json".into());
        let out = run(&runner, args, &[0, 3, 4]).await?;
        decode(parse_json(&out)?)
    }

    /// Your main licence, without its text (online only).
    pub async fn main_licence(&self) -> Result<MainLicence> {
        let t = self.online(ONLINE_ONLY)?;
        decode(t.json(Request::get("/licensing")).await?)
    }

    /// Your main licence with its text, to load into the binaries or your licence server
    /// (online only).
    pub async fn master(&self) -> Result<MainLicence> {
        let t = self.online(ONLINE_ONLY)?;
        decode(t.json(Request::get("/licensing/master")).await?)
    }

    /// One short-lived licence, with its text (online only).
    pub async fn licence(&self, licence_id: &str) -> Result<ShortLivedLicence> {
        let t = self.online(ONLINE_ONLY)?;
        let path = format!("/licensing/licences/{}", seg(licence_id, "licence id")?);
        decode(t.json(Request::get(path)).await?)
    }

    /// Stops a short-lived licence counting towards your limit (online only). A copy already
    /// given keeps working offline until its last day.
    pub async fn revoke(&self, licence_id: &str) -> Result<ShortLivedLicence> {
        let t = self.online(
            "a licence your licence server gave works until its last day: give shorter ones",
        )?;
        let path = format!(
            "/licensing/licences/{}/revoke",
            seg(licence_id, "licence id")?
        );
        decode(t.json(Request::post(path)).await?)
    }

    /// A link your client opens to download the binaries for their licence (online only).
    /// `version` may be `latest`.
    pub async fn download_link(
        &self,
        licence_id: &str,
        version: &str,
        platform: &str,
    ) -> Result<DownloadLink> {
        let t = self.online(ONLINE_ONLY)?;
        let path = format!(
            "/licensing/licences/{}/download-links",
            seg(licence_id, "licence id")?
        );
        let body = json!({ "version": version, "platform": platform });
        decode(t.json(Request::post(path).json(body)).await?)
    }

    /// The versions of the offline binaries you can download (online only).
    pub async fn releases(&self) -> Result<Releases> {
        let t = self.online(ONLINE_ONLY)?;
        decode(t.json(Request::get("/licensing/releases")).await?)
    }

    /// A short-lived link to one file of a release; `version` may be `latest` (online only).
    pub async fn release_file(&self, version: &str, file_name: &str) -> Result<ReleaseFileLink> {
        let t = self.online(ONLINE_ONLY)?;
        let path = format!(
            "/licensing/releases/{}/files/{}",
            seg(version, "version")?,
            seg(file_name, "file name")?
        );
        decode(t.json(Request::get(path)).await?)
    }
}

const ONLINE_ONLY: &str =
    "this comes from your Unforged workspace, so it needs an online client with your API key";

async fn give_online(t: &Transport, new: &NewLicence) -> Result<ShortLivedLicence> {
    let mut includes = new.names();
    let mut days = new.days;
    if includes.is_empty() || days.is_none() {
        let main: MainLicence = decode(t.json(Request::get("/licensing")).await?)?;
        let terms = main.short_lived.ok_or_else(|| {
            Error::not_supported("your plan doesn't include short-lived licences")
        })?;
        if includes.is_empty() {
            includes = terms
                .includes
                .iter()
                .filter_map(|n| ["verify", "extract", "loupe"].into_iter().find(|k| k == n))
                .collect();
        }
        days = days.or(Some(match terms.longest_days {
            0 => 30,
            n => n.min(30),
        }));
    }
    let from = new.checked_from()?.unwrap_or_else(crate::days::today);
    let until = from + i64::from(days.unwrap_or(30)) - 1;
    let body = json!({
        "for": new.for_name,
        "valid_from": crate::days::iso(from),
        "valid_until": crate::days::iso(until),
        "includes": includes,
    });
    decode(
        t.json(Request::post("/licensing/licences").json(body))
            .await?,
    )
}

async fn give_offline(server: &Runner, new: &NewLicence) -> Result<ShortLivedLicence> {
    // the server writes the licence to a file too; it lives only in this folder, which goes
    // when the call returns
    let scratch = server.scratch()?;
    let file: PathBuf = scratch.path().join("licence.lic");
    let mut args = vec!["issue".to_string(), "--for".into(), new.for_name.clone()];
    if let Some(d) = new.days {
        args.extend(["--days".into(), d.to_string()]);
    }
    if let Some(f) = &new.from {
        args.extend(["--from".into(), f.clone()]);
    }
    let names = new.names();
    if !names.is_empty() {
        args.extend(["--includes".into(), names.join(",")]);
    }
    args.extend(["--out".into(), path_arg(&file), "--json".into()]);
    let out = run(server, args, &[0]).await?;
    let mut v = parse_json(&out)?;
    if let Value::Object(m) = &mut v {
        m.remove("file");
    }
    Ok(decode::<ShortLivedLicence>(v)?.with_status())
}

/// Runs the licence server; its own messages come back as they are.
async fn run(server: &Runner, args: Vec<String>, ok: &[i32]) -> Result<crate::offline::Output> {
    server.run(args, None, ok).await.map_err(|e| match e {
        Error::CheckFailed { detail, .. } => Error::invalid(detail),
        other => other,
    })
}

/// The server's JSON answer; when it answered with only a sentence instead, that sentence.
fn parse_json(out: &crate::offline::Output) -> Result<Value> {
    if out.stdout.trim().is_empty() && !out.notices.is_empty() {
        return Err(Error::Licence {
            message: out.notices.join(" "),
        });
    }
    serde_json::from_str(out.stdout.trim()).map_err(|_| Error::Offline {
        message: "the licence server's answer wasn't JSON".into(),
        exit_code: Some(0),
        stderr: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nib_means_both_its_parts_once() {
        let n = NewLicence::new("Acme").includes([Include::Nib, Include::Verify, Include::Loupe]);
        assert_eq!(n.names(), vec!["verify", "extract", "loupe"]);
    }

    #[test]
    fn bad_licences_are_refused_before_anything_runs() {
        assert!(NewLicence::new(" ").check().is_err());
        assert!(NewLicence::new("Acme").days(0).check().is_err());
        assert!(NewLicence::new("Acme").from("1 Dec").check().is_err());
        assert!(NewLicence::new("Acme")
            .from("2026-12-01")
            .days(7)
            .check()
            .is_ok());
    }

    #[test]
    fn licences_without_a_status_take_one_from_their_dates() {
        let l = |from: &str, until: &str| {
            ShortLivedLicence {
                valid_from: from.into(),
                valid_until: until.into(),
                ..Default::default()
            }
            .with_status()
            .status
        };
        assert_eq!(l("2000-01-01", "2000-01-31"), "ended");
        assert_eq!(l("2000-01-01", "9999-01-01"), "active");
        assert_eq!(l("9998-01-01", "9999-01-01"), "scheduled");
    }
}

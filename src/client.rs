use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::error::{ApiError, ApiErrorKind, Error, Result};
use crate::http::{base_url_for, HttpConfig, Transport};
use crate::offline::{find_binary, Licence, ReferenceSource, Runner, LOUPE_BINARY, NIB_BINARY};

/// Where checks run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Through the Unforged API.
    Hosted,
    /// On this machine, with the binaries from the offline delivery pack.
    Offline,
}

impl Mode {
    fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "hosted" | "online" => Ok(Mode::Hosted),
            "offline" => Ok(Mode::Offline),
            other => Err(Error::invalid(format!(
                "mode must be 'hosted' or 'offline', not {other:?}"
            ))),
        }
    }
}

pub(crate) struct OfflineSettings {
    nib_binary: Option<PathBuf>,
    loupe_binary: Option<PathBuf>,
    licence: Option<Licence>,
    timeout: Option<Duration>,
    temp_dir: Option<PathBuf>,
    pub reference_source: Option<ReferenceSource>,
    pub templates: Vec<PathBuf>,
    pub auto_approve: bool,
}

impl OfflineSettings {
    fn runner(&self, given: &Option<PathBuf>, env: &str, name: &str) -> Result<Runner> {
        Ok(Runner {
            binary: find_binary(given.clone(), env, name)?,
            licence: self.licence.clone(),
            timeout: self.timeout,
            temp_dir: self.temp_dir.clone(),
        })
    }

    pub fn nib(&self) -> Result<Runner> {
        self.runner(&self.nib_binary, "UNFORGED_NIB_BINARY", NIB_BINARY)
    }

    pub fn loupe(&self) -> Result<Runner> {
        self.runner(&self.loupe_binary, "UNFORGED_LOUPE_BINARY", LOUPE_BINARY)
    }
}

pub(crate) enum Backend {
    Hosted(Transport),
    Offline(OfflineSettings),
}

/// One client for Unforged: through the API, or offline with the binaries from the delivery
/// pack. It's cheap to clone and safe to share between tasks.
#[derive(Clone)]
pub struct Unforged {
    pub(crate) backend: Arc<Backend>,
}

impl Unforged {
    /// A client for the API with this key.
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        Self::builder().api_key(api_key).build()
    }

    /// A client set up entirely from the environment: `UNFORGED_MODE` (`hosted`, the default, or
    /// `offline`) and the variables for that mode.
    pub fn from_env() -> Result<Self> {
        Self::builder().build()
    }

    /// An offline client, with the binaries and licence from the environment.
    pub fn offline() -> Result<Self> {
        Self::builder().mode(Mode::Offline).build()
    }

    /// Every setting, each falling back to its environment variable.
    pub fn builder() -> Builder {
        Builder::default()
    }

    pub fn mode(&self) -> Mode {
        match *self.backend {
            Backend::Hosted(_) => Mode::Hosted,
            Backend::Offline(_) => Mode::Offline,
        }
    }

    /// The API host in use (online only).
    pub fn base_url(&self) -> Option<String> {
        match &*self.backend {
            Backend::Hosted(t) => Some(t.base_url()),
            Backend::Offline(_) => None,
        }
    }

    /// Nib: signature checks.
    pub fn nib(&self) -> crate::nib::Nib<'_> {
        crate::nib::Nib::new(self)
    }

    /// Loupe: checks that documents haven't been altered or made up.
    pub fn loupe(&self) -> crate::loupe::Loupe<'_> {
        crate::loupe::Loupe::new(self)
    }

    pub(crate) fn hosted(&self, what: &str) -> Result<&Transport> {
        match &*self.backend {
            Backend::Hosted(t) => Ok(t),
            Backend::Offline(_) => Err(Error::not_supported(what.to_string())),
        }
    }
}

impl fmt::Debug for Unforged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &*self.backend {
            Backend::Hosted(t) => f
                .debug_struct("Unforged")
                .field("mode", &Mode::Hosted)
                .field("base_url", &t.base_url())
                .finish_non_exhaustive(),
            Backend::Offline(o) => f
                .debug_struct("Unforged")
                .field("mode", &Mode::Offline)
                .field("nib_binary", &o.nib_binary)
                .field("loupe_binary", &o.loupe_binary)
                .finish_non_exhaustive(),
        }
    }
}

/// Sets up an [`Unforged`] client. Anything left unset comes from the environment.
#[derive(Default, Clone)]
pub struct Builder {
    mode: Option<Mode>,
    api_key: Option<String>,
    region: Option<String>,
    base_url: Option<String>,
    api_version: Option<String>,
    org: Option<String>,
    timeout: Option<Duration>,
    max_retries: Option<u32>,
    poll_interval: Option<Duration>,
    nib_binary: Option<PathBuf>,
    loupe_binary: Option<PathBuf>,
    licence: Option<Licence>,
    reference_source: Option<ReferenceSource>,
    temp_dir: Option<PathBuf>,
    templates: Vec<PathBuf>,
    auto_approve: bool,
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

impl Builder {
    /// Hosted or offline (else `UNFORGED_MODE`, else hosted).
    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Your API key (else `UNFORGED_API_KEY`).
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Your region, as your API host names it (else `UNFORGED_REGION`).
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// The API's address, in place of the one for your region (else `UNFORGED_BASE_URL`).
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// The API version to use, as a date such as `2026-10-14` (else `UNFORGED_API_VERSION`, else
    /// your organisation's).
    pub fn api_version(mut self, version: impl Into<String>) -> Self {
        self.api_version = Some(version.into());
        self
    }

    /// Act for one of your child organisations.
    pub fn org(mut self, org: impl Into<String>) -> Self {
        self.org = Some(org.into());
        self
    }

    /// How long one request (online) or one run of a binary (offline) may take.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// How many times a busy or rate-limited request is tried again (2 unless set).
    pub fn max_retries(mut self, n: u32) -> Self {
        self.max_retries = Some(n);
        self
    }

    /// How often waiting calls ask whether a check has finished (1 second unless set).
    pub fn poll_interval(mut self, every: Duration) -> Self {
        self.poll_interval = Some(every);
        self
    }

    /// The `unforged-nib` binary (else `UNFORGED_NIB_BINARY`, else the one on `PATH`).
    pub fn nib_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.nib_binary = Some(path.into());
        self
    }

    /// The `unforged-loupe` binary (else `UNFORGED_LOUPE_BINARY`, else the one on `PATH`).
    pub fn loupe_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.loupe_binary = Some(path.into());
        self
    }

    /// The offline licence: the path to `nib.lic` or its contents (else `UNFORGED_LICENSE_FILE`
    /// or `UNFORGED_LICENSE`).
    pub fn licence(mut self, licence: impl Into<String>) -> Self {
        self.licence = Some(Licence::guess(licence));
        self
    }

    /// The offline licence, said exactly.
    pub fn licence_from(mut self, licence: Licence) -> Self {
        self.licence = Some(licence);
        self
    }

    /// Where offline checks find a subject's references (else `UNFORGED_REFERENCE_SOURCE`, a
    /// folder with `{subject}` in its path).
    pub fn reference_source(mut self, source: ReferenceSource) -> Self {
        self.reference_source = Some(source);
        self
    }

    /// Where files you pass as bytes are kept for the moment a check runs offline, such as an
    /// encrypted volume (else the system's temporary folder).
    pub fn temp_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.temp_dir = Some(dir.into());
        self
    }

    /// A form template JSON for offline checks (call again for more).
    pub fn template(mut self, path: impl Into<PathBuf>) -> Self {
        self.templates.push(path.into());
        self
    }

    /// Let offline checks reach `AUTO_APPROVE`.
    pub fn auto_approve(mut self, on: bool) -> Self {
        self.auto_approve = on;
        self
    }

    pub fn build(self) -> Result<Unforged> {
        let mode = match self.mode {
            Some(m) => m,
            None => Mode::parse(&env("UNFORGED_MODE").unwrap_or_default())?,
        };
        let backend = match mode {
            Mode::Offline => Backend::Offline(OfflineSettings {
                nib_binary: self.nib_binary,
                loupe_binary: self.loupe_binary,
                licence: self.licence.or_else(Licence::from_env),
                timeout: self.timeout,
                temp_dir: self.temp_dir,
                reference_source: self
                    .reference_source
                    .or_else(|| env("UNFORGED_REFERENCE_SOURCE").map(ReferenceSource::Folder)),
                templates: self.templates,
                auto_approve: self.auto_approve,
            }),
            Mode::Hosted => {
                let api_key = self
                    .api_key
                    .or_else(|| env("UNFORGED_API_KEY"))
                    .ok_or_else(|| {
                        Error::Api(Box::new(ApiError::local(
                            ApiErrorKind::Authentication,
                            "unauthorized",
                            "no API key: pass one to the client or set UNFORGED_API_KEY",
                        )))
                    })?;
                let base_url = self
                    .base_url
                    .or_else(|| env("UNFORGED_BASE_URL"))
                    .unwrap_or_else(|| {
                        base_url_for(self.region.or_else(|| env("UNFORGED_REGION")).as_deref())
                    });
                Backend::Hosted(Transport::new(HttpConfig {
                    api_key,
                    base_url,
                    api_version: self.api_version.or_else(|| env("UNFORGED_API_VERSION")),
                    org: self.org,
                    timeout: self.timeout.unwrap_or(Duration::from_secs(60)),
                    max_retries: self.max_retries.unwrap_or(2),
                    poll_interval: self.poll_interval.unwrap_or(Duration::from_secs(1)),
                })?)
            }
        };
        Ok(Unforged {
            backend: Arc::new(backend),
        })
    }
}

impl fmt::Debug for Builder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Builder")
            .field("mode", &self.mode)
            .field("api_key", &self.api_key.as_ref().map(|_| ".."))
            .field("base_url", &self.base_url)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_read_like_python() {
        assert_eq!(Mode::parse("online").unwrap(), Mode::Hosted);
        assert_eq!(Mode::parse(" OFFLINE ").unwrap(), Mode::Offline);
        assert!(Mode::parse("cloud").is_err());
    }

    #[test]
    fn debug_never_shows_the_key() {
        let c = Unforged::builder()
            .mode(Mode::Hosted)
            .api_key("unf_secret_key")
            .base_url("https://api.unforged.sh")
            .build()
            .unwrap();
        assert!(!format!("{c:?}").contains("unf_secret_key"));
        let b = Unforged::builder().api_key("unf_secret_key");
        assert!(!format!("{b:?}").contains("unf_secret_key"));
    }
}

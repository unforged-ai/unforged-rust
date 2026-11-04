use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::files::{self, File};

pub(crate) const NIB_BINARY: &str = "unforged-nib";
pub(crate) const LOUPE_BINARY: &str = "unforged-loupe";
pub(crate) const LICENCE_SERVER_BINARY: &str = "unforged-licence-server";

/// The offline licence: the file from the delivery pack, or its contents.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Licence {
    /// The path to `nib.lic`.
    File(PathBuf),
    /// The licence itself, the text inside `nib.lic`.
    Text(String),
}

impl Licence {
    /// Either the licence's path or its contents, told apart by their shape.
    pub fn guess(s: impl Into<String>) -> Self {
        let s = s.into();
        let t = s.trim();
        if !Path::new(t).is_file() && looks_like_token(t) {
            Licence::Text(t.to_string())
        } else {
            Licence::File(PathBuf::from(s))
        }
    }

    /// The licence's text, read from its file when it's a file.
    pub(crate) fn text(&self) -> Result<String> {
        match self {
            Licence::Text(t) => Ok(t.trim().to_string()),
            Licence::File(p) => std::fs::read_to_string(p)
                .map(|t| t.trim().to_string())
                .map_err(|e| {
                    Error::Io(std::io::Error::new(
                        e.kind(),
                        format!("{} couldn't be read: {e}", p.display()),
                    ))
                }),
        }
    }

    pub(crate) fn from_env() -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(p) = var("UNFORGED_LICENSE_FILE").or_else(|| var("UNFORGED_LICENCE_FILE")) {
            return Some(Licence::File(p.into()));
        }
        var("UNFORGED_LICENSE")
            .or_else(|| var("UNFORGED_LICENCE"))
            .map(|t| Licence::Text(t.trim().to_string()))
    }
}

impl fmt::Debug for Licence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Licence::File(p) => f.debug_tuple("File").field(p).finish(),
            Licence::Text(_) => f.write_str("Text(..)"),
        }
    }
}

/// Three base64url segments, the way a licence token reads.
fn looks_like_token(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    let ok = |p: &str, min: usize| {
        p.len() >= min
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    parts.len() == 3 && ok(parts[0], 10) && ok(parts[1], 10) && ok(parts[2], 1)
}

type SourceFn = dyn Fn(&str) -> Result<Vec<File>> + Send + Sync;

/// Where offline checks find a person's reference signatures in your own systems. The SDK only
/// ever reads from it.
#[derive(Clone)]
#[non_exhaustive]
pub enum ReferenceSource {
    /// A folder with `{subject}` in its path, such as `/secure/references/{subject}`.
    Folder(String),
    /// References by subject.
    Map(HashMap<String, Vec<File>>),
    /// A function that returns a subject's references.
    Function(Arc<SourceFn>),
}

impl ReferenceSource {
    /// A folder with `{subject}` in its path.
    pub fn folder(pattern: impl Into<String>) -> Self {
        ReferenceSource::Folder(pattern.into())
    }

    /// A function that returns a subject's references.
    pub fn function(f: impl Fn(&str) -> Result<Vec<File>> + Send + Sync + 'static) -> Self {
        ReferenceSource::Function(Arc::new(f))
    }

    pub(crate) fn find(&self, subject: &str) -> Result<Vec<File>> {
        let found = match self {
            ReferenceSource::Function(f) => f(subject)?,
            ReferenceSource::Map(m) => m.get(subject).cloned().unwrap_or_default(),
            ReferenceSource::Folder(pattern) => {
                if !pattern.contains("{subject}") {
                    return Err(Error::invalid(
                        "a reference source folder needs {subject} in its path",
                    ));
                }
                let dir = PathBuf::from(pattern.replace("{subject}", subject));
                if dir.is_dir() {
                    vec![File::Path(dir)]
                } else {
                    Vec::new()
                }
            }
        };
        files::expand(found)
    }
}

impl fmt::Debug for ReferenceSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReferenceSource::Folder(p) => f.debug_tuple("Folder").field(p).finish(),
            ReferenceSource::Map(m) => f.debug_tuple("Map").field(&m.len()).finish(),
            ReferenceSource::Function(_) => f.write_str("Function(..)"),
        }
    }
}

impl<K: Into<String>, V: IntoIterator<Item = F>, F: Into<File>> FromIterator<(K, V)>
    for ReferenceSource
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        ReferenceSource::Map(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into_iter().map(Into::into).collect()))
                .collect(),
        )
    }
}

/// Finds an offline binary: the one given, its environment variable, or the one on `PATH`.
pub(crate) fn find_binary(given: Option<PathBuf>, env: &str, name: &str) -> Result<PathBuf> {
    if let Some(b) = given {
        return Ok(b);
    }
    if let Some(b) = std::env::var_os(env).filter(|v| !v.is_empty()) {
        return Ok(b.into());
    }
    which(name).ok_or_else(|| Error::Offline {
        message: format!("couldn't find {name}: pass its path to the client or set {env}"),
        exit_code: None,
        stderr: String::new(),
    })
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&path)
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
}

/// What a run of a binary printed.
pub(crate) struct Output {
    pub stdout: String,
    /// The licence's own notices on stderr, such as a term ending soon.
    pub notices: Vec<String>,
}

/// Runs one offline binary under the licence.
#[derive(Clone)]
pub(crate) struct Runner {
    pub binary: PathBuf,
    pub licence: Option<Licence>,
    pub timeout: Option<Duration>,
    pub temp_dir: Option<PathBuf>,
    /// Secrets the binary reads from its environment.
    pub env: Vec<(&'static str, String)>,
}

impl fmt::Debug for Runner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runner")
            .field("binary", &self.binary)
            .field("licence", &self.licence)
            .finish_non_exhaustive()
    }
}

impl Runner {
    fn name(&self) -> String {
        self.binary
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.binary.display().to_string())
    }

    /// Runs the binary with `args`; exit codes in `ok` count as success.
    pub async fn run(
        &self,
        args: Vec<String>,
        timeout: Option<Duration>,
        ok: &[i32],
    ) -> Result<Output> {
        let mut cmd = tokio::process::Command::new(&self.binary);
        match &self.licence {
            Some(Licence::File(p)) => {
                cmd.arg("--license-file").arg(p);
            }
            // through the environment, never the command line, which other users can see
            Some(Licence::Text(t)) => {
                cmd.env("UNFORGED_LICENSE", t);
            }
            None => {}
        }
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let name = self.name();
        let fail = |message: String| Error::Offline {
            message,
            exit_code: None,
            stderr: String::new(),
        };
        let run = cmd.output();
        let out = match timeout.or(self.timeout) {
            Some(limit) => tokio::time::timeout(limit, run)
                .await
                .map_err(|_| fail(format!("{name} took longer than {limit:?}")))?,
            None => run.await,
        }
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => fail(format!(
                "couldn't run {}: no such file",
                self.binary.display()
            )),
            std::io::ErrorKind::PermissionDenied => fail(format!(
                "couldn't run {}: it isn't executable",
                self.binary.display()
            )),
            _ => fail(format!("couldn't run {}: {e}", self.binary.display())),
        })?;
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let code = out.status.code();
        if code.is_some_and(|c| ok.contains(&c)) {
            return Ok(Output {
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                notices: stderr.lines().map(str::to_owned).collect(),
            });
        }
        let message = if stderr.is_empty() {
            match code {
                Some(c) => format!("{name} exited with {c}"),
                None => format!("{name} was stopped before it finished"),
            }
        } else {
            stderr.clone()
        };
        Err(match code {
            Some(3) => Error::Licence { message },
            Some(4) => Error::NotLicensed { message },
            Some(1) => Error::CheckFailed {
                kind: "unreadable".into(),
                detail: last_error_line(&message),
                job_id: None,
            },
            _ => Error::Offline {
                message,
                exit_code: code,
                stderr,
            },
        })
    }

    /// A private folder for files that exist only while a check runs.
    pub fn scratch(&self) -> Result<tempfile::TempDir> {
        let mut b = tempfile::Builder::new();
        b.prefix("unforged-");
        let dir = match &self.temp_dir {
            Some(d) => b.tempdir_in(d)?,
            None => b.tempdir()?,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(dir)
    }

    pub async fn licence(&self) -> Result<crate::nib::OfflineLicence> {
        let out = self
            .run(vec!["licence".into(), "--json".into()], None, &[0])
            .await?;
        let v: Value = serde_json::from_str(out.stdout.trim()).map_err(|_| Error::Offline {
            message: format!("{}'s licence answer wasn't JSON", self.name()),
            exit_code: Some(0),
            stderr: String::new(),
        })?;
        Ok(crate::nib::OfflineLicence::from_json(v, out.notices))
    }
}

fn last_error_line(s: &str) -> String {
    let line = s.trim().lines().last().unwrap_or("").trim();
    line.strip_prefix("error:")
        .unwrap_or(line)
        .trim()
        .to_string()
}

/// The file on disk: its own path, or `data` written to a private file in `dir` as `name`.
pub(crate) async fn on_disk(f: &File, dir: &Path, name: &str) -> Result<PathBuf> {
    match f {
        File::Url(_) => Err(Error::not_supported(
            "offline checks read local files only; download the file first and pass its path",
        )),
        File::PartName(_) => Err(Error::not_supported(
            "offline, files from earlier uploads don't exist; pass the file itself",
        )),
        File::Path(p) => {
            if p.is_file() {
                Ok(p.clone())
            } else {
                Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{} isn't a file", p.display()),
                )))
            }
        }
        File::Bytes { data, .. } => {
            let path = dir.join(format!("{name}{}", files::extension(data)));
            write_private(&path, data).await?;
            Ok(path)
        }
    }
}

pub(crate) async fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut fh = opts.open(path).await?;
    tokio::io::AsyncWriteExt::write_all(&mut fh, data).await?;
    tokio::io::AsyncWriteExt::flush(&mut fh).await?;
    Ok(())
}

pub(crate) fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licence_text_and_paths_are_told_apart() {
        let token = "eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJhY21lIn0.c2lnbmF0dXJl";
        assert_eq!(Licence::guess(token), Licence::Text(token.into()));
        assert_eq!(
            Licence::guess("/opt/unforged/nib.lic"),
            Licence::File("/opt/unforged/nib.lic".into())
        );
        assert_eq!(format!("{:?}", Licence::Text(token.into())), "Text(..)");
    }

    #[test]
    fn error_lines_lose_their_prefix() {
        assert_eq!(
            last_error_line("warning: x\nerror: not a document"),
            "not a document"
        );
    }

    #[test]
    fn folder_sources_need_the_subject() {
        let s = ReferenceSource::folder("/nowhere");
        assert!(matches!(s.find("a"), Err(Error::InvalidInput(_))));
        let s = ReferenceSource::folder("/nowhere/{subject}");
        assert!(s.find("a").unwrap().is_empty());
    }
}

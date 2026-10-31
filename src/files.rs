use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// A file to check or enrol: a path, bytes you already hold, or (online) a URL from your
/// allowlist.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum File {
    /// A file on disk. A folder stands for the files directly inside it, in name order.
    Path(PathBuf),
    /// Bytes you already hold, with the file name to send them under, if any.
    Bytes { name: Option<String>, data: Vec<u8> },
    /// A file we fetch from your allowlisted URL (online only).
    Url(String),
    /// A file already uploaded with an earlier job, by the name it was sent under.
    PartName(String),
}

impl File {
    /// A file on disk.
    pub fn path(p: impl Into<PathBuf>) -> Self {
        File::Path(p.into())
    }

    /// Bytes you already hold.
    pub fn bytes(data: impl Into<Vec<u8>>) -> Self {
        File::Bytes {
            name: None,
            data: data.into(),
        }
    }

    /// Bytes you already hold, sent under a file name.
    pub fn named(name: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        File::Bytes {
            name: Some(name.into()),
            data: data.into(),
        }
    }

    /// A file at a URL on your allowlist, fetched by us (online only).
    pub fn url(url: impl Into<String>) -> Self {
        File::Url(url.into())
    }

    /// A file sent with an earlier job, by the name it was sent under (a signer match's
    /// `from_job`).
    pub fn part_name(name: impl Into<String>) -> Self {
        File::PartName(name.into())
    }

    pub(crate) fn local_path(&self) -> Option<&Path> {
        match self {
            File::Path(p) => Some(p),
            _ => None,
        }
    }

    /// The file's name and contents, read from disk when it's a path.
    pub(crate) async fn read(&self) -> Result<(Option<String>, Vec<u8>)> {
        match self {
            File::Path(p) => {
                let data = tokio::fs::read(p).await.map_err(|e| {
                    Error::Io(std::io::Error::new(
                        e.kind(),
                        format!("{} can't be read: {e}", p.display()),
                    ))
                })?;
                Ok((
                    p.file_name().map(|n| n.to_string_lossy().into_owned()),
                    data,
                ))
            }
            File::Bytes { name, data } => Ok((name.clone(), data.clone())),
            File::Url(_) | File::PartName(_) => Err(Error::invalid(
                "a URL or an earlier upload has no local contents to read",
            )),
        }
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            File::Path(p) => f.debug_tuple("Path").field(p).finish(),
            File::Bytes { name, data } => f
                .debug_struct("Bytes")
                .field("name", name)
                .field("len", &data.len())
                .finish(),
            File::Url(u) => f.debug_tuple("Url").field(u).finish(),
            File::PartName(n) => f.debug_tuple("PartName").field(n).finish(),
        }
    }
}

impl From<&str> for File {
    fn from(p: &str) -> Self {
        File::Path(p.into())
    }
}

impl From<String> for File {
    fn from(p: String) -> Self {
        File::Path(p.into())
    }
}

impl From<&String> for File {
    fn from(p: &String) -> Self {
        File::Path(p.into())
    }
}

impl From<PathBuf> for File {
    fn from(p: PathBuf) -> Self {
        File::Path(p)
    }
}

impl From<&PathBuf> for File {
    fn from(p: &PathBuf) -> Self {
        File::Path(p.clone())
    }
}

impl From<&Path> for File {
    fn from(p: &Path) -> Self {
        File::Path(p.to_path_buf())
    }
}

impl From<Vec<u8>> for File {
    fn from(data: Vec<u8>) -> Self {
        File::bytes(data)
    }
}

impl From<&[u8]> for File {
    fn from(data: &[u8]) -> Self {
        File::bytes(data.to_vec())
    }
}

impl<const N: usize> From<&[u8; N]> for File {
    fn from(data: &[u8; N]) -> Self {
        File::bytes(data.to_vec())
    }
}

/// Where on a form to look for the signature.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Target {
    /// A named box of the matched form template.
    Box { page: u32, name: String },
    /// A region of a page, in pixels: x, y, width, height.
    Region { page: u32, bbox: [i64; 4] },
}

impl Target {
    /// A named box of the matched form template.
    pub fn named_box(name: impl Into<String>) -> Self {
        Target::Box {
            page: 1,
            name: name.into(),
        }
    }

    /// A region of a page.
    pub fn region(page: u32, x: i64, y: i64, w: i64, h: i64) -> Self {
        Target::Region {
            page,
            bbox: [x, y, w, h],
        }
    }

    /// `box:NAME` or `PAGE:X,Y,W,H`.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if let Some(name) = s.strip_prefix("box:") {
            return Ok(Target::named_box(name));
        }
        let bad = || Error::invalid(format!("target {s:?} must be 'box:NAME' or 'PAGE:X,Y,W,H'"));
        let (page, rest) = s.split_once(':').ok_or_else(bad)?;
        let page: u32 = page.trim().parse().map_err(|_| bad())?;
        let nums: Vec<i64> = rest
            .split(',')
            .map(|n| n.trim().parse())
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| bad())?;
        let bbox: [i64; 4] = nums.try_into().map_err(|_| bad())?;
        Ok(Target::Region { page, bbox })
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        match self {
            Target::Box { page, name } => serde_json::json!({ "page": page, "box": name }),
            Target::Region { page, bbox } => serde_json::json!({ "page": page, "bbox": bbox }),
        }
    }

    pub(crate) fn to_arg(&self) -> String {
        match self {
            Target::Box { name, .. } => format!("box:{name}"),
            Target::Region { page, bbox } => {
                format!("{page}:{},{},{},{}", bbox[0], bbox[1], bbox[2], bbox[3])
            }
        }
    }
}

impl std::str::FromStr for Target {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Target::parse(s)
    }
}

/// Checks an id we put in a path: 1 to 128 letters, digits, `.`, `_` or `-` (not `.` or `..`).
pub(crate) fn check_id<'a>(value: &'a str, what: &str) -> Result<&'a str> {
    let ok = !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(value)
    } else {
        Err(Error::invalid(format!(
            "{what} {value:?} must be 1 to 128 letters, digits, '.', '_' or '-' (not '.' or '..')"
        )))
    }
}

/// Files, with each folder replaced by the files directly inside it (in name order).
pub(crate) fn expand(files: Vec<File>) -> Result<Vec<File>> {
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        match f.local_path() {
            Some(p) if p.is_dir() => out.extend(folder(p)?.into_iter().map(File::Path)),
            _ => out.push(f),
        }
    }
    Ok(out)
}

pub(crate) fn folder(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if !hidden && path.is_file() {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// A file extension from the first bytes, for files we keep on disk for a moment.
pub(crate) fn extension(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG") {
        ".png"
    } else if data.starts_with(b"\xff\xd8") {
        ".jpg"
    } else if data.starts_with(b"%PDF") {
        ".pdf"
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        ".tif"
    } else if data.starts_with(b"PK\x03\x04") {
        ".zip"
    } else {
        ".bin"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_parse_both_forms() {
        assert_eq!(
            Target::parse("box:applicant").unwrap(),
            Target::named_box("applicant")
        );
        assert_eq!(
            Target::parse("2: 10,20,300,80").unwrap(),
            Target::region(2, 10, 20, 300, 80)
        );
        assert_eq!(Target::region(2, 1, 2, 3, 4).to_arg(), "2:1,2,3,4");
        assert!(Target::parse("2:1,2,3").is_err());
        assert!(Target::parse("nope").is_err());
    }

    #[test]
    fn ids_refuse_paths() {
        assert!(check_id("cust-88", "subject").is_ok());
        assert!(check_id("..", "subject").is_err());
        assert!(check_id("a/b", "subject").is_err());
        assert!(check_id("", "subject").is_err());
        assert!(check_id(&"x".repeat(129), "subject").is_err());
    }

    #[test]
    fn extensions_from_magic_bytes() {
        assert_eq!(extension(b"%PDF-1.7"), ".pdf");
        assert_eq!(extension(b"\x89PNG\r\n"), ".png");
        assert_eq!(extension(b"hello"), ".bin");
    }
}

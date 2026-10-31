//! Against the real offline binaries, when you point at them:
//! `UNFORGED_TEST_BINARY` (unforged-nib), `UNFORGED_TEST_LOUPE_BINARY` (unforged-loupe),
//! `UNFORGED_TEST_LICENCE` (a licence file) and `UNFORGED_TEST_FILES` (the test kit's `files/`,
//! searched with its subfolders).
use std::path::{Path, PathBuf};

use unforged::loupe::LoupeOptions;
use unforged::nib::{ExtractOptions, VerifyOptions};
use unforged::{Licence, Mode, Unforged};

fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The binary, licence and files, or `None` when none are set. Setting only some is a mistake.
fn setup(binary: &str) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let keys = [binary, "UNFORGED_TEST_LICENCE", "UNFORGED_TEST_FILES"];
    let vals: Vec<Option<PathBuf>> = keys.iter().map(|k| env(k)).collect();
    if vals.iter().all(Option::is_none) {
        eprintln!(
            "skipped: set {} to run this against the real binary",
            keys.join(", ")
        );
        return None;
    }
    let missing: Vec<&str> = keys
        .iter()
        .zip(&vals)
        .filter(|(_, v)| v.is_none())
        .map(|(k, _)| *k)
        .collect();
    assert!(missing.is_empty(), "also set {}", missing.join(", "));
    let mut v = vals.into_iter().map(Option::unwrap);
    Some((v.next()?, v.next()?, v.next()?))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn all(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(dir, &mut found);
    found.retain(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
    });
    found.sort();
    found
}

fn first(dir: &Path, exts: &[&str]) -> PathBuf {
    all(dir, exts)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no {} file under {}", exts.join("/"), dir.display()))
}

fn nib(bin: PathBuf, lic: PathBuf) -> Unforged {
    Unforged::builder()
        .mode(Mode::Offline)
        .nib_binary(bin)
        .licence_from(Licence::File(lic))
        .build()
        .unwrap()
}

#[tokio::test]
async fn nib_verifies_documents_and_crops() {
    let Some((bin, lic, files)) = setup("UNFORGED_TEST_BINARY") else {
        return;
    };
    let client = nib(bin, lic);
    let l = client.nib().licence().await.unwrap();
    assert!(!l.issued_to.is_empty());

    let doc = first(&files, &["pdf"]);
    let card = first(&files, &["png"]);
    let v = client
        .nib()
        .verify(&doc, VerifyOptions::references([&card]))
        .await
        .unwrap();
    eprintln!(
        "{} against {}: {:?}",
        doc.display(),
        card.display(),
        v.decision
    );
    assert!(v.decision.is_some());
    assert!(v.licensed_to.is_some());

    let again = client
        .nib()
        .verify(
            std::fs::read(&doc).unwrap(),
            VerifyOptions::references([&card]),
        )
        .await
        .unwrap();
    assert_eq!(again.decision, v.decision);
}

#[tokio::test]
async fn nib_takes_references_in_every_format() {
    let Some((bin, lic, files)) = setup("UNFORGED_TEST_BINARY") else {
        return;
    };
    let client = nib(bin, lic);
    let crops = all(&files, &["png"]);
    let questioned = crops.first().expect("a PNG crop").clone();
    let mut refs: Vec<PathBuf> = crops.into_iter().skip(1).take(2).collect();
    refs.extend(all(&files, &["jpg", "jpeg"]).into_iter().take(1));
    refs.extend(all(&files, &["tif", "tiff"]).into_iter().take(1));
    refs.extend(all(&files, &["pdf"]).into_iter().take(1));
    for r in &refs {
        let v = client
            .nib()
            .verify(&questioned, VerifyOptions::references([r]))
            .await;
        match v {
            Ok(v) => eprintln!("reference {}: {:?} {:?}", r.display(), v.decision, v.notes),
            Err(e) => eprintln!("reference {}: {e}", r.display()),
        }
    }
    let together = client
        .nib()
        .verify(&questioned, VerifyOptions::references(&refs))
        .await
        .unwrap();
    eprintln!("all {} together: {:?}", refs.len(), together.decision);
    assert!(together.decision.is_some());
}

#[tokio::test]
async fn nib_reads_a_photo_as_a_page_when_asked() {
    let Some((bin, lic, files)) = setup("UNFORGED_TEST_BINARY") else {
        return;
    };
    let Some(photo) = all(&files, &["jpg", "jpeg"]).into_iter().next() else {
        eprintln!("skipped: no JPEG under {}", files.display());
        return;
    };
    let client = nib(bin, lic);
    let card = first(&files, &["png"]);
    let v = client
        .nib()
        .verify(&photo, VerifyOptions::references([&card]).document())
        .await
        .unwrap();
    eprintln!(
        "{} as a page: {:?}, {} signature(s)",
        photo.display(),
        v.decision,
        v.signatures.len()
    );
    assert!(v.decision.is_some());
}

#[tokio::test]
async fn nib_extracts_every_signature() {
    let Some((bin, lic, files)) = setup("UNFORGED_TEST_BINARY") else {
        return;
    };
    let client = nib(bin, lic);
    let doc = first(&files, &["pdf"]);
    let out = tempfile::tempdir().unwrap();
    let found = client
        .nib()
        .extract_signatures([&doc], out.path(), ExtractOptions::default())
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    for s in &found[0].signatures {
        assert!(s.crop.exists(), "{} missing", s.crop.display());
    }
    eprintln!(
        "{}: {} signature(s)",
        doc.display(),
        found[0].signatures.len()
    );
}

#[tokio::test]
async fn loupe_checks_with_the_real_binary() {
    let Some((bin, lic, files)) = setup("UNFORGED_TEST_LOUPE_BINARY") else {
        return;
    };
    let client = Unforged::builder()
        .mode(Mode::Offline)
        .loupe_binary(bin)
        .licence_from(Licence::File(lic))
        .build()
        .unwrap();
    let doc = first(&files, &["pdf"]);
    let r = client
        .loupe()
        .check([&doc], LoupeOptions::default())
        .await
        .unwrap();
    eprintln!("{}: {:?}", doc.display(), r.result.verdict);
    assert_eq!(r.result.documents.len(), 1);
    assert!(r.result.verdict.is_some());
}

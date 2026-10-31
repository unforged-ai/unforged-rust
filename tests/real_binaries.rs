//! Against the real offline binaries, when you point at them:
//! `UNFORGED_TEST_BINARY` (unforged-nib), `UNFORGED_TEST_LOUPE_BINARY` (unforged-loupe),
//! `UNFORGED_TEST_LICENCE` (a licence file) and `UNFORGED_TEST_FILES` (the test kit's `files/`).
use std::path::PathBuf;

use unforged::loupe::LoupeOptions;
use unforged::nib::VerifyOptions;
use unforged::{Licence, Mode, Unforged};

fn env(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn first(dir: &std::path::Path, ext: &str) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
        .collect();
    found.sort();
    found.into_iter().next()
}

#[tokio::test]
async fn nib_verifies_with_the_real_binary() {
    let (Some(bin), Some(lic), Some(files)) = (
        env("UNFORGED_TEST_BINARY"),
        env("UNFORGED_TEST_LICENCE"),
        env("UNFORGED_TEST_FILES"),
    ) else {
        return;
    };
    let client = Unforged::builder()
        .mode(Mode::Offline)
        .nib_binary(bin)
        .licence_from(Licence::File(lic))
        .build()
        .unwrap();
    let lic = client.nib().licence().await.unwrap();
    assert!(!lic.issued_to.is_empty());
    let doc = first(&files, "pdf")
        .or_else(|| first(&files, "png"))
        .unwrap();
    let card = first(&files, "png").unwrap();
    let v = client
        .nib()
        .verify(&doc, VerifyOptions::references([&card]))
        .await
        .unwrap();
    assert!(v.decision.is_some());
    assert!(v.licensed_to.is_some());
    let bytes = std::fs::read(&doc).unwrap();
    let again = client
        .nib()
        .verify(bytes, VerifyOptions::references([&card]))
        .await
        .unwrap();
    assert_eq!(again.decision, v.decision);
}

#[tokio::test]
async fn loupe_checks_with_the_real_binary() {
    let (Some(bin), Some(lic), Some(files)) = (
        env("UNFORGED_TEST_LOUPE_BINARY"),
        env("UNFORGED_TEST_LICENCE"),
        env("UNFORGED_TEST_FILES"),
    ) else {
        return;
    };
    let client = Unforged::builder()
        .mode(Mode::Offline)
        .loupe_binary(bin)
        .licence_from(Licence::File(lic))
        .build()
        .unwrap();
    let doc = first(&files, "pdf").unwrap();
    let r = client
        .loupe()
        .check([&doc], LoupeOptions::default())
        .await
        .unwrap();
    assert_eq!(r.result.documents.len(), 1);
    assert!(r.result.verdict.is_some());
}

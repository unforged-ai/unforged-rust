#![cfg(unix)]
//! The offline client against stand-in binaries that answer like `unforged-nib` and
//! `unforged-loupe`, and write down what they were given.
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use unforged::loupe::LoupeOptions;
use unforged::nib::{Decision, ExtractOptions, VerifyOptions};
use unforged::{Error, File, Licence, Mode, ReferenceSource, Target, Unforged};

const TOKEN: &str = "eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJhY21lIn0.c2lnbmF0dXJl";

const VERIFY_JSON: &str = r#"{"model":"nib-2.6","decision":"APPROVE","input_type":"pdf","pages":1,"notes":[],
"signatures":[{"index":0,"page":1,"box":[1,2,3,4],"match_probability":0.93,"band":[0.9,0.96],
"per_reference":[0.93],"flags":[],"decision":"APPROVE","reasons":["Matches"],"best_match":true,
"quality":{"ok":true,"issues":[]}}],"references":[{"file":"card.png"}],"policy":"assist",
"licensed_to":"Acme Bank"}"#;

const LOUPE_JSON: &str = r#"{"kind":"submission","licensed_to":"Acme Bank","text_read":true,
"register_lookups":true,"verdict":"APPROVE","probability":0.05,"top_reasons":[],"trusted_because":[],
"documents":[{"name":"a.pdf","verdict":"APPROVE","probability":0.05},{"name":"b.pdf","verdict":"APPROVE","probability":0.04}],
"signals":[],"versions":{"engine":"1.7.0"}}"#;

const LICENCE_JSON: &str = r#"{"id":"lic_1","kind":"licence","issued_to":"Acme Bank","issued_by":"Unforged Ltd",
"valid_from":"2026-01-01","valid_until":"2026-12-31","works_until":"2027-01-30","includes":["verify","extract","loupe"],
"state":"renew_soon","days_left":61}"#;

struct Stand {
    dir: tempfile::TempDir,
}

impl Stand {
    /// A stand-in binary that logs its arguments, licence and the questioned file's folder, then
    /// runs `body`.
    fn new(body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().display().to_string();
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" > '{log}/args'\n\
             printf '%s' \"$UNFORGED_LICENSE\" > '{log}/licence-env'\n\
             if [ \"$1\" = verify ] || [ \"$1\" = check ]; then\n\
               ls -ld \"$(dirname \"$2\")\" > '{log}/folder'\n\
               ls -l \"$2\" > '{log}/file'\n\
               cat \"$2\" > '{log}/questioned'\n\
             fi\n\
             {body}\n"
        );
        let bin = dir.path().join("bin");
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Stand { dir }
    }

    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
    }

    fn args(&self) -> Vec<String> {
        self.read("args").lines().map(str::to_owned).collect()
    }

    fn nib(&self) -> unforged::Builder {
        Unforged::builder()
            .mode(Mode::Offline)
            .nib_binary(self.bin())
            .loupe_binary(self.bin())
            .licence_from(Licence::Text(TOKEN.into()))
    }
}

fn answer(json: &str) -> String {
    format!("cat <<'EOF'\n{json}\nEOF")
}

fn write(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

#[tokio::test]
async fn verify_runs_the_binary_with_references_and_form() {
    let stand = Stand::new(&answer(VERIFY_JSON));
    let files = tempfile::tempdir().unwrap();
    let doc = write(files.path(), "application.pdf", b"%PDF");
    let card = write(files.path(), "card.png", b"\x89PNG");
    let client = stand
        .nib()
        .template("/forms/loan.json")
        .auto_approve(true)
        .build()
        .unwrap();
    let v = client
        .nib()
        .verify(
            &doc,
            VerifyOptions::references([&card])
                .target(Target::region(2, 10, 20, 300, 80))
                .reference("APP-1042"),
        )
        .await
        .unwrap();
    assert_eq!(v.decision, Some(Decision::Approve));
    assert_eq!(v.licensed_to.as_deref(), Some("Acme Bank"));
    assert_eq!(v.reference.as_deref(), Some("APP-1042"));
    assert_eq!(
        v.best_match().unwrap().per_reference[0]
            .reference
            .as_deref(),
        Some("card.png")
    );
    let args = stand.args();
    assert_eq!(args[0], "verify");
    assert_eq!(args[1], doc.display().to_string());
    let has = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
    assert!(has("--ref", &card.display().to_string()));
    assert!(has("--target", "2:10,20,300,80"));
    assert!(has("--template", "/forms/loan.json"));
    assert!(args.contains(&"--auto-approve".to_string()));
}

#[tokio::test]
async fn the_licence_goes_through_the_environment_never_the_arguments() {
    let stand = Stand::new(&answer(VERIFY_JSON));
    let client = stand.nib().build().unwrap();
    client
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::references([b"\x89PNG"]))
        .await
        .unwrap();
    assert_eq!(stand.read("licence-env"), TOKEN);
    assert!(!stand.read("args").contains(TOKEN));

    let file_client = Unforged::builder()
        .mode(Mode::Offline)
        .nib_binary(stand.bin())
        .licence_from(Licence::File("/opt/unforged/nib.lic".into()))
        .build()
        .unwrap();
    file_client
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::references([b"\x89PNG"]))
        .await
        .unwrap();
    let args = stand.args();
    assert_eq!(args[..2], ["--license-file", "/opt/unforged/nib.lic"]);
}

#[tokio::test]
async fn bytes_live_in_a_private_folder_only_while_the_check_runs() {
    let stand = Stand::new(&answer(VERIFY_JSON));
    let scratch = tempfile::tempdir().unwrap();
    let client = stand.nib().temp_dir(scratch.path()).build().unwrap();
    client
        .nib()
        .verify(
            b"%PDF-secret".as_slice(),
            VerifyOptions::references([b"\x89PNG"]),
        )
        .await
        .unwrap();
    assert!(
        stand.read("folder").starts_with("drwx------"),
        "{}",
        stand.read("folder")
    );
    assert!(
        stand.read("file").starts_with("-rw-------"),
        "{}",
        stand.read("file")
    );
    assert_eq!(stand.read("questioned"), "%PDF-secret");
    assert!(stand.args()[1].ends_with(".pdf"));
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn subjects_come_from_the_reference_source() {
    let stand = Stand::new(&answer(VERIFY_JSON));
    let refs = tempfile::tempdir().unwrap();
    let person = refs.path().join("cust-88");
    std::fs::create_dir(&person).unwrap();
    write(&person, "b.png", b"\x89PNG");
    write(&person, "a.png", b"\x89PNG");
    write(&person, ".hidden", b"x");
    let pattern = format!("{}/{{subject}}", refs.path().display());
    let client = stand
        .nib()
        .reference_source(ReferenceSource::folder(pattern))
        .build()
        .unwrap();
    let v = client
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::subject("cust-88"))
        .await
        .unwrap();
    assert_eq!(v.subject_reference.as_deref(), Some("cust-88"));
    let refs_given: Vec<String> = stand
        .args()
        .windows(2)
        .filter(|w| w[0] == "--ref")
        .map(|w| w[1].clone())
        .collect();
    assert_eq!(refs_given.len(), 2);
    assert!(refs_given[0].ends_with("a.png") && refs_given[1].ends_with("b.png"));

    let e = client
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::subject("cust-99"))
        .await
        .unwrap_err();
    assert!(e.is_not_found());
}

#[tokio::test]
async fn sources_can_be_maps_or_functions() {
    let stand = Stand::new(&answer(VERIFY_JSON));
    let map: ReferenceSource = HashMap::from([("cust-88", vec![File::bytes(b"\x89PNG".to_vec())])])
        .into_iter()
        .collect();
    let client = stand.nib().reference_source(map).build().unwrap();
    client
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::subject("cust-88"))
        .await
        .unwrap();

    let f = ReferenceSource::function(|subject| {
        if subject == "cust-88" {
            Ok(vec![File::bytes(b"\x89PNG".to_vec())])
        } else {
            Ok(vec![])
        }
    });
    let client = stand.nib().reference_source(f).build().unwrap();
    assert!(client
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::subject("nobody"))
        .await
        .unwrap_err()
        .is_not_found());
}

#[tokio::test]
async fn without_a_source_a_subject_says_what_to_do() {
    let stand = Stand::new(&answer(VERIFY_JSON));
    let e = stand
        .nib()
        .build()
        .unwrap()
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::subject("cust-88"))
        .await
        .unwrap_err();
    assert!(matches!(e, Error::NotSupported(_)));
}

#[tokio::test]
async fn exit_codes_become_errors() {
    for (code, check) in [
        (
            3,
            (|e: &Error| matches!(e, Error::Licence { .. })) as fn(&Error) -> bool,
        ),
        (4, |e| matches!(e, Error::NotLicensed { .. })),
        (
            1,
            |e| matches!(e, Error::CheckFailed { detail, .. } if detail == "not a document"),
        ),
        (2, |e| {
            matches!(
                e,
                Error::Offline {
                    exit_code: Some(2),
                    ..
                }
            )
        }),
    ] {
        let stand = Stand::new(&format!("echo 'error: not a document' >&2\nexit {code}"));
        let e = stand
            .nib()
            .build()
            .unwrap()
            .nib()
            .verify(b"%PDF".as_slice(), VerifyOptions::references([b"\x89PNG"]))
            .await
            .unwrap_err();
        assert!(check(&e), "exit {code}: {e:?}");
    }
}

#[tokio::test]
async fn licence_notices_come_back_with_the_result() {
    let stand = Stand::new(&format!(
        "echo 'Your licence ends on 31 December 2026.' >&2\n{}",
        answer(VERIFY_JSON)
    ));
    let v = stand
        .nib()
        .build()
        .unwrap()
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::references([b"\x89PNG"]))
        .await
        .unwrap();
    assert_eq!(v.notices, ["Your licence ends on 31 December 2026."]);
}

#[tokio::test]
async fn urls_and_online_only_calls_are_refused_offline() {
    let stand = Stand::new(&answer(VERIFY_JSON));
    let client = stand.nib().build().unwrap();
    let e = client
        .nib()
        .verify(
            File::url("https://x.example.com/a.pdf"),
            VerifyOptions::references([b"\x89PNG"]),
        )
        .await
        .unwrap_err();
    assert!(matches!(e, Error::NotSupported(_)));
    assert!(matches!(
        client.nib().usage().await,
        Err(Error::NotSupported(_))
    ));
    assert!(matches!(client.nib().checks(), Err(Error::NotSupported(_))));
    assert!(matches!(
        client.nib().subjects(),
        Err(Error::NotSupported(_))
    ));
    assert!(matches!(
        client.loupe().checks(),
        Err(Error::NotSupported(_))
    ));
    let e = client
        .nib()
        .verify(
            b"%PDF".as_slice(),
            VerifyOptions::references([b"\x89PNG"]).policy("strict"),
        )
        .await
        .unwrap_err();
    assert!(matches!(e, Error::InvalidInput(_)));
}

#[tokio::test]
async fn policies_as_json_go_to_a_private_file() {
    let stand = Stand::new(&format!(
        "here=\"$(dirname \"$0\")\"\nwhile [ $# -gt 0 ]; do if [ \"$1\" = --policy ]; then cat \"$2\" > \"$here/policy\"; fi; shift; done\n{}",
        answer(VERIFY_JSON)
    ));
    stand
        .nib()
        .build()
        .unwrap()
        .nib()
        .verify(
            b"%PDF".as_slice(),
            VerifyOptions::references([b"\x89PNG"]).policy(serde_json::json!({"name": "strict"})),
        )
        .await
        .unwrap();
    assert_eq!(stand.read("policy"), r#"{"name":"strict"}"#);
}

#[tokio::test]
async fn extract_signatures_reads_the_manifest() {
    let stand = Stand::new(
        "out=''; while [ $# -gt 0 ]; do if [ \"$1\" = --out ]; then out=\"$2\"; fi; shift; done\n\
         mkdir -p \"$out\"\n\
         echo '[{\"file\":\"a.pdf\",\"status\":\"ok\",\"input_type\":\"pdf\",\"pages\":2,\"notes\":[],\
         \"signatures\":[{\"page\":2,\"box\":[5,6,7,8],\"score\":0.9,\"crop\":\"a-p2-1.png\"}]}]' > \"$out/manifest.json\"\n\
         exit 1",
    );
    let out = tempfile::tempdir().unwrap();
    let found = stand
        .nib()
        .build()
        .unwrap()
        .nib()
        .extract_signatures([b"%PDF".as_slice()], out.path(), ExtractOptions::default())
        .await
        .unwrap();
    assert_eq!(found[0].pages, 2);
    assert_eq!(found[0].signatures[0].bbox, [5, 6, 7, 8]);
    assert_eq!(found[0].signatures[0].crop, out.path().join("a-p2-1.png"));
    assert_eq!(stand.args()[0], "extract-signature");
}

#[tokio::test]
async fn the_licence_reads_as_json() {
    let stand = Stand::new(&answer(LICENCE_JSON));
    let lic = stand.nib().build().unwrap().nib().licence().await.unwrap();
    assert_eq!(lic.issued_to, "Acme Bank");
    assert_eq!(lic.days_left, Some(61));
    assert!(lic.includes.contains(&"loupe".to_string()));
    assert_eq!(stand.args(), ["licence", "--json"]);
}

#[tokio::test]
async fn loupe_checks_offline_keep_document_names() {
    let stand = Stand::new(&answer(LOUPE_JSON));
    let r = stand
        .nib()
        .build()
        .unwrap()
        .loupe()
        .check(
            [
                File::named("a.pdf", b"%PDF".to_vec()),
                File::named("../b.pdf", b"%PDF".to_vec()),
            ],
            LoupeOptions::default().lookups(true),
        )
        .await
        .unwrap();
    assert!(r.result.approved());
    assert_eq!(r.result.documents.len(), 2);
    assert!(r.register_lookups);
    let args = stand.args();
    assert_eq!(args[0], "check");
    assert!(args[1].ends_with("/1/a.pdf"), "{}", args[1]);
    assert!(args[2].ends_with("/2/..-b.pdf"), "{}", args[2]);
    assert_eq!(args[3], "--lookups");
}

#[tokio::test]
async fn a_missing_binary_says_where_to_point() {
    let client = Unforged::builder()
        .mode(Mode::Offline)
        .nib_binary("/nowhere/unforged-nib")
        .build()
        .unwrap();
    let e = client
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::references([b"\x89PNG"]))
        .await
        .unwrap_err();
    assert!(
        matches!(e, Error::Offline { ref message, .. } if message.contains("no such file")),
        "{e:?}"
    );
}

#[tokio::test]
async fn references_in_any_form_reach_the_binary_and_photos_can_be_pages() {
    let stand = Stand::new(&answer(VERIFY_JSON));
    let files = tempfile::tempdir().unwrap();
    let crop = write(files.path(), "crop.png", b"\x89PNG");
    let refs = files.path().join("refs");
    std::fs::create_dir(&refs).unwrap();
    write(&refs, "card.jpg", b"\xff\xd8");
    write(&refs, "scan.tif", b"II*\0");
    write(&refs, ".hidden", b"x");
    let client = stand.nib().build().unwrap();
    client
        .nib()
        .verify(
            File::named("photo.jpg", b"\xff\xd8photo".to_vec()),
            VerifyOptions::references([File::path(&crop), File::path(&refs)])
                .with_references([
                    File::named("signed.pdf", b"%PDF".to_vec()),
                    File::bytes(b"II*\0tiff".to_vec()),
                ])
                .document(),
        )
        .await
        .unwrap();
    let args = stand.args();
    let refs_given: Vec<&String> = args
        .windows(2)
        .filter(|w| w[0] == "--ref")
        .map(|w| &w[1])
        .collect();
    assert_eq!(refs_given.len(), 5, "{args:?}");
    assert_eq!(refs_given[0], &crop.display().to_string());
    assert!(refs_given[1].ends_with("card.jpg"));
    assert!(refs_given[2].ends_with("scan.tif"));
    assert!(refs_given[3].ends_with(".pdf"));
    assert!(refs_given[4].ends_with(".tif"));
    assert!(args.contains(&"--document".to_string()));
    assert_eq!(
        std::fs::read(stand.dir.path().join("questioned")).unwrap(),
        b"\xff\xd8photo"
    );
}

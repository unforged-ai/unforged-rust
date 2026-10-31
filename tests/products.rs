use std::time::Duration;

use serde_json::json;
use sha2::{Digest, Sha256};
use unforged::folio::{
    AccessReason, Change, DownloadOptions, FilingOptions, Scope, SearchOptions, Why,
};
use unforged::sig::{PackageOptions, Signer};
use unforged::{Error, File, Mode, Unforged};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn client(server: &MockServer) -> Unforged {
    Unforged::builder()
        .mode(Mode::Hosted)
        .api_key("unf_test_key")
        .base_url(server.uri())
        .poll_interval(Duration::from_millis(5))
        .build()
        .unwrap()
}

fn hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn filing_sends_the_bytes_and_returns_the_receipt() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/folio/records"))
        .and(query_param("folder", "loans/2026"))
        .and(query_param("name", "agreement.pdf"))
        .and(query_param("class", "loan-7y"))
        .and(header("content-type", "application/octet-stream"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "record_id": "rec_1", "sha256": "ab", "size": 8, "class_id": "loan-7y",
            "lock": "retained", "earliest_delete": "2033-10-31", "receipt_v": 2
        })))
        .expect(1)
        .mount(&server)
        .await;

    let receipt = client(&server)
        .folio()
        .unwrap()
        .records()
        .file(
            File::named("agreement.pdf", b"%PDF-1.7".to_vec()),
            FilingOptions::folder("loans/2026").retention_class("loan-7y"),
        )
        .await
        .unwrap();
    assert_eq!(receipt.record_id, "rec_1");
    assert_eq!(receipt.earliest_delete.as_deref(), Some("2033-10-31"));
    assert_eq!(receipt.extra["receipt_v"], 2);
    let sent = &server.received_requests().await.unwrap()[0];
    assert_eq!(sent.body, b"%PDF-1.7");
}

#[tokio::test]
async fn reads_carry_the_reason_and_downloads_are_checked_against_the_receipt() {
    let server = MockServer::start().await;
    let doc = b"the lease".to_vec();
    Mock::given(method("GET"))
        .and(path("/folio/records/rec_1"))
        .and(header("unforged-reason-code", "audit"))
        .and(header("unforged-reason", "quarterly check"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "rec_1", "folder": "leases", "name": "unit-4.pdf", "size": 9,
            "class_id": "lease", "filed_at_ms": 1761900000000_i64, "storage": {"readable_now": false}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/folio/records/rec_1/content"))
        .and(header("unforged-reason-id", "rsn_9"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("digest", format!("sha-256={}", hex(&doc)).as_str())
                .insert_header(
                    "content-disposition",
                    "attachment; filename=\"../unit-4.pdf\"",
                )
                .set_body_bytes(doc.clone()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/folio/records/rec_2/content"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("digest", format!("sha-256={}", hex(b"other")).as_str())
                .set_body_bytes(doc.clone()),
        )
        .mount(&server)
        .await;

    let c = client(&server);
    let records = c.folio().unwrap().records();
    let r = records
        .get(
            "rec_1",
            Some(AccessReason::code("audit").text("quarterly check")),
        )
        .await
        .unwrap();
    assert_eq!(r.path(), "leases/unit-4.pdf");
    assert!(!r.readable_now());

    let open = DownloadOptions::reason(AccessReason::Open("rsn_9".into()));
    assert_eq!(records.download("rec_1", open.clone()).await.unwrap(), doc);

    let dir = tempfile::tempdir().unwrap();
    let saved = records
        .download_to("rec_1", dir.path(), open)
        .await
        .unwrap();
    assert_eq!(saved, dir.path().join("unit-4.pdf"));
    assert_eq!(std::fs::read(&saved).unwrap(), doc);

    let to = dir.path().join("bad.pdf");
    let err = records
        .download_to("rec_2", &to, DownloadOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Integrity { ref record_id, .. } if record_id == "rec_2"));
    assert!(!to.exists());
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1);
}

#[tokio::test]
async fn changes_that_need_a_colleague_come_back_pending() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/folio/records/rec_1/move"))
        .and(body_partial_json(
            json!({"folder": "archive", "code": "tidy-up"}),
        ))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({
            "id": "apr_1", "action": "move", "state": "pending", "target": "rec_1"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/folio/records/rec_2/move"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "record_id": "rec_2", "from": "inbox", "to": "archive"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/folio/holds"))
        .and(body_partial_json(json!({
            "scope": {"kind": "matter", "value": "M-12"}, "code": "litigation"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "hold_1", "scope": {"kind": "matter", "value": "M-12"},
            "reason_code": "litigation", "records_covered": 14
        })))
        .mount(&server)
        .await;

    let c = client(&server);
    let folio = c.folio().unwrap();
    match folio
        .records()
        .move_to("rec_1", "archive", Why::code("tidy-up"))
        .await
        .unwrap()
    {
        Change::Pending(a) => assert!(a.pending()),
        other => panic!("expected an approval, got {other:?}"),
    }
    match folio
        .records()
        .move_to("rec_2", "archive", Why::code("tidy-up"))
        .await
        .unwrap()
    {
        Change::Done(m) => assert_eq!(m.from_folder, "inbox"),
        other => panic!("expected a move, got {other:?}"),
    }
    let hold = folio
        .holds()
        .place(Scope::Matter("M-12".into()), Why::code("litigation"))
        .await
        .unwrap();
    assert!(hold.active());
    assert_eq!(hold.records_covered, Some(14));
    assert!(folio
        .holds()
        .place(Scope::Records(vec!["a".into()]), Why::code("x"))
        .await
        .is_err());
    assert!(folio
        .records()
        .extend_retention("rec_1", "31/03/2031", Why::code("x"))
        .await
        .is_err());
}

#[tokio::test]
async fn queued_filings_are_waited_for() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/folio/filings/fil_1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "fil_1", "state": "queued", "name": "a.pdf"})),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/folio/filings/fil_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id": "fil_1", "state": "filed", "name": "a.pdf", "record_id": "rec_7"}),
        ))
        .mount(&server)
        .await;

    let f = client(&server)
        .folio()
        .unwrap()
        .filings()
        .wait("fil_1", Some(Duration::from_secs(5)))
        .await
        .unwrap();
    assert!(f.done());
    assert_eq!(f.record_id.as_deref(), Some("rec_7"));
}

#[tokio::test]
async fn searches_read_hits_and_snippets() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/folio/search"))
        .and(query_param("scope", "content"))
        .and(query_param("q", "break clause"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "scope": "content", "matched_records": 1, "records": [{
                "id": "rec_1", "folder": "leases", "name": "unit-4.pdf", "pages": [3],
                "snippets": [{"page": 3, "parts": [
                    {"text": "the ", "hit": false}, {"text": "break clause", "hit": true}
                ]}]
            }]
        })))
        .mount(&server)
        .await;

    let c = client(&server);
    let folio = c.folio().unwrap();
    let found = folio
        .search("break clause", SearchOptions::content())
        .await
        .unwrap();
    let hit = &found.hits[0];
    assert_eq!(hit.record.path(), "leases/unit-4.pdf");
    assert_eq!(hit.pages, vec![3]);
    assert_eq!(
        hit.snippets.as_ref().unwrap()[0].marked("[", "]"),
        "the [break clause]"
    );

    let mut narrowed = SearchOptions::names();
    narrowed.matter = Some("M-1".into());
    assert!(matches!(
        folio.search("x", narrowed).await,
        Err(Error::InvalidInput(_))
    ));
}

#[tokio::test]
async fn sig_packages_go_up_as_one_form() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sig/packages"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "pkg_1", "status": "sent", "title": "Loan agreement",
            "signers": [{"id": "sgn_1", "name": "Ada Lovelace", "email": "ada@example.com",
                "order": 1, "status": "sent"}]
        })))
        .mount(&server)
        .await;

    let p = client(&server)
        .sig()
        .unwrap()
        .create_package(
            vec![File::named("loan.pdf", b"%PDF-1.7".to_vec())],
            &[Signer::new("Ada Lovelace", "ada@example.com").order(1)],
            PackageOptions {
                title: Some("Loan agreement".into()),
                ..PackageOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(p.signers[0].name, "Ada Lovelace");

    let sent: &Request = &server.received_requests().await.unwrap()[0];
    let body = String::from_utf8_lossy(&sent.body);
    assert!(body.contains("name=\"package\""));
    assert!(body.contains("\"send\":true"));
    assert!(body.contains("filename=\"loan.pdf\""));
    assert!(body.contains("Content-Type: application/pdf"));
}

#[tokio::test]
async fn new_keys_and_secrets_stay_out_of_debug_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/keys"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "key_1", "name": "ci", "key": "unf_live_secret", "permissions": ["nib:check"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/nib/webhooks/wh_1/secret/roll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "wh_1", "url": "https://example.com/hook", "secret": "whsec_abc"
        })))
        .mount(&server)
        .await;

    let c = client(&server);
    let key = c
        .account()
        .unwrap()
        .create_key("ci", &["nib:check"], None)
        .await
        .unwrap();
    assert_eq!(key.key.expose(), "unf_live_secret");
    assert!(!format!("{key:?}").contains("unf_live_secret"));

    let hook = c
        .nib()
        .webhooks()
        .unwrap()
        .roll_secret("wh_1")
        .await
        .unwrap();
    assert_eq!(hook.secret.expose(), "whsec_abc");
    assert!(!format!("{hook:?}").contains("whsec_abc"));
}

#[test]
fn online_only_products_say_so_offline() {
    let c = Unforged::builder()
        .mode(Mode::Offline)
        .licence("x.y.z")
        .build()
        .unwrap();
    for e in [
        c.folio().map(drop).unwrap_err(),
        c.sig().map(drop).unwrap_err(),
        c.account().map(drop).unwrap_err(),
        c.nib().policies().map(drop).unwrap_err(),
    ] {
        assert!(matches!(e, Error::NotSupported(_)), "{e:?}");
    }
}

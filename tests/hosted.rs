use std::time::Duration;

use serde_json::json;
use unforged::loupe::{LoupeOptions, Outcome};
use unforged::nib::{CheckItem, CreateOptions, Decision, VerifyOptions};
use unforged::{ApiErrorKind, Error, File, Mode, Target, Unforged};
use wiremock::matchers::{body_partial_json, header, method, path};
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

fn body(r: &Request) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

fn result(decision: &str) -> serde_json::Value {
    json!({
        "status": "completed", "decision": decision, "item_index": 0, "input_type": "pdf",
        "versions": {"model": "nib-2.6", "policy": "assist"},
        "signatures": [{"index": 0, "page": 1, "bbox": [10, 20, 300, 80], "decision": decision,
            "match_probability": 0.94, "per_reference": [{"ref": "card", "probability": 0.94}],
            "reasons": ["Matches the signature card"], "best_match": true,
            "quality": {"ok": true, "issues": []}}]
    })
}

#[tokio::test]
async fn verify_queues_the_check_and_waits_for_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/nib/checks"))
        .and(header("authorization", "Bearer unf_test_key"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"job_id": "job_1"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/nib/checks/job_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"job_id": "job_1", "status": "completed", "results": [result("APPROVE")]}),
        ))
        .mount(&server)
        .await;

    let v = client(&server)
        .nib()
        .verify(
            File::named("application.pdf", b"%PDF-1.7".to_vec()),
            VerifyOptions::subject("cust-88")
                .reference("APP-1042")
                .target(Target::named_box("applicant"))
                .metadata("branch", "leeds"),
        )
        .await
        .unwrap();
    assert_eq!(v.decision, Some(Decision::Approve));
    assert_eq!(v.job_id.as_deref(), Some("job_1"));
    assert_eq!(v.model.as_deref(), Some("nib-2.6"));
    let best = v.best_match().unwrap();
    assert_eq!(best.per_reference[0].reference.as_deref(), Some("card"));

    let sent = &server.received_requests().await.unwrap()[0];
    let text = body(sent);
    assert!(text.contains("name=\"request\""));
    assert!(text.contains("name=\"questioned\"; filename=\"application.pdf\""));
    assert!(text.contains("\"subject_reference\":\"cust-88\""));
    assert!(text.contains("\"box\":\"applicant\""));
    assert!(text.contains("\"branch\":\"leeds\""));
    let key = sent
        .headers
        .get("idempotency-key")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(text.contains(&format!("\"job_id\":\"{key}\"")));
}

#[tokio::test]
async fn a_quick_check_answers_at_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/nib/checks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(result("FLAG")))
        .mount(&server)
        .await;
    let v = client(&server)
        .nib()
        .verify(
            File::url("https://files.example.com/a.pdf"),
            VerifyOptions::references([File::url("https://files.example.com/card.png")]),
        )
        .await
        .unwrap();
    assert_eq!(v.decision, Some(Decision::Flag));
    let text = body(&server.received_requests().await.unwrap()[0]);
    assert!(text.contains("\"url\":\"https://files.example.com/a.pdf\""));
}

#[tokio::test]
async fn a_subject_with_no_references_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/nib/checks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "failed",
            "error": {"type": "no_usable_references", "detail": "cust-88 has no references"}
        })))
        .mount(&server)
        .await;
    let e = client(&server)
        .nib()
        .verify(b"%PDF", VerifyOptions::subject("cust-88"))
        .await
        .unwrap_err();
    assert!(e.is_not_found(), "{e:?}");
    assert_eq!(e.code(), Some("no_usable_references"));
}

#[tokio::test]
async fn an_unreadable_document_fails_the_check() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/nib/checks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "failed", "error": {"type": "unreadable", "detail": "That isn't a document"}
        })))
        .mount(&server)
        .await;
    let e = client(&server)
        .nib()
        .verify(b"hello", VerifyOptions::subject("cust-88"))
        .await
        .unwrap_err();
    assert!(
        matches!(e, Error::CheckFailed { ref kind, .. } if kind == "unreadable"),
        "{e:?}"
    );
}

#[tokio::test]
async fn busy_answers_are_retried_with_the_same_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/nib/checks"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("retry-after", "0")
                .set_body_json(json!({"code": "busy", "detail": "Busy"})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/nib/checks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(result("APPROVE")))
        .mount(&server)
        .await;
    client(&server)
        .nib()
        .verify(b"%PDF", VerifyOptions::subject("cust-88"))
        .await
        .unwrap();
    let sent = server.received_requests().await.unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        sent[0].headers.get("idempotency-key"),
        sent[1].headers.get("idempotency-key")
    );
}

#[tokio::test]
async fn a_used_up_quota_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nib/usage"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("x-request-id", "req_9")
                .set_body_json(
                    json!({"code": "quota_exceeded", "detail": "This month's checks are used up"}),
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    let e = client(&server).nib().usage().await.unwrap_err();
    let api = e.api().unwrap();
    assert_eq!(api.kind, ApiErrorKind::RateLimited);
    assert_eq!(api.request_id.as_deref(), Some("req_9"));
    assert_eq!(
        e.to_string(),
        "This month's checks are used up [quota_exceeded] (request req_9)"
    );
}

#[tokio::test]
async fn another_region_is_followed_only_to_our_hosts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/nib/usage"))
        .respond_with(ResponseTemplate::new(421).set_body_json(json!({
            "code": "wrong_region", "detail": "Your organisation lives elsewhere",
            "api_host": "https://evil.example.com"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let e = client(&server).nib().usage().await.unwrap_err();
    let api = e.api().unwrap();
    assert_eq!(api.kind, ApiErrorKind::WrongRegion);
    assert_eq!(api.api_host(), Some("https://evil.example.com"));
}

#[tokio::test]
async fn a_batch_is_one_multipart_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/nib/checks"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({"job_id": "batch-7"})))
        .mount(&server)
        .await;
    let job = client(&server)
        .nib()
        .checks()
        .unwrap()
        .create(
            vec![
                CheckItem::new(File::named("a.pdf", b"%PDF".to_vec()), "cust-88")
                    .reference("APP-1"),
                CheckItem::new(File::named("b.pdf", b"%PDF".to_vec()), "cust-91")
                    .reference("APP-2"),
            ],
            CreateOptions {
                job_id: Some("batch-7".into()),
                process: Some("loan-application".into()),
                ..CreateOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(job.job_id, "batch-7");
    assert_eq!(job.status, "queued");
    assert_eq!(job.items, 2);
    let sent = &server.received_requests().await.unwrap()[0];
    assert_eq!(sent.headers.get("idempotency-key").unwrap(), "batch-7");
    let text = body(sent);
    assert!(text.contains("name=\"questioned\";"));
    assert!(text.contains("name=\"questioned-2\";"));
    assert!(text.contains("\"process\":\"loan-application\""));
}

#[tokio::test]
async fn enrolment_names_references_after_their_files() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/nib/subjects/cust-88/references"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "subject_reference": "cust-88",
            "references": [{"ref_id": "card", "status": "enrolled", "source": "signature_card"}]
        })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let card = dir.path().join("card.png");
    std::fs::write(&card, b"\x89PNG").unwrap();
    let e = client(&server)
        .nib()
        .enrol("cust-88", [card], "signature_card")
        .await
        .unwrap();
    assert_eq!(e.references[0].ref_id, "card");
    assert_eq!(e.rejected().count(), 0);
    let text = body(&server.received_requests().await.unwrap()[0]);
    assert!(text.contains("name=\"card\"; filename=\"card.png\""));
    assert!(text.contains("name=\"source.card\""));
    assert!(text.contains("signature_card"));
}

#[tokio::test]
async fn offline_only_calls_say_so_online() {
    let server = MockServer::start().await;
    let c = client(&server);
    let e = c
        .nib()
        .extract_signatures(["a.pdf"], "out", Default::default())
        .await
        .unwrap_err();
    assert!(matches!(e, Error::NotSupported(_)));
    assert!(matches!(
        c.nib().licence().await,
        Err(Error::NotSupported(_))
    ));
    assert!(matches!(
        c.loupe().licence().await,
        Err(Error::NotSupported(_))
    ));
}

#[tokio::test]
async fn subject_ids_never_reach_a_path_unchecked() {
    let server = MockServer::start().await;
    let e = client(&server)
        .nib()
        .subjects()
        .unwrap()
        .get("../keys")
        .await
        .unwrap_err();
    assert!(matches!(e, Error::InvalidInput(_)));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn loupe_checks_documents_and_waits() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/loupe/checks"))
        .respond_with(
            ResponseTemplate::new(202)
                .set_body_json(json!({"id": "lck_1", "status": "queued", "created_at": "x"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/loupe/checks/lck_1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "lck_1", "status": "completed", "created_at": "x",
            "result": {"verdict": "FLAG", "probability": 0.8, "top_reasons": ["The balance doesn't add up"],
                "documents": [{"name": "statement.pdf", "verdict": "FLAG", "probability": 0.8}],
                "signals": [], "versions": {"engine": "1.7.0"}}
        })))
        .mount(&server)
        .await;
    let r = client(&server)
        .loupe()
        .check(
            [File::named("statement.pdf", b"%PDF".to_vec())],
            LoupeOptions::default().policy("strict"),
        )
        .await
        .unwrap();
    assert_eq!(r.check_id.as_deref(), Some("lck_1"));
    assert_eq!(r.result.verdict, Some(Decision::Flag));
    assert_eq!(r.result.documents[0].name, "statement.pdf");
    let text = body(&server.received_requests().await.unwrap()[0]);
    assert!(text.contains("name=\"file\"; filename=\"statement.pdf\""));
    assert!(text.contains("{\"name\":\"strict\"}"));
}

#[tokio::test]
async fn a_failed_loupe_check_says_why() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/loupe/checks"))
        .respond_with(
            ResponseTemplate::new(202).set_body_json(json!({"id": "lck_2", "status": "queued"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/loupe/checks/lck_2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "lck_2", "status": "failed", "error": "None of the files could be opened"
        })))
        .mount(&server)
        .await;
    let e = client(&server)
        .loupe()
        .check([b"junk"], LoupeOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(e, Error::CheckFailed { ref detail, .. } if detail == "None of the files could be opened")
    );
}

#[tokio::test]
async fn loupe_reviews_record_the_decision() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/loupe/checks/lck_1/review"))
        .and(body_partial_json(json!({"outcome": "fraud", "note": "Not the bank's copy"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "lck_1", "status": "completed",
            "review": {"outcome": "fraud", "note": "Not the bank's copy", "by": "Sam Okafor", "at": "x"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server)
        .loupe()
        .checks()
        .unwrap()
        .review("lck_1", Outcome::Fraud, "Not the bank's copy")
        .await
        .unwrap();
    assert_eq!(c.review.unwrap().by, "Sam Okafor");
}

#[tokio::test]
async fn a_slow_check_times_out_with_its_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/loupe/checks/lck_3"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "lck_3", "status": "running"})),
        )
        .mount(&server)
        .await;
    let e = client(&server)
        .loupe()
        .checks()
        .unwrap()
        .wait("lck_3", Duration::from_millis(30))
        .await
        .unwrap_err();
    assert!(matches!(e, Error::CheckTimeout { job_id: Some(ref j), .. } if j == "lck_3"));
}

#[test]
fn hosted_needs_a_key() {
    if std::env::var_os("UNFORGED_API_KEY").is_some() {
        return;
    }
    let e = Unforged::builder().mode(Mode::Hosted).build().unwrap_err();
    assert_eq!(e.api().unwrap().kind, ApiErrorKind::Authentication);
}

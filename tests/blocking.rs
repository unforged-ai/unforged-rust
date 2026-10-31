#![cfg(feature = "blocking")]
use serde_json::json;
use unforged::nib::VerifyOptions;
use unforged::{blocking, Mode};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn the_blocking_client_waits_for_the_answer() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(MockServer::start());
    rt.block_on(
        Mock::given(method("POST"))
            .and(path("/nib/checks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "completed", "decision": "APPROVE", "signatures": []
            })))
            .mount(&server),
    );
    let inner = unforged::Unforged::builder()
        .mode(Mode::Hosted)
        .api_key("unf_test_key")
        .base_url(server.uri())
        .build()
        .unwrap();
    let client = blocking::Unforged::from_client(inner).unwrap();
    let v = client
        .nib()
        .verify(b"%PDF".as_slice(), VerifyOptions::subject("cust-88"))
        .unwrap();
    assert!(v.approved());
    assert!(matches!(
        client.nib().licence(),
        Err(unforged::Error::NotSupported(_))
    ));
    assert!(client.loupe().checks().is_ok());
}

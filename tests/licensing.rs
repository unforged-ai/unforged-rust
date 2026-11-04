//! Licences online against a stand-in API, and offline against a stand-in licence server that
//! answers like `unforged-licence-server` and writes down what it was given.
use serde_json::json;
use unforged::licensing::{Include, NewLicence};
use unforged::{Error, Licence, Mode, Unforged};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJhY21lIn0.c2lnbmF0dXJl";

fn online(server: &MockServer) -> Unforged {
    Unforged::builder()
        .mode(Mode::Hosted)
        .api_key("unf_test_key")
        .base_url(server.uri())
        .build()
        .unwrap()
}

fn main_licence() -> serde_json::Value {
    json!({
        "id": "lic_main", "kind": "licence", "issued_to": "Acme Bank", "issued_by": "Unforged",
        "issued_on": "2026-01-01", "valid_from": "2026-01-01", "valid_until": "2026-12-31",
        "works_until": "2027-01-14", "includes": ["verify", "extract", "loupe"],
        "licence_server": true,
        "short_lived": {"at_once": 10, "longest_days": 14, "includes": ["verify", "extract"],
            "until": "2026-12-31", "counted": 2}
    })
}

fn given(from: &str, until: &str, includes: serde_json::Value) -> serde_json::Value {
    json!({
        "id": "sll_1", "for": "Leeds branch", "includes": includes, "valid_from": from,
        "valid_until": until, "status": "scheduled", "counts": true, "created_by": "someone",
        "created_at": "2026-11-04T10:00:00Z", "revoked_at": null, "revoked_by": null,
        "token": TOKEN
    })
}

#[tokio::test]
async fn online_licences_take_their_days_and_products() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/licensing/licences"))
        .and(body_json(json!({
            "for": "Leeds branch", "valid_from": "2026-12-01", "valid_until": "2026-12-07",
            "includes": ["verify", "extract", "loupe"]
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(given(
            "2026-12-01",
            "2026-12-07",
            json!(["verify", "extract", "loupe"]),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let l = online(&server)
        .licensing()
        .give(
            NewLicence::new("Leeds branch")
                .from("2026-12-01")
                .days(7)
                .includes([Include::Nib, Include::Loupe]),
        )
        .await
        .unwrap();
    assert_eq!(l.for_name, "Leeds branch");
    assert_eq!(l.token.expose(), TOKEN);
    assert_eq!(l.status, "scheduled");
    assert!(!format!("{l:?}").contains(TOKEN));
}

#[tokio::test]
async fn online_licences_left_open_follow_what_the_plan_allows() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/licensing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(main_licence()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/licensing/licences"))
        .and(body_json(json!({
            "for": "Leeds branch", "valid_from": "2026-12-01", "valid_until": "2026-12-14",
            "includes": ["verify", "extract"]
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(given(
            "2026-12-01",
            "2026-12-14",
            json!(["verify", "extract"]),
        )))
        .expect(1)
        .mount(&server)
        .await;

    let l = online(&server)
        .licensing()
        .give(NewLicence::new("Leeds branch").from("2026-12-01"))
        .await
        .unwrap();
    assert_eq!(l.valid_until, "2026-12-14");
}

#[tokio::test]
async fn the_main_licence_releases_and_links_read_back() {
    let server = MockServer::start().await;
    let mut master = main_licence();
    master["token"] = TOKEN.into();
    Mock::given(method("GET"))
        .and(path("/licensing/master"))
        .respond_with(ResponseTemplate::new(200).set_body_json(master))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/licensing/releases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "open": true, "new_version": "2.7.5",
            "releases": [{"version": "2.7.5", "published_at": null, "notes": null,
                "platforms": ["linux-x86_64", "macos-universal"],
                "licence_server": ["linux-x86_64"],
                "files": [{"name": "SHA256SUMS", "program": null, "platform": null, "size": 210,
                    "sha256": null}]}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/licensing/licences/sll_1/download-links"))
        .and(body_json(
            json!({"version": "latest", "platform": "linux-x86_64"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "odl_1", "licence_id": "sll_1", "for": "Leeds branch", "version": "2.7.5",
            "platform": "linux-x86_64", "expires_at": "2026-11-11T10:00:00Z", "token": "link-secret",
            "url": "https://unforged.ai/download/odl_1"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/licensing/licences/sll_1/revoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json({
            let mut v = given("2026-12-01", "2026-12-07", json!(["verify"]));
            v["status"] = "revoked".into();
            v
        }))
        .mount(&server)
        .await;

    let client = online(&server);
    let licensing = client.licensing();
    let m = licensing.master().await.unwrap();
    assert!(m.licence_server);
    assert_eq!(m.short_lived.unwrap().longest_days, 14);
    assert_eq!(m.token.expose(), TOKEN);

    let r = licensing.releases().await.unwrap();
    assert_eq!(r.releases[0].licence_server, vec!["linux-x86_64"]);
    assert_eq!(r.releases[0].files[0].program, None);

    let link = licensing
        .download_link("sll_1", "latest", "linux-x86_64")
        .await
        .unwrap();
    assert_eq!(link.token.expose(), "link-secret");
    assert_eq!(link.for_name, "Leeds branch");

    assert_eq!(licensing.revoke("sll_1").await.unwrap().status, "revoked");
}

#[cfg(unix)]
mod offline {
    use super::*;
    use std::path::PathBuf;

    /// A stand-in licence server: logs its arguments, its licence and the `--out` file's folder,
    /// then runs `body`.
    struct Server {
        dir: tempfile::TempDir,
    }

    impl Server {
        fn new(body: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().display().to_string();
            let script = format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$@\" > '{log}/args'\n\
                 printf '%s' \"$UNFORGED_LICENSE\" > '{log}/licence-env'\n\
                 printf '%s|%s' \"$UNFORGED_SERVER_PASS\" \"$UNFORGED_MAIN_LICENSE\" > '{log}/setup-env'\n\
                 out=''; prev=''\n\
                 for a in \"$@\"; do [ \"$prev\" = --out ] && out=\"$a\"; prev=\"$a\"; done\n\
                 if [ -n \"$out\" ]; then echo token > \"$out\"; dirname \"$out\" > '{log}/out-dir'; fi\n\
                 {body}\n"
            );
            let bin = dir.path().join("bin");
            // written by a child process, so no test here forks while holding it open for
            // writing (which makes running it fail with "text file busy")
            let src = dir.path().join("bin.src");
            std::fs::write(&src, script).unwrap();
            let ok = std::process::Command::new("sh")
                .arg("-c")
                .arg("cp \"$0\" \"$1\" && chmod 755 \"$1\"")
                .arg(&src)
                .arg(&bin)
                .status()
                .unwrap();
            assert!(ok.success());
            Server { dir }
        }

        fn client(&self) -> Unforged {
            Unforged::builder()
                .mode(Mode::Offline)
                .licence_server_binary(self.dir.path().join("bin"))
                .build()
                .unwrap()
        }

        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
        }

        fn args(&self) -> Vec<String> {
            self.read("args").lines().map(str::to_owned).collect()
        }
    }

    fn answer(json: &str) -> String {
        format!("cat <<'EOF'\n{json}\nEOF")
    }

    #[tokio::test]
    async fn the_same_call_gives_a_licence_from_the_licence_server() {
        let s = Server::new(&answer(&format!(
            r#"{{"id":"sll_9","for":"Leeds branch","includes":["verify","extract"],"valid_from":"2000-01-01","valid_until":"9999-01-07","file":"x","licence":"{TOKEN}"}}"#
        )));
        let l = s
            .client()
            .licensing()
            .give(
                NewLicence::new("Leeds branch")
                    .days(7)
                    .from("2026-12-01")
                    .includes([Include::Nib]),
            )
            .await
            .unwrap();
        assert_eq!(l.token.expose(), TOKEN);
        assert_eq!(l.status, "active");
        assert!(!l.extra.contains_key("file"));
        let args = s.args();
        assert_eq!(
            &args[..9],
            [
                "issue",
                "--for",
                "Leeds branch",
                "--days",
                "7",
                "--from",
                "2026-12-01",
                "--includes",
                "verify,extract"
            ]
        );
        assert!(args.contains(&"--json".to_string()));
        // the file it wrote went with the call
        assert!(!PathBuf::from(s.read("out-dir").trim()).exists());
    }

    #[tokio::test]
    async fn status_list_and_test_read_the_servers_answers() {
        let s = Server::new(&format!(
            "case \"$1\" in\n\
             status) {}\nexit 3;;\n\
             list) {}\n;;\n\
             test) {}\nexit 3;;\n\
             esac",
            answer(
                r#"{"ready":false,"customer":"Acme Bank","message":"Load your main licence first."}"#
            ),
            answer(
                r#"[{"id":"sll_1","for":"Leeds","includes":["loupe"],"valid_from":"2000-01-01","valid_until":"2000-01-02","made_at":1}]"#
            ),
            answer(
                r#"{"is":"short-lived","made_by":"licence_server","works":false,"state":"expired","for":"Leeds","includes":["loupe"]}"#
            ),
        ));
        let client = s.client();
        let licensing = client.licensing();

        let st = licensing.status().await.unwrap();
        assert!(!st.ready);
        assert_eq!(st.message.as_deref(), Some("Load your main licence first."));

        let all = licensing.licences().await.unwrap();
        assert_eq!(all[0].for_name, "Leeds");
        assert_eq!(all[0].status, "ended");
        assert!(all[0].token.is_empty());

        let t = licensing.test(Licence::Text(TOKEN.into())).await.unwrap();
        assert!(!t.works);
        assert_eq!(t.made_by, "licence_server");
        assert_eq!(s.args(), ["test", "--json"]);
        assert_eq!(s.read("licence-env"), TOKEN);
    }

    #[tokio::test]
    async fn its_pass_and_main_licence_reach_it_through_its_environment() {
        let s = Server::new(&answer(r#"{"ready":true}"#));
        let pass = s.dir.path().join("server.lic");
        std::fs::write(&pass, "ufsrv1.pass\n").unwrap();
        let client = Unforged::builder()
            .mode(Mode::Offline)
            .licence_server_binary(s.dir.path().join("bin"))
            .licence_server_pass(Licence::File(pass))
            .licence_server_main_licence(Licence::Text(TOKEN.into()))
            .build()
            .unwrap();
        assert!(client.licensing().status().await.unwrap().ready);
        assert_eq!(s.read("setup-env"), format!("ufsrv1.pass|{TOKEN}"));
        assert_eq!(s.args(), ["status", "--json"]);
    }

    #[tokio::test]
    async fn a_server_with_nothing_set_up_says_so_in_its_own_words() {
        let s = Server::new(
            "echo \"This licence server hasn't been set up yet. Load its pass and your main licence.\" >&2; exit 3",
        );
        let client = s.client();
        let st = client.licensing().status().await.unwrap();
        assert!(!st.ready);
        assert!(st
            .message
            .unwrap()
            .starts_with("This licence server hasn't been set up"));
        match client
            .licensing()
            .test(Licence::File("/nowhere.lic".into()))
            .await
        {
            Err(Error::Licence { message }) => assert!(message.starts_with("This licence server")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn the_servers_own_refusals_come_back_as_they_are() {
        let s = Server::new(
            "echo 'Your main licence ends on 31 December 2026, so this one can run to that day at the latest.' >&2; exit 3",
        );
        let e = s
            .client()
            .licensing()
            .give(NewLicence::new("Leeds").days(90))
            .await
            .unwrap_err();
        match e {
            Error::Licence { message } => assert!(message.starts_with("Your main licence ends")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn workspace_only_calls_say_what_to_do_offline() {
        let s = Server::new("exit 0");
        let client = s.client();
        let licensing = client.licensing();
        assert!(matches!(
            licensing.revoke("sll_1").await,
            Err(Error::NotSupported(_))
        ));
        assert!(matches!(
            licensing.master().await,
            Err(Error::NotSupported(_))
        ));
    }
}

#[tokio::test]
async fn the_licence_server_needs_an_offline_client() {
    let server = MockServer::start().await;
    assert!(matches!(
        online(&server).licensing().status().await,
        Err(Error::NotSupported(_))
    ));
}

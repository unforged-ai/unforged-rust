use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::Method;
use serde_json::Value;

use crate::error::{ApiError, ApiErrorKind, Error, Result};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.unforged.sh";
const REGION_SUFFIX: &str = ".unforged.sh";
const RETRY_STATUS: [u16; 5] = [408, 429, 502, 503, 504];
const RETRY_CODES: [&str; 5] = [
    "idempotency_in_progress",
    "busy",
    "not_ready",
    "org_moving",
    "storage_provisioning",
];

/// POSTs that create something new each time, so they carry no idempotency key and aren't
/// retried once sent.
fn not_idempotent(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    const ONCE: [&str; 9] = [
        "/keys",
        "/nib/sessions",
        "/nib/bucket-keys",
        "/nib/webhooks",
        "/folio/webhooks",
        "/sig/webhooks",
        "/loupe/webhooks",
        "/folio/s3/keys",
        "/folio/s3/sessions",
    ];
    if ONCE.contains(&p) {
        return true;
    }
    p.strip_prefix("/nib/webhooks/")
        .and_then(|rest| rest.strip_suffix("/secret/roll"))
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

pub(crate) fn base_url_for(region: Option<&str>) -> String {
    match region {
        Some(r) if !r.trim().is_empty() => format!("https://{}.api.unforged.sh", r.trim()),
        _ => DEFAULT_BASE_URL.into(),
    }
}

/// A request body.
#[derive(Clone, Default)]
pub(crate) enum Body {
    #[default]
    None,
    Json(Value),
    Raw {
        content_type: String,
        data: Vec<u8>,
    },
}

/// One request: what to send, and which error statuses come back as answers.
#[derive(Clone)]
pub(crate) struct Request {
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Body,
    pub idempotency_key: Option<String>,
    pub accept: &'static str,
}

impl Request {
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Vec::new(),
            body: Body::None,
            idempotency_key: None,
            accept: "application/json",
        }
    }

    pub fn get(path: impl Into<String>) -> Self {
        Self::new(Method::GET, path)
    }

    pub fn post(path: impl Into<String>) -> Self {
        Self::new(Method::POST, path)
    }

    pub fn param(mut self, k: &str, v: impl ToString) -> Self {
        self.query.push((k.into(), v.to_string()));
        self
    }

    pub fn opt(self, k: &str, v: Option<impl ToString>) -> Self {
        match v {
            Some(v) => self.param(k, v),
            None => self,
        }
    }

    pub fn json(mut self, v: Value) -> Self {
        self.body = Body::Json(v);
        self
    }

    pub fn raw(mut self, content_type: String, data: Vec<u8>) -> Self {
        self.body = Body::Raw { content_type, data };
        self
    }

    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    pub fn accept(mut self, accept: &'static str) -> Self {
        self.accept = accept;
        self
    }

}

/// An answer.
#[derive(Debug)]
pub(crate) struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(&self) -> Result<Value> {
        if self.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&self.body)
            .map_err(|e| Error::Decode(format!("the API's answer wasn't JSON: {e}")))
    }
}

/// One part of a multipart body: name, file name, content type, bytes.
pub(crate) struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: String,
    pub data: Vec<u8>,
}

impl Part {
    pub fn json(name: &str, v: &Value) -> Self {
        Self {
            name: name.into(),
            filename: None,
            content_type: "application/json".into(),
            data: v.to_string().into_bytes(),
        }
    }

    pub fn text(name: &str, s: &str) -> Self {
        Self {
            name: name.into(),
            filename: None,
            content_type: "text/plain".into(),
            data: s.as_bytes().to_vec(),
        }
    }

    pub fn file(name: &str, filename: String, data: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            filename: Some(filename),
            content_type: "application/octet-stream".into(),
            data,
        }
    }
}

/// A multipart/form-data body and its content type.
pub(crate) fn multipart(parts: Vec<Part>) -> (String, Vec<u8>) {
    let boundary = format!("unforged-{}", uuid::Uuid::new_v4().simple());
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let mut disp = format!("form-data; name=\"{}\"", quote(&p.name));
        if let Some(f) = &p.filename {
            disp.push_str(&format!("; filename=\"{}\"", quote(f)));
        }
        out.extend_from_slice(
            format!(
                "Content-Disposition: {disp}\r\nContent-Type: {}\r\n\r\n",
                p.content_type
            )
            .as_bytes(),
        );
        out.extend_from_slice(&p.data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), out)
}

fn quote(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\r', '\n'], " ")
}

/// Settings for the API client.
#[derive(Clone)]
pub(crate) struct HttpConfig {
    pub api_key: String,
    pub base_url: String,
    pub api_version: Option<String>,
    pub org: Option<String>,
    pub timeout: Duration,
    pub max_retries: u32,
    pub poll_interval: Duration,
}

/// Sends requests, retrying the ones that are safe to retry.
#[derive(Clone)]
pub(crate) struct Transport {
    client: reqwest::Client,
    config: Arc<HttpConfig>,
    base_url: Arc<RwLock<String>>,
}

impl Transport {
    pub fn new(config: HttpConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.timeout)
            .build()
            .map_err(|e| Error::Connection {
                message: format!("the HTTP client couldn't be set up: {e}"),
                sent: false,
            })?;
        let base = config.base_url.trim_end_matches('/').to_string();
        Ok(Self {
            client,
            base_url: Arc::new(RwLock::new(base)),
            config: Arc::new(config),
        })
    }

    pub fn base_url(&self) -> String {
        self.base_url
            .read()
            .map(|b| b.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    pub fn poll_interval(&self) -> Duration {
        self.config.poll_interval
    }

    fn set_base_url(&self, url: &str) {
        let mut b = self.base_url.write().unwrap_or_else(|p| p.into_inner());
        *b = url.trim_end_matches('/').to_string();
    }

    fn headers(&self, req: &Request, once: bool) -> Result<HeaderMap> {
        let mut h = HeaderMap::new();
        let bad = |what: &str| Error::invalid(format!("the {what} has characters a header can't carry"));
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", self.config.api_key))
            .map_err(|_| bad("API key"))?;
        auth.set_sensitive(true);
        h.insert(AUTHORIZATION, auth);
        h.insert(ACCEPT, HeaderValue::from_static(req.accept));
        h.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("unforged-rust/", env!("CARGO_PKG_VERSION"))),
        );
        if let Some(v) = &self.config.api_version {
            h.insert(
                "unforged-version",
                HeaderValue::from_str(v).map_err(|_| bad("API version"))?,
            );
        }
        if let Some(o) = &self.config.org {
            h.insert(
                "unforged-org",
                HeaderValue::from_str(o).map_err(|_| bad("organisation"))?,
            );
        }
        match &req.body {
            Body::Json(_) => {
                h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            }
            Body::Raw { content_type, .. } => {
                h.insert(
                    CONTENT_TYPE,
                    HeaderValue::from_str(content_type).map_err(|_| bad("content type"))?,
                );
            }
            Body::None => {}
        }
        if req.method == Method::POST && !once {
            let key = req
                .idempotency_key
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            h.insert(
                "idempotency-key",
                HeaderValue::from_str(&key).map_err(|_| bad("idempotency key"))?,
            );
        }
        Ok(h)
    }

    /// Sends `req`; error answers become errors, except statuses in `ok`.
    pub async fn send(&self, req: Request, ok: &[u16]) -> Result<Response> {
        let once = req.method == Method::POST && not_idempotent(&req.path);
        let headers = self.headers(&req, once)?;
        let body: Option<Vec<u8>> = match &req.body {
            Body::None => None,
            Body::Json(v) => Some(v.to_string().into_bytes()),
            Body::Raw { data, .. } => Some(data.clone()),
        };
        let mut followed_region = false;
        let mut attempt = 0u32;
        loop {
            let base = self.base_url();
            let mut rb = self
                .client
                .request(req.method.clone(), format!("{base}{}", req.path))
                .headers(headers.clone());
            if !req.query.is_empty() {
                rb = rb.query(&req.query);
            }
            if let Some(b) = &body {
                rb = rb.body(b.clone());
            }
            let resp = match self.exchange(rb, &base).await {
                Ok(r) => r,
                Err(Error::Connection { message, sent }) => {
                    if attempt >= self.config.max_retries || (once && sent) {
                        return Err(Error::Connection { message, sent });
                    }
                    attempt += 1;
                    tokio::time::sleep(backoff(attempt, None)).await;
                    continue;
                }
                Err(e) => return Err(e),
            };
            if resp.status < 400 || ok.contains(&resp.status) {
                return Ok(resp);
            }
            let err = to_error(&resp);
            if err.kind == ApiErrorKind::WrongRegion && !followed_region {
                if let Some(host) = err.api_host().filter(|h| may_follow(h)) {
                    self.set_base_url(host);
                    followed_region = true;
                    continue;
                }
            }
            let mut retryable = (RETRY_STATUS.contains(&resp.status)
                || err.code.as_deref().is_some_and(|c| RETRY_CODES.contains(&c)))
                && !(resp.status == 504 && err.job_id().is_some());
            if once && resp.status != 429 {
                retryable = false;
            }
            if retryable
                && err.code.as_deref() != Some("quota_exceeded")
                && attempt < self.config.max_retries
            {
                attempt += 1;
                tokio::time::sleep(backoff(attempt, err.retry_after)).await;
                continue;
            }
            return Err(Error::Api(Box::new(err)));
        }
    }

    async fn exchange(&self, rb: reqwest::RequestBuilder, base: &str) -> Result<Response> {
        let resp = rb.send().await.map_err(|e| Error::Connection {
            message: format!("couldn't reach {base}: {}", describe(&e)),
            sent: !e.is_connect(),
        })?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp.bytes().await.map_err(|_| Error::Connection {
            message: format!("the answer from {base} was cut short; please try again"),
            sent: true,
        })?;
        Ok(Response {
            status,
            headers,
            body: body.to_vec(),
        })
    }

    pub async fn json(&self, req: Request) -> Result<Value> {
        self.send(req, &[]).await?.json()
    }

    pub async fn bytes(&self, req: Request) -> Result<Vec<u8>> {
        Ok(self.send(req, &[]).await?.body)
    }
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("base_url", &self.base_url())
            .finish_non_exhaustive()
    }
}

fn describe(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "it took too long to answer"
    } else if e.is_connect() {
        "the connection couldn't be made"
    } else {
        "the request didn't complete"
    }
}

fn may_follow(host: &str) -> bool {
    let Ok(u) = url::Url::parse(host) else {
        return false;
    };
    let name = u.host_str().unwrap_or("").to_ascii_lowercase();
    u.scheme() == "https"
        && (name == REGION_SUFFIX.trim_start_matches('.') || name.ends_with(REGION_SUFFIX))
}

fn to_error(resp: &Response) -> ApiError {
    let problem = match serde_json::from_slice::<Value>(&resp.body) {
        Ok(v @ Value::Object(_)) => v,
        _ => {
            let text = String::from_utf8_lossy(&resp.body);
            let text: String = text.trim().chars().take(500).collect();
            let detail = if text.is_empty() {
                format!("HTTP {}", resp.status)
            } else {
                text
            };
            serde_json::json!({ "detail": detail })
        }
    };
    let header = |k: &str| {
        resp.headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let retry_after = header("retry-after")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64);
    ApiError::new(
        Some(resp.status),
        problem,
        header("x-request-id"),
        retry_after,
    )
}

fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(r) = retry_after {
        return r.min(Duration::from_secs(60));
    }
    let base = (0.5 * 2f64.powi(attempt.saturating_sub(1) as i32)).min(8.0);
    Duration::from_secs_f64(base * (0.8 + 0.4 * jitter()))
}

fn jitter() -> f64 {
    let n = uuid::Uuid::new_v4().as_u128() as u64;
    (n >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creating_posts_are_sent_once() {
        assert!(not_idempotent("/keys"));
        assert!(not_idempotent("/nib/webhooks/wh_1/secret/roll"));
        assert!(!not_idempotent("/nib/webhooks/a/b/secret/roll"));
        assert!(!not_idempotent("/nib/checks"));
    }

    #[test]
    fn only_our_hosts_are_followed() {
        assert!(may_follow("https://eu.api.unforged.sh"));
        assert!(may_follow("https://unforged.sh"));
        assert!(!may_follow("http://eu.api.unforged.sh"));
        assert!(!may_follow("https://unforged.sh.example.com"));
        assert!(!may_follow("https://evilunforged.sh"));
    }

    #[test]
    fn regions_pick_their_host() {
        assert_eq!(base_url_for(None), DEFAULT_BASE_URL);
        assert_eq!(base_url_for(Some("uk")), "https://uk.api.unforged.sh");
    }

    #[test]
    fn backoff_stays_in_bounds() {
        for a in 1..10 {
            let d = backoff(a, None);
            assert!(d <= Duration::from_secs_f64(9.7), "{d:?}");
        }
        assert_eq!(
            backoff(1, Some(Duration::from_secs(600))),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn multipart_quotes_names() {
        let (ct, body) = multipart(vec![Part::file("questioned", "a\"b.pdf".into(), b"x".to_vec())]);
        assert!(ct.starts_with("multipart/form-data; boundary=unforged-"));
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("filename=\"a\\\"b.pdf\""));
    }
}

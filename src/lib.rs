//! Official SDK for [Movoa](https://movoa.ai), the AI video generator with sound.
//!
//! One API key turns a prompt into text to video, or a photo into photo to
//! video, and every clip comes back with sound. Models: MiniMax H3 (default),
//! Veo 3.1 Fast, Seedance 2.5 and Seedance 2.0 (Fast, Mini). Create a key at
//! <https://movoa.ai/settings/apikeys> and set `MOVOA_API_KEY`. Runs cost
//! credits on your Movoa account, at the prices on <https://movoa.ai/pricing>.
//!
//! ```no_run
//! let clip = movoa::generate_video("a paper lantern drifting over a night river", &movoa::Options::new())?;
//! let moved = movoa::generate_video("the camera slowly orbits the statue",
//!     &movoa::Options::new().image_url("https://example.com/statue.jpg"))?;
//! # Ok::<(), movoa::Error>(())
//! ```

use serde_json::{json, Map, Value};
use std::fmt;
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The service this crate talks to.
pub const BASE_URL: &str = "https://movoa.ai";
/// Where an API key is created.
pub const KEYS_URL: &str = "https://movoa.ai/settings/apikeys";
/// Default model, the cheapest one.
pub const DEFAULT_MODEL: &str = "minimax-h3";
/// Every model id the API takes.
pub const MODELS: [&str; 6] = ["minimax-h3", "veo-3-1-fast", "seedance-2-5", "seedance-2", "seedance-2-fast", "seedance-2-mini"];

/// What went wrong, split by what to do next.
#[derive(Debug)]
pub enum Error {
    /// No API key, or the key is invalid or deleted.
    Auth(String),
    /// The account does not have enough credits for this run.
    Quota(String),
    /// The model or quality needs a plan, or the account has never bought credits.
    Plan(String),
    /// Too many runs on this account are generating at once.
    Busy(String),
    /// The content filter refused the prompt (code -2) or a cooldown is active (code -3). Reword it.
    Rejected(String),
    /// The run was accepted but ended as failed or canceled.
    Generation { task_id: String, error_message: Option<String>, failure_kind: Option<String> },
    /// The wait ran out; the run may still finish. Check it later with `status`.
    Timeout { task_id: String },
    /// Any other refusal (carries the server message), a network failure or an unexpected answer.
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Auth(m) if m.contains("settings/apikeys") => write!(f, "movoa: {m}"),
            Error::Auth(m) => write!(f, "movoa: {m} (create a key at {KEYS_URL})"),
            Error::Quota(m) | Error::Plan(m) | Error::Busy(m) | Error::Other(m) => write!(f, "movoa: {m}"),
            Error::Rejected(m) => write!(f, "movoa: {m} (reword the prompt rather than retrying)"),
            Error::Generation { task_id, error_message, .. } => match error_message {
                Some(m) => write!(f, "movoa: run {task_id} failed: {m}"),
                None => write!(f, "movoa: run {task_id} failed"),
            },
            Error::Timeout { task_id } => write!(f, "movoa: run {task_id} is still generating; check it later with status()"),
        }
    }
}

impl std::error::Error for Error {}

/// What to generate. Leave a field out to use the model default.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub model: Option<String>,
    /// A public https image; turns the run into photo to video with it as the first frame.
    pub image_url: Option<String>,
    /// Model options sent as they are (`duration`, `resolution`, `aspect_ratio`, ...).
    pub options: Map<String, Value>,
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn model(mut self, m: &str) -> Self {
        self.model = Some(m.into());
        self
    }
    pub fn image_url(mut self, u: &str) -> Self {
        self.image_url = Some(u.into());
        self
    }
    /// Set one option, e.g. `.set("duration", 6).set("aspect_ratio", "9:16")`.
    pub fn set(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.options.insert(key.into(), value.into());
        self
    }
    /// The request body for `POST /api/ai/generate`.
    pub fn body(&self, prompt: &str) -> Value {
        let mut opts = self.options.clone();
        opts.insert("sound".into(), Value::Bool(true));
        if let Some(u) = &self.image_url {
            opts.insert("image_input".into(), json!([u]));
        }
        json!({
            // The server routes by model; this only has to be non-empty.
            "provider": "auto",
            "mediaType": "video",
            "model": self.model.as_deref().unwrap_or(DEFAULT_MODEL),
            "scene": if self.image_url.is_some() { "image-to-video" } else { "text-to-video" },
            "prompt": prompt,
            "options": Value::Object(opts),
        })
    }
}

/// One run, from `/api/ai/query`.
#[derive(Clone, Debug)]
pub struct Task {
    pub id: String,
    /// "pending", "processing", "success", "failed" or "canceled".
    pub status: String,
    pub videos: Vec<String>,
    pub error_message: Option<String>,
    pub failure_kind: Option<String>,
    pub queued: bool,
    /// Seconds until the run leaves the queue, when queued.
    pub queue_eta: Option<f64>,
    /// The whole `data` object as the API sent it.
    pub raw: Value,
}

/// `(url, body) -> (http status, raw body)`. Swap it in tests; `Err` means the request never got an answer.
pub type Transport = Arc<dyn Fn(&str, Option<&Value>, &str) -> Result<(u16, String), Error> + Send + Sync>;

/// A client bound to one API key.
#[derive(Clone)]
pub struct Client {
    key: String,
    base: String,
    transport: Transport,
    /// Time between status checks. Default 10 seconds.
    pub poll_every: Duration,
    /// Total wait for one run. Default 30 minutes; runs can queue.
    pub timeout: Duration,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").field("base", &self.base).field("poll_every", &self.poll_every).field("timeout", &self.timeout).finish()
    }
}

fn http(url: &str, body: Option<&Value>, key: &str) -> Result<(u16, String), Error> {
    let req = ureq::post(url)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .set("User-Agent", concat!("movoa-rust/", env!("CARGO_PKG_VERSION")));
    let resp = match body {
        Some(b) => req.send_string(&b.to_string()),
        None => req.call(),
    };
    match resp {
        Ok(r) => Ok((r.status(), r.into_string().unwrap_or_default())),
        Err(ureq::Error::Status(code, r)) => Ok((code, r.into_string().unwrap_or_default())),
        Err(e) => Err(Error::Other(format!("could not reach movoa.ai: {e}"))),
    }
}

impl Client {
    /// A client for this key. The host is `MOVOA_BASE_URL` when set, else movoa.ai.
    pub fn new(api_key: impl Into<String>) -> Self {
        let base = std::env::var("MOVOA_BASE_URL").ok().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| BASE_URL.into());
        Client {
            key: api_key.into().trim().into(),
            base: base.trim_end_matches('/').into(),
            transport: Arc::new(http),
            poll_every: Duration::from_secs(10),
            timeout: Duration::from_secs(30 * 60),
        }
    }

    /// Read the key from `MOVOA_API_KEY`.
    pub fn from_env() -> Result<Self, Error> {
        match std::env::var("MOVOA_API_KEY") {
            Ok(k) if !k.trim().is_empty() => Ok(Client::new(k)),
            _ => Err(Error::Auth("an API key is required: set MOVOA_API_KEY".into())),
        }
    }

    /// Point the client at another host (tests, a local build of the site).
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into().trim_end_matches('/').into();
        self
    }

    /// Replace the HTTP layer (tests).
    pub fn with_transport(mut self, t: Transport) -> Self {
        self.transport = t;
        self
    }

    /// Start a run and return its task id without waiting.
    pub fn submit(&self, prompt: &str, opts: &Options) -> Result<String, Error> {
        if prompt.trim().is_empty() {
            return Err(Error::Other("prompt is required".into()));
        }
        let env = self.post("/api/ai/generate", Some(opts.body(prompt)))?;
        match env.pointer("/data/id") {
            Some(Value::String(id)) if !id.is_empty() => Ok(id.clone()),
            Some(Value::Number(n)) => Ok(n.to_string()),
            _ => Err(Error::Other("movoa.ai accepted the run but returned no id".into())),
        }
    }

    /// Look up a run.
    pub fn status(&self, task_id: &str) -> Result<Task, Error> {
        let env = self.post("/api/ai/query", Some(json!({ "taskId": task_id })))?;
        Ok(parse_task(env.get("data").cloned().unwrap_or(Value::Null)))
    }

    /// Credits left on the account.
    pub fn credits(&self) -> Result<i64, Error> {
        let env = self.post("/api/user/get-user-credits", None)?;
        Ok(env.pointer("/data/remainingCredits").and_then(Value::as_i64).unwrap_or(0))
    }

    /// Submit, wait, and return the URL of the finished clip.
    pub fn generate_video(&self, prompt: &str, opts: &Options) -> Result<String, Error> {
        let id = self.submit(prompt, opts)?;
        self.wait(&id)
    }

    /// Wait for a submitted run and return its video URL.
    pub fn wait(&self, task_id: &str) -> Result<String, Error> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let t = self.status(task_id)?;
            match t.status.as_str() {
                "success" => {
                    return t.videos.first().cloned().ok_or_else(|| Error::Generation {
                        task_id: task_id.into(),
                        error_message: Some("finished with no video".into()),
                        failure_kind: None,
                    })
                }
                "failed" | "canceled" => {
                    return Err(Error::Generation { task_id: task_id.into(), error_message: t.error_message, failure_kind: t.failure_kind })
                }
                _ => {}
            }
            if Instant::now() + self.poll_every > deadline {
                return Err(Error::Timeout { task_id: task_id.into() });
            }
            sleep(self.poll_every);
        }
    }

    fn post(&self, path: &str, body: Option<Value>) -> Result<Value, Error> {
        if self.key.is_empty() {
            return Err(Error::Auth("an API key is required: set MOVOA_API_KEY".into()));
        }
        let (code, raw) = (self.transport)(&format!("{}{path}", self.base), body.as_ref(), &self.key)?;
        parse_envelope(code, &raw)
    }
}

/// Read one answer: the envelope on `code: 0`, else the error that says what to do next.
///
/// movoa.ai answers almost every refusal as HTTP 200 with a non-zero `code`
/// and a `data.reason`, so the status code alone reads a refusal as success.
pub fn parse_envelope(http: u16, raw: &str) -> Result<Value, Error> {
    let env: Option<Value> = if raw.trim().is_empty() { Some(json!({})) } else { serde_json::from_str(raw).ok() };
    match env {
        Some(v) if v.is_object() => {
            if http < 400 && v.get("code").and_then(Value::as_i64) == Some(0) {
                Ok(v)
            } else {
                Err(classify(http, &v))
            }
        }
        _ if http == 401 => Err(classify(401, &json!({}))),
        _ => Err(Error::Other(format!("movoa.ai returned a non-JSON response (HTTP {http})"))),
    }
}

/// Map an HTTP status and envelope to the error that says what to do next.
pub fn classify(http: u16, env: &Value) -> Error {
    let msg = env.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()).map(String::from).unwrap_or_else(|| format!("request failed (HTTP {http})"));
    let code = env.get("code").and_then(Value::as_i64);
    let reason = env.pointer("/data/reason").and_then(Value::as_str).unwrap_or("");
    if http == 401 {
        return Error::Auth(msg);
    }
    if code == Some(-2) || code == Some(-3) {
        return Error::Rejected(msg);
    }
    match reason {
        "insufficient_credits" => Error::Quota(msg),
        "plan_required" => Error::Plan(msg),
        "concurrency" => Error::Busy(msg),
        _ if msg.to_lowercase().contains("no auth") => Error::Auth(msg),
        _ => Error::Other(msg),
    }
}

/// Read the `data` object of `/api/ai/query`.
pub fn parse_task(d: Value) -> Task {
    let s = |k: &str| d.get(k).and_then(Value::as_str).map(String::from);
    let id = match d.get("id") {
        Some(Value::Number(n)) => n.to_string(),
        _ => s("id").unwrap_or_default(),
    };
    Task {
        id,
        status: s("status").unwrap_or_default(),
        videos: d.get("videos").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default(),
        error_message: s("errorMessage"),
        failure_kind: s("failureKind"),
        queued: d.get("queued").and_then(Value::as_bool).unwrap_or(false),
        queue_eta: d.get("queueEta").and_then(Value::as_f64),
        raw: d,
    }
}

/// Generate one clip with sound (key from `MOVOA_API_KEY`) and return its URL.
pub fn generate_video(prompt: &str, opts: &Options) -> Result<String, Error> {
    Client::from_env()?.generate_video(prompt, opts)
}

/// Start a run (key from `MOVOA_API_KEY`) and return its task id.
pub fn submit(prompt: &str, opts: &Options) -> Result<String, Error> {
    Client::from_env()?.submit(prompt, opts)
}

/// Look up a run (key from `MOVOA_API_KEY`).
pub fn status(task_id: &str) -> Result<Task, Error> {
    Client::from_env()?.status(task_id)
}

/// Credits left on the account (key from `MOVOA_API_KEY`).
pub fn credits() -> Result<i64, Error> {
    Client::from_env()?.credits()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Calls = Arc<Mutex<Vec<(String, Option<Value>, String)>>>;

    /// Answers in the real response shapes of movoa.ai.
    fn fake(calls: Calls, polls: Vec<Value>, submit_reply: (u16, String)) -> Transport {
        let polls = Arc::new(Mutex::new(polls));
        Arc::new(move |url: &str, body: Option<&Value>, key: &str| {
            calls.lock().unwrap().push((url.into(), body.cloned(), key.into()));
            if url.ends_with("/api/ai/generate") {
                return Ok(submit_reply.clone());
            }
            if url.ends_with("/api/ai/query") {
                let mut p = polls.lock().unwrap();
                let d = if p.len() > 1 { p.remove(0) } else { p[0].clone() };
                return Ok((200, json!({"code": 0, "message": "ok", "data": d}).to_string()));
            }
            Ok((200, json!({"code": 0, "message": "ok", "data": {"remainingCredits": 42}}).to_string()))
        })
    }

    fn ok_submit() -> (u16, String) {
        (200, json!({"code": 0, "message": "ok", "data": {"id": "t1", "status": "pending"}}).to_string())
    }

    fn client(calls: Calls, polls: Vec<Value>, submit_reply: (u16, String)) -> Client {
        let mut c = Client::new("sk-test").with_base("https://t").with_transport(fake(calls, polls, submit_reply));
        c.poll_every = Duration::from_millis(0);
        c
    }

    #[test]
    fn text_to_video_success() {
        let calls: Calls = Default::default();
        let polls = vec![
            json!({"id": "t1", "status": "pending", "queued": true, "queueEta": 30}),
            json!({"id": "t1", "status": "processing"}),
            json!({"id": "t1", "status": "success", "videos": ["https://cdn/v.mp4"]}),
        ];
        let c = client(calls.clone(), polls, ok_submit());
        let url = c.generate_video("a boat", &Options::new().set("duration", 6)).unwrap();
        assert_eq!(url, "https://cdn/v.mp4");
        let calls = calls.lock().unwrap();
        assert_eq!(calls[0].0, "https://t/api/ai/generate");
        assert_eq!(calls[0].2, "sk-test");
        assert_eq!(
            calls[0].1.clone().unwrap(),
            json!({"provider": "auto", "mediaType": "video", "model": "minimax-h3", "scene": "text-to-video", "prompt": "a boat", "options": {"sound": true, "duration": 6}})
        );
        assert_eq!(calls[1].1.clone().unwrap(), json!({"taskId": "t1"}));
        assert_eq!(calls.len(), 4);
    }

    #[test]
    fn photo_to_video_body() {
        let b = Options::new().model("seedance-2-5").image_url("https://x/y.jpg").set("aspect_ratio", "9:16").body("orbit");
        assert_eq!(b["scene"], "image-to-video");
        assert_eq!(b["model"], "seedance-2-5");
        assert_eq!(b["options"], json!({"sound": true, "aspect_ratio": "9:16", "image_input": ["https://x/y.jpg"]}));
    }

    #[test]
    fn error_mapping() {
        let cases: Vec<(u16, &str, fn(&Error) -> bool)> = vec![
            (401, r#"{"code":-1,"message":"Invalid API key. Create one at https://movoa.ai/settings/apikeys","data":{"reason":"invalid_api_key"}}"#, |e| matches!(e, Error::Auth(_))),
            (401, "<html>unauthorized</html>", |e| matches!(e, Error::Auth(_))),
            (200, r#"{"code":-1,"message":"insufficient credits","data":{"reason":"insufficient_credits"}}"#, |e| matches!(e, Error::Quota(_))),
            (200, r#"{"code":-1,"message":"This model or quality needs a plan.","data":{"reason":"plan_required"}}"#, |e| matches!(e, Error::Plan(_))),
            (200, r#"{"code":-1,"message":"busy","data":{"reason":"concurrency"}}"#, |e| matches!(e, Error::Busy(_))),
            (200, r#"{"code":-2,"message":"blocked"}"#, |e| matches!(e, Error::Rejected(_))),
            (200, r#"{"code":-3,"message":"cooldown"}"#, |e| matches!(e, Error::Rejected(_))),
            (200, r#"{"code":-1,"message":"no auth, please sign in"}"#, |e| matches!(e, Error::Auth(_))),
            (200, r#"{"code":-1,"message":"generation failed, please try again"}"#, |e| matches!(e, Error::Other(m) if m.contains("generation failed"))),
            (502, "Bad gateway", |e| matches!(e, Error::Other(_))),
        ];
        for (code, body, want) in cases {
            let c = client(Default::default(), vec![], (code, body.into()));
            let e = c.submit("x", &Options::new()).unwrap_err();
            assert!(want(&e), "{code} {body} -> {e:?}");
        }
    }

    #[test]
    fn failed_and_canceled() {
        let c = client(Default::default(), vec![json!({"id": "t1", "status": "failed", "errorMessage": "provider down", "failureKind": "upstream"})], ok_submit());
        match c.generate_video("x", &Options::new()) {
            Err(Error::Generation { task_id, error_message, failure_kind }) => {
                assert_eq!(task_id, "t1");
                assert_eq!(error_message.as_deref(), Some("provider down"));
                assert_eq!(failure_kind.as_deref(), Some("upstream"));
            }
            e => panic!("{e:?}"),
        }
        let c = client(Default::default(), vec![json!({"id": "t1", "status": "canceled"})], ok_submit());
        assert!(matches!(c.wait("t1"), Err(Error::Generation { .. })));
    }

    #[test]
    fn timeout_carries_task_id() {
        let mut c = client(Default::default(), vec![json!({"id": "t1", "status": "processing", "queued": true, "queueEta": 60})], ok_submit());
        c.timeout = Duration::from_millis(1);
        c.poll_every = Duration::from_millis(5);
        assert!(matches!(c.wait("t1"), Err(Error::Timeout { task_id }) if task_id == "t1"));
    }

    #[test]
    fn status_and_credits() {
        let c = client(Default::default(), vec![json!({"id": "t1", "status": "processing", "queued": true, "queueEta": 60})], ok_submit());
        let t = c.status("t1").unwrap();
        assert!(t.queued);
        assert_eq!(t.queue_eta, Some(60.0));
        assert_eq!(c.credits().unwrap(), 42);
        let no_auth: Transport = Arc::new(|_: &str, _: Option<&Value>, _: &str| Ok((200, r#"{"code":-1,"message":"no auth, please sign in"}"#.into())));
        assert!(matches!(Client::new("sk").with_transport(no_auth).credits(), Err(Error::Auth(_))));
    }

    #[test]
    fn empty_key_and_prompt() {
        let boom: Transport = Arc::new(|_: &str, _: Option<&Value>, _: &str| panic!("no request without a key"));
        assert!(matches!(Client::new("  ").with_transport(boom).credits(), Err(Error::Auth(_))));
        assert!(Client::new("sk").submit(" ", &Options::new()).is_err());
    }

    /// Live (after key auth is live on movoa.ai): MOVOA_API_KEY=... cargo test -- --ignored
    #[test]
    #[ignore]
    fn live_smoke() {
        let c = Client::from_env().unwrap();
        assert!(c.credits().unwrap() >= 0);
        let wrong = Client::new("sk-wrong-key-for-test");
        assert!(matches!(wrong.submit("test", &Options::new()), Err(Error::Auth(_))));
    }
}

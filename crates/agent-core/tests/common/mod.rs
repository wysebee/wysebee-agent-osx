//! What the tests share: a fake Wysebee Cloud on 127.0.0.1 speaking the agent-link protocol's
//! app side, and an agent over temporary folders and a secret store in memory. Nothing
//! here reaches the internet, the real Keychain or a real folder of the person's.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_core::secrets::MemorySecrets;
use agent_core::{Agent, Options, Step, Timing};
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

pub const LINK_ID: &str = "0123456789abcdef0123456789abcdef";

/// What the fake answers a poll with.
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Pending,
    Refused(String),
    Paired,
    Expired,
    Unpaired,
    /// 500, no body worth reading.
    Down,
    /// 200 with a state nobody knows.
    Garbage,
}

pub struct Cloud {
    pub mode: Mode,
    /// What `/start` answers instead of a code: a status and a body.
    pub start_answer: Option<(u16, Value)>,
    pub starts: Vec<Value>,
    /// The SHA-256 the last `/start` carried: the only secret that gets in.
    pub hash: Option<String>,
    /// `programs` in a paired answer; None leaves the key out.
    pub programs: Option<Vec<Value>>,
    /// `skills` in a paired answer; None leaves the key out.
    pub skills: Option<Vec<Value>>,
    /// A skill's files the cloud serves, by id (the bytes need not hash to it).
    pub files: HashMap<String, Vec<u8>>,
    /// Every file id asked for.
    pub file_gets: Vec<String>,
    pub calls: VecDeque<Value>,
    pub cancelled: Vec<String>,
    /// Every poll's `status`, and when it came.
    pub polls: Vec<(Instant, Value)>,
    /// Every bearer token seen, on any route.
    pub bearers: Vec<String>,
    pub results: HashMap<String, Value>,
    /// What posting a result answers, and how many were posted.
    pub result_status: u16,
    pub result_posts: usize,
    pub delete_status: u16,
    pub deletes: usize,
    /// How long a poll that brings no call is held (the long poll).
    pub hold: Duration,
    /// How long a skill's file takes to come (a large one, on a slow line).
    pub file_hold: Duration,
    /// `expires_in` in a pending answer; None leaves the key out.
    pub expires_in: Option<u64>,
    /// When set, a poll the cloud knows waits for a permit before it is answered: the test
    /// decides when it comes back.
    pub poll_gate: Option<Arc<Semaphore>>,
    /// When set, a skill's file, once asked for (in `file_gets`), waits for a permit.
    pub file_gate: Option<Arc<Semaphore>>,
}

pub type Shared = Arc<Mutex<Cloud>>;

pub struct Fake {
    pub url: String,
    pub cloud: Shared,
}

fn bearer(headers: &HeaderMap) -> String {
    headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("").to_string()
}

fn known(cloud: &mut Cloud, id: &str, headers: &HeaderMap) -> bool {
    let token = bearer(headers);
    cloud.bearers.push(token.clone());
    id == LINK_ID && cloud.hash.as_deref() == Some(hex::encode(Sha256::digest(token.as_bytes())).as_str())
}

fn unpaired() -> (StatusCode, Json<Value>) {
    (StatusCode::UNAUTHORIZED, Json(json!({"detail": "This link is not paired.", "code": "unpaired"})))
}

async fn start(State(cloud): State<Shared>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut c = cloud.lock().unwrap();
    c.starts.push(body.clone());
    if let Some((status, answer)) = c.start_answer.clone() {
        return (StatusCode::from_u16(status).unwrap(), Json(answer));
    }
    c.hash = body["secret_hash"].as_str().map(str::to_string);
    (StatusCode::OK, Json(json!({"id": LINK_ID, "code": "ABCDEFGH", "expires_in": 600})))
}

async fn poll(State(cloud): State<Shared>, UrlPath(id): UrlPath<String>, headers: HeaderMap, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let (hold, gate) = {
        let mut c = cloud.lock().unwrap();
        if !known(&mut c, &id, &headers) {
            return unpaired();
        }
        c.polls.push((Instant::now(), body["status"].clone()));
        let hold = if c.mode == Mode::Paired && !c.calls.is_empty() { Duration::ZERO } else { c.hold };
        (hold, c.poll_gate.clone())
    };
    tokio::time::sleep(hold).await;
    if let Some(gate) = gate {
        gate.acquire().await.unwrap().forget();
    }
    let mut c = cloud.lock().unwrap();
    match c.mode.clone() {
        Mode::Pending => match c.expires_in {
            Some(left) => (StatusCode::OK, Json(json!({"state": "pending", "expires_in": left}))),
            None => (StatusCode::OK, Json(json!({"state": "pending"}))),
        },
        Mode::Refused(message) => (StatusCode::OK, Json(json!({"state": "refused", "reason": "name_taken", "message": message}))),
        Mode::Expired => (StatusCode::GONE, Json(json!({"detail": "The code ran out.", "code": "expired"}))),
        Mode::Unpaired => unpaired(),
        Mode::Down => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!("oops"))),
        Mode::Garbage => (StatusCode::OK, Json(json!({"state": "dancing"}))),
        Mode::Paired => {
            let calls: Vec<Value> = c.calls.drain(..).collect();
            let cancelled: Vec<String> = std::mem::take(&mut c.cancelled);
            let mut answer = json!({"state": "paired", "agent": {"name": "mac-helper", "label": "Mac helper"}, "calls": calls, "cancelled": cancelled});
            if let Some(programs) = &c.programs {
                answer["programs"] = json!(programs);
            }
            if let Some(skills) = &c.skills {
                answer["skills"] = json!(skills);
            }
            (StatusCode::OK, Json(answer))
        }
    }
}

async fn result(State(cloud): State<Shared>, UrlPath((id, call)): UrlPath<(String, String)>, headers: HeaderMap, Json(body): Json<Value>) -> StatusCode {
    let mut c = cloud.lock().unwrap();
    if !known(&mut c, &id, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    c.result_posts += 1;
    if c.result_status < 300 {
        c.results.insert(call, body);
    }
    StatusCode::from_u16(c.result_status).unwrap()
}

async fn skill_file(State(cloud): State<Shared>, UrlPath((id, file)): UrlPath<(String, String)>, headers: HeaderMap) -> (StatusCode, Vec<u8>) {
    let hold = cloud.lock().unwrap().file_hold;
    tokio::time::sleep(hold).await;
    let gate = {
        let mut c = cloud.lock().unwrap();
        if !known(&mut c, &id, &headers) {
            return (StatusCode::UNAUTHORIZED, vec![]);
        }
        c.file_gets.push(file.clone());
        c.file_gate.clone()
    };
    if let Some(gate) = gate {
        gate.acquire().await.unwrap().forget();
    }
    let c = cloud.lock().unwrap();
    match c.files.get(&file) {
        Some(bytes) => (StatusCode::OK, bytes.clone()),
        None => (StatusCode::NOT_FOUND, br#"{"detail": "There is no such file."}"#.to_vec()),
    }
}

async fn unpair(State(cloud): State<Shared>, UrlPath(id): UrlPath<String>, headers: HeaderMap) -> StatusCode {
    let mut c = cloud.lock().unwrap();
    if !known(&mut c, &id, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    c.deletes += 1;
    if c.delete_status < 300 {
        c.hash = None;
    }
    StatusCode::from_u16(c.delete_status).unwrap()
}

/// A fake Wysebee Cloud, listening on a free port of 127.0.0.1 for as long as the test's
/// runtime lives.
pub async fn fake() -> Fake {
    let cloud: Shared = Arc::new(Mutex::new(Cloud {
        mode: Mode::Pending,
        start_answer: None,
        starts: vec![],
        hash: None,
        programs: Some(vec![]),
        skills: Some(vec![]),
        files: HashMap::new(),
        file_gets: vec![],
        calls: VecDeque::new(),
        cancelled: vec![],
        polls: vec![],
        bearers: vec![],
        results: HashMap::new(),
        result_status: 204,
        result_posts: 0,
        delete_status: 204,
        deletes: 0,
        hold: Duration::ZERO,
        file_hold: Duration::ZERO,
        expires_in: Some(432),
        poll_gate: None,
        file_gate: None,
    }));
    let app = Router::new()
        .route("/api/v1/agent-links/start", post(start))
        .route("/api/v1/agent-links/{id}/poll", post(poll))
        .route("/api/v1/agent-links/{id}/calls/{call}/result", post(result))
        .route("/api/v1/agent-links/{id}/skill-files/{file}", get(skill_file))
        .route("/api/v1/agent-links/{id}", delete(unpair))
        .with_state(cloud.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Fake { url, cloud }
}

impl Fake {
    pub fn set(&self, change: impl FnOnce(&mut Cloud)) {
        change(&mut self.cloud.lock().unwrap());
    }

    pub fn get<T>(&self, read: impl FnOnce(&Cloud) -> T) -> T {
        read(&self.cloud.lock().unwrap())
    }

    /// A file the cloud serves, by its real id; the entry a skill's list has for it.
    pub fn file(&self, path: &str, bytes: &[u8]) -> Value {
        let id = hex::encode(Sha256::digest(bytes));
        self.set(|c| {
            c.files.insert(id.clone(), bytes.to_vec());
        });
        json!({"path": path, "id": id, "size": bytes.len()})
    }

    /// Queue a call for the next paired poll.
    pub fn call(&self, id: &str, op: &str, args: Value) {
        self.set(|c| c.calls.push_back(json!({"id": id, "op": op, "args": args})));
    }

    /// The last status a poll carried.
    pub fn last_status(&self) -> Value {
        self.get(|c| c.polls.last().map(|(_, s)| s.clone()).unwrap_or(Value::Null))
    }

    /// The answer posted for a call, waited for.
    pub async fn answer(&self, call: &str) -> Value {
        for _ in 0..1000 {
            if let Some(v) = self.get(|c| c.results.get(call).cloned()) {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no answer was posted for call {call}");
    }
}

/// An agent's surroundings in a temporary folder: its state folder, the person's home
/// and a folder of programs that is its whole PATH.
pub struct Bench {
    pub dir: tempfile::TempDir,
    pub secrets: Arc<MemorySecrets>,
    pub screen: Arc<FakeScreen>,
}

/// A screen of 1600 × 900, white, the pointer at its middle: what was done to it is kept, and
/// it can be made to fail. Never the real one.
pub struct FakeScreen {
    pub actions: Mutex<Vec<agent_core::screen::Action>>,
    pub failing: Mutex<Option<String>>,
}

impl agent_core::screen::Screen for FakeScreen {
    fn capture(&self) -> Result<agent_core::screen::Frame, String> {
        self.usable()?;
        Ok(agent_core::screen::Frame { width: 1600, height: 900, rgba: vec![255; 1600 * 900 * 4], cursor: Some((800, 450)) })
    }
    fn size(&self) -> Result<(u32, u32), String> {
        self.usable()?;
        Ok((1600, 900))
    }
    fn act(&self, action: &agent_core::screen::Action) -> Result<(), String> {
        self.usable()?;
        self.actions.lock().unwrap().push(action.clone());
        Ok(())
    }
}

impl FakeScreen {
    fn usable(&self) -> Result<(), String> {
        self.failing.lock().unwrap().clone().map_or(Ok(()), Err)
    }
}

impl Bench {
    pub fn new() -> Bench {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["state", "home", "bin"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        Bench { dir, secrets: Arc::new(MemorySecrets::new()), screen: Arc::new(FakeScreen { actions: Mutex::new(vec![]), failing: Mutex::new(None) }) }
    }

    pub fn state_dir(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    pub fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    pub fn state_text(&self) -> String {
        std::fs::read_to_string(self.state_dir().join("state.json")).unwrap_or_default()
    }

    /// Short times, so a test takes milliseconds.
    pub fn timing() -> Timing {
        Timing {
            wait_secs: 0,
            backoff: [Duration::from_millis(10), Duration::from_millis(20), Duration::from_millis(30), Duration::from_millis(40)],
            time_limit: Duration::from_secs(60),
            min_gap: Duration::from_millis(10),
            quick: Duration::from_secs(5),
            grace: Duration::from_secs(10),
            download: Duration::from_secs(5),
            settle: Duration::from_millis(1),
        }
    }

    pub fn options(&self, cloud_url: Option<&str>, timing: Timing) -> Options {
        Options {
            state_dir: self.state_dir(),
            home: self.home(),
            path: Some(self.bin().into_os_string()),
            cloud_url: cloud_url.map(str::to_string),
            secrets: self.secrets.clone(),
            mac_name: "Test MacBook Pro with a very long name indeed.local".into(),
            version: "0.1.0".into(),
            timing,
            screen: Some(self.screen.clone()),
        }
    }

    pub fn agent(&self, fake: &Fake) -> Agent {
        Agent::open(self.options(Some(&fake.url), Bench::timing()))
    }

    /// An executable `sh` script in the programs folder; its path.
    pub fn script(&self, name: &str, body: &str) -> String {
        script(&self.bin(), name, body)
    }
}

pub fn script(dir: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.display().to_string()
}

/// A program as the agent's settings hold it.
pub fn program(name: &str, command: &str, args: Value) -> Value {
    json!({"name": name, "description": format!("What {name} does"), "command": command, "args": args})
}

/// Pair `agent` with the fake, as the agent "Mac helper".
pub async fn pair(agent: &Agent, fake: &Fake) {
    agent.start_pairing("Mac helper").await.unwrap();
    fake.set(|c| c.mode = Mode::Paired);
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(agent).await;
}

/// The skills' files a poll listed, downloaded beside the loop: wait until that is done.
pub async fn stored(agent: &Agent) {
    until("the skills' files are stored", || !agent.status().storing).await;
}

/// One call through the link: queued, handed out by a poll, run, and its answer.
pub async fn call(agent: &Agent, fake: &Fake, id: &str, op: &str, args: Value) -> Value {
    fake.call(id, op, args);
    assert_eq!(agent.step().await, Step::Paired { calls: 1 });
    stored(agent).await;
    fake.answer(id).await
}

/// Whether a process is gone (ended and reaped, or ended and waiting to be).
pub fn gone(pid: &str) -> bool {
    let out = std::process::Command::new("/bin/ps").args(["-o", "stat=", "-p", pid]).output().unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
    stat.is_empty() || stat.starts_with('Z')
}

/// Wait until `check` holds, for a few seconds at most.
pub async fn until(what: &str, check: impl Fn() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never happened: {what}");
}

/// A `SKILL.md` declaring `scripts` (`(name, command)`).
pub fn skill_md(name: &str, scripts: &[(&str, &str)]) -> String {
    let mut out = format!("---\nname: {name}\ndescription: What {name} is for.\n");
    if !scripts.is_empty() {
        out.push_str("scripts:\n");
        for (s, command) in scripts {
            out.push_str(&format!("  - name: {s}\n    command: {}\n    description: Runs {s}.\n", serde_json::to_string(command).unwrap()));
        }
    }
    out.push_str("---\n\nHow to use it.\n");
    out
}

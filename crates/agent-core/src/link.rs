//! The protocol's HTTP calls (the agent-link protocol): starting a pairing, the app's poll,
//! answering a call, downloading a skill's file and unpairing. What they mean for the app is [`crate::agent`]'s.
//!
//! All under `/api/v1/agent-links` on the cloud the link was made with. The app's own
//! routes carry its secret as `Authorization: Bearer`; redirects are not followed, so the
//! secret goes nowhere else.

use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine;
use rand::RngCore;
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const ROUTES: &str = "/api/v1/agent-links";
/// The agent type this app is: its key in Wysebee Cloud's `agent_types` (the agent-link protocol).
pub const KIND: &str = "wysebee-osx";
/// The Mac's name is sent in at most this many characters.
pub const MAX_MAC_NAME: usize = 32;
/// The agent name the person types: one line of 1 to this many characters.
pub const MAX_AGENT_NAME: usize = 60;
/// The poll reports at most this many allowed programs.
pub const MAX_APPROVED: usize = 20;
/// The poll reports at most this many allowed skills.
pub const MAX_APPROVED_SKILLS: usize = 50;

/// An id the cloud gave (a link's, a call's), which goes into a path: nothing but these.
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,64}$").unwrap());

pub fn valid_id(id: &str) -> bool {
    ID.is_match(id)
}

/// A new link secret: 32 random bytes, base64url without padding.
pub fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// What the cloud keeps of a secret: its SHA-256, in hex.
pub fn secret_hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// The Mac's name as sent: one line, at most [`MAX_MAC_NAME`] characters; "Mac" when
/// there is none.
pub fn mac_name(raw: &str) -> String {
    let line: String = raw.trim().trim_end_matches(".local").chars().filter(|c| !c.is_control()).take(MAX_MAC_NAME).collect();
    let line = line.trim();
    if line.is_empty() {
        "Mac".into()
    } else {
        line.into()
    }
}

/// The agent name the person typed, tidied; a sentence when it cannot be one.
pub fn agent_name(typed: &str) -> Result<String, String> {
    let name = typed.trim();
    let count = name.chars().count();
    if count == 0 || count > MAX_AGENT_NAME || name.chars().any(|c| c.is_control()) {
        return Err(format!("Type a name for the agent: one line, up to {MAX_AGENT_NAME} characters."));
    }
    Ok(name.to_string())
}

/// A pairing code as shown: `XXXX-XXXX`.
pub fn shown_code(code: &str) -> String {
    let code = code.trim().to_ascii_uppercase();
    match code.char_indices().nth(4) {
        Some((at, _)) if !code.contains('-') => format!("{}-{}", &code[..at], &code[at..]),
        _ => code,
    }
}

/// A pairing that started.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    pub id: String,
    pub code: String,
    pub expires_in: u64,
}

/// A call handed to this app.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub id: String,
    pub op: String,
    pub args: Value,
}

/// What a poll answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Poll {
    /// Nobody typed the code yet.
    Pending { expires_in: Option<u64> },
    /// The claim was refused; start again with another name.
    Refused { message: String },
    /// Paired. `programs` and `skills` are none when the answer carried no list.
    Paired { name: String, label: String, programs: Option<Vec<Value>>, skills: Option<Vec<Value>>, calls: Vec<Call>, cancelled: Vec<String> },
    /// 401: unpaired in Wysebee, or the agent was deleted.
    Unpaired,
    /// 410: the code ran out before anyone typed it.
    Expired,
}

/// A request that did not get its answer: the cloud's sentence when it gave one.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The cloud answered and refused, in its words.
    Refused(String),
    /// No usable answer: offline, the cloud down, or an answer that could not be read.
    Unreachable(String),
}

impl Failure {
    pub fn message(&self) -> &str {
        match self {
            Failure::Refused(m) | Failure::Unreachable(m) => m,
        }
    }
}

fn unreachable(e: reqwest::Error) -> Failure {
    // Without the address: an error's text names the URL, which holds nothing secret, but
    // the sentence is for the person.
    Failure::Unreachable(if e.is_timeout() { "Wysebee Cloud did not answer in time.".into() } else { "Wysebee Cloud could not be reached.".into() })
}

/// The cloud's sentence in an error answer (`{"detail": "..."}`), else one naming the
/// status.
async fn refusal(response: reqwest::Response) -> Failure {
    let status = response.status().as_u16();
    let detail = response.json::<Value>().await.ok().and_then(|v| v.get("detail").and_then(Value::as_str).map(str::to_string));
    match detail {
        Some(d) if !d.trim().is_empty() => Failure::Refused(d),
        _ => Failure::Unreachable(format!("Wysebee Cloud answered {status}.")),
    }
}

pub struct Client {
    http: reqwest::Client,
}

impl Default for Client {
    fn default() -> Self {
        Client::new()
    }
}

impl Client {
    pub fn new() -> Client {
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(10)).build().expect("the HTTP client's settings are fixed");
        Client { http }
    }

    /// Step 1 of pairing: `POST /start`, no auth.
    pub async fn start(&self, base: &str, name: &str, secret: &str, mac: &str, version: &str, timeout: Duration) -> Result<Started, Failure> {
        let body = json!({"kind": KIND, "name": name, "secret_hash": secret_hash(secret), "browser": mac_name(mac), "extension_version": version});
        let response = self.http.post(format!("{base}{ROUTES}/start")).json(&body).timeout(timeout).send().await.map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(refusal(response).await);
        }
        let v: Value = response.json().await.map_err(unreachable)?;
        let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let (id, code) = (text("id"), text("code"));
        if !valid_id(&id) || code.is_empty() || code.chars().count() > 16 || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(Failure::Unreachable("Wysebee Cloud's answer could not be read.".into()));
        }
        Ok(Started { id, code, expires_in: v.get("expires_in").and_then(Value::as_u64).unwrap_or(600) })
    }

    /// The app's poll: `POST /{id}/poll?wait=<wait>` with its status.
    pub async fn poll(&self, base: &str, id: &str, secret: &str, status: &Value, wait: u64, timeout: Duration) -> Result<Poll, Failure> {
        let response = self.http.post(format!("{base}{ROUTES}/{id}/poll?wait={wait}")).bearer_auth(secret).json(&json!({"status": status})).timeout(timeout).send().await.map_err(unreachable)?;
        match response.status().as_u16() {
            401 => return Ok(Poll::Unpaired),
            410 => return Ok(Poll::Expired),
            200 => {}
            _ => return Err(refusal(response).await),
        }
        let v: Value = response.json().await.map_err(unreachable)?;
        let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        match v.get("state").and_then(Value::as_str) {
            Some("pending") => Ok(Poll::Pending { expires_in: v.get("expires_in").and_then(Value::as_u64) }),
            Some("refused") => {
                let message = text(&v, "message");
                Ok(Poll::Refused { message: if message.trim().is_empty() { "Wysebee refused that name. Start again with another.".into() } else { message } })
            }
            Some("paired") => {
                let agent = v.get("agent").cloned().unwrap_or(Value::Null);
                let calls = v.get("calls").and_then(Value::as_array).cloned().unwrap_or_default();
                let calls = calls.iter().map(|c| Call { id: text(c, "id"), op: text(c, "op"), args: c.get("args").cloned().unwrap_or(Value::Null) }).filter(|c| valid_id(&c.id)).collect();
                let cancelled = v.get("cancelled").and_then(Value::as_array).cloned().unwrap_or_default();
                Ok(Poll::Paired {
                    name: text(&agent, "name"),
                    label: text(&agent, "label"),
                    programs: v.get("programs").and_then(Value::as_array).cloned(),
                    skills: v.get("skills").and_then(Value::as_array).cloned(),
                    calls,
                    cancelled: cancelled.iter().filter_map(|c| c.as_str().map(str::to_string)).collect(),
                })
            }
            _ => Err(Failure::Unreachable("Wysebee Cloud's answer could not be read.".into())),
        }
    }

    /// A call's answer: `POST /{id}/calls/{call_id}/result`. A call that is gone (404:
    /// cancelled, expired) is dropped, which is fine.
    pub async fn result(&self, base: &str, id: &str, secret: &str, call_id: &str, body: &Value, timeout: Duration) -> Result<(), Failure> {
        let response = self.http.post(format!("{base}{ROUTES}/{id}/calls/{call_id}/result")).bearer_auth(secret).json(body).timeout(timeout).send().await.map_err(unreachable)?;
        match response.status().as_u16() {
            200..=299 | 404 => Ok(()),
            _ => Err(refusal(response).await),
        }
    }

    /// A skill's file: `GET /{id}/skill-files/{file_id}`, at most `max` bytes. Whether the
    /// bytes are the file is the caller's to check (their SHA-256 is its id).
    pub async fn skill_file(&self, base: &str, id: &str, secret: &str, file_id: &str, max: u64, timeout: Duration) -> Result<Vec<u8>, Failure> {
        let mut response = self.http.get(format!("{base}{ROUTES}/{id}/skill-files/{file_id}")).bearer_auth(secret).timeout(timeout).send().await.map_err(unreachable)?;
        if !response.status().is_success() {
            return Err(refusal(response).await);
        }
        let mut out = vec![];
        while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
            out.extend_from_slice(&chunk);
            if out.len() as u64 > max {
                return Err(Failure::Refused("The file is larger than a skill's file may be.".into()));
            }
        }
        Ok(out)
    }

    /// Unpair from the app: `DELETE /{id}`. The agent stays, without this Mac.
    pub async fn unpair(&self, base: &str, id: &str, secret: &str, timeout: Duration) -> Result<(), Failure> {
        let response = self.http.delete(format!("{base}{ROUTES}/{id}")).bearer_auth(secret).timeout(timeout).send().await.map_err(unreachable)?;
        match response.status().as_u16() {
            // Already gone there is unpaired too.
            200..=299 | 401 | 404 => Ok(()),
            _ => Err(refusal(response).await),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_32_random_bytes_and_the_cloud_gets_its_hash() {
        let (a, b) = (new_secret(), new_secret());
        assert_ne!(a, b);
        assert_eq!(a.len(), 43, "32 bytes, base64url, no padding");
        assert_eq!(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&a).unwrap().len(), 32);
        assert_eq!(secret_hash("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn names_codes_and_ids_are_tidied() {
        assert_eq!(mac_name(" Alex's MacBook Pro.local\n"), "Alex's MacBook Pro");
        assert_eq!(mac_name(&"m".repeat(40)).chars().count(), MAX_MAC_NAME);
        assert_eq!(mac_name("  "), "Mac");
        assert_eq!(agent_name("  Mac helper ").unwrap(), "Mac helper");
        assert!(agent_name("").is_err() && agent_name("a\nb").is_err() && agent_name(&"n".repeat(61)).is_err());
        assert_eq!(agent_name(&"é".repeat(60)).unwrap().chars().count(), 60);
        assert_eq!(shown_code("abcdefgh"), "ABCD-EFGH");
        assert_eq!(shown_code("ABCD-EFGH"), "ABCD-EFGH");
        assert_eq!(shown_code("abc"), "ABC");
        assert!(valid_id("0123abcdEF_-") && !valid_id("") && !valid_id("a/b") && !valid_id("..") && !valid_id(&"a".repeat(65)));
        assert_eq!(Failure::Refused("no".into()).message(), "no");
        assert_eq!(Failure::Unreachable("down".into()).message(), "down");
    }
}

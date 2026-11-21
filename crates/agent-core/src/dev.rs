//! Developer access:
//! **work on this Mac as Claude Code would here** — a shell (`dev_shell`: zsh, bash or sh,
//! anywhere, as the person), background jobs for long builds (`dev_job`), and files read,
//! written, edited and searched anywhere (`dev_file`). Only while the person has turned on
//! "Let Wysebee use this Mac as a developer"
//! (`agent.rs`); nothing else limits it: **a shell can do anything the person can**, so Run
//! any program's list of refused programs (guard.rs) does not apply here.
//!
//! The shell is found as programs are (`zsh`, `bash` or `sh` on the `PATH`), started as a
//! login shell (so the person's `.zprofile` — Homebrew's PATH — is read), with no input, in
//! the folder the call names (the person's home if none), for
//! at most [`MAX_SECS`] (the cloud waits 240 s for any call); output cut at [`MAX_OUT`] each.
//! A command run in the background is a [`Jobs`] entry, its output kept as it comes.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use regex::RegexBuilder;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

/// A foreground command's time, at most (the cloud gives any call 240 s).
pub const MAX_SECS: u64 = 220;
pub const DEFAULT_SECS: u64 = 120;
/// Output and errors, each, cut at this many characters.
pub const MAX_OUT: usize = 30_000;
/// A command's text, at most.
pub const MAX_COMMAND: usize = 20_000;
/// Background jobs running at once, and kept in all.
pub const MAX_RUNNING: usize = 8;
pub const MAX_KEPT: usize = 30;
/// A job's output kept: its newest characters.
pub const MAX_JOB_OUT: usize = 200_000;
/// Files: read at most this many lines at once, each cut at this many characters.
pub const MAX_LINES: usize = 2_000;
pub const MAX_LINE: usize = 2_000;
/// A file read or written whole, at most.
pub const MAX_FILE: u64 = 20 * 1024 * 1024;
/// A search: entries looked at, and matches answered, at most.
pub const MAX_SCAN: usize = 200_000;
pub const MAX_HITS: usize = 500;
/// Folders a search never goes into.
pub const SKIPPED: [&str; 9] = [".git", "node_modules", ".gradle", "build", "target", ".idea", "__pycache__", ".venv", "dist"];

/// A text cut at `most` characters, its newest kept when `tail`.
pub fn cut(text: &str, most: usize, tail: bool) -> String {
    let n = text.chars().count();
    if n <= most {
        return text.to_string();
    }
    if tail {
        let kept: String = text.chars().skip(n - most).collect();
        format!("…[{} characters before cut]\n{kept}", n - most)
    } else {
        format!("{}\n…[cut at {most} characters]", text.chars().take(most).collect::<String>())
    }
}

/// A path a call gives: full, or relative to the person's home.
pub fn path_of(home: &Path, given: &str) -> Result<PathBuf, String> {
    let given = given.trim();
    if given.is_empty() {
        return Err("Give the 'path'.".into());
    }
    let p = Path::new(given);
    Ok(if p.is_absolute() { p.to_path_buf() } else { home.join(p) })
}

fn text<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

// ---- The shell ----

/// The shell a call names, and the words before its command: `zsh` (the default, macOS'
/// own), `bash` or `sh`; zsh and bash as login shells.
pub fn shell_of(name: &str) -> Result<(&'static str, Vec<&'static str>), String> {
    match name {
        "" | "zsh" => Ok(("zsh", vec!["-l", "-c"])),
        "bash" => Ok(("bash", vec!["-l", "-c"])),
        "sh" => Ok(("sh", vec!["-c"])),
        other => Err(format!("'{other}' is not a shell here: zsh, bash or sh.")),
    }
}

/// What a `dev_shell` call asks, checked: the command, the shell, the folder, the time.
#[derive(Debug)]
pub struct Ask {
    pub command: String,
    pub shell: String,
    pub cwd: PathBuf,
    pub limit: Duration,
    pub background: bool,
}

pub fn ask(args: &Value, home: &Path) -> Result<Ask, String> {
    let command = text(args, "command");
    if command.trim().is_empty() || command.chars().count() > MAX_COMMAND {
        return Err(format!("Give the 'command' to run: 1 to {MAX_COMMAND} characters."));
    }
    let cwd = match text(args, "cwd").trim() {
        "" => home.to_path_buf(),
        given => path_of(home, given)?,
    };
    if !cwd.is_dir() {
        return Err(format!("There is no folder {}.", cwd.display()));
    }
    let secs = match args.get("timeout_secs") {
        None | Some(Value::Null) => DEFAULT_SECS,
        Some(v) => v.as_u64().filter(|s| (1..=MAX_SECS).contains(s)).ok_or(format!("'timeout_secs' is 1 to {MAX_SECS}: run a longer command in the background."))?,
    };
    let shell = text(args, "shell").trim().to_lowercase();
    shell_of(&shell)?;
    Ok(Ask { command: command.to_string(), shell, cwd, limit: Duration::from_secs(secs), background: args.get("background") == Some(&Value::Bool(true)) })
}

/// The shell started for `ask` (`exe` found on the PATH), its output piped.
pub fn command(exe: &Path, ask: &Ask) -> tokio::process::Command {
    let (_, words) = shell_of(&ask.shell).unwrap_or(("zsh", vec![]));
    let mut c = tokio::process::Command::new(exe);
    c.args(&words).arg(&ask.command).current_dir(&ask.cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    c
}

/// A foreground run's answer: `{exit_code, stdout, stderr}`, each cut.
pub fn answer(out: &std::process::Output) -> Value {
    json!({
        "exit_code": out.status.code(),
        "stdout": cut(&String::from_utf8_lossy(&out.stdout), MAX_OUT, false),
        "stderr": cut(&String::from_utf8_lossy(&out.stderr), MAX_OUT, false),
    })
}

// ---- Background jobs ----

/// One command run in the background: its output as it comes (stdout and stderr together),
/// and how it ended.
pub struct Job {
    pub command: String,
    pub started: Instant,
    output: Mutex<String>,
    /// None while it runs; then its exit code (None: killed, or stopped).
    ended: Mutex<Option<Option<i32>>>,
    stop: tokio::sync::Notify,
}

impl Job {
    fn new(command: &str) -> Job {
        Job { command: command.to_string(), started: Instant::now(), output: Mutex::new(String::new()), ended: Mutex::new(None), stop: tokio::sync::Notify::new() }
    }

    /// More output: past twice `most` characters, only the newest `most` are kept.
    fn add(&self, more: &str, most: usize) {
        let mut out = self.output.lock().unwrap();
        out.push_str(more);
        let n = out.chars().count();
        if n > most * 2 {
            *out = out.chars().skip(n - most).collect();
        }
    }

    pub fn running(&self) -> bool {
        self.ended.lock().unwrap().is_none()
    }

    /// `{job, command, status, exit_code, seconds, output}`: its newest `tail` characters.
    pub fn report(&self, id: &str, tail: usize) -> Value {
        let ended = *self.ended.lock().unwrap();
        let status = match ended {
            None => "running",
            Some(Some(_)) => "exited",
            Some(None) => "stopped",
        };
        json!({
            "job": id,
            "command": cut(&self.command, 200, false),
            "status": status,
            "exit_code": ended.flatten(),
            "seconds": self.started.elapsed().as_secs(),
            "output": cut(&self.output.lock().unwrap(), tail, true),
        })
    }
}

/// The background jobs, by id.
#[derive(Default)]
pub struct Jobs {
    all: Mutex<Vec<(String, Arc<Job>)>>,
}

impl Jobs {
    /// Start `child`'s job: its output read as it comes, its end kept. Refused when
    /// [`MAX_RUNNING`] run already; the oldest ended ones go past [`MAX_KEPT`].
    pub fn start(&self, command: &str, mut child: tokio::process::Child) -> Result<String, String> {
        let mut all = self.all.lock().unwrap();
        if all.iter().filter(|(_, j)| j.running()).count() >= MAX_RUNNING {
            return Err(format!("{MAX_RUNNING} jobs are running already: stop one (dev_job) first."));
        }
        evict(&mut all, MAX_KEPT);
        let id = format!("job-{}", crate::folders::new_id().get(..8).unwrap_or("0"));
        let job = Arc::new(Job::new(command));
        for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)].into_iter().flatten() {
            let job = job.clone();
            tokio::spawn(async move {
                let mut pipe = pipe;
                let mut buf = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    job.add(&String::from_utf8_lossy(&buf[..n]), MAX_JOB_OUT);
                }
            });
        }
        let ending = job.clone();
        tokio::spawn(async move {
            let ended = tokio::select! {
                status = child.wait() => status.ok().and_then(|s| s.code()),
                _ = ending.stop.notified() => {
                    let _ = child.kill().await;
                    None
                }
            };
            *ending.ended.lock().unwrap() = Some(ended);
        });
        all.push((id.clone(), job));
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Job>> {
        self.all.lock().unwrap().iter().find(|(i, _)| i == id).map(|(_, j)| j.clone())
    }

    /// Every job, newest last: `{job, command, status, exit_code, seconds}`.
    pub fn list(&self) -> Value {
        Value::Array(self.all.lock().unwrap().iter().map(|(id, j)| {
            let mut r = j.report(id, 0);
            r.as_object_mut().unwrap().remove("output");
            r
        }).collect())
    }

    /// Stop one: true when it was running.
    pub fn stop(&self, id: &str) -> bool {
        match self.get(id) {
            Some(job) if job.running() => {
                job.stop.notify_one();
                true
            }
            _ => false,
        }
    }

    /// Stop every running job (paused, turned off, unpaired); the count.
    pub fn stop_all(&self) -> usize {
        let ids: Vec<String> = self.all.lock().unwrap().iter().filter(|(_, j)| j.running()).map(|(i, _)| i.clone()).collect();
        ids.iter().filter(|id| self.stop(id)).count()
    }
}

/// The oldest ended jobs dropped until fewer than `most` are kept (running ones stay).
fn evict(all: &mut Vec<(String, Arc<Job>)>, most: usize) {
    while all.len() >= most {
        let Some(i) = all.iter().position(|(_, j)| !j.running()) else { break };
        all.remove(i);
    }
}

// ---- Files ----

/// One `dev_file` call: `read`, `write`, `edit`, `glob`, `grep` or `list` (blocking: off the
/// async threads). Answers `{"text"}`.
pub fn file(home: &Path, args: &Value) -> Result<Value, String> {
    let tool = text(args, "tool");
    let text = match tool {
        "read" => read(home, args)?,
        "write" => write(home, args)?,
        "edit" => edit(home, args)?,
        "glob" => glob(home, args)?,
        "grep" => grep(home, args)?,
        "list" => list(home, args)?,
        other => return Err(format!("'{other}' is not a file tool: read, write, edit, glob, grep or list.")),
    };
    Ok(json!({"text": text}))
}

fn io(path: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

/// A file's text: refused when it is larger than [`MAX_FILE`] or not text.
fn text_file(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| io(path, e))?;
    if meta.is_dir() {
        return Err(format!("{} is a folder: list it.", path.display()));
    }
    if meta.len() > MAX_FILE {
        return Err(format!("{} is larger than {} MB.", path.display(), MAX_FILE / 1024 / 1024));
    }
    let bytes = std::fs::read(path).map_err(|e| io(path, e))?;
    String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text.", path.display()))
}

/// Lines `offset` (from 1) on, at most `limit`, each numbered as `cat -n` does.
fn read(home: &Path, args: &Value) -> Result<String, String> {
    let path = path_of(home, text(args, "path"))?;
    let all = text_file(&path)?;
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
    let limit = (args.get("limit").and_then(Value::as_u64).unwrap_or(MAX_LINES as u64) as usize).clamp(1, MAX_LINES);
    let lines: Vec<&str> = all.lines().collect();
    if lines.is_empty() {
        return Ok(format!("{} is empty.", path.display()));
    }
    if offset > lines.len() {
        return Err(format!("{} has {} lines.", path.display(), lines.len()));
    }
    let shown: Vec<String> = lines.iter().enumerate().skip(offset - 1).take(limit).map(|(i, l)| format!("{:>6}\t{}", i + 1, cut(l, MAX_LINE, false))).collect();
    let more = lines.len().saturating_sub(offset - 1 + shown.len());
    let mut out = shown.join("\n");
    if more > 0 {
        out.push_str(&format!("\n…[{more} more lines: read on with offset {}]", offset + shown.len()));
    }
    Ok(out)
}

/// The whole file written (its folders made), replacing what was there.
fn write(home: &Path, args: &Value) -> Result<String, String> {
    let path = path_of(home, text(args, "path"))?;
    let content = args.get("content").and_then(Value::as_str).ok_or("Give the file's whole 'content'.")?;
    if content.len() as u64 > MAX_FILE {
        return Err(format!("At most {} MB.", MAX_FILE / 1024 / 1024));
    }
    let dir = path.parent().unwrap_or(&path);
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    std::fs::write(&path, content).map_err(|e| io(&path, e))?;
    Ok(format!("Wrote {} ({} bytes).", path.display(), content.len()))
}

/// `old` replaced by `new`: it must be in the file once, or `replace_all`.
fn edit(home: &Path, args: &Value) -> Result<String, String> {
    let path = path_of(home, text(args, "path"))?;
    let (old, new) = (text(args, "old"), text(args, "new"));
    if old.is_empty() {
        return Err("Give 'old', the exact text to replace.".into());
    }
    if old == new {
        return Err("'old' and 'new' are the same.".into());
    }
    let all = text_file(&path)?;
    let found = all.matches(old).count();
    let every = args.get("replace_all") == Some(&Value::Bool(true));
    match found {
        0 => return Err(format!("'old' is not in {}: read it again, and give the text exactly (spaces too).", path.display())),
        1 => {}
        n if !every => return Err(format!("'old' is in {} {n} times: give more of the text around it, or replace_all.", path.display())),
        _ => {}
    }
    std::fs::write(&path, all.replace(old, new)).map_err(|e| io(&path, e))?;
    Ok(format!("Replaced {found} time{} in {}.", if found == 1 { "" } else { "s" }, path.display()))
}

/// A glob pattern (`*`, `?`, `**` across folders, `{a,b}`) as a regex over `/` paths.
pub fn glob_regex(pattern: &str) -> Result<regex::Regex, String> {
    let mut re = String::from("^");
    let chars: Vec<char> = pattern.replace('\\', "/").chars().collect();
    let mut i = 0;
    let mut braces = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                i += 1;
                if chars.get(i + 1) == Some(&'/') {
                    i += 1;
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            '{' => {
                braces += 1;
                re.push_str("(?:");
            }
            '}' if braces > 0 => {
                braces -= 1;
                re.push(')');
            }
            ',' if braces > 0 => re.push('|'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push('$');
    RegexBuilder::new(&re).case_insensitive(true).build().map_err(|_| format!("'{pattern}' is not a pattern."))
}

/// Every file under `root` (skipping [`SKIPPED`]), as `/` paths relative to it, at most
/// [`MAX_SCAN`] entries looked at; true when it stopped there.
fn walk(root: &Path, most: usize) -> (Vec<(String, PathBuf)>, bool) {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    let mut scanned = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            scanned += 1;
            if scanned > most {
                return (out, true);
            }
            let path = entry.path();
            let kind = entry.file_type().ok();
            if kind.is_some_and(|k| k.is_dir()) {
                if !SKIPPED.contains(&entry.file_name().to_string_lossy().as_ref()) {
                    stack.push(path);
                }
            } else if kind.is_some_and(|k| k.is_file()) {
                let rel = path.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                out.push((rel, path));
            }
        }
    }
    (out, false)
}

fn root_of(home: &Path, args: &Value) -> Result<PathBuf, String> {
    let root = match text(args, "path").trim() {
        "" => home.to_path_buf(),
        given => path_of(home, given)?,
    };
    if !root.is_dir() {
        return Err(format!("There is no folder {}.", root.display()));
    }
    Ok(root)
}

/// Files under `path` whose relative path matches `pattern`, newest first.
fn glob(home: &Path, args: &Value) -> Result<String, String> {
    glob_within(home, args, MAX_SCAN)
}

fn glob_within(home: &Path, args: &Value, most: usize) -> Result<String, String> {
    let root = root_of(home, args)?;
    let pattern = text(args, "pattern").trim();
    if pattern.is_empty() {
        return Err("Give a 'pattern', like **/*.kt or src/**/Main*.java.".into());
    }
    let re = glob_regex(pattern)?;
    let (all, stopped) = walk(&root, most);
    let mut found: Vec<(std::time::SystemTime, String)> = all.into_iter().filter(|(rel, _)| re.is_match(rel)).map(|(rel, p)| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), rel)).collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let total = found.len();
    let mut out: Vec<String> = found.into_iter().take(MAX_HITS).map(|(_, rel)| rel).collect();
    if out.is_empty() {
        return Ok(format!("No files under {} match {pattern}.", root.display()));
    }
    out.insert(0, format!("Under {}:", root.display()));
    if total > MAX_HITS {
        out.push(format!("…[{} more]", total - MAX_HITS));
    }
    if stopped {
        out.push(format!("…[stopped after {most} entries: give a narrower path]"));
    }
    Ok(out.join("\n"))
}

/// Lines matching a regex in the files under `path` (only those matching `glob`):
/// `file:line:text`.
fn grep(home: &Path, args: &Value) -> Result<String, String> {
    grep_within(home, args, MAX_SCAN)
}

fn grep_within(home: &Path, args: &Value, most: usize) -> Result<String, String> {
    let root = root_of(home, args)?;
    let pattern = text(args, "pattern");
    if pattern.is_empty() {
        return Err("Give a 'pattern', a regular expression.".into());
    }
    let re = RegexBuilder::new(pattern).case_insensitive(args.get("ignore_case") == Some(&Value::Bool(true))).size_limit(1 << 20).build().map_err(|e| format!("'pattern' is not a regular expression: {e}"))?;
    let only = match text(args, "glob").trim() {
        "" => None,
        g => Some(glob_regex(&if g.contains('/') { g.to_string() } else { format!("**/{g}") })?),
    };
    let (all, stopped) = walk(&root, most);
    let mut hits = vec![];
    let mut more = 0;
    for (rel, path) in all {
        if only.as_ref().is_some_and(|o| !o.is_match(&rel)) || std::fs::metadata(&path).map(|m| m.len() > MAX_FILE).unwrap_or(true) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        for (i, line) in content.lines().enumerate().filter(|(_, l)| re.is_match(l)) {
            if hits.len() < MAX_HITS {
                hits.push(format!("{rel}:{}:{}", i + 1, cut(line.trim_end(), 300, false)));
            } else {
                more += 1;
            }
        }
    }
    if hits.is_empty() {
        return Ok(format!("Nothing under {} matches.", root.display()));
    }
    hits.insert(0, format!("Under {}:", root.display()));
    if more > 0 {
        hits.push(format!("…[{more} more matches]"));
    }
    if stopped {
        hits.push(format!("…[stopped after {most} entries: give a narrower path]"));
    }
    Ok(hits.join("\n"))
}

/// A folder's entries: folders first, each with `/`, then files and their sizes.
fn list(home: &Path, args: &Value) -> Result<String, String> {
    let root = root_of(home, args)?;
    let mut entries: Vec<(bool, String, u64)> = std::fs::read_dir(&root)
        .map_err(|e| io(&root, e))?
        .flatten()
        .map(|e| {
            let meta = e.metadata().ok();
            (meta.as_ref().is_some_and(|m| m.is_dir()), e.file_name().to_string_lossy().to_string(), meta.map(|m| m.len()).unwrap_or(0))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let total = entries.len();
    let mut out: Vec<String> = entries.into_iter().take(MAX_HITS).map(|(dir, name, size)| if dir { format!("{name}/") } else { format!("{name}  ({size} bytes)") }).collect();
    out.insert(0, format!("{} ({total} entries):", root.display()));
    if total > MAX_HITS {
        out.push(format!("…[{} more]", total - MAX_HITS));
    }
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(home: &Path, args: Value) -> Result<String, String> {
        file(home, &args).map(|v| v["text"].as_str().unwrap().to_string())
    }

    #[test]
    fn a_shell_call_is_checked() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir(h.join("app")).unwrap();
        let a = ask(&json!({"command": "./gradlew build", "cwd": "app", "shell": "BASH", "timeout_secs": 200, "background": true}), h).unwrap();
        assert_eq!((a.shell.as_str(), a.cwd.clone(), a.limit, a.background), ("bash", h.join("app"), Duration::from_secs(200), true));
        let a = ask(&json!({"command": "ls"}), h).unwrap();
        assert_eq!((a.shell.as_str(), a.cwd.clone(), a.limit, a.background), ("", h.to_path_buf(), Duration::from_secs(DEFAULT_SECS), false));
        assert!(ask(&json!({"command": " "}), h).unwrap_err().starts_with("Give the 'command'"));
        assert!(ask(&json!({"command": "x".repeat(MAX_COMMAND + 1)}), h).unwrap_err().starts_with("Give the 'command'"));
        assert!(ask(&json!({"command": "x", "cwd": "nope"}), h).unwrap_err().starts_with("There is no folder"));
        assert!(ask(&json!({"command": "x", "timeout_secs": 221}), h).unwrap_err().contains("background"));
        assert!(ask(&json!({"command": "x", "timeout_secs": 0}), h).is_err());
        assert_eq!(ask(&json!({"command": "x", "shell": "powershell"}), h).err().unwrap(), "'powershell' is not a shell here: zsh, bash or sh.");
        assert_eq!(shell_of("zsh").unwrap(), ("zsh", vec!["-l", "-c"]));
        assert_eq!(shell_of("sh").unwrap(), ("sh", vec!["-c"]));
    }

    #[test]
    fn text_is_cut_from_the_end_or_the_start() {
        assert_eq!(cut("abc", 5, false), "abc");
        assert_eq!(cut("abcdef", 3, false), "abc\n…[cut at 3 characters]");
        assert_eq!(cut("abcdef", 3, true), "…[3 characters before cut]\ndef");
        let home = Path::new("/home/me");
        assert_eq!(path_of(home, " notes.txt ").unwrap(), home.join("notes.txt"));
        assert!(path_of(home, "").is_err());
        let full = std::env::temp_dir();
        assert_eq!(path_of(home, full.to_str().unwrap()).unwrap(), full);
    }

    #[test]
    fn files_are_read_by_lines_written_and_edited() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        assert_eq!(run(h, json!({"tool": "write", "path": "src/a.kt", "content": "one\ntwo\nthree\ntwo\n"})).unwrap(), format!("Wrote {} (18 bytes).", h.join("src/a.kt").display()));
        assert_eq!(run(h, json!({"tool": "read", "path": "src/a.kt"})).unwrap(), "     1\tone\n     2\ttwo\n     3\tthree\n     4\ttwo");
        assert_eq!(run(h, json!({"tool": "read", "path": "src/a.kt", "offset": 2, "limit": 1})).unwrap(), "     2\ttwo\n…[2 more lines: read on with offset 3]");
        assert!(run(h, json!({"tool": "read", "path": "src/a.kt", "offset": 9})).unwrap_err().ends_with("has 4 lines."));
        assert!(run(h, json!({"tool": "edit", "path": "src/a.kt", "old": "two", "new": "2"})).unwrap_err().contains("2 times"));
        assert_eq!(run(h, json!({"tool": "edit", "path": "src/a.kt", "old": "two", "new": "2", "replace_all": true})).unwrap(), format!("Replaced 2 times in {}.", h.join("src/a.kt").display()));
        assert_eq!(run(h, json!({"tool": "edit", "path": "src/a.kt", "old": "three", "new": "3"})).unwrap(), format!("Replaced 1 time in {}.", h.join("src/a.kt").display()));
        assert_eq!(std::fs::read_to_string(h.join("src/a.kt")).unwrap(), "one\n2\n3\n2\n");
        assert!(run(h, json!({"tool": "edit", "path": "src/a.kt", "old": "zzz", "new": "y"})).unwrap_err().contains("is not in"));
        assert!(run(h, json!({"tool": "edit", "path": "src/a.kt", "old": "", "new": "y"})).unwrap_err().starts_with("Give 'old'"));
        assert!(run(h, json!({"tool": "edit", "path": "src/a.kt", "old": "y", "new": "y"})).unwrap_err().contains("the same"));
        assert!(run(h, json!({"tool": "write", "path": "x.txt"})).unwrap_err().contains("'content'"));
        assert!(run(h, json!({"tool": "write", "path": "src/a.kt/b", "content": "x"})).is_err());
        assert!(run(h, json!({"tool": "write", "path": "big", "content": "x".repeat(MAX_FILE as usize + 1)})).unwrap_err().starts_with("At most"));
        std::fs::write(h.join("empty.txt"), "").unwrap();
        assert!(run(h, json!({"tool": "read", "path": "empty.txt"})).unwrap().ends_with("is empty."));
        std::fs::write(h.join("bin.dat"), [0xff, 0xfe, 0]).unwrap();
        assert!(run(h, json!({"tool": "read", "path": "bin.dat"})).unwrap_err().ends_with("is not UTF-8 text."));
        assert!(run(h, json!({"tool": "read", "path": "src"})).unwrap_err().contains("is a folder"));
        assert!(run(h, json!({"tool": "read", "path": "gone.txt"})).is_err());
        let huge = h.join("huge.txt");
        std::fs::File::create(&huge).unwrap().set_len(MAX_FILE + 1).unwrap();
        assert!(run(h, json!({"tool": "read", "path": "huge.txt"})).unwrap_err().contains("larger than 20 MB"));
        let long = "y".repeat(MAX_LINE + 10);
        std::fs::write(h.join("long.txt"), &long).unwrap();
        assert!(run(h, json!({"tool": "read", "path": "long.txt"})).unwrap().contains("…[cut at 2000 characters]"));
        assert!(run(h, json!({"tool": "fly"})).unwrap_err().starts_with("'fly' is not a file tool"));
    }

    #[test]
    fn files_are_found_by_pattern_and_searched_by_regex() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        for (p, c) in [("app/src/Main.kt", "fun main() {\n  println(\"Hi\")\n}\n"), ("app/src/util/Util.kt", "// TODO: hi\n"), ("app/build.gradle", "plugins {}\n"), ("app/build/out.kt", "println(\"built\")\n"), (".git/HEAD", "ref\n")] {
            std::fs::create_dir_all(h.join(p).parent().unwrap()).unwrap();
            std::fs::write(h.join(p), c).unwrap();
        }
        let kt = run(h, json!({"tool": "glob", "pattern": "**/*.kt", "path": "app"})).unwrap();
        assert!(kt.contains("src/Main.kt") && kt.contains("src/util/Util.kt") && !kt.contains("build/out.kt"), "{kt}");
        assert!(run(h, json!({"tool": "glob", "pattern": "app/*.{gradle,kts}"})).unwrap().contains("app/build.gradle"));
        assert!(run(h, json!({"tool": "glob", "pattern": "**/*.java"})).unwrap().starts_with("No files under"));
        assert!(run(h, json!({"tool": "glob", "pattern": " "})).unwrap_err().starts_with("Give a 'pattern'"));
        assert!(run(h, json!({"tool": "glob", "pattern": "x", "path": "nope"})).unwrap_err().starts_with("There is no folder"));
        let hits = run(h, json!({"tool": "grep", "pattern": "println", "path": "app"})).unwrap();
        assert!(hits.contains("src/Main.kt:2:  println(\"Hi\")") && !hits.contains("built"), "{hits}");
        assert!(run(h, json!({"tool": "grep", "pattern": "todo: HI", "ignore_case": true, "glob": "*.kt"})).unwrap().contains("app/src/util/Util.kt:1:"));
        assert!(run(h, json!({"tool": "grep", "pattern": "plugins", "glob": "app/src/**"})).unwrap().starts_with("Nothing under"));
        assert!(run(h, json!({"tool": "grep", "pattern": ""})).unwrap_err().starts_with("Give a 'pattern'"));
        assert!(run(h, json!({"tool": "grep", "pattern": "("})).unwrap_err().contains("not a regular expression"));
        let listed = run(h, json!({"tool": "list", "path": "app"})).unwrap();
        assert!(listed.contains("build/\nsrc/\nbuild.gradle  (11 bytes)"), "{listed}");
        assert!(run(h, json!({"tool": "list", "path": "nope"})).is_err());
        assert!(glob_regex("a/**/b?.{x,y}").unwrap().is_match("a/c/d/b1.y"));
        assert!(glob_regex("**").unwrap().is_match("any/where"));
        assert!(glob_regex("a}b").unwrap().is_match("a}b"));
    }

    #[test]
    fn a_search_stops_at_its_limit_and_old_jobs_and_output_go() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        for i in 0..5 {
            std::fs::write(h.join(format!("f{i}.txt")), "hit\n").unwrap();
        }
        assert!(glob_within(h, &json!({"pattern": "*.txt"}), 2).unwrap().ends_with("…[stopped after 2 entries: give a narrower path]"));
        // A link is neither a file nor a folder here: a search passes it by.
        std::os::unix::fs::symlink(h.join("f0.txt"), h.join("linked.txt")).unwrap();
        assert!(!glob(h, &json!({"pattern": "*.txt"})).unwrap().contains("linked.txt"));
        assert!(grep_within(h, &json!({"pattern": "hit"}), 2).unwrap().ends_with("…[stopped after 2 entries: give a narrower path]"));
        // A job's output keeps its newest characters.
        let job = Job::new("build");
        job.add("abcdefgh", 3);
        assert_eq!(*job.output.lock().unwrap(), "fgh");
        // Ended jobs go, oldest first, running ones stay.
        let ended = |c: &str| {
            let j = Arc::new(Job::new(c));
            *j.ended.lock().unwrap() = Some(Some(0));
            (c.to_string(), j)
        };
        let running = ("r".to_string(), Arc::new(Job::new("r")));
        let mut all = vec![running.clone(), ended("a"), ended("b")];
        evict(&mut all, 2);
        assert_eq!(all.iter().map(|(i, _)| i.as_str()).collect::<Vec<_>>(), vec!["r"]);
        let mut busy = vec![running.clone(), running];
        evict(&mut busy, 1);
        assert_eq!(busy.len(), 2);
    }

    #[test]
    fn many_files_and_matches_are_cut() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir(h.join("many")).unwrap();
        for i in 0..MAX_HITS + 3 {
            std::fs::write(h.join("many").join(format!("f{i}.txt")), "hit\n").unwrap();
        }
        assert!(run(h, json!({"tool": "glob", "pattern": "*.txt", "path": "many"})).unwrap().ends_with("…[3 more]"));
        assert!(run(h, json!({"tool": "grep", "pattern": "hit", "path": "many"})).unwrap().ends_with("…[3 more matches]"));
        assert!(run(h, json!({"tool": "list", "path": "many"})).unwrap().ends_with("…[3 more]"));
    }
}

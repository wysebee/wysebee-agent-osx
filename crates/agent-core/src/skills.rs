//! A skill's scripts (the agent-link protocol, "Skill scripts"): reading a skill's `SKILL.md`, the
//! synced file list the poll carries, and putting a skill's files in place on this Mac.
//!
//! **What runs is only what a skill's `SKILL.md` declares** (`scripts:`, each a `name`, a
//! `command` and a `description`), run as an argument list — never through a shell — in this
//! Mac's approved copy of the skill's folder ([`crate::agent`]'s `skill_run`). The person
//! allows each skill here first ([`crate::approvals`]); a skill is the same while its synced
//! file list (each path and id) is unchanged.
//!
//! [`parse`], [`check_scripts`] and [`split_command`] are Wysebee Desktop's `skills.rs`,
//! copied: the two must read a `SKILL.md` the same way.
//!
//! The copies live under the state folder: `skills/<name>/` (the approved copy, which also
//! holds the files the person put there themselves — never sent anywhere, never part of the
//! approval) and `skill-cache/<sha-256>` (downloaded and checked files, content-addressed:
//! the staging a waiting skill's `SKILL.md` is read from; **nothing there ever runs**).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const SKILL_FILE: &str = "SKILL.md";
/// Skills one agent has, at most (and so in a poll, and in `approved_skills`).
pub const MAX_ATTACHED: usize = 50;
pub const MAX_SCRIPTS: usize = 20;
pub const MAX_DESCRIPTION: usize = 1_024;
pub const MAX_TEXT: usize = 1_000;
pub const MAX_BODY: usize = 100_000;
/// Files of one skill that are synced, at most.
pub const MAX_FILES: usize = 200;
/// A synced file, at most.
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Arguments a call adds to a script, at most.
pub const MAX_ARGS: usize = 30;
/// A synced file's path, at most, in characters.
pub const MAX_PATH: usize = 1_024;
/// The approved copies, under the state folder.
pub const FOLDER: &str = "skills";
/// Downloaded, checked files by their id, under the state folder.
pub const CACHE: &str = "skill-cache";

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_-]{0,39}$").unwrap());
static SCRIPT_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]{0,39}$").unwrap());
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").unwrap());

/// A skill's name: its folder's name too.
pub fn valid_name(name: &str) -> bool {
    NAME.is_match(name)
}

pub const NAME_RULE: &str = "a skill's name starts with a lowercase letter and holds only lowercase letters, digits, '-' or '_' (40 at most)";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Script {
    pub name: String,
    pub command: String,
    pub description: String,
}

/// What `SKILL.md` says.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub scripts: Vec<Script>,
    /// The instructions: everything after the front matter.
    pub body: String,
}

/// A front matter value: a quoted string (`"…"` with JSON's escapes, `'…'` with `''`), else
/// the text as it is.
fn unquote(raw: &str) -> Result<String, String> {
    let v = raw.trim();
    if v.starts_with('"') {
        return serde_json::from_str::<String>(v).map_err(|_| format!("The value {v} could not be read."));
    }
    if let Some(inner) = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return Ok(inner.replace("''", "'"));
    }
    Ok(v.to_string())
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The lines after `at` indented more than `under`, joined as a block value (`>` folded with
/// spaces, `|` kept line by line). Answers the value and where the block ended.
fn block(lines: &[&str], at: usize, under: usize, style: &str) -> (String, usize) {
    let mut end = at + 1;
    let mut parts = vec![];
    while end < lines.len() && (lines[end].trim().is_empty() || indent(lines[end]) > under) {
        parts.push(lines[end].trim());
        end += 1;
    }
    let joiner = if style.starts_with('|') { "\n" } else { " " };
    (parts.join(joiner).trim().to_string(), end)
}

/// `SKILL.md` read: its front matter between two `---` lines (`name`, `description`,
/// `scripts`; other keys a skill made elsewhere carries are ignored), then the instructions.
/// `folder` is the name when the front matter has none.
pub fn parse(text: &str, folder: &str) -> Result<Manifest, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    let all: Vec<&str> = text.split('\n').collect();
    let shape = "SKILL.md starts with a line '---', then 'name:' and 'description:', then another '---'.";
    if all.first().map(|l| l.trim_end()) != Some("---") {
        return Err(shape.into());
    }
    let close = all.iter().skip(1).position(|l| l.trim_end() == "---").ok_or(shape)? + 1;
    let front = &all[1..close];
    let body = all[close + 1..].join("\n").trim().to_string();
    let (mut name, mut description) = (String::new(), String::new());
    let mut scripts: Vec<serde_json::Map<String, Value>> = vec![];
    let mut in_scripts = false;
    let mut i = 0;
    while i < front.len() {
        let line = front[i];
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            i += 1;
            continue;
        }
        let depth = indent(line);
        if depth == 0 {
            let (key, value) = t.split_once(':').ok_or(format!("The line '{t}' of SKILL.md could not be read."))?;
            let value = value.trim();
            let (value, next) = if value.starts_with('>') || value.starts_with('|') { block(front, i, 0, value) } else { (unquote(value)?, i + 1) };
            in_scripts = key.trim() == "scripts";
            match key.trim() {
                "name" => name = value,
                "description" => description = value,
                _ => {}
            }
            i = next;
            continue;
        }
        if in_scripts {
            let item = t.strip_prefix('-').filter(|_| t == "-" || t.starts_with("- "));
            if let Some(rest) = item {
                scripts.push(Default::default());
                if rest.trim().is_empty() {
                    i += 1;
                    continue;
                }
            }
            let entry = item.unwrap_or(t).trim();
            let current = scripts.last_mut().ok_or(format!("The line '{t}' of SKILL.md could not be read."))?;
            let (key, value) = entry.split_once(':').ok_or(format!("The line '{t}' of SKILL.md could not be read."))?;
            let value = value.trim();
            let (value, next) = if value.starts_with('>') || value.starts_with('|') { block(front, i, depth, value) } else { (unquote(value)?, i + 1) };
            current.insert(key.trim().to_string(), json!(value));
            i = next;
            continue;
        }
        i += 1; // a nested value of a key not read here
    }
    let name = if name.is_empty() { folder.to_string() } else { name };
    let scripts = check_scripts(&Value::Array(scripts.into_iter().map(Value::Object).collect()))?;
    check(&name, &description, &body)?;
    Ok(Manifest { name, description, scripts, body })
}

/// A name, a description and instructions that fit.
fn check(name: &str, description: &str, body: &str) -> Result<(), String> {
    if !valid_name(name) {
        return Err(format!("'{name}': {NAME_RULE}."));
    }
    if description.trim().is_empty() || description.chars().count() > MAX_DESCRIPTION || description.contains('\n') {
        return Err(format!("'{name}': describe what it is for and when to use it, on one line of 1 to {MAX_DESCRIPTION} characters."));
    }
    if body.chars().count() > MAX_BODY {
        return Err(format!("'{name}': the instructions are at most {MAX_BODY} characters."));
    }
    Ok(())
}

/// A skill's scripts as sent or read: `[{name, command, description}]`, unique names.
pub fn check_scripts(v: &Value) -> Result<Vec<Script>, String> {
    let list = match v {
        Value::Null => return Ok(vec![]),
        Value::Array(a) if a.len() <= MAX_SCRIPTS => a,
        _ => return Err(format!("A skill has at most {MAX_SCRIPTS} scripts.")),
    };
    let mut out: Vec<Script> = vec![];
    for (i, item) in list.iter().enumerate() {
        let text = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let (name, command, description) = (text("name"), text("command"), text("description"));
        if !SCRIPT_NAME.is_match(&name) {
            return Err(format!("Script {}: its name is lowercase letters, digits and _, starting with a letter (up to 40).", i + 1));
        }
        if command.is_empty() || command.chars().count() > MAX_TEXT {
            return Err(format!("Script '{name}': give the command that runs it, like 'python3 scripts/{name}.py'."));
        }
        if split_command(&command).is_none() {
            return Err(format!("Script '{name}': a quote in its command is not closed."));
        }
        if description.chars().count() > MAX_TEXT {
            return Err(format!("Script '{name}': the description is at most {MAX_TEXT} characters."));
        }
        if out.iter().any(|s| s.name == name) {
            return Err(format!("Two scripts are named '{name}'."));
        }
        out.push(Script { name, command, description });
    }
    Ok(out)
}

/// A command line as words: spaces separate them, quotes (`"` or `'`) keep spaces together.
/// None when a quote is not closed.
pub fn split_command(command: &str) -> Option<Vec<String>> {
    let (mut words, mut word, mut quote, mut started) = (vec![], String::new(), None, false);
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            (None, c) => word.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

// ---- The synced file list, as the poll carries it ----

/// One synced file of a skill: its path inside the folder, its SHA-256 (hex) and size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncedFile {
    pub path: String,
    pub id: String,
    pub size: u64,
}

/// One skill the agent has, as the cloud listed it. Its files are sorted by path.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub files: Vec<SyncedFile>,
}

/// What makes a file list the one it is: each path and id, in path order. Sizes are not
/// part of it.
pub fn identity(files: &[SyncedFile]) -> Vec<(&str, &str)> {
    let mut out: Vec<(&str, &str)> = files.iter().map(|f| (f.path.as_str(), f.id.as_str())).collect();
    out.sort_unstable();
    out
}

impl Entry {
    pub fn file(&self, path: &str) -> Option<&SyncedFile> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// A synced file's path: relative, `/`-separated, with no empty, `.` or `..` part, no
/// backslash and nothing that is not printable.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty() && path.chars().count() <= MAX_PATH && !path.starts_with('/') && !path.chars().any(|c| c == '\\' || c.is_control()) && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// A file's id: its SHA-256, 64 lowercase hex digits.
pub fn valid_id(id: &str) -> bool {
    ID.is_match(id)
}

/// Whether `bytes` are the file whose id is `id`.
pub fn hash_ok(bytes: &[u8], id: &str) -> bool {
    hex::encode(Sha256::digest(bytes)) == id
}

/// One skill of the poll's list, or why it is no skill here.
pub fn parse_entry(v: &Value) -> Result<Entry, String> {
    let name = v.get("name").and_then(Value::as_str).unwrap_or("");
    if !valid_name(name) {
        return Err(format!("'{name}': {NAME_RULE}."));
    }
    let raw = v.get("files").and_then(Value::as_array).filter(|a| a.len() <= MAX_FILES).ok_or(format!("'{name}': its files could not be read."))?;
    let mut files = vec![];
    for f in raw {
        let path = f.get("path").and_then(Value::as_str).unwrap_or("");
        let id = f.get("id").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        let size = f.get("size").and_then(Value::as_u64).unwrap_or(0);
        if !safe_path(path) {
            return Err(format!("'{name}': the path '{path}' is not one inside the skill's folder."));
        }
        if !valid_id(&id) || size > MAX_FILE_BYTES {
            return Err(format!("'{name}': '{path}' could not be read."));
        }
        files.push(SyncedFile { path: path.to_string(), id, size });
    }
    // The Mac's disk ignores case: two paths that differ only by it, or a file where another
    // needs a folder, cannot both be put in place.
    let lower: HashSet<String> = files.iter().map(|f| f.path.to_lowercase()).collect();
    if lower.len() != files.len() {
        return Err(format!("'{name}': two of its files have one path."));
    }
    for f in &files {
        let parts: Vec<&str> = f.path.split('/').collect();
        if (1..parts.len()).any(|n| lower.contains(&parts[..n].join("/").to_lowercase())) {
            return Err(format!("'{name}': '{}' is in a folder that is also a file.", f.path));
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Entry { name: name.to_string(), files })
}

/// The skills in a list as the cloud sent it: the first [`MAX_ATTACHED`], one that cannot
/// be read left out (logged), and of two with one name only the first.
pub fn parse_list(list: &[Value]) -> Vec<Entry> {
    let mut out: Vec<Entry> = vec![];
    for v in list.iter().take(MAX_ATTACHED) {
        match parse_entry(v) {
            Ok(e) if !out.iter().any(|o| o.name == e.name) => out.push(e),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "skill_unreadable"),
        }
    }
    out
}

// ---- A script's program ----

/// Where `rel` is inside `root`, through links: never absolute, never `..`, it exists, and
/// it is still inside the folder once resolved.
pub fn inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let given = Path::new(rel);
    if given.is_absolute() || given.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(format!("'{rel}' is not a path inside the skill's folder."));
    }
    let root = root.canonicalize().map_err(|_| "The skill's folder is not there.".to_string())?;
    let real = root.join(given).canonicalize().map_err(|_| format!("There is no '{rel}' in the skill's folder."))?;
    if !real.starts_with(&root) {
        return Err(format!("'{rel}' leads outside the skill's folder."));
    }
    Ok(real)
}

/// The program a script's command starts: its first word. A word holding `/` is a file
/// inside the skill's folder ([`inside`]); any other a program on `path`.
pub fn program(folder: &Path, word: &str, path: Option<&OsStr>) -> Result<PathBuf, String> {
    if word.contains('/') {
        let at = inside(folder, word)?;
        return if at.is_file() { Ok(at) } else { Err(format!("There is no '{word}' in the skill's folder.")) };
    }
    crate::programs::resolve(word, path).ok_or(format!("There is no program '{word}' on this Mac."))
}

/// The synced path a script's program is, when it is one of the folder's files
/// (`./run.sh` and `run.sh` alike): such a file is put in place executable.
pub fn local_program(command: &str) -> Option<String> {
    let first = split_command(command)?.into_iter().next()?;
    if !first.contains('/') {
        return None;
    }
    let mut rest = first.as_str();
    while let Some(r) = rest.strip_prefix("./") {
        rest = r;
    }
    Some(rest.to_string())
}

// ---- Putting a skill in place ----

fn io(what: &str, e: std::io::Error) -> String {
    format!("{what} could not be written: {e}")
}

/// The skill's synced files put in `folder` from `cache` (each checked against its id
/// again), replacing what is at their paths; files of `previous` (what was put there last
/// time) not in `files` removed; everything else in the folder — the person's own files —
/// left as it is. `executable` are the paths written runnable. Nothing is written through a
/// link: a folder on the way that is a link, or a file where a folder is needed, is refused.
pub fn place(folder: &Path, cache: &Path, files: &[SyncedFile], previous: &[SyncedFile], executable: &HashSet<String>) -> Result<(), String> {
    if folder.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
        return Err("The skill's folder is a link or a file: move it away, then allow it again.".into());
    }
    std::fs::create_dir_all(folder).map_err(|e| io("The skill's folder", e))?;
    // Read and checked first: nothing in the folder changes unless every file is here.
    let mut contents = vec![];
    for f in files {
        let bytes = std::fs::read(cache.join(&f.id)).ok().filter(|b| hash_ok(b, &f.id)).ok_or(format!("'{}' was not downloaded as listed.", f.path))?;
        contents.push((f, bytes));
    }
    let now: HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    for old in previous.iter().filter(|o| !now.contains(o.path.as_str())) {
        if let Some(parent) = through(folder, &old.path) {
            let target = parent.join(old.path.rsplit('/').next().unwrap_or(""));
            if target.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
                std::fs::remove_file(&target).map_err(|e| io(&old.path, e))?;
            }
        }
    }
    for (f, bytes) in contents {
        let parts: Vec<&str> = f.path.split('/').collect();
        let mut dir = folder.to_path_buf();
        for part in &parts[..parts.len() - 1] {
            dir.push(part);
            match dir.symlink_metadata() {
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(format!("'{}' in the skill's folder is in the way of '{}': move it, then allow it again.", part, f.path)),
                Err(_) => std::fs::create_dir(&dir).map_err(|e| io(&f.path, e))?,
            }
        }
        let file = parts[parts.len() - 1];
        let target = dir.join(file);
        if target.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            return Err(format!("A folder is in the way of '{}': move it, then allow it again.", f.path));
        }
        let part = dir.join(format!(".{file}.wysebee-part"));
        std::fs::write(&part, &bytes).map_err(|e| io(&f.path, e))?;
        set_mode(&part, executable.contains(&f.path)).map_err(|e| io(&f.path, e))?;
        std::fs::rename(&part, &target).map_err(|e| io(&f.path, e))?;
    }
    Ok(())
}

/// The folder `path`'s file is in, when every folder on the way is a real folder (no link).
fn through(folder: &Path, path: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = path.split('/').collect();
    let mut dir = folder.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        dir.push(part);
        if !dir.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            return None;
        }
    }
    Some(dir)
}

fn set_mode(p: &Path, executable: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEATHER: &str = "---\nname: weather\ndescription: \"Forecasts: by city\"\nlicense: MIT\nmetadata:\n  author: someone\nscripts:\n  - name: forecast\n    command: python3 scripts/forecast.py --days 3\n    description: >\n      Prints the forecast\n      for a city.\n  -\n    name: today\n    command: 'sh' \"run me.sh\"\n# a comment\n---\n\nUse the forecast script.\n";

    fn id(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn skill_md_is_read_with_its_scripts() {
        let m = parse(WEATHER, "folder").unwrap();
        assert_eq!((m.name.as_str(), m.description.as_str(), m.body.as_str()), ("weather", "Forecasts: by city", "Use the forecast script."));
        assert_eq!(m.scripts, vec![
            Script { name: "forecast".into(), command: "python3 scripts/forecast.py --days 3".into(), description: "Prints the forecast for a city.".into() },
            Script { name: "today".into(), command: "'sh' \"run me.sh\"".into(), description: "".into() },
        ]);
        let literal = parse("\u{feff}---\r\ndescription: |\r\n  one\r\n---\r\nbody", "named-by-folder").unwrap();
        assert_eq!((literal.name.as_str(), literal.description.as_str()), ("named-by-folder", "one"));
        assert_eq!(parse("---\ndescription: 'it''s'\n---", "a").unwrap().description, "it's");
    }

    #[test]
    fn a_skill_md_that_does_not_fit_says_why() {
        let shape = "SKILL.md starts with a line '---', then 'name:' and 'description:', then another '---'.";
        assert_eq!(parse("no front matter", "a").unwrap_err(), shape);
        assert_eq!(parse("---\nname: a\n", "a").unwrap_err(), shape);
        assert!(parse("---\nnot a pair\n---", "a").unwrap_err().contains("'not a pair'"));
        assert!(parse("---\ndescription: \"open\n---", "a").unwrap_err().contains("could not be read"));
        assert!(parse("---\nscripts:\n    command: x\n---", "a").unwrap_err().contains("could not be read"), "a script key before any '-'");
        assert!(parse("---\nscripts:\n  - name x\n---", "a").unwrap_err().contains("could not be read"));
        assert!(parse("---\nscripts:\n  - name: \"x\n---", "a").unwrap_err().contains("could not be read"));
        assert!(parse("---\nname: Bad\ndescription: d\n---", "a").unwrap_err().contains(NAME_RULE));
        assert!(parse("---\nname: a\n---", "a").unwrap_err().contains("describe what it is for"));
        assert!(parse(&format!("---\ndescription: d\n---\n{}", "x".repeat(MAX_BODY + 1)), "a").unwrap_err().contains("instructions are at most"));
        let s = |v: Value| check_scripts(&v).unwrap_err();
        assert!(s(json!("x")).contains("at most 20"));
        assert!(s(json!([{"name": "1"}])).contains("Script 1"));
        assert!(s(json!([{"name": "a"}])).contains("give the command"));
        assert!(s(json!([{"name": "a", "command": "sh 'x"}])).contains("not closed"));
        assert!(s(json!([{"name": "a", "command": "sh", "description": "d".repeat(1001)}])).contains("description is at most"));
        assert_eq!(s(json!([{"name": "a", "command": "sh"}, {"name": "a", "command": "sh"}])), "Two scripts are named 'a'.");
        assert_eq!(check_scripts(&Value::Null).unwrap(), vec![]);
    }

    #[test]
    fn commands_split_on_spaces_and_keep_quoted_words_whole() {
        assert_eq!(split_command("  python3  'my file.py' \"a b\" '' c").unwrap(), vec!["python3", "my file.py", "a b", "", "c"]);
        assert_eq!(split_command("sh x\"y z\"").unwrap(), vec!["sh", "xy z"]);
        assert!(split_command("sh \"open").is_none());
        assert_eq!(split_command("''").unwrap(), vec![""]);
        assert_eq!(split_command("sh  ").unwrap(), vec!["sh"]);
    }

    #[test]
    fn the_polls_file_lists_are_checked() {
        let a = id(b"a");
        let ok = parse_entry(&json!({"name": "notes", "files": [{"path": "b/c.txt", "id": a.to_uppercase(), "size": 1}, {"path": "SKILL.md", "id": a, "size": 1}]})).unwrap();
        assert_eq!(ok.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["SKILL.md", "b/c.txt"], "sorted by path");
        assert_eq!(ok.files[1].id, a, "an id is kept in lowercase");
        assert!(ok.file("SKILL.md").is_some() && ok.file("x").is_none());
        let refuse = |files: Value| parse_entry(&json!({"name": "notes", "files": files})).unwrap_err();
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a//b", "./a", "a/.", "a\\b", "a\nb", &"x".repeat(MAX_PATH + 1)] {
            assert!(refuse(json!([{"path": bad, "id": a, "size": 1}])).contains("is not one inside"), "{bad:?}");
        }
        assert!(refuse(json!([{"path": "x", "id": "abc", "size": 1}])).contains("could not be read"));
        assert!(refuse(json!([{"path": "x", "id": a, "size": MAX_FILE_BYTES + 1}])).contains("could not be read"));
        assert!(refuse(json!([{"path": "x", "id": a}, {"path": "X", "id": a}])).contains("one path"));
        assert!(refuse(json!([{"path": "x", "id": a}, {"path": "x/y", "id": a}])).contains("also a file"));
        assert!(refuse(json!("no")).contains("files could not be read"));
        assert!(refuse(json!(vec![json!({"path": "x", "id": a}); MAX_FILES + 1])).contains("files could not be read"));
        assert!(parse_entry(&json!({"name": "Bad", "files": []})).unwrap_err().contains(NAME_RULE));

        let list = parse_list(&[json!({"name": "a", "files": []}), json!({"name": "a", "files": [{"path": "x", "id": a}]}), json!({"name": "bad name"}), json!({"name": "b", "files": []})]);
        assert_eq!(list.iter().map(|e| (e.name.as_str(), e.files.len())).collect::<Vec<_>>(), vec![("a", 0), ("b", 0)]);
        let many: Vec<Value> = (0..60).map(|i| json!({"name": format!("s{i}"), "files": []})).collect();
        assert_eq!(parse_list(&many).len(), MAX_ATTACHED);
        let f = |p: &str, i: &str| SyncedFile { path: p.into(), id: i.into(), size: 9 };
        assert_eq!(identity(&[f("b", "2"), f("a", "1")]), vec![("a", "1"), ("b", "2")]);
        assert!(hash_ok(b"a", &a) && !hash_ok(b"b", &a));
    }

    #[test]
    fn a_scripts_program_is_in_the_folder_or_on_path_and_never_outside() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("skill");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(folder.join("scripts")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(folder.join("scripts/run.sh"), "x").unwrap();
        std::fs::write(bin.join("tool"), "x").unwrap();
        std::fs::write(dir.path().join("outside.sh"), "x").unwrap();
        symlink(dir.path().join("outside.sh"), folder.join("escape.sh")).unwrap();
        symlink(folder.join("scripts/run.sh"), folder.join("alias.sh")).unwrap();
        let path = std::env::join_paths([&bin]).unwrap();
        let real = folder.canonicalize().unwrap();
        assert_eq!(program(&folder, "./scripts/run.sh", None).unwrap(), real.join("scripts/run.sh"));
        assert_eq!(program(&folder, "./alias.sh", None).unwrap(), real.join("scripts/run.sh"), "a link that stays inside is followed");
        assert_eq!(program(&folder, "./escape.sh", None).unwrap_err(), "'./escape.sh' leads outside the skill's folder.");
        assert_eq!(program(&folder, "../outside.sh", None).unwrap_err(), "'../outside.sh' is not a path inside the skill's folder.");
        assert_eq!(program(&folder, "/bin/sh", None).unwrap_err(), "'/bin/sh' is not a path inside the skill's folder.");
        assert_eq!(program(&folder, "./scripts", None).unwrap_err(), "There is no './scripts' in the skill's folder.");
        assert_eq!(program(&folder, "./none.sh", None).unwrap_err(), "There is no './none.sh' in the skill's folder.");
        assert_eq!(program(&folder, "tool", Some(&path)).unwrap(), bin.join("tool"));
        assert_eq!(program(&folder, "nope", Some(&path)).unwrap_err(), "There is no program 'nope' on this Mac.");
        assert_eq!(program(&dir.path().join("gone"), "./x", None).unwrap_err(), "The skill's folder is not there.");

        assert_eq!(local_program("./run.sh a"), Some("run.sh".into()));
        assert_eq!(local_program("././scripts/x.py"), Some("scripts/x.py".into()));
        assert_eq!(local_program("python3 x.py"), None);
        assert_eq!(local_program("sh 'open"), None);
        assert_eq!(local_program(""), None);
    }

    #[test]
    fn a_skill_is_put_in_place_and_the_persons_own_files_stay() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (folder, cache) = (dir.path().join("skills/notes"), dir.path().join("cache"));
        std::fs::create_dir_all(&cache).unwrap();
        let put = |bytes: &[u8]| {
            let i = id(bytes);
            std::fs::write(cache.join(&i), bytes).unwrap();
            i
        };
        let f = |p: &str, i: &str| SyncedFile { path: p.into(), id: i.into(), size: 1 };
        let (one, two, three) = (put(b"one"), put(b"two"), put(b"three"));
        let first = vec![f("SKILL.md", &one), f("run.sh", &two), f("old/gone.txt", &three)];
        place(&folder, &cache, &first, &[], &HashSet::from(["run.sh".to_string()])).unwrap();
        assert_eq!(std::fs::read(folder.join("old/gone.txt")).unwrap(), b"three");
        let mode = |p: &str| std::fs::metadata(folder.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode("run.sh"), mode("SKILL.md")), (0o755, 0o644));

        // The person's own file, and the next list: one changed, one gone, one new.
        std::fs::write(folder.join(".env"), "KEY=1").unwrap();
        let four = put(b"four");
        let second = vec![f("SKILL.md", &four), f("run.sh", &two), f("new/n.txt", &one)];
        place(&folder, &cache, &second, &first, &HashSet::new()).unwrap();
        assert_eq!(std::fs::read(folder.join("SKILL.md")).unwrap(), b"four");
        assert!(!folder.join("old/gone.txt").exists() && folder.join("new/n.txt").exists());
        assert_eq!(std::fs::read_to_string(folder.join(".env")).unwrap(), "KEY=1", "the person's file stays");
        assert!(std::fs::read_dir(&folder).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with("wysebee-part")));

        // A file not in the cache, or whose bytes are not its id: nothing changes.
        std::fs::write(cache.join(&three), "tampered").unwrap();
        assert_eq!(place(&folder, &cache, &[f("SKILL.md", &three)], &second, &HashSet::new()).unwrap_err(), "'SKILL.md' was not downloaded as listed.");
        assert_eq!(std::fs::read(folder.join("SKILL.md")).unwrap(), b"four");

        // Nothing is written through a link, nor over a folder.
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("linked")).unwrap();
        assert!(place(&folder, &cache, &[f("linked/x", &one)], &[], &HashSet::new()).unwrap_err().contains("'linked' in the skill's folder is in the way"));
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        // A previous file behind a link is not removed through it.
        std::fs::write(outside.join("x"), "keep").unwrap();
        place(&folder, &cache, &[], &[f("linked/x", &one), f("never/was", &one)], &HashSet::new()).unwrap();
        assert!(outside.join("x").exists());
        // A previous file the person removed already, or put a folder of their own in place
        // of: nothing to remove, and their folder stays.
        std::fs::create_dir_all(folder.join("theirs")).unwrap();
        std::fs::write(folder.join("theirs/mine.txt"), "mine").unwrap();
        place(&folder, &cache, &[], &[f("removed-already.txt", &one), f("theirs", &one)], &HashSet::new()).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join("theirs/mine.txt")).unwrap(), "mine");
        std::fs::create_dir_all(folder.join("adir")).unwrap();
        assert!(place(&folder, &cache, &[f("adir", &one)], &[], &HashSet::new()).unwrap_err().contains("A folder is in the way of 'adir'"));
        let link_folder = dir.path().join("skills/linked");
        std::os::unix::fs::symlink(&outside, &link_folder).unwrap();
        assert!(place(&link_folder, &cache, &[], &[], &HashSet::new()).unwrap_err().contains("is a link or a file"));
        // A place the folder cannot be made.
        let blocked = dir.path().join("file");
        std::fs::write(&blocked, "x").unwrap();
        assert!(place(&blocked.join("sub"), &cache, &[], &[], &HashSet::new()).unwrap_err().contains("could not be written"));
    }
}

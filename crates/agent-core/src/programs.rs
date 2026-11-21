//! The programs an agent's settings hold ("Run local program"), and running one.
//!
//! **The person decides everything that runs; the model only fills the blanks.** A program
//! is set up in Wysebee (the agent's Tools tab) and allowed on this Mac
//! ([`crate::approvals`]). Each argument is fixed text the person wrote or a blank the
//! model fills. What runs is that program with those arguments, as an argument list —
//! **never through a shell**, so a value the model chose is one argument, whatever it
//! holds, and can neither start another program nor redirect anything.
//!
//! Bounded: [`TIME_LIMIT`] per run (the program is killed after it), no input, its output
//! and errors cut to [`MAX_OUTPUT_CHARS`] each.
//!
//! The parsing, filling, resolving and running are Wysebee Desktop's `local_programs.rs`,
//! copied: the two must read a program the same way (the agent-link protocol).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub const MAX_PROGRAMS: usize = 20;
pub const MAX_ARGS: usize = 30;
pub const MAX_TEXT: usize = 1_000;
pub const TIME_LIMIT: Duration = Duration::from_secs(60);
pub const MAX_OUTPUT_CHARS: usize = 20_000;

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]{0,39}$").unwrap());

/// One argument: fixed text, or a blank the model fills.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Fixed(String),
    Param { name: String, description: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub name: String,
    pub description: String,
    pub command: String,
    pub args: Vec<Arg>,
}

/// One argument as approval sees it: a fixed value, or a blank's name. A blank's
/// description is not part of what the person allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Part {
    Value(String),
    Param(String),
}

impl Program {
    /// The arguments as approval sees them, in order.
    pub fn parts(&self) -> Vec<Part> {
        self.args
            .iter()
            .map(|a| match a {
                Arg::Fixed(v) => Part::Value(v.clone()),
                Arg::Param { name, .. } => Part::Param(name.clone()),
            })
            .collect()
    }

    /// The whole command line, for the person to read before allowing it: the program,
    /// each fixed argument (quoted when it holds a space or nothing) and each blank as
    /// `<name>`.
    pub fn command_line(&self) -> String {
        let mut words = vec![shown(&self.command)];
        for a in &self.args {
            words.push(match a {
                Arg::Fixed(v) => shown(v),
                Arg::Param { name, .. } => format!("<{name}>"),
            });
        }
        words.join(" ")
    }
}

/// A word as shown in a command line: as is, or as a JSON string when it would not read
/// as one word.
fn shown(word: &str) -> String {
    let plain = !word.is_empty() && !word.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>'));
    if plain {
        word.to_string()
    } else {
        Value::String(word.to_string()).to_string()
    }
}

fn text<'a>(o: &'a Map<String, Value>, key: &str) -> &'a str {
    o.get(key).and_then(Value::as_str).unwrap_or("")
}

fn parse_arg(v: &Value, at: usize) -> Result<Arg, String> {
    let o = v.as_object().ok_or(format!("Argument {at} could not be read."))?;
    if let Some(name) = o.get("param") {
        let name = name.as_str().unwrap_or("").trim();
        if !NAME.is_match(name) {
            return Err(format!("Argument {at}: a blank's name is lowercase letters, digits and _, starting with a letter (up to 40)."));
        }
        let description = text(o, "description").trim();
        if description.chars().count() > MAX_TEXT {
            return Err(format!("Argument {at}: the description is at most {MAX_TEXT} characters."));
        }
        return Ok(Arg::Param { name: name.into(), description: description.into() });
    }
    match o.get("value").and_then(Value::as_str) {
        Some(value) if value.chars().count() <= MAX_TEXT => Ok(Arg::Fixed(value.into())),
        Some(_) => Err(format!("Argument {at} is at most {MAX_TEXT} characters.")),
        None => Err(format!("Argument {at} needs a value, or a name to be filled in.")),
    }
}

/// One program as the agent's settings hold it; `at` numbers it in an error.
pub fn parse_one(v: &Value, at: usize) -> Result<Program, String> {
    let o = v.as_object().ok_or(format!("Tool {at} could not be read."))?;
    let name = text(o, "name").trim();
    if !NAME.is_match(name) {
        return Err(format!("Tool {at}: its name is lowercase letters, digits and _, starting with a letter (up to 40)."));
    }
    let description = text(o, "description").trim();
    if description.is_empty() || description.chars().count() > MAX_TEXT {
        return Err(format!("'{name}': describe what it does, in 1 to {MAX_TEXT} characters."));
    }
    let command = text(o, "command").trim();
    if command.is_empty() || command.chars().count() > MAX_TEXT {
        return Err(format!("'{name}': choose the program to run."));
    }
    let raw = o.get("args").cloned().unwrap_or_else(|| json!([]));
    let list = raw.as_array().filter(|a| a.len() <= MAX_ARGS).ok_or(format!("'{name}': at most {MAX_ARGS} arguments."))?;
    let args = list.iter().enumerate().map(|(i, a)| parse_arg(a, i + 1)).collect::<Result<Vec<_>, _>>().map_err(|e| format!("'{name}': {e}"))?;
    let mut blanks: Vec<&str> = args.iter().filter_map(|a| if let Arg::Param { name, .. } = a { Some(name.as_str()) } else { None }).collect();
    blanks.sort_unstable();
    if blanks.windows(2).any(|w| w[0] == w[1]) {
        return Err(format!("'{name}': two blanks have the same name."));
    }
    Ok(Program { name: name.into(), description: description.into(), command: command.into(), args })
}

/// The programs in a list as the cloud sent it: the first [`MAX_PROGRAMS`] entries, one
/// that cannot be read left out, and of two with one name only the first (a name stands
/// for one program here).
pub fn parse_list(list: &[Value]) -> Vec<Program> {
    let mut out: Vec<Program> = vec![];
    for (i, v) in list.iter().take(MAX_PROGRAMS).enumerate() {
        if let Ok(p) = parse_one(v, i + 1) {
            if !out.iter().any(|o| o.name == p.name) {
                out.push(p);
            }
        }
    }
    out
}

/// Where a program is: an absolute path that is a file, or a bare name found in one of
/// `path`'s folders (a `PATH`). None when it is neither.
pub fn resolve(command: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let given = Path::new(command);
    if given.is_absolute() {
        return given.is_file().then(|| given.to_path_buf());
    }
    if given.components().count() != 1 {
        return None;
    }
    std::env::split_paths(path?).map(|dir| dir.join(command)).find(|p| p.is_absolute() && p.is_file())
}

/// The folders programs are looked for in: `path` (the app's `PATH`), then Homebrew's and
/// `/usr/local/bin`, which an app started from Finder does not have on its `PATH`.
pub fn search_path(path: Option<&OsStr>) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = path.map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if !dirs.iter().any(|d| d == Path::new(extra)) {
            dirs.push(extra.into());
        }
    }
    // A folder name holding the separator cannot be on a PATH; such a PATH is left as given.
    std::env::join_paths(dirs).unwrap_or_else(|_| path.unwrap_or_default().to_os_string())
}

/// The argument list: the fixed text and the value given for each blank, in order.
pub fn fill(parts: &[Part], values: &Value) -> Result<Vec<String>, String> {
    let given = values.as_object().cloned().unwrap_or_default();
    let mut argv = vec![];
    for part in parts {
        match part {
            Part::Value(v) => argv.push(v.clone()),
            Part::Param(name) => match given.get(name) {
                Some(Value::String(s)) => argv.push(s.clone()),
                Some(v) if !v.is_null() => argv.push(v.to_string()),
                _ => return Err(format!("Fill in '{name}'.")),
            },
        }
    }
    Ok(argv)
}

/// Output as text, cut at [`MAX_OUTPUT_CHARS`].
pub fn cut(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text.into_owned();
    }
    format!("{}\n…[cut at {MAX_OUTPUT_CHARS} characters]", text.chars().take(MAX_OUTPUT_CHARS).collect::<String>())
}

/// Start it and wait for it: its output, or None when `limit` passed first (it is killed —
/// `kill_on_drop` — as the wait is dropped; so is one whose caller stops waiting).
pub async fn execute(exe: &Path, argv: &[String], cwd: &Path, limit: Duration) -> std::io::Result<Option<std::process::Output>> {
    let child = tokio::process::Command::new(exe).args(argv).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
    tokio::time::timeout(limit, child.wait_with_output()).await.ok().transpose()
}

/// What a finished run answers: `{"exit_code", "stdout", "stderr"}`.
pub fn answer(out: &std::process::Output) -> Value {
    json!({"exit_code": out.status.code(), "stdout": cut(&out.stdout), "stderr": cut(&out.stderr)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_are_read_and_checked() {
        let list = vec![
            json!({"name": "word_count", "description": " Count words ", "command": "wc", "args": [{"value": "-w"}, {"param": "file", "description": "The file"}]}),
            json!({"name": "Bad", "description": "x", "command": "wc"}),
            json!({"name": "word_count", "description": "another", "command": "rm"}),
        ];
        let progs = parse_list(&list);
        assert_eq!(progs.len(), 1, "an entry that cannot be read is left out, and a second of one name");
        assert_eq!(progs[0].description, "Count words");
        assert_eq!(progs[0].command, "wc");
        assert_eq!(progs[0].args, vec![Arg::Fixed("-w".into()), Arg::Param { name: "file".into(), description: "The file".into() }]);
        assert_eq!(progs[0].parts(), vec![Part::Value("-w".into()), Part::Param("file".into())]);
        let many: Vec<Value> = (0..25).map(|i| json!({"name": format!("p{i}"), "description": "d", "command": "wc"})).collect();
        assert_eq!(parse_list(&many).len(), MAX_PROGRAMS);

        let refuse = |v: Value| parse_one(&v, 1).unwrap_err();
        assert_eq!(refuse(json!(1)), "Tool 1 could not be read.");
        assert!(refuse(json!({"name": "1x"})).contains("its name is lowercase"));
        assert!(refuse(json!({"name": "x", "description": ""})).contains("describe what it does"));
        assert!(refuse(json!({"name": "x", "description": "d", "command": " "})).contains("choose the program"));
        assert!(refuse(json!({"name": "x", "description": "d", "command": "wc", "args": "no"})).contains("at most 30 arguments"));
        assert!(refuse(json!({"name": "x", "description": "d", "command": "wc", "args": [3]})).contains("Argument 1 could not be read."));
        assert!(refuse(json!({"name": "x", "description": "d", "command": "wc", "args": [{"param": "A"}]})).contains("a blank's name"));
        assert!(refuse(json!({"name": "x", "description": "d", "command": "wc", "args": [{"param": "a", "description": "d".repeat(1001)}]})).contains("description is at most"));
        assert!(refuse(json!({"name": "x", "description": "d", "command": "wc", "args": [{"value": "v".repeat(1001)}]})).contains("at most 1000 characters"));
        assert!(refuse(json!({"name": "x", "description": "d", "command": "wc", "args": [{}]})).contains("needs a value"));
        assert!(refuse(json!({"name": "x", "description": "d", "command": "wc", "args": [{"param": "a"}, {"param": "a"}]})).contains("same name"));
    }

    #[test]
    fn a_command_line_reads_as_what_will_run() {
        let p = Program {
            name: "p".into(),
            description: "d".into(),
            command: "/usr/bin/say".into(),
            args: vec![Arg::Fixed("-v".into()), Arg::Fixed("Good News".into()), Arg::Fixed("".into()), Arg::Param { name: "words".into(), description: "".into() }],
        };
        assert_eq!(p.command_line(), r#"/usr/bin/say -v "Good News" "" <words>"#);
        // A fixed value that looks like a blank is quoted, so it is not taken for one.
        let tricky = Program { args: vec![Arg::Fixed("<words>".into())], ..p };
        assert_eq!(tricky.command_line(), r#"/usr/bin/say "<words>""#);
    }

    #[test]
    fn programs_are_found_by_path_or_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("tool");
        std::fs::write(&exe, "x").unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(resolve("tool", Some(&path)), Some(exe.clone()));
        assert_eq!(resolve(exe.to_str().unwrap(), None), Some(exe.clone()));
        assert_eq!(resolve(dir.path().to_str().unwrap(), None), None, "a folder is not a program");
        assert_eq!(resolve("sub/tool", Some(&path)), None, "a relative path is not looked up");
        assert_eq!(resolve("tool", None), None);
        assert_eq!(resolve("other", Some(&path)), None);
        assert_eq!(resolve("tool", Some(OsStr::new("."))), None, "a relative folder on PATH is not searched");

        let searched = search_path(Some(&path));
        let dirs: Vec<PathBuf> = std::env::split_paths(&searched).collect();
        assert_eq!(dirs, vec![dir.path().to_path_buf(), "/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
        let twice = search_path(Some(OsStr::new("/usr/local/bin:/bin")));
        assert_eq!(twice, "/usr/local/bin:/bin:/opt/homebrew/bin");
        assert_eq!(search_path(None), "/opt/homebrew/bin:/usr/local/bin");
    }

    #[test]
    fn blanks_are_filled_in_order() {
        let parts = vec![Part::Value("-n".into()), Part::Param("what".into()), Part::Param("count".into())];
        assert_eq!(fill(&parts, &json!({"what": "a b", "count": 2, "extra": "ignored"})).unwrap(), vec!["-n", "a b", "2"]);
        assert_eq!(fill(&parts, &json!({"what": "x"})).unwrap_err(), "Fill in 'count'.");
        assert_eq!(fill(&parts, &json!({"what": null, "count": "1"})).unwrap_err(), "Fill in 'what'.");
        assert_eq!(fill(&parts, &json!("no")).unwrap_err(), "Fill in 'what'.");
        assert_eq!(fill(&[], &Value::Null).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn long_output_is_cut() {
        assert_eq!(cut(b"short"), "short");
        let long = "é".repeat(MAX_OUTPUT_CHARS + 5);
        let out = cut(long.as_bytes());
        assert!(out.ends_with("\n…[cut at 20000 characters]"));
        assert_eq!(out.chars().filter(|c| *c == 'é').count(), MAX_OUTPUT_CHARS);
        assert_eq!(cut(&[0xff, b'a']), "\u{fffd}a");
    }
}

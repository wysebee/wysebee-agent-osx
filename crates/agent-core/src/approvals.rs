//! What this Mac keeps, and what the person allowed on it (the agent-link protocol, "Approval, on
//! the Mac").
//!
//! - A program is **the same** while its name, command and arguments (each fixed value and
//!   each blank's name, in order) are unchanged; its description is not part of it.
//! - Allowing resolves the program **then** ([`State::allow`]); the approved copy keeps the
//!   resolved absolute path, and that path is what runs. A call carries only a name and
//!   the blanks' values, so the cloud cannot make this app run a command the person did
//!   not see here.
//! - A program no longer in the cloud's list loses its approval ([`State::sync`]).
//! - **A skill** (the agent-link protocol, "Skill scripts") is the same while its synced file list
//!   (each path and id) is unchanged ([`ApprovedSkill::is`]); one changed since it was
//!   allowed keeps its approved copy on file but runs nothing until allowed again. The
//!   approved copy keeps each declared script's resolved program. [`State::placed_skills`]
//!   records which files of a skill's folder are synced ones; every other file there is the
//!   person's own.
//! - Everything is one JSON file, [`STATE_FILE`], in a folder the caller names (the app:
//!   `~/Library/Application Support/Wysebee Agent`), written atomically. **The link's
//!   secret is never in it** ([`crate::secrets`]).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::folders::Folder;
use crate::programs::{self, Part, Program};
use crate::skills::{self, Entry, SyncedFile};

pub const STATE_FILE: &str = "state.json";
/// The app's folder under `~/Library/Application Support`.
pub const APP_FOLDER: &str = "Wysebee Agent";

/// The folder the app keeps its state in, under the person's home.
pub fn default_dir(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support").join(APP_FOLDER)
}

/// The agent a link was claimed as.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentName {
    pub name: String,
    pub label: String,
}

/// The link with Wysebee Cloud: waiting for its code to be typed (`agent` none), or paired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub id: String,
    /// The cloud it was made with — the only place its secret is ever sent.
    pub cloud_url: String,
    /// The agent name the person typed.
    pub name: String,
    #[serde(default)]
    pub code: Option<String>,
    /// When the code runs out, in seconds since 1970.
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub agent: Option<AgentName>,
}

/// A program the person allowed: what they saw, and where it was then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approved {
    pub name: String,
    pub command: String,
    pub args: Vec<Part>,
    /// The program's absolute path when it was allowed: what runs.
    pub path: PathBuf,
}

impl Approved {
    /// Whether this is the approval of `program` as it is now.
    pub fn is(&self, program: &Program) -> bool {
        self.name == program.name && self.command == program.command && self.args == program.parts()
    }
}

/// A script of an allowed skill: what `SKILL.md` declared, and its program as resolved when
/// the person allowed the skill (what runs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovedScript {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub description: String,
    /// The program's absolute path when the skill was allowed.
    pub program: PathBuf,
    /// Whether the program is a file of the skill's folder (checked again at each run to
    /// still be inside it).
    pub in_folder: bool,
}

/// A skill the person allowed: its synced file list as it was then, and its scripts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovedSkill {
    pub name: String,
    pub files: Vec<SyncedFile>,
    pub scripts: Vec<ApprovedScript>,
}

impl ApprovedSkill {
    /// Whether this is the approval of `entry` as it is now: the same paths and ids.
    pub fn is(&self, entry: &Entry) -> bool {
        self.name == entry.name && skills::identity(&self.files) == skills::identity(&entry.files)
    }
}

/// The synced files last put in a skill's folder here: the ones a later Allow replaces or
/// removes. Every other file there is the person's own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedSkill {
    pub name: String,
    pub files: Vec<SyncedFile>,
}

/// Where a program (or a skill) of the cloud's list stands on this Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Standing {
    /// Allowed as it is, and its program is where it was.
    Approved,
    /// Never allowed here.
    Waiting,
    /// Allowed once, and changed in Wysebee since: waiting again.
    Changed,
    /// Allowed as it is, but the program is no longer where it was.
    MissingProgram,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The cloud address saved from the window; none: the default.
    #[serde(default)]
    pub cloud_url: Option<String>,
    #[serde(default)]
    pub link: Option<Link>,
    /// "Let Wysebee run programs on this Mac" turned off.
    #[serde(default)]
    pub paused: bool,
    /// The agent's programs as the cloud last sent them.
    #[serde(default)]
    pub programs: Vec<Value>,
    #[serde(default)]
    pub approved: Vec<Approved>,
    /// The agent's skills as the cloud last sent them (each a name and its synced files).
    #[serde(default)]
    pub skills: Vec<Value>,
    #[serde(default)]
    pub approved_skills: Vec<ApprovedSkill>,
    /// What was put in each skill's folder here, kept until unpaired.
    #[serde(default)]
    pub placed_skills: Vec<PlacedSkill>,
    /// The folders the person picked here for the agent, in the order reported (the agent-link protocol,
    /// "Local folders"). Nothing to approve: the person chose each one here.
    #[serde(default)]
    pub folders: Vec<Folder>,
    /// "Let Wysebee run any program on this Mac": `program_run`,
    /// the program the AI names, runs only while this is on. Off by default, and forgotten on
    /// unpairing: a new pairing is a new agent.
    #[serde(default)]
    pub any_program: bool,
    /// "Let Wysebee see and control this Mac's screen" (2026-11-16): `screen_screenshot` and
    /// `screen_action` work only while this is on. Off by default, forgotten on unpairing.
    #[serde(default)]
    pub screen_control: bool,
    /// "Let Wysebee use this Mac as a developer" (2026-11-16): `dev_shell`, `dev_job` and
    /// `dev_file` — a shell and files anywhere. Off by default, forgotten on unpairing.
    #[serde(default)]
    pub dev_access: bool,
    /// "Let Wysebee use Android phones on this Mac" (2026-11-16): `android_dev`, the Android
    /// Development Toolkit through adb. Off by default, forgotten on unpairing.
    #[serde(default)]
    pub android_dev: bool,
    /// "Let Wysebee use iPhones and simulators on this Mac" (2026-11-17): `ios_dev`, the iPhone
    /// Development Toolkit through xcrun. Off by default, forgotten on unpairing.
    #[serde(default)]
    pub ios_dev: bool,
}

impl State {
    /// The cloud's programs that can be read.
    pub fn programs(&self) -> Vec<Program> {
        programs::parse_list(&self.programs)
    }

    pub fn program(&self, name: &str) -> Option<Program> {
        self.programs().into_iter().find(|p| p.name == name)
    }

    pub fn approval(&self, name: &str) -> Option<&Approved> {
        self.approved.iter().find(|a| a.name == name)
    }

    pub fn standing(&self, program: &Program) -> Standing {
        match self.approval(&program.name) {
            None => Standing::Waiting,
            Some(a) if !a.is(program) => Standing::Changed,
            Some(a) if !a.path.is_file() => Standing::MissingProgram,
            Some(_) => Standing::Approved,
        }
    }

    /// The approval `name` runs from: only when the cloud's program of that name is
    /// exactly what the person allowed.
    pub fn runnable(&self, name: &str) -> Option<&Approved> {
        let program = self.program(name)?;
        self.approval(name).filter(|a| a.is(&program))
    }

    /// The names the poll reports as allowed: the cloud's programs this Mac allowed as
    /// they are.
    pub fn allowed_names(&self) -> Vec<String> {
        self.programs().iter().filter(|p| self.approval(&p.name).is_some_and(|a| a.is(p))).map(|p| p.name.clone()).collect()
    }

    /// The cloud's list, as a poll carried it. Approvals of programs it no longer holds
    /// are lost.
    pub fn sync(&mut self, list: &[Value]) {
        self.programs = list.iter().take(programs::MAX_PROGRAMS).cloned().collect();
        let names: Vec<String> = self.programs().into_iter().map(|p| p.name).collect();
        self.approved.retain(|a| names.contains(&a.name));
    }

    /// The person allowed `name` as the cloud's list has it now. The program is resolved
    /// now, on `path` (a `PATH`), and that absolute path is kept.
    pub fn allow(&mut self, name: &str, path: Option<&OsStr>) -> Result<(), String> {
        let program = self.program(name).ok_or(format!("There is no program '{name}' to allow."))?;
        let found = programs::resolve(&program.command, path).ok_or(format!("There is no program '{}' on this Mac.", program.command))?;
        self.approved.retain(|a| a.name != name);
        self.approved.push(Approved { name: program.name.clone(), command: program.command.clone(), args: program.parts(), path: found });
        Ok(())
    }

    /// The person took the approval of `name` back.
    pub fn revoke(&mut self, name: &str) {
        self.approved.retain(|a| a.name != name);
    }

    /// The cloud's skills that can be read.
    pub fn skills(&self) -> Vec<Entry> {
        skills::parse_list(&self.skills)
    }

    pub fn skill(&self, name: &str) -> Option<Entry> {
        self.skills().into_iter().find(|s| s.name == name)
    }

    pub fn skill_approval(&self, name: &str) -> Option<&ApprovedSkill> {
        self.approved_skills.iter().find(|a| a.name == name)
    }

    /// The synced files last put in `name`'s folder (none: nothing was).
    pub fn placed(&self, name: &str) -> &[SyncedFile] {
        self.placed_skills.iter().find(|p| p.name == name).map(|p| p.files.as_slice()).unwrap_or(&[])
    }

    pub fn skill_standing(&self, entry: &Entry) -> Standing {
        match self.skill_approval(&entry.name) {
            None => Standing::Waiting,
            Some(a) if !a.is(entry) => Standing::Changed,
            Some(a) if a.scripts.iter().any(|s| !s.program.is_file()) => Standing::MissingProgram,
            Some(_) => Standing::Approved,
        }
    }

    /// The approval a script of `name` runs from: only when the cloud's skill of that name
    /// has exactly the files the person allowed.
    pub fn runnable_skill(&self, name: &str) -> Option<&ApprovedSkill> {
        let entry = self.skill(name)?;
        self.skill_approval(name).filter(|a| a.is(&entry))
    }

    /// The skills the poll reports as allowed.
    pub fn allowed_skill_names(&self) -> Vec<String> {
        self.skills().iter().filter(|e| self.skill_approval(&e.name).is_some_and(|a| a.is(e))).map(|e| e.name.clone()).collect()
    }

    /// The cloud's skills, as a poll carried them. Approvals of skills it no longer lists
    /// are lost; their folders stay (the person's own files may be there) until unpaired.
    pub fn sync_skills(&mut self, list: &[Value]) {
        self.skills = list.iter().take(skills::MAX_ATTACHED).cloned().collect();
        let names: Vec<String> = self.skills().into_iter().map(|e| e.name).collect();
        self.approved_skills.retain(|a| names.contains(&a.name));
    }

    /// The person allowed a skill: `approved` was put in place in its folder.
    pub fn record_skill(&mut self, approved: ApprovedSkill) {
        self.placed_skills.retain(|p| p.name != approved.name);
        self.placed_skills.push(PlacedSkill { name: approved.name.clone(), files: approved.files.clone() });
        self.approved_skills.retain(|a| a.name != approved.name);
        self.approved_skills.push(approved);
    }

    /// Files were put in `name`'s folder, but it was not allowed (a script's program could
    /// not be found): what is there is known, and nothing of it runs.
    pub fn record_placed(&mut self, name: &str, files: Vec<SyncedFile>) {
        self.placed_skills.retain(|p| p.name != name);
        self.placed_skills.push(PlacedSkill { name: name.to_string(), files });
        self.approved_skills.retain(|a| a.name != name);
    }

    pub fn revoke_skill(&mut self, name: &str) {
        self.approved_skills.retain(|a| a.name != name);
    }

    /// The pairing is over: the link, the agent's programs and skills and their approvals,
    /// and the folders picked for it, go (a new pairing is a new agent, which gets no folder
    /// the person did not pick for it). The saved cloud address and the pause switch stay.
    pub fn forget_link(&mut self) {
        self.link = None;
        self.programs.clear();
        self.approved.clear();
        self.skills.clear();
        self.approved_skills.clear();
        self.placed_skills.clear();
        self.folders.clear();
        self.any_program = false;
        self.screen_control = false;
        self.dev_access = false;
        self.android_dev = false;
        self.ios_dev = false;
    }
}

/// The state and its file.
pub struct Store {
    path: PathBuf,
    state: Mutex<State>,
}

impl Store {
    /// The state kept in `dir`; a fresh one when there is no file, or it cannot be read
    /// (a damaged file approves nothing).
    pub fn open(dir: &Path) -> Store {
        let path = dir.join(STATE_FILE);
        let state = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "state_unreadable");
                State::default()
            }),
            Err(_) => State::default(),
        };
        Store { path, state: Mutex::new(state) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Change the state and, when that changed anything, write it. A write that fails is
    /// logged; the change stands for this launch.
    pub fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.state.lock().unwrap();
        let before = state.clone();
        let out = change(&mut state);
        if *state != before {
            if let Err(e) = write(&self.path, &state) {
                tracing::warn!(error = %e, "state_not_saved");
            }
        }
        out
    }
}

/// Write the file whole: to a temporary file beside it, then a rename over it.
fn write(path: &Path, state: &State) -> std::io::Result<()> {
    // The file is always `<state folder>/state.json`, so it has a folder.
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("")))?;
    let part = path.with_extension("json.part");
    std::fs::write(&part, serde_json::to_vec_pretty(state).expect("the state is plain JSON"))?;
    std::fs::rename(&part, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn say(description: &str, command: &str, args: Value) -> Value {
        json!({"name": "say", "description": description, "command": command, "args": args})
    }

    fn tool(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "x").unwrap();
        p
    }

    #[test]
    fn a_program_waits_until_allowed_and_again_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let exe = tool(dir.path(), "speak");
        let path = std::env::join_paths([dir.path()]).unwrap();
        let args = json!([{"value": "-v"}, {"param": "words", "description": "What to say"}]);
        let mut s = State::default();
        s.sync(&[say("Say it", "speak", args.clone())]);
        let p = s.program("say").unwrap();
        assert_eq!(s.standing(&p), Standing::Waiting);
        assert!(s.runnable("say").is_none() && s.allowed_names().is_empty());

        s.allow("say", Some(&path)).unwrap();
        assert_eq!(s.standing(&p), Standing::Approved);
        assert_eq!(s.runnable("say").unwrap().path, exe, "the resolved absolute path is kept");
        assert_eq!(s.allowed_names(), vec!["say"]);

        // The description is not part of what was allowed — nor is a blank's.
        s.sync(&[say("Say it aloud", "speak", json!([{"value": "-v"}, {"param": "words", "description": "Other"}]))]);
        assert_eq!(s.standing(&s.program("say").unwrap()), Standing::Approved);

        // The command, a fixed value, a blank's name or the order: waiting again.
        for changed in [
            say("Say it", "/bin/rm", args.clone()),
            say("Say it", "speak", json!([{"value": "-x"}, {"param": "words"}])),
            say("Say it", "speak", json!([{"value": "-v"}, {"param": "other"}])),
            say("Say it", "speak", json!([{"param": "words"}, {"value": "-v"}])),
            say("Say it", "speak", json!([{"value": "-v"}, {"param": "words"}, {"value": "more"}])),
        ] {
            s.sync(&[changed]);
            assert_eq!(s.standing(&s.program("say").unwrap()), Standing::Changed);
            assert!(s.runnable("say").is_none() && s.allowed_names().is_empty());
        }
        // Allowed again as it is now.
        s.allow("say", Some(&path)).unwrap();
        assert_eq!(s.approved.len(), 1);
        assert_eq!(s.runnable("say").unwrap().args, vec![Part::Value("-v".into()), Part::Param("words".into()), Part::Value("more".into())]);

        // The program moved away: allowed, but it cannot run until allowed again.
        std::fs::remove_file(&exe).unwrap();
        assert_eq!(s.standing(&s.program("say").unwrap()), Standing::MissingProgram);
        assert_eq!(s.allow("say", Some(&path)).unwrap_err(), "There is no program 'speak' on this Mac.");
        assert_eq!(s.allow("nope", Some(&path)).unwrap_err(), "There is no program 'nope' to allow.");

        s.revoke("say");
        assert_eq!(s.standing(&s.program("say").unwrap()), Standing::Waiting);
    }

    #[test]
    fn a_program_gone_from_the_clouds_list_loses_its_approval() {
        let dir = tempfile::tempdir().unwrap();
        tool(dir.path(), "speak");
        let path = std::env::join_paths([dir.path()]).unwrap();
        let mut s = State::default();
        let other = json!({"name": "other", "description": "d", "command": "speak"});
        s.sync(&[say("d", "speak", json!([])), other.clone()]);
        s.allow("say", Some(&path)).unwrap();
        s.allow("other", Some(&path)).unwrap();
        s.sync(std::slice::from_ref(&other));
        assert_eq!(s.allowed_names(), vec!["other"]);
        // Back in the list: it waits, as if new.
        s.sync(&[say("d", "speak", json!([])), other]);
        assert_eq!(s.standing(&s.program("say").unwrap()), Standing::Waiting);
        // An entry that cannot be read is no program, and keeps no approval.
        s.sync(&[json!({"name": "other"})]);
        assert!(s.approved.is_empty() && s.programs().is_empty());
        // At most twenty are kept.
        let many: Vec<Value> = (0..30).map(|i| json!({"name": format!("p{i}"), "description": "d", "command": "speak"})).collect();
        s.sync(&many);
        assert_eq!(s.programs.len(), 20);
    }

    #[test]
    fn the_state_is_kept_in_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("nested").join("Wysebee Agent");
        let exe = tool(dir.path(), "speak");
        let store = Store::open(&folder);
        assert_eq!(store.get(), State::default());
        assert_eq!(store.path(), folder.join(STATE_FILE));
        store.update(|s| {
            s.cloud_url = Some("https://cloud.example".into());
            s.paused = true;
            s.link = Some(Link { id: "abc".into(), cloud_url: "https://cloud.example".into(), name: "Mac helper".into(), code: None, expires_at: None, agent: Some(AgentName { name: "mac-helper".into(), label: "Mac helper".into() }) });
            s.sync(&[say("d", exe.to_str().unwrap(), json!([{"param": "words"}]))]);
            s.allow("say", None).unwrap();
        });
        let again = Store::open(&folder);
        assert_eq!(again.get(), store.get());
        assert_eq!(again.get().runnable("say").unwrap().path, exe);
        assert!(!folder.join("state.json.part").exists(), "written beside it, then renamed over it");
        let text = std::fs::read_to_string(folder.join(STATE_FILE)).unwrap();
        assert!(text.contains("\"approved\"") && !text.contains("secret"));

        again.update(|s| s.forget_link());
        let after = Store::open(&folder).get();
        assert!(after.link.is_none() && after.programs.is_empty() && after.approved.is_empty());
        assert!(after.paused && after.cloud_url.is_some(), "the address and the switch stay");

        // A damaged file approves nothing.
        std::fs::write(folder.join(STATE_FILE), "{not json").unwrap();
        assert_eq!(Store::open(&folder).get(), State::default());
        // An older file with fewer fields still reads.
        std::fs::write(folder.join(STATE_FILE), "{}").unwrap();
        assert_eq!(Store::open(&folder).get(), State::default());
    }

    #[test]
    fn a_state_that_cannot_be_written_still_stands_for_this_launch() {
        let dir = tempfile::tempdir().unwrap();
        // The folder's place is taken by a file, so nothing can be written under it.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "x").unwrap();
        let store = Store::open(&blocked);
        store.update(|s| s.paused = true);
        assert!(store.get().paused);
        assert_eq!(default_dir(Path::new("/Users/x")), Path::new("/Users/x/Library/Application Support/Wysebee Agent"));
    }
}

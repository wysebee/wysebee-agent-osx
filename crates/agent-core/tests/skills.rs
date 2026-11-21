//! A skill's scripts, through the link (the agent-link protocol, "Skill scripts"): the skills arrive
//! in the poll, wait for the person, are downloaded and checked on Allow, and only their
//! declared scripts run, from the approved copy.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_core::agent::PAUSED;
use agent_core::approvals::Standing;
use agent_core::{Agent, Step, Timing};
use common::{call, fake, gone, pair, skill_md, stored, until, Bench, Fake};
use serde_json::{json, Value};

/// The skill `name` with these files, as the cloud lists it.
fn skill(fake: &Fake, name: &str, files: &[(&str, &[u8])]) -> Value {
    json!({"name": name, "files": files.iter().map(|(p, b)| fake.file(p, b)).collect::<Vec<_>>()})
}

fn status_of(agent: &Agent, name: &str) -> agent_core::agent::SkillStatus {
    agent.status().skills.into_iter().find(|s| s.name == name).unwrap()
}

const SHOW: &str = "#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\"; done; echo; pwd\n";

#[tokio::test]
async fn a_skill_waits_then_allow_downloads_checks_and_puts_it_in_place() {
    let (fake, bench) = (fake().await, Bench::new());
    bench.script("echoargs", r#"for a in "$@"; do printf '<%s>' "$a"; done; echo"#);
    let agent = bench.agent(&fake);
    let looks = Arc::new(AtomicUsize::new(0));
    let counted = looks.clone();
    agent.set_attention_hook(Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    })));
    let md = skill_md("weather", &[("show", "./run.sh fixed"), ("echo", "echoargs --flag 'two words'")]);
    let weather = skill(&fake, "weather", &[("SKILL.md", md.as_bytes()), ("run.sh", SHOW.as_bytes()), ("data/cities.txt", b"Paris\n")]);
    fake.set(|c| c.skills = Some(vec![weather]));
    pair(&agent, &fake).await;

    // It waits; every file was downloaded and kept on this Mac at once (2026-11-13), checked,
    // in the cache — not put in place, nothing runnable; the person is asked to look once.
    let shown = status_of(&agent, "weather");
    assert_eq!(shown.status, Standing::Waiting);
    let scripts: Vec<(String, String)> = shown.scripts.unwrap().into_iter().map(|s| (s.name, s.command)).collect();
    assert_eq!(scripts, vec![("show".into(), "./run.sh fixed".into()), ("echo".into(), "echoargs --flag 'two words'".into())]);
    assert_eq!(shown.changes.iter().map(|c| (c.path.as_str(), c.change)).collect::<Vec<_>>(), vec![("SKILL.md", "added"), ("data/cities.txt", "added"), ("run.sh", "added")]);
    assert_eq!(fake.get(|c| c.file_gets.len()), 3);
    assert_eq!(fake.get(|c| c.file_gets[0].clone()), fake.get(|c| c.skills.as_ref().unwrap()[0]["files"][0]["id"].as_str().unwrap().to_string()), "SKILL.md first");
    assert!(shown.stored && shown.download_error.is_none() && !agent.status().storing);
    assert_eq!((agent.status().waiting, looks.load(Ordering::SeqCst)), (1, 1));
    assert!(!bench.state_dir().join("skills").exists(), "nothing is put in place before Allow");
    let cache = bench.state_dir().join("skill-cache");
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 3);
    assert!(std::fs::read_dir(&cache).unwrap().flatten().all(|f| f.metadata().unwrap().permissions().mode() & 0o111 == 0), "nothing in the cache is executable");
    // Polled again: nothing downloaded again.
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    assert_eq!(fake.get(|c| c.file_gets.len()), 3);
    let refused = call(&agent, &fake, "c1", "skill_run", json!({"skill": "weather", "script": "show"})).await;
    assert_eq!(refused, json!({"ok": false, "error": "The skill 'weather' is waiting for approval on the Mac. The person can allow it in Wysebee Agent, on that Mac."}));
    assert_eq!(fake.last_status()["approved_skills"], json!([]));
    assert_eq!(looks.load(Ordering::SeqCst), 1, "not asked again for the same skill");

    // Allowed: from what is kept here, nothing downloaded again — checked again, put in place,
    // the folder's own program runnable.
    agent.allow_skill("weather").await.unwrap();
    assert_eq!(fake.get(|c| c.file_gets.len()), 3);
    let folder = bench.state_dir().join("skills/weather");
    assert_eq!(std::fs::read_to_string(folder.join("data/cities.txt")).unwrap(), "Paris\n");
    assert_eq!(std::fs::metadata(folder.join("run.sh")).unwrap().permissions().mode() & 0o777, 0o755);
    assert_eq!(std::fs::metadata(folder.join("SKILL.md")).unwrap().permissions().mode() & 0o777, 0o644);
    let shown = status_of(&agent, "weather");
    assert!(shown.status == Standing::Approved && shown.changes.is_empty() && shown.folder == folder.display().to_string());
    assert_eq!(agent.status().waiting, 0);
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    assert_eq!(fake.last_status()["approved_skills"], json!(["weather"]));

    // A script runs: its declared words, then each argument as one, never a shell, in the
    // skill's folder.
    let nasty = "a b; rm -rf / $(x) `y` | z > /tmp/w && \"q\"";
    let ran = call(&agent, &fake, "c2", "skill_run", json!({"skill": "weather", "script": "show", "args": [nasty, 2, "$HOME"]})).await;
    let real = folder.canonicalize().unwrap();
    assert_eq!(ran, json!({"ok": true, "result": {"exit_code": 0, "stdout": format!("[fixed][{nasty}][2][$HOME]\n{}\n", real.display()), "stderr": ""}}));
    // A program on the PATH, with quoted words kept whole.
    let ran = call(&agent, &fake, "c3", "skill_run", json!({"skill": "weather", "script": "echo", "args": null})).await;
    assert_eq!(ran["result"]["stdout"], "<--flag><two words>\n");

    // What is not declared, or not the agent's, runs nothing.
    let no_script = call(&agent, &fake, "c4", "skill_run", json!({"skill": "weather", "script": "rm", "args": ["-rf", "/"]})).await;
    assert_eq!(no_script, json!({"ok": false, "error": "The skill 'weather' has no script 'rm'."}));
    let no_skill = call(&agent, &fake, "c5", "skill_run", json!({"skill": "other", "script": "show"})).await;
    assert_eq!(no_skill["error"], "This agent has no skill 'other'.");
    let too_many = call(&agent, &fake, "c6", "skill_run", json!({"skill": "weather", "script": "show", "args": vec!["x"; 31]})).await;
    assert_eq!(too_many["error"], "'args' is a list of at most 30 arguments.");
    let not_list = call(&agent, &fake, "c7", "skill_run", json!({"skill": "weather", "script": "show", "args": "a b"})).await;
    assert_eq!(not_list["error"], "'args' is a list of at most 30 arguments.");
    let shapeless = call(&agent, &fake, "c8", "skill_run", json!("rm -rf /")).await;
    assert_eq!(shapeless["error"], "This agent has no skill ''.");

    // Paused: refused.
    agent.set_paused(true).await;
    let paused = call(&agent, &fake, "c9", "skill_run", json!({"skill": "weather", "script": "show"})).await;
    assert_eq!(paused, json!({"ok": false, "error": PAUSED}));
    agent.set_paused(false).await;

    // Taken back: waiting again, the folder kept.
    agent.revoke_skill("weather");
    assert_eq!(status_of(&agent, "weather").status, Standing::Waiting);
    assert!(folder.join("run.sh").exists());
    let refused = call(&agent, &fake, "c10", "skill_run", json!({"skill": "weather", "script": "show"})).await;
    assert!(refused["error"].as_str().unwrap().contains("waiting for approval"));
    assert_eq!(looks.load(Ordering::SeqCst), 1, "revoked by the person: no need to ask them to look");
}

#[tokio::test]
async fn a_file_that_does_not_match_its_id_is_refused_and_nothing_is_allowed() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let md = skill_md("notes", &[("show", "./run.sh")]);
    let notes = skill(&fake, "notes", &[("SKILL.md", md.as_bytes()), ("run.sh", SHOW.as_bytes())]);
    let run_id = notes["files"][1]["id"].as_str().unwrap().to_string();
    // The cloud serves other bytes under the script's id.
    fake.set(|c| {
        c.files.insert(run_id.clone(), b"#!/bin/sh\nrm -rf ~\n".to_vec());
        c.skills = Some(vec![notes.clone()]);
    });
    pair(&agent, &fake).await;
    let err = agent.allow_skill("notes").await.unwrap_err();
    assert_eq!(err, "'run.sh' did not arrive as Wysebee Cloud listed it: its contents do not match. Nothing was allowed.");
    assert!(agent.state().approved_skills.is_empty() && !bench.state_dir().join("skills").exists());
    assert!(!bench.state_dir().join("skill-cache").join(&run_id).exists(), "a file that does not match is not kept");

    // One the cloud does not have.
    fake.set(|c| {
        c.files.remove(&run_id);
    });
    assert_eq!(agent.allow_skill("notes").await.unwrap_err(), "'run.sh' could not be downloaded: There is no such file.");
    // One larger than a skill's file may be is not read to its end.
    fake.set(|c| {
        c.files.insert(run_id.clone(), vec![b'x'; 5 * 1024 * 1024 + 1]);
    });
    assert_eq!(agent.allow_skill("notes").await.unwrap_err(), "'run.sh' could not be downloaded: The file is larger than a skill's file may be.");
    // Served right: allowed.
    fake.set(|c| {
        c.files.insert(run_id.clone(), SHOW.as_bytes().to_vec());
    });
    agent.allow_skill("notes").await.unwrap();
    assert_eq!(status_of(&agent, "notes").status, Standing::Approved);
}

#[tokio::test]
async fn a_skill_whose_paths_leave_the_folder_is_no_skill() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let md = skill_md("sneaky", &[]);
    for bad in ["../escape.sh", "/etc/evil", "a/../../b"] {
        let entry = json!({"name": "sneaky", "files": [fake.file("SKILL.md", md.as_bytes()), fake.file(bad, b"x")]});
        fake.set(|c| c.skills = Some(vec![entry]));
        if agent.status().state != "paired" {
            pair(&agent, &fake).await;
        } else {
            assert_eq!(agent.step().await, Step::Paired { calls: 0 });
            stored(&agent).await;
    stored(&agent).await;
        }
        assert!(agent.status().skills.is_empty(), "{bad}");
        assert_eq!(agent.allow_skill("sneaky").await.unwrap_err(), "There is no skill 'sneaky' to allow.");
        let refused = call(&agent, &fake, "c1", "skill_run", json!({"skill": "sneaky", "script": "x"})).await;
        assert_eq!(refused["error"], "This agent has no skill 'sneaky'.");
        fake.set(|c| c.results.clear());
    }
    assert!(!bench.dir.path().join("escape.sh").exists() && !bench.state_dir().join("skills").exists());
}

#[tokio::test]
async fn a_changed_skill_waits_and_a_re_allow_keeps_the_persons_own_files() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let looks = Arc::new(AtomicUsize::new(0));
    let counted = looks.clone();
    agent.set_attention_hook(Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    })));
    let md = skill_md("notes", &[("show", "./run.sh")]);
    let first = skill(&fake, "notes", &[("SKILL.md", md.as_bytes()), ("run.sh", SHOW.as_bytes()), ("old.txt", b"old")]);
    fake.set(|c| c.skills = Some(vec![first.clone()]));
    pair(&agent, &fake).await;
    agent.allow_skill("notes").await.unwrap();
    let folder = bench.state_dir().join("skills/notes");

    // The person puts a file of their own in the folder (Show in Finder).
    assert_eq!(agent.skill_folder("notes").unwrap(), folder);
    let secret = "API_KEY=only-on-this-mac-1234";
    std::fs::write(folder.join(".env"), secret).unwrap();

    // A file changes in Wysebee: changed, waiting, not runnable; the approved copy is left
    // as it was until the person allows it again.
    let second = skill(&fake, "notes", &[("SKILL.md", md.as_bytes()), ("run.sh", b"#!/bin/sh\necho changed\n"), ("new.txt", b"new")]);
    fake.set(|c| c.skills = Some(vec![second.clone()]));
    let refused = call(&agent, &fake, "c1", "skill_run", json!({"skill": "notes", "script": "show"})).await;
    assert!(refused["error"].as_str().unwrap().contains("waiting for approval"), "{refused}");
    let shown = status_of(&agent, "notes");
    assert_eq!(shown.status, Standing::Changed);
    assert_eq!(shown.changes.iter().map(|c| (c.path.as_str(), c.change)).collect::<Vec<_>>(), vec![("new.txt", "added"), ("old.txt", "removed"), ("run.sh", "changed")]);
    assert_eq!(shown.scripts.unwrap()[0].command, "./run.sh", "a changed skill's scripts are read from its new SKILL.md");
    // Pushed from Wysebee, so downloaded and kept here at once (2026-11-13): the new files are
    // on this Mac, checked, and nothing of them runs until the person allows it.
    assert!(shown.stored && shown.download_error.is_none());
    assert_eq!(looks.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read_to_string(folder.join("run.sh")).unwrap(), SHOW);
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    assert_eq!(fake.last_status()["approved_skills"], json!([]));

    // Put back exactly as allowed: allowed again.
    fake.set(|c| c.skills = Some(vec![first.clone()]));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    assert_eq!(status_of(&agent, "notes").status, Standing::Approved);

    // Changed again and allowed: synced files replaced and removed, the person's kept.
    fake.set(|c| c.skills = Some(vec![second]));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    assert!(status_of(&agent, "notes").stored);
    let gets = fake.get(|c| c.file_gets.len());
    agent.allow_skill("notes").await.unwrap();
    assert_eq!(fake.get(|c| c.file_gets.len()), gets, "Allow used what was kept here");
    assert_eq!(std::fs::read_to_string(folder.join("run.sh")).unwrap(), "#!/bin/sh\necho changed\n");
    assert!(!folder.join("old.txt").exists() && folder.join("new.txt").exists());
    assert_eq!(std::fs::read_to_string(folder.join(".env")).unwrap(), secret);
    let ran = call(&agent, &fake, "c2", "skill_run", json!({"skill": "notes", "script": "show"})).await;
    assert_eq!(ran["result"]["stdout"], "changed\n");
    // Never sent: nothing that went to the cloud holds it.
    let sent = fake.get(|c| format!("{:?}{:?}", c.polls, c.results));
    assert!(!sent.contains("only-on-this-mac") && !sent.contains(".env"));

    // No longer listed: its approval is lost, its folder stays; Show in Finder still works.
    fake.set(|c| c.skills = Some(vec![]));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    assert!(agent.state().approved_skills.is_empty() && folder.join(".env").exists());
    assert_eq!(agent.skill_folder("notes").unwrap(), folder);
    assert_eq!(agent.skill_folder("never").unwrap_err(), "There is no skill 'never' here.");
    assert_eq!(agent.skill_folder("../x").unwrap_err(), "There is no skill '../x' here.");
    // An answer without the list changes nothing (an older cloud).
    fake.set(|c| c.skills = None);
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
}

#[tokio::test]
async fn a_scripts_program_must_be_in_the_folder_or_on_path() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let outside = common::script(bench.dir.path(), "outside.sh", "echo outside");
    let escapes = skill_md("escapes", &[("go", "../outside.sh")]);
    let linked = skill_md("linked", &[("go", "./link.sh")]);
    let absent = skill_md("absent", &[("go", "not-installed --x")]);
    let renamed = skill_md("other-name", &[]);
    fake.set(|c| c.skills = Some(vec![]));
    let list = vec![
        skill(&fake, "escapes", &[("SKILL.md", escapes.as_bytes())]),
        skill(&fake, "linked", &[("SKILL.md", linked.as_bytes())]),
        skill(&fake, "absent", &[("SKILL.md", absent.as_bytes())]),
        skill(&fake, "renamed", &[("SKILL.md", renamed.as_bytes())]),
        skill(&fake, "bare", &[("notes.txt", b"no manifest")]),
        skill(&fake, "broken", &[("SKILL.md", b"not front matter")]),
    ];
    fake.set(|c| c.skills = Some(list));
    pair(&agent, &fake).await;

    assert_eq!(agent.allow_skill("escapes").await.unwrap_err(), "Script 'go': '../outside.sh' is not a path inside the skill's folder.");
    // The person's link in the folder that leads outside it.
    std::os::unix::fs::symlink(&outside, agent.skill_folder("linked").unwrap().join("link.sh")).unwrap();
    assert_eq!(agent.allow_skill("linked").await.unwrap_err(), "Script 'go': './link.sh' leads outside the skill's folder.");
    assert_eq!(agent.allow_skill("absent").await.unwrap_err(), "Script 'go': There is no program 'not-installed' on this Mac.");
    assert!(agent.state().approved_skills.is_empty(), "none of them is allowed");
    assert_eq!(status_of(&agent, "escapes").status, Standing::Waiting);
    // Installed since: allowed.
    bench.script("not-installed", "echo here");
    agent.allow_skill("absent").await.unwrap();
    let ran = call(&agent, &fake, "c1", "skill_run", json!({"skill": "absent", "script": "go"})).await;
    assert_eq!(ran["result"]["stdout"], "here\n");
    // Gone again: allowed, but the program is missing.
    std::fs::remove_file(bench.bin().join("not-installed")).unwrap();
    assert_eq!(status_of(&agent, "absent").status, Standing::MissingProgram);
    let gone_now = call(&agent, &fake, "c2", "skill_run", json!({"skill": "absent", "script": "go"})).await;
    assert!(gone_now["error"].as_str().unwrap().starts_with("The program of 'go' is no longer where it was"), "{gone_now}");

    assert_eq!(agent.allow_skill("renamed").await.unwrap_err(), "Its SKILL.md is named 'other-name', but the skill 'renamed': make them the same in Wysebee.");
    assert_eq!(agent.allow_skill("bare").await.unwrap_err(), "The skill 'bare' has no SKILL.md.");
    assert_eq!(status_of(&agent, "bare").note.as_deref(), Some("It has no SKILL.md."));
    assert!(agent.allow_skill("broken").await.unwrap_err().starts_with("SKILL.md starts with a line '---'"));
    assert!(status_of(&agent, "broken").note.unwrap().starts_with("SKILL.md starts with"));
    assert_eq!(agent.allow_skill("nope").await.unwrap_err(), "There is no skill 'nope' to allow.");
}

#[tokio::test]
async fn an_allowed_program_in_the_folder_swapped_for_a_link_outside_is_refused() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let outside = common::script(bench.dir.path(), "outside.sh", "echo outside");
    let md = skill_md("notes", &[("show", "./run.sh")]);
    fake.set(|c| c.skills = Some(vec![]));
    let notes = skill(&fake, "notes", &[("SKILL.md", md.as_bytes()), ("run.sh", SHOW.as_bytes())]);
    fake.set(|c| c.skills = Some(vec![notes]));
    pair(&agent, &fake).await;
    agent.allow_skill("notes").await.unwrap();
    let run = bench.state_dir().join("skills/notes/run.sh");
    std::fs::remove_file(&run).unwrap();
    std::os::unix::fs::symlink(&outside, &run).unwrap();
    let refused = call(&agent, &fake, "c1", "skill_run", json!({"skill": "notes", "script": "show"})).await;
    assert!(refused["error"].as_str().unwrap().starts_with("The program of 'show' is no longer where it was"), "{refused}");
    // A file that is not a program: it cannot be started.
    std::fs::remove_file(&run).unwrap();
    std::fs::write(&run, "not a program").unwrap();
    let refused = call(&agent, &fake, "c2", "skill_run", json!({"skill": "notes", "script": "show"})).await;
    assert!(refused["error"].as_str().unwrap().starts_with("'./run.sh' could not be started: "), "{refused}");
}

#[tokio::test]
async fn a_script_that_does_not_finish_in_time_is_killed() {
    let (fake, bench) = (fake().await, Bench::new());
    let pid_file = bench.dir.path().join("pid");
    let slow = format!("#!/bin/sh\necho $$ > '{}'; exec sleep 30\n", pid_file.display());
    let md = skill_md("slow", &[("wait", "./slow.sh")]);
    let agent = Agent::open(bench.options(Some(&fake.url), Timing { time_limit: Duration::from_millis(400), ..Bench::timing() }));
    let entry = skill(&fake, "slow", &[("SKILL.md", md.as_bytes()), ("slow.sh", slow.as_bytes())]);
    fake.set(|c| c.skills = Some(vec![entry]));
    pair(&agent, &fake).await;
    agent.allow_skill("slow").await.unwrap();
    let late = call(&agent, &fake, "c1", "skill_run", json!({"skill": "slow", "script": "wait"})).await;
    assert_eq!(late, json!({"ok": false, "error": "'wait' did not finish within 0 seconds and was stopped."}));
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    until("the script is killed", || gone(&pid)).await;
}

#[tokio::test]
async fn unpairing_clears_the_skills_and_their_folders() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let md = skill_md("notes", &[("show", "./run.sh")]);
    let notes = skill(&fake, "notes", &[("SKILL.md", md.as_bytes()), ("run.sh", SHOW.as_bytes())]);
    fake.set(|c| c.skills = Some(vec![notes]));
    pair(&agent, &fake).await;
    agent.allow_skill("notes").await.unwrap();
    std::fs::write(bench.state_dir().join("skills/notes/mine.txt"), "x").unwrap();
    // A file the cache holds that no listed skill has is dropped at the next poll.
    std::fs::write(bench.state_dir().join("skill-cache/stale"), "x").unwrap();
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    assert!(!bench.state_dir().join("skill-cache/stale").exists());

    agent.unpair().await;
    let state = agent.state();
    assert!(state.skills.is_empty() && state.approved_skills.is_empty() && state.placed_skills.is_empty());
    assert!(!bench.state_dir().join("skills").exists() && !bench.state_dir().join("skill-cache").exists());
    assert!(agent.status().skills.is_empty());
    // Not paired: nothing to allow.
    assert_eq!(agent.allow_skill("notes").await.unwrap_err(), "There is no skill 'notes' to allow.");
}

#[tokio::test]
async fn a_skill_whose_files_cannot_be_downloaded_stays_unread() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let md = skill_md("notes", &[]);
    let notes = skill(&fake, "notes", &[("SKILL.md", md.as_bytes())]);
    fake.set(|c| c.skills = Some(vec![notes]));
    pair(&agent, &fake).await;
    // A cloud that cannot be reached for a file of a waiting skill only logs it.
    std::fs::remove_dir_all(bench.state_dir().join("skill-cache")).unwrap();
    fake.set(|c| c.files.clear());
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    let shown = status_of(&agent, "notes");
    assert!(shown.scripts.is_none() && shown.note.is_none(), "not read yet");
    assert!(!shown.stored && shown.download_error.as_deref().unwrap().contains("could not be downloaded"), "{shown:?}");
    assert!(agent.allow_skill("notes").await.unwrap_err().contains("could not be downloaded"));
    // Reachable again: kept at the next poll, and the error is gone.
    fake.file("SKILL.md", md.as_bytes());
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    let shown = status_of(&agent, "notes");
    assert!(shown.stored && shown.download_error.is_none() && shown.scripts.is_some());
}

#[tokio::test]
async fn skills_are_downloaded_beside_the_poll_one_run_at_a_time() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let a = skill(&fake, "alpha", &[("SKILL.md", skill_md("alpha", &[]).as_bytes()), ("a.txt", b"a")]);
    fake.set(|c| c.skills = Some(vec![a.clone()]));
    pair(&agent, &fake).await;
    // A slow line: the poll answers while the files are still coming.
    fake.set(|c| c.file_hold = Duration::from_millis(300));
    let b = skill(&fake, "beta", &[("SKILL.md", skill_md("beta", &[]).as_bytes()), ("b.txt", b"b")]);
    fake.set(|c| c.skills = Some(vec![a.clone(), b.clone()]));
    let polled = std::time::Instant::now();
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert!(polled.elapsed() < Duration::from_millis(300) && agent.status().storing, "the poll does not wait for the files");
    assert!(!status_of(&agent, "beta").stored);
    // The list changes meanwhile: looked at again when the run ends, not twice at once.
    let c = skill(&fake, "gamma", &[("SKILL.md", skill_md("gamma", &[]).as_bytes())]);
    fake.set(|c2| c2.skills = Some(vec![a.clone(), b.clone(), c.clone()]));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    stored(&agent).await;
    assert!(status_of(&agent, "beta").stored && status_of(&agent, "gamma").stored);
    // Unpaired while files come: nothing more is kept, and nothing of it stays.
    let d = skill(&fake, "delta", &[("SKILL.md", skill_md("delta", &[]).as_bytes()), ("d1.txt", b"1"), ("d2.txt", b"2")]);
    fake.set(|c2| c2.skills = Some(vec![d]));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    agent.unpair().await;
    stored(&agent).await;
    assert!(!bench.state_dir().join("skill-cache").exists(), "nothing kept after unpairing");
}

#[tokio::test]
async fn a_skill_changed_in_wysebee_while_allow_downloads_it_is_not_allowed() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let md = skill_md("notes", &[("show", "./run.sh")]);
    let first = skill(&fake, "notes", &[("SKILL.md", md.as_bytes()), ("run.sh", SHOW.as_bytes())]);
    fake.set(|c| c.skills = Some(vec![first.clone()]));
    pair(&agent, &fake).await;
    // Its script is no longer kept here, so Allow downloads it, and the cloud holds it.
    let run_id = first["files"][1]["id"].as_str().unwrap().to_string();
    std::fs::remove_file(bench.state_dir().join("skill-cache").join(&run_id)).unwrap();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    fake.set(|c| c.file_gate = Some(gate.clone()));
    let allowing = agent.clone();
    let allow = tokio::spawn(async move { allowing.allow_skill("notes").await });
    until("the script is asked for", || fake.get(|c| c.file_gets.contains(&run_id))).await;

    // Meanwhile the person changes the script in Wysebee, and a poll brings the new list.
    let second = skill(&fake, "notes", &[("SKILL.md", md.as_bytes()), ("run.sh", b"#!/bin/sh\necho changed\n")]);
    fake.set(|c| c.skills = Some(vec![second]));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    gate.add_permits(100);
    let refused = tokio::time::timeout(Duration::from_secs(5), allow).await.unwrap().unwrap();
    assert_eq!(refused.unwrap_err(), "'notes' changed in Wysebee meanwhile. Look at it again, then allow it.");
    assert!(agent.state().approved_skills.is_empty() && !bench.state_dir().join("skills/notes").exists(), "nothing was put in place");
    stored(&agent).await;
    assert_eq!(status_of(&agent, "notes").status, Standing::Waiting);

    // Looked at again and allowed: the new version.
    fake.set(|c| c.file_gate = None);
    agent.allow_skill("notes").await.unwrap();
    let ran = call(&agent, &fake, "c1", "skill_run", json!({"skill": "notes", "script": "show"})).await;
    assert_eq!(ran["result"]["stdout"], "changed\n");
}

#[tokio::test]
async fn unpairing_forgets_all_the_same_when_a_skills_folder_cannot_be_removed() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let md = skill_md("notes", &[("show", "./run.sh")]);
    let notes = skill(&fake, "notes", &[("SKILL.md", md.as_bytes()), ("run.sh", SHOW.as_bytes())]);
    fake.set(|c| c.skills = Some(vec![notes]));
    pair(&agent, &fake).await;
    agent.allow_skill("notes").await.unwrap();
    // The person locked the skill's folder: what is in it cannot be removed.
    let folder = bench.state_dir().join("skills/notes");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();

    agent.unpair().await;
    let left = folder.join("run.sh").exists();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(left, "the locked folder is left as it is");
    // The pairing is forgotten all the same, and the cache (which could be removed) is gone.
    let state = agent.state();
    assert!(state.link.is_none() && state.skills.is_empty() && state.approved_skills.is_empty());
    assert!(bench.secrets.peek().is_none() && agent.status().state == "unpaired");
    assert!(!bench.state_dir().join("skill-cache").exists());
}

#[tokio::test]
async fn a_program_whose_place_cannot_be_made_out_is_refused() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let md = skill_md("notes", &[("show", "./run.sh")]);
    let notes = skill(&fake, "notes", &[("SKILL.md", md.as_bytes()), ("run.sh", SHOW.as_bytes())]);
    fake.set(|c| c.skills = Some(vec![notes]));
    pair(&agent, &fake).await;
    agent.allow_skill("notes").await.unwrap();

    // The program swapped for a link into folders nested so deep that its full path is
    // longer than a path may be: it can be opened, one folder at a time, but where it is
    // cannot be made out, so it is not known to be inside the skill's folder.
    let folder = bench.state_dir().join("skills/notes");
    let long = "d".repeat(200);
    let mut at = folder.clone();
    for _ in 0..6 {
        std::fs::create_dir(at.join(&long)).unwrap();
        std::os::unix::fs::symlink(&long, at.join("n")).unwrap();
        at = at.join("n");
    }
    common::script(&at, "deep.sh", "echo deep");
    let run = folder.join("run.sh");
    std::fs::remove_file(&run).unwrap();
    std::os::unix::fs::symlink("n/n/n/n/n/n/deep.sh", &run).unwrap();
    assert!(run.is_file() && run.canonicalize().is_err());

    let refused = call(&agent, &fake, "c1", "skill_run", json!({"skill": "notes", "script": "show"})).await;
    assert!(refused["error"].as_str().unwrap().starts_with("The program of 'show' is no longer where it was"), "{refused}");
}

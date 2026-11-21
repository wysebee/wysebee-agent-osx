//! Approval on the Mac, through the link (the agent-link protocol, "Approval, on the Mac"): the
//! cloud holds the program definitions, and cannot make this app run anything the person
//! did not allow here.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::approvals::Standing;
use agent_core::{Agent, Step};
use common::{call, fake, pair, program, Bench};
use serde_json::{json, Value};

/// A program that leaves a mark when it runs, so "nothing ran" can be seen.
fn marker(bench: &Bench, name: &str) -> (String, std::path::PathBuf) {
    let mark = bench.dir.path().join(format!("{name}.ran"));
    let exe = bench.script(name, &format!("echo ran >> '{}'; echo \"$@\"", mark.display()));
    (exe, mark)
}

fn standing(agent: &Agent, name: &str) -> Standing {
    agent.status().programs.iter().find(|p| p.name == name).unwrap().status
}

#[tokio::test]
async fn a_program_waits_for_the_person_and_runs_only_as_allowed() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let looks = Arc::new(AtomicUsize::new(0));
    let counted = looks.clone();
    agent.set_attention_hook(Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    })));
    let (exe, mark) = marker(&bench, "greet");
    let args = json!([{"value": "hello"}, {"param": "who", "description": "Who to greet"}]);
    fake.set(|c| c.programs = Some(vec![program("greet", "greet", args.clone())]));
    pair(&agent, &fake).await;

    // New: it waits, the person is asked to look (once), and a call is refused.
    let shown = agent.status().programs[0].clone();
    assert_eq!((shown.status, shown.command_line.as_str(), shown.path.as_deref()), (Standing::Waiting, "greet hello <who>", Some(exe.as_str())));
    assert_eq!((agent.status().waiting, looks.load(Ordering::SeqCst)), (1, 1));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert_eq!(looks.load(Ordering::SeqCst), 1, "not asked again for the same program");
    let refused = call(&agent, &fake, "c1", "local_run", json!({"name": "greet", "values": {"who": "you"}})).await;
    assert_eq!(refused, json!({"ok": false, "error": "'greet' is waiting for approval on the Mac. The person can allow it in Wysebee Agent, on that Mac."}));
    assert!(!mark.exists(), "nothing ran");
    assert_eq!(fake.last_status()["approved"], json!([]));

    // Allowed: the cloud is told, and it runs.
    agent.allow("greet").unwrap();
    assert_eq!(standing(&agent, "greet"), Standing::Approved);
    let ran = call(&agent, &fake, "c2", "local_run", json!({"name": "greet", "values": {"who": "you"}})).await;
    assert_eq!(ran, json!({"ok": true, "result": {"exit_code": 0, "stdout": "hello you\n", "stderr": ""}}));
    assert_eq!(fake.last_status()["approved"], json!(["greet"]));
    assert!(bench.state_text().contains(&exe), "the approved copy keeps the resolved path");

    // Its description changes in Wysebee: still allowed.
    fake.set(|c| c.programs = Some(vec![json!({"name": "greet", "description": "Greets, now described better", "command": "greet", "args": [{"value": "hello"}, {"param": "who", "description": "Anyone"}]})]));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert_eq!((standing(&agent, "greet"), looks.load(Ordering::SeqCst)), (Standing::Approved, 1));

    // Its fixed argument changes: waiting again, the person is asked to look, a call is
    // refused and nothing runs.
    std::fs::remove_file(&mark).unwrap();
    fake.set(|c| c.programs = Some(vec![program("greet", "greet", json!([{"value": "goodbye"}, {"param": "who"}]))]));
    let refused = call(&agent, &fake, "c3", "local_run", json!({"name": "greet", "values": {"who": "you"}})).await;
    assert_eq!(refused["ok"], false);
    assert!(refused["error"].as_str().unwrap().contains("waiting for approval"));
    assert!(!mark.exists());
    assert_eq!((standing(&agent, "greet"), agent.status().waiting, looks.load(Ordering::SeqCst)), (Standing::Changed, 1, 2));
    // The next poll no longer reports it as allowed.
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert_eq!(fake.last_status()["approved"], json!([]));

    // Its command changes to another program: the same.
    let (_, other_mark) = marker(&bench, "other");
    fake.set(|c| c.programs = Some(vec![program("greet", "other", args.clone())]));
    let refused = call(&agent, &fake, "c4", "local_run", json!({"name": "greet", "values": {"who": "you"}})).await;
    assert!(refused["error"].as_str().unwrap().contains("waiting for approval"));
    assert!(!mark.exists() && !other_mark.exists());
    assert_eq!(looks.load(Ordering::SeqCst), 3);

    // Back to exactly what the person allowed: allowed still.
    fake.set(|c| c.programs = Some(vec![program("greet", "greet", args.clone())]));
    let ran = call(&agent, &fake, "c5", "local_run", json!({"name": "greet", "values": {"who": "again"}})).await;
    assert_eq!(ran["result"]["stdout"], "hello again\n");

    // Taken back by the person.
    agent.revoke("greet");
    assert_eq!(standing(&agent, "greet"), Standing::Waiting);
    let refused = call(&agent, &fake, "c6", "local_run", json!({"name": "greet", "values": {"who": "you"}})).await;
    assert!(refused["error"].as_str().unwrap().contains("waiting for approval"));
    assert_eq!(looks.load(Ordering::SeqCst), 3, "the person revoked it: no need to ask them to look");
}

#[tokio::test]
async fn a_program_removed_in_wysebee_loses_its_approval() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let (_, mark) = marker(&bench, "greet");
    let greet = program("greet", "greet", json!([]));
    fake.set(|c| c.programs = Some(vec![greet.clone()]));
    pair(&agent, &fake).await;
    agent.allow("greet").unwrap();

    // An answer without the list changes nothing (an older cloud).
    fake.set(|c| c.programs = None);
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert_eq!(standing(&agent, "greet"), Standing::Approved);

    fake.set(|c| c.programs = Some(vec![]));
    let gone = call(&agent, &fake, "c1", "local_run", json!({"name": "greet", "values": {}})).await;
    assert_eq!(gone, json!({"ok": false, "error": "This agent has no program 'greet'."}));
    assert!(agent.state().approved.is_empty() && agent.status().programs.is_empty());

    // Back in the list: it waits like a new one.
    fake.set(|c| c.programs = Some(vec![greet]));
    let waits = call(&agent, &fake, "c2", "local_run", json!({"name": "greet", "values": {}})).await;
    assert!(waits["error"].as_str().unwrap().contains("waiting for approval"));
    assert!(!mark.exists());
}

#[tokio::test]
async fn the_cloud_cannot_inject_a_command() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let (exe, mark) = marker(&bench, "greet");
    let (evil, evil_mark) = marker(&bench, "evil");
    fake.set(|c| c.programs = Some(vec![program("greet", "greet", json!([{"param": "who"}]))]));
    pair(&agent, &fake).await;
    agent.allow("greet").unwrap();

    // A call carries a name and values; a command, arguments or a path beside them are
    // not looked at. What runs is the approved copy.
    let ran = call(&agent, &fake, "c1", "local_run", json!({"name": "greet", "values": {"who": "you"}, "command": evil, "args": [{"value": "x"}], "path": evil, "program": {"command": evil}})).await;
    assert_eq!(ran["result"]["stdout"], "you\n");
    assert!(mark.exists() && !evil_mark.exists());

    // A name that is a path, or that names nothing, runs nothing.
    for name in [evil.as_str(), "evil", "../bin/evil", ""] {
        let refused = call(&agent, &fake, "c2", "local_run", json!({"name": name, "values": {}})).await;
        assert_eq!(refused["ok"], false, "{name}");
        fake.set(|c| c.results.clear());
    }
    let refused = call(&agent, &fake, "c3", "local_run", json!("not an object")).await;
    assert_eq!(refused["error"], "This agent has no program ''.");

    // A second program of the same name in the list is not the one that runs, and a
    // new program of the cloud's waits whatever it says of itself.
    fake.set(|c| {
        c.programs = Some(vec![
            program("greet", "greet", json!([{"param": "who"}])),
            program("greet", "evil", json!([])),
            json!({"name": "sneaky", "description": "d", "command": evil, "args": [], "approved": true, "path": evil, "status": "approved"}),
        ])
    });
    let ran = call(&agent, &fake, "c4", "local_run", json!({"name": "greet", "values": {"who": "me"}})).await;
    assert_eq!(ran["result"]["stdout"], "me\n");
    let refused = call(&agent, &fake, "c5", "local_run", json!({"name": "sneaky", "values": {}})).await;
    assert!(refused["error"].as_str().unwrap().contains("waiting for approval"));
    assert!(!evil_mark.exists(), "nothing the person did not allow ever ran");

    // The approved copy's path is what runs: another program of that name put earlier on
    // the PATH since is not picked up.
    let state: Value = serde_json::from_str(&bench.state_text()).unwrap();
    assert_eq!(state["approved"], json!([{"name": "greet", "command": "greet", "args": [{"param": "who"}], "path": exe}]));

    // Any other op is refused.
    for op in ["shell", "browser_navigate", "local_exec", ""] {
        let refused = call(&agent, &fake, "c6", op, json!({"command": "rm -rf /"})).await;
        assert_eq!(refused, json!({"ok": false, "error": format!("Wysebee Agent for Mac cannot do '{op}'.")}));
        fake.set(|c| c.results.clear());
    }
    assert_eq!(call(&agent, &fake, "c7", "ping", json!({})).await, json!({"ok": true, "result": {"pong": true}}));
}

#[tokio::test]
async fn allowing_resolves_the_program_then_and_a_missing_one_cannot_be_allowed() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    fake.set(|c| c.programs = Some(vec![program("later", "not-here-yet", json!([])), program("full", "/nonexistent/wysebee/tool", json!([]))]));
    pair(&agent, &fake).await;
    let shown = agent.status().programs;
    assert!(shown.iter().all(|p| p.status == Standing::Waiting && p.path.is_none()));
    assert_eq!(agent.allow("later").unwrap_err(), "There is no program 'not-here-yet' on this Mac.");
    assert_eq!(agent.allow("full").unwrap_err(), "There is no program '/nonexistent/wysebee/tool' on this Mac.");
    assert_eq!(agent.allow("nope").unwrap_err(), "There is no program 'nope' to allow.");
    assert!(agent.state().approved.is_empty());

    // Installed since: allowed, by the path it is at now.
    let exe = bench.script("not-here-yet", "echo here");
    agent.allow("later").unwrap();
    assert_eq!(agent.status().programs[0].path.as_deref(), Some(exe.as_str()));
    let ran = call(&agent, &fake, "c1", "local_run", json!({"name": "later"})).await;
    assert_eq!(ran["result"]["stdout"], "here\n");

    // Gone again: allowed, but there is nothing to run until it is allowed again.
    std::fs::remove_file(&exe).unwrap();
    assert_eq!(agent.status().programs[0].status, Standing::MissingProgram);
    let gone = call(&agent, &fake, "c2", "local_run", json!({"name": "later"})).await;
    assert_eq!(gone["ok"], false);
    assert!(gone["error"].as_str().unwrap().starts_with("The program 'not-here-yet' is no longer on the Mac"), "{gone}");
}

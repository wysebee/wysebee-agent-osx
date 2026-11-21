//! Local folders, through the link (the agent-link protocol, "Local folders"): picked here, reported
//! in the poll, and used by `folder_call` only as the person allowed.

mod common;

use agent_core::agent::PAUSED;
use agent_core::{Agent, Step};
use base64::Engine as _;
use common::{call, fake, pair, until, Bench};
use serde_json::{json, Value};

/// A paired agent and two folders: `docs` (read only) and `out` (read and write). Their ids.
async fn ready(bench: &Bench, fake: &common::Fake) -> (Agent, String, String) {
    let agent = bench.agent(fake);
    pair(&agent, fake).await;
    let (docs, out) = (bench.dir.path().join("docs"), bench.dir.path().join("out"));
    std::fs::create_dir_all(docs.join("reports")).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(docs.join("notes.txt"), "Buy milk").unwrap();
    std::fs::write(docs.join("reports/q3 notes.md"), "# Q3").unwrap();
    std::fs::write(docs.join("photo.jpg"), b"\xff\xd8\xff\xe0 a picture").unwrap();
    let d = agent.add_folder(docs.to_str().unwrap(), "", "read", "My documents").unwrap();
    let o = agent.add_folder(out.to_str().unwrap(), "Drafts", "write", "").unwrap();
    (agent, d, o)
}

fn folder_call(tool: &str, folder: Option<&str>, args: Value) -> Value {
    json!({"tool": tool, "folder": folder, "args": args})
}

#[tokio::test]
async fn the_folders_are_reported_in_the_poll_and_kept() {
    let (fake, bench) = (fake().await, Bench::new());
    // Unpaired: a folder cannot be picked.
    let lone = bench.agent(&fake);
    assert_eq!(lone.add_folder(bench.home().to_str().unwrap(), "", "read", "").unwrap_err(), "This Mac is not paired.");
    drop(lone);
    let (agent, d, o) = ready(&bench, &fake).await;
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    let s = agent.status();
    assert_eq!(fake.last_status()["folders"], json!([
        {"id": d, "name": "docs", "path": s.folders[0].path, "mode": "read", "description": "My documents"},
        {"id": o, "name": "Drafts", "path": s.folders[1].path, "mode": "write", "description": ""},
    ]));
    assert_eq!((s.folders[0].number, s.folders[1].number, s.folders[1].present), (1, 2, true));
    assert!(bench.state_text().contains("\"folders\""), "kept in the state file");

    // Changed and removed: the cloud learns at the next poll.
    agent.update_folder(&o, "Out box", "read", "Finished things").unwrap();
    assert_eq!(agent.update_folder(&o, "two\nlines", "read", "").unwrap_err(), "A folder's name is one line of at most 60 characters.");
    assert!(agent.update_folder(&o, "", "everything", "").unwrap_err().contains("read only"));
    assert!(agent.update_folder(&o, "", "read", &"x".repeat(1001)).unwrap_err().contains("at most 1000"));
    assert_eq!(agent.update_folder("nope", "", "read", "").unwrap_err(), "There is no folder 'nope' here.");
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert_eq!(fake.last_status()["folders"][1], json!({"id": o, "name": "Out box", "path": s.folders[1].path, "mode": "read", "description": "Finished things"}));
    // An empty name is the folder's own again.
    agent.update_folder(&o, " ", "write", "").unwrap();
    assert_eq!(agent.status().folders[1].name, "out");
    agent.remove_folder(&d).unwrap();
    assert_eq!(agent.remove_folder(&d).unwrap_err(), format!("There is no folder '{d}' here."));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert_eq!(fake.last_status()["folders"].as_array().unwrap().len(), 1);
    assert_eq!(agent.status().folders[0].number, 1, "numbered in the order reported");

    // Kept across launches; a folder gone from the disk is shown as not there.
    let again = bench.agent(&fake);
    assert_eq!(again.status().folders, agent.status().folders);
    std::fs::remove_dir_all(&agent.status().folders[0].path).unwrap();
    assert!(!agent.status().folders[0].present);

    // A new pairing is a new agent: unpairing forgets the folders.
    agent.unpair().await;
    assert!(agent.status().folders.is_empty() && agent.state().folders.is_empty());
}

#[tokio::test]
async fn each_folder_tool_answers_through_the_link() {
    let (fake, bench) = (fake().await, Bench::new());
    let (agent, d, o) = ready(&bench, &fake).await;

    let found = call(&agent, &fake, "c1", "folder_call", folder_call("folder_find", None, json!({"query": "notes"}))).await;
    let text: Value = serde_json::from_str(found["result"]["text"].as_str().unwrap()).unwrap();
    let paths: Vec<(&str, i64)> = text["files"].as_array().unwrap().iter().map(|f| (f["path"].as_str().unwrap(), f["folder"].as_i64().unwrap())).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(sorted, [("notes.txt", 1), ("reports/q3 notes.md", 1)]);
    assert_eq!((found["ok"].clone(), text["incomplete"].clone(), found["result"].get("picture")), (json!(true), json!(false), None));

    let listed = call(&agent, &fake, "c2", "folder_call", folder_call("folder_list", Some(&d), json!({"path": "reports"}))).await;
    assert_eq!(listed, json!({"ok": true, "result": {"text": r#"{"entries":[{"name":"q3 notes.md","kind":"file","size":4}],"total":1}"#}}));

    let read = call(&agent, &fake, "c3", "folder_call", folder_call("folder_read", Some(&d), json!({"path": "notes.txt"}))).await;
    assert_eq!(read, json!({"ok": true, "result": {"text": "Buy milk"}}));

    let wrote = call(&agent, &fake, "c4", "folder_call", folder_call("folder_write", Some(&o), json!({"path": "notes/today.md", "content": "Hello"}))).await;
    assert_eq!(wrote, json!({"ok": true, "result": {"text": r#"{"written":"notes/today.md","bytes":5}"#}}));
    assert_eq!(std::fs::read_to_string(bench.dir.path().join("out/notes/today.md")).unwrap(), "Hello");

    // A search in one folder only: the second's files are folder 2.
    let only = call(&agent, &fake, "c5", "folder_call", folder_call("folder_find", Some(&o), json!({"query": "*.md"}))).await;
    assert!(only["result"]["text"].as_str().unwrap().contains(r#""folder":2,"path":"notes/today.md""#), "{only}");

    // A picture comes back for the person, with a note for the model.
    let pic = call(&agent, &fake, "c6", "folder_call", folder_call("folder_read", Some(&d), json!({"path": "photo.jpg"}))).await;
    assert_eq!(pic["result"]["picture"], json!({"media_type": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(b"\xff\xd8\xff\xe0 a picture")}));
    assert_eq!(pic["result"]["text"], "Shown to the person in the chat: photo.jpg. Pictures from local folders are not shown to you.");
}

#[tokio::test]
async fn what_the_person_did_not_allow_is_refused() {
    let (fake, bench) = (fake().await, Bench::new());
    let (agent, d, o) = ready(&bench, &fake).await;
    let refused = |answer: &Value| {
        assert_eq!(answer["ok"], false, "{answer}");
        answer["error"].as_str().unwrap().to_string()
    };
    let mut n = 0;
    let mut ask = |args: Value| {
        n += 1;
        let (agent, fake) = (agent.clone(), &fake);
        async move { call(&agent, fake, &format!("r{n}"), "folder_call", args).await }
    };

    assert_eq!(refused(&ask(folder_call("folder_read", Some("someone-elses"), json!({"path": "notes.txt"}))).await), "This Mac has no folder 'someone-elses': the person may have removed it in Wysebee Agent.");
    for path in ["../out/x", "/etc/hosts"] {
        assert!(refused(&ask(folder_call("folder_read", Some(&d), json!({"path": path}))).await).contains("no full paths and no '..'"), "{path}");
    }
    std::os::unix::fs::symlink(bench.dir.path().join("state"), bench.dir.path().join("docs/escape")).unwrap();
    assert_eq!(refused(&ask(folder_call("folder_read", Some(&d), json!({"path": "escape/state.json"}))).await), "That path leads outside the folder.");
    // Whatever the call says, a read-only folder is not written.
    assert!(refused(&ask(folder_call("folder_write", Some(&d), json!({"path": "x.txt", "content": "x", "mode": "write"}))).await).ends_with("is read only: the person did not let Wysebee write there."));
    assert!(!bench.dir.path().join("docs/x.txt").exists());
    assert_eq!(refused(&ask(folder_call("folder_delete", Some(&o), json!({}))).await), "There is no folder tool 'folder_delete'.");
    assert_eq!(refused(&ask(folder_call("folder_read", None, json!({"path": "notes.txt"}))).await), "Say which folder.");
    assert_eq!(refused(&ask(json!({"tool": "folder_read", "folder": 1, "args": {}})).await), "'folder' is a folder's id, or null.");
    assert_eq!(refused(&ask(json!({"tool": "folder_read", "folder": d, "args": "notes.txt"})).await), "'args' is an object.");
    // No arguments: the folder itself is listed.
    assert_eq!(ask(json!({"tool": "folder_list", "folder": o})).await["ok"], true);

    // A removed folder is no longer reachable.
    agent.remove_folder(&d).unwrap();
    assert!(refused(&ask(folder_call("folder_list", Some(&d), json!({}))).await).contains("This Mac has no folder"));

    // Paused: refused like every op.
    agent.set_paused(true).await;
    assert_eq!(ask(folder_call("folder_list", Some(&o), json!({}))).await, json!({"ok": false, "error": PAUSED}));
    until("nothing runs", || agent.running() == 0).await;
}

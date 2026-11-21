//! Wysebee Agent with no window, for trying the link end to end against a Wysebee
//! Cloud (it is not shipped, and it never touches the Keychain: the secret stays in memory,
//! so a pairing lasts as long as the process).
//!
//!     WYSEBEE_CLOUD_URL=https://cloud.example.com \
//!       cargo run -p agent-core --example headless -- <state dir> "<agent name>"
//!
//! **A test harness, not a way to run the app**: it stands in for the window so the link can
//! be tried end to end. It pairs under that name, prints its status as JSON whenever it
//! changes (the code to type in Wysebee is in it), and runs the link. **The person's Allow
//! click is a file here**: each line of `<state dir>/allow` names a program to allow, or,
//! as `skill:<name>`, a skill (its files downloaded, checked and put in place, as the
//! window's Allow does), read every second. Nothing else is widened: a program or skill not
//! named there stays waiting and is refused.
//!
//! **The person's folder picker is that file too**: a line `folder:<absolute path>` adds that
//! folder read only, `folder-write:<absolute path>` read and write (its name the folder's own,
//! no description), once paired and once per path — as picking it in the window does. A
//! folder not named there is not reachable.

use std::sync::Arc;
use std::time::Duration;

use agent_core::agent::{Agent, Options, Timing};
use agent_core::approvals::Standing;
use agent_core::programs;
use agent_core::secrets::MemorySecrets;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(name)) = (args.next(), args.next()) else {
        eprintln!("usage: headless <state dir> <agent name>");
        std::process::exit(2);
    };
    let dir = std::path::PathBuf::from(dir);
    let agent = Agent::open(Options {
        state_dir: dir.clone(),
        home: std::env::var_os("HOME").map(Into::into).unwrap_or_else(|| dir.clone()),
        path: Some(programs::search_path(std::env::var_os("PATH").as_deref())),
        cloud_url: std::env::var("WYSEBEE_CLOUD_URL").ok(),
        secrets: Arc::new(MemorySecrets::new()),
        mac_name: "Headless Mac".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        timing: Timing::default(),
        // No screen control without the window: its switch is there.
        screen: None,
    });
    if let Err(e) = agent.start_pairing(&name).await {
        eprintln!("could not start pairing: {e}");
        std::process::exit(1);
    }
    let link = agent.clone();
    tokio::spawn(async move { link.run().await });
    let mut last = String::new();
    let mut tried_folders = std::collections::HashSet::new();
    loop {
        for line in std::fs::read_to_string(dir.join("allow")).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()) {
            let folder = match (line.strip_prefix("folder:"), line.strip_prefix("folder-write:")) {
                (Some(path), _) => Some((path.trim(), "read")),
                (_, Some(path)) => Some((path.trim(), "write")),
                _ => None,
            };
            if let Some((path, mode)) = folder {
                if agent.status().state == "paired" && tried_folders.insert(line.to_string()) {
                    match agent.add_folder(path, "", mode, "") {
                        Ok(id) => eprintln!("added folder {path} ({mode}) as {id}"),
                        Err(e) => eprintln!("could not add folder {path}: {e}"),
                    }
                }
                continue;
            }
            if let Some(skill) = line.strip_prefix("skill:") {
                let waiting = agent.status().skills.iter().any(|s| s.name == skill && s.status != Standing::Approved);
                if waiting {
                    match agent.allow_skill(skill).await {
                        Ok(()) => eprintln!("allowed skill {skill}"),
                        Err(e) => eprintln!("could not allow skill {skill}: {e}"),
                    }
                }
                continue;
            }
            let waiting = agent.status().programs.iter().any(|p| p.name == line && p.status != Standing::Approved);
            if waiting {
                match agent.allow(line) {
                    Ok(()) => eprintln!("allowed {line}"),
                    Err(e) => eprintln!("could not allow {line}: {e}"),
                }
            }
        }
        let mut status = serde_json::to_value(agent.status()).unwrap_or_default();
        if let Some(o) = status.as_object_mut() {
            o.remove("expires_in");
        }
        let now = status.to_string();
        if now != last {
            println!("{now}");
            last = now;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

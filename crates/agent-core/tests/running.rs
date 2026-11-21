//! Running a program, through the link (the agent-link protocol, "Running a program"), and the
//! loop that keeps the link up.

mod common;

use std::sync::Arc;
use std::time::Duration;

use agent_core::agent::{listen_for_wake, single_instance, wake_running, PAUSED};
use agent_core::{Agent, Step, Timing};
use common::{call, fake, gone, pair, program, until, Bench, Mode};
use serde_json::json;
use tokio::sync::Semaphore;

/// A paired agent with `programs` in its settings, all allowed.
async fn ready(bench: &Bench, fake: &common::Fake, timing: Timing, programs: Vec<serde_json::Value>) -> Agent {
    let agent = Agent::open(bench.options(Some(&fake.url), timing));
    let names: Vec<String> = programs.iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
    fake.set(|c| c.programs = Some(programs));
    pair(&agent, fake).await;
    for name in names {
        agent.allow(&name).unwrap();
    }
    agent
}

#[tokio::test]
async fn a_program_runs_with_each_value_as_one_argument_and_never_through_a_shell() {
    let (fake, bench) = (fake().await, Bench::new());
    bench.script("show", r#"for a in "$@"; do printf '[%s]' "$a"; done; echo; echo oops >&2; exit 3"#);
    bench.script("where", "pwd; cat; echo done");
    bench.script("loud", "head -c 30000 /dev/zero | tr '\\0' 'a'; head -c 25000 /dev/zero | tr '\\0' 'b' >&2");
    let plain = bench.bin().join("plain");
    std::fs::write(&plain, "not a program").unwrap();
    let agent = ready(
        &bench,
        &fake,
        Bench::timing(),
        vec![
            program("show", "show", json!([{"value": "-n"}, {"param": "what"}, {"value": "$HOME; echo x"}, {"param": "count"}])),
            program("where", "where", json!([])),
            program("loud", "loud", json!([])),
            program("plain", plain.to_str().unwrap(), json!([])),
        ],
    )
    .await;

    // Whatever a value holds, it is one argument, as is; so is a fixed one.
    let nasty = "a b; rm -rf / $(x) `y` | z > /tmp/w && \"q\" 'r' \\ \n*";
    let ran = call(&agent, &fake, "c1", "local_run", json!({"name": "show", "values": {"what": nasty, "count": 2}})).await;
    assert_eq!(ran, json!({"ok": true, "result": {"exit_code": 3, "stdout": format!("[-n][{nasty}][$HOME; echo x][2]\n"), "stderr": "oops\n"}}));

    // A blank left empty by the caller.
    let missing = call(&agent, &fake, "c2", "local_run", json!({"name": "show", "values": {"what": "x"}})).await;
    assert_eq!(missing, json!({"ok": false, "error": "Fill in 'count'."}));
    let none = call(&agent, &fake, "c3", "local_run", json!({"name": "show"})).await;
    assert_eq!(none["error"], "Fill in 'what'.");

    // In the person's home folder, with no input.
    let here = call(&agent, &fake, "c4", "local_run", json!({"name": "where", "values": {}})).await;
    let home = std::fs::canonicalize(bench.home()).unwrap();
    assert_eq!(here["result"]["stdout"], format!("{}\ndone\n", home.display()));

    // Long output and long errors are each cut.
    let cut = call(&agent, &fake, "c5", "local_run", json!({"name": "loud", "values": {}})).await;
    for (stream, letter) in [("stdout", "a"), ("stderr", "b")] {
        let text = cut["result"][stream].as_str().unwrap();
        assert_eq!(text, format!("{}\n…[cut at 20000 characters]", letter.repeat(20_000)), "{stream}");
    }

    // A file that is not a program cannot be started.
    let refused = call(&agent, &fake, "c6", "local_run", json!({"name": "plain", "values": {}})).await;
    assert_eq!(refused["ok"], false);
    assert!(refused["error"].as_str().unwrap().starts_with(&format!("'{}' could not be started: ", plain.display())), "{refused}");
    assert_eq!(agent.running(), 0);
}

#[tokio::test]
async fn a_program_that_does_not_finish_in_time_is_killed() {
    let (fake, bench) = (fake().await, Bench::new());
    let pid_file = bench.dir.path().join("pid");
    bench.script("slow", &format!("echo $$ > '{}'; exec sleep 30", pid_file.display()));
    let timing = Timing { time_limit: Duration::from_millis(400), ..Bench::timing() };
    let agent = ready(&bench, &fake, timing, vec![program("slow", "slow", json!([]))]).await;
    let late = call(&agent, &fake, "c1", "local_run", json!({"name": "slow", "values": {}})).await;
    assert_eq!(late, json!({"ok": false, "error": "'slow' did not finish within 0 seconds and was stopped."}));
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    until("the program is killed", || gone(&pid)).await;
    // The real limit is the protocol's.
    assert_eq!(Timing::default().time_limit, Duration::from_secs(60));
    assert_eq!(Timing::default().wait_secs, 20);
}

#[tokio::test]
async fn a_cancelled_call_is_killed_and_answers_nothing() {
    let (fake, bench) = (fake().await, Bench::new());
    let pid_file = bench.dir.path().join("pid");
    bench.script("slow", &format!("echo $$ > '{}'; exec sleep 30", pid_file.display()));
    let agent = ready(&bench, &fake, Bench::timing(), vec![program("slow", "slow", json!([]))]).await;
    fake.call("c1", "local_run", json!({"name": "slow", "values": {}}));
    assert_eq!(agent.step().await, Step::Paired { calls: 1 });
    until("the program started", || pid_file.exists() && !std::fs::read_to_string(&pid_file).unwrap().trim().is_empty()).await;
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    assert!(!gone(&pid));
    assert_eq!(agent.running(), 1);

    // Cancelling a call nobody runs is nothing; cancelling this one kills it.
    fake.set(|c| c.cancelled = vec!["unknown".into(), "c1".into()]);
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    until("the program is killed", || gone(&pid)).await;
    assert_eq!(agent.running(), 0);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fake.get(|c| c.result_posts), 0, "a cancelled call answers nothing");
}

#[tokio::test]
async fn paused_refuses_and_stops_what_runs() {
    let (fake, bench) = (fake().await, Bench::new());
    let pid_file = bench.dir.path().join("pid");
    bench.script("slow", &format!("echo $$ > '{}'; exec sleep 30", pid_file.display()));
    let agent = ready(&bench, &fake, Bench::timing(), vec![program("slow", "slow", json!([]))]).await;

    // Running when the person turns the switch off: stopped, and answered as paused.
    fake.call("c1", "local_run", json!({"name": "slow", "values": {}}));
    assert_eq!(agent.step().await, Step::Paired { calls: 1 });
    until("the program started", || std::fs::read_to_string(&pid_file).is_ok_and(|t| !t.trim().is_empty())).await;
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    agent.set_paused(true).await;
    assert_eq!(fake.answer("c1").await, json!({"ok": false, "error": PAUSED}));
    until("the program is killed", || gone(&pid)).await;
    assert!(agent.status().paused && agent.running() == 0);

    // While paused: refused, nothing started; the cloud is told; a ping still answers.
    std::fs::remove_file(&pid_file).unwrap();
    let refused = call(&agent, &fake, "c2", "local_run", json!({"name": "slow", "values": {}})).await;
    assert_eq!(refused, json!({"ok": false, "error": PAUSED}));
    assert!(!pid_file.exists());
    assert_eq!(fake.last_status()["paused"], true);
    assert_eq!(call(&agent, &fake, "c3", "ping", json!({})).await["result"]["pong"], true);
    assert!(bench.state_text().contains("\"paused\": true"), "kept across launches");

    // Back on.
    agent.set_paused(false).await;
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert_eq!(fake.last_status()["paused"], false);
}

#[tokio::test]
async fn an_answer_is_sent_again_when_the_cloud_fails_and_dropped_when_the_call_is_gone() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = ready(&bench, &fake, Bench::timing(), vec![]).await;
    // The call is gone there (cancelled, expired): 404, dropped after one try.
    fake.set(|c| c.result_status = 404);
    fake.call("c1", "ping", json!({}));
    assert_eq!(agent.step().await, Step::Paired { calls: 1 });
    until("the call is done", || agent.running() == 0).await;
    assert_eq!(fake.get(|c| c.result_posts), 1);
    // The cloud fails: three tries, then given up.
    fake.set(|c| c.result_status = 500);
    fake.call("c2", "ping", json!({}));
    assert_eq!(agent.step().await, Step::Paired { calls: 1 });
    until("the call is given up", || agent.running() == 0).await;
    assert_eq!(fake.get(|c| c.result_posts), 4);
    // A call whose id could not go into an address is not taken at all.
    fake.set(|c| c.result_status = 204);
    fake.call("../../start", "ping", json!({}));
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
}

#[tokio::test]
async fn a_poll_that_fails_is_a_failure_and_the_pairing_stays() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    pair(&agent, &fake).await;
    fake.set(|c| c.mode = Mode::Down);
    assert_eq!(agent.step().await, Step::Failed("Wysebee Cloud answered 500.".into()));
    assert!(!agent.status().connected && agent.status().state == "paired");
    fake.set(|c| c.mode = Mode::Garbage);
    assert_eq!(agent.step().await, Step::Failed("Wysebee Cloud's answer could not be read.".into()));
    fake.set(|c| c.mode = Mode::Paired);
    assert_eq!(agent.step().await, Step::Paired { calls: 0 });
    assert!(agent.status().connected);
    assert!(bench.secrets.peek().is_some());
}

#[tokio::test]
async fn the_loop_waits_for_a_pairing_runs_calls_and_backs_off_while_the_cloud_is_down() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    let looping = agent.clone();
    let task = tokio::spawn(async move { looping.run().await });

    // Idle until a pairing starts.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fake.get(|c| c.polls.len()), 0);
    agent.start_pairing("Mac helper").await.unwrap();
    until("the loop polls", || fake.get(|c| c.polls.len()) > 0).await;
    fake.set(|c| c.mode = Mode::Paired);
    until("paired", || agent.status().state == "paired").await;

    // A call is run and answered without anyone stepping.
    fake.call("c1", "ping", json!({}));
    assert_eq!(fake.answer("c1").await["result"]["pong"], true);

    // The cloud goes down: polls go on, further and further apart, and it recovers.
    fake.set(|c| {
        c.mode = Mode::Down;
        c.polls.clear();
    });
    until("it keeps trying", || fake.get(|c| c.polls.len()) >= 5).await;
    assert!(!agent.status().connected);
    let times: Vec<_> = fake.get(|c| c.polls.iter().map(|(at, _)| *at).collect());
    let gap = |i: usize| times[i + 1].duration_since(times[i]);
    assert!(gap(0) >= Duration::from_millis(10) && gap(1) >= Duration::from_millis(20) && gap(2) >= Duration::from_millis(30) && gap(3) >= Duration::from_millis(40), "{:?}", (gap(0), gap(1), gap(2), gap(3)));
    fake.set(|c| c.mode = Mode::Paired);
    fake.call("c2", "ping", json!({}));
    assert_eq!(fake.answer("c2").await["ok"], true);
    assert!(agent.status().connected);

    // Unpaired in Wysebee: the loop forgets and goes idle.
    fake.set(|c| c.mode = Mode::Unpaired);
    until("unpaired", || agent.status().state == "unpaired").await;
    let polls = fake.get(|c| c.polls.len());
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(fake.get(|c| c.polls.len()), polls, "nothing is asked while unpaired");

    // The other endings leave the loop running too.
    fake.set(|c| c.mode = Mode::Refused("Taken.".into()));
    agent.start_pairing("Taken").await.unwrap();
    until("refused", || agent.status().state == "refused").await;
    fake.set(|c| c.mode = Mode::Expired);
    agent.start_pairing("Slow").await.unwrap();
    until("expired", || agent.status().note.is_some()).await;
    task.abort();

    let t = Timing::default();
    assert_eq!([t.backoff_after(0), t.backoff_after(1), t.backoff_after(2), t.backoff_after(3), t.backoff_after(99)].map(|d| d.as_secs()), [2, 5, 15, 30, 30]);
}

#[tokio::test]
async fn a_change_of_status_cuts_the_poll_short_so_the_cloud_learns_at_once() {
    let (fake, bench) = (fake().await, Bench::new());
    bench.script("hello", "echo hi");
    fake.set(|c| c.programs = Some(vec![program("hello", "hello", json!([]))]));
    let agent = Agent::open(bench.options(Some(&fake.url), Timing { wait_secs: 20, ..Bench::timing() }));
    pair(&agent, &fake).await;

    // A long poll is out…
    fake.set(|c| c.hold = Duration::from_secs(30));
    let polls = fake.get(|c| c.polls.len());
    let stepping = agent.clone();
    let step = tokio::spawn(async move { stepping.step().await });
    until("the poll is out", || fake.get(|c| c.polls.len()) == polls + 1).await;
    assert_eq!(fake.last_status()["approved"], json!([]));
    // …the person allows a program: it is asked again at once, saying so.
    fake.set(|c| c.hold = Duration::ZERO);
    agent.allow("hello").unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), step).await.unwrap().unwrap(), Step::Paired { calls: 0 });
    assert_eq!(fake.last_status()["approved"], json!(["hello"]));

    // Unpaired here while a poll is out: the turn ends idle.
    fake.set(|c| c.hold = Duration::from_secs(30));
    let polls = fake.get(|c| c.polls.len());
    let stepping = agent.clone();
    let step = tokio::spawn(async move { stepping.step().await });
    until("the poll is out", || fake.get(|c| c.polls.len()) == polls + 1).await;
    agent.unpair().await;
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), step).await.unwrap().unwrap(), Step::Idle);
}

#[test]
fn one_instance_at_a_time_holds_a_state_folder() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("state");
    let first = single_instance(&folder).expect("the first instance gets it");
    assert!(single_instance(&folder).is_none(), "a second does not");
    drop(first);
    assert!(single_instance(&folder).is_some());
    // A folder that cannot be made: no lock.
    let blocked = dir.path().join("file");
    std::fs::write(&blocked, "x").unwrap();
    assert!(single_instance(&blocked.join("sub")).is_none());
}

#[test]
fn a_second_launch_wakes_the_running_one() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let woken = Arc::new(AtomicUsize::new(0));
    let counter = woken.clone();
    listen_for_wake(dir.path(), move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    assert!(wake_running(dir.path()), "the running one answers");
    assert!(wake_running(dir.path()));
    assert_eq!(woken.load(Ordering::SeqCst), 2, "and is woken each time");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("wake")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the token is this account's only");
    }

    // A wrong token is not answered, and wakes nothing.
    let text = std::fs::read_to_string(dir.path().join("wake")).unwrap();
    let port = text.split_once(' ').unwrap().0.to_string();
    std::fs::write(dir.path().join("wake"), format!("{port} not-the-token")).unwrap();
    assert!(!wake_running(dir.path()));
    assert_eq!(woken.load(Ordering::SeqCst), 2);

    // Nothing running, or nothing to read: no.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    for wake in [format!("{closed} token"), "no-space".into(), "port token".into()] {
        std::fs::write(dir.path().join("wake"), wake).unwrap();
        assert!(!wake_running(dir.path()));
    }
    std::fs::remove_file(dir.path().join("wake")).unwrap();
    assert!(!wake_running(dir.path()));
    // A folder it cannot write in: no listener.
    assert!(listen_for_wake(&dir.path().join("missing"), || {}).is_err());
}

#[tokio::test]
async fn unpairing_stops_what_runs() {
    let (fake, bench) = (fake().await, Bench::new());
    let pid_file = bench.dir.path().join("pid");
    bench.script("slow", &format!("echo $$ > '{}'; exec sleep 30", pid_file.display()));
    let agent = ready(&bench, &fake, Bench::timing(), vec![program("slow", "slow", json!([]))]).await;
    fake.call("c1", "local_run", json!({"name": "slow", "values": {}}));
    assert_eq!(agent.step().await, Step::Paired { calls: 1 });
    until("the program started", || std::fs::read_to_string(&pid_file).is_ok_and(|t| !t.trim().is_empty())).await;
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    agent.unpair().await;
    until("the program is killed", || gone(&pid)).await;
    assert_eq!((agent.running(), agent.status().state), (0, "unpaired"));
}

#[tokio::test]
async fn paused_while_unpaired_is_kept_and_answers_nothing() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    agent.set_paused(true).await;
    assert!(agent.status().paused && bench.state_text().contains("\"paused\": true"));
    assert_eq!(fake.get(|c| (c.result_posts, c.polls.len(), c.bearers.len())), (0, 0, 0), "nobody to tell");
    // Paired later: the first poll says so.
    pair(&agent, &fake).await;
    assert_eq!(fake.last_status()["paused"], true);
}

#[tokio::test]
async fn an_answer_that_comes_after_the_pairing_ended_is_nobodys() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    pair(&agent, &fake).await;

    // A poll is out, and the cloud holds it…
    let gate = Arc::new(Semaphore::new(0));
    fake.set(|c| c.poll_gate = Some(gate.clone()));
    let polls = fake.get(|c| c.polls.len());
    let stepping = agent.clone();
    let step = tokio::spawn(async move { stepping.step().await });
    until("the poll is out", || fake.get(|c| c.polls.len()) == polls + 1).await;
    // …while another turn learns the agent was deleted in Wysebee: forgotten here.
    fake.set(|c| c.hash = None);
    assert_eq!(agent.step().await, Step::Unpaired);
    // The held answer then comes, with a call in it: nothing of it is taken.
    fake.call("c1", "ping", json!({}));
    gate.add_permits(10);
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), step).await.unwrap().unwrap(), Step::Idle);
    assert_eq!(fake.get(|c| c.calls.len()), 0, "the cloud handed the call out");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!((agent.running(), fake.get(|c| c.result_posts)), (0, 0), "and nothing ran or answered");
    let s = agent.status();
    assert_eq!((s.state, s.agent.is_none()), ("unpaired", true));
    assert_eq!(s.note.as_deref(), Some("This Mac was unpaired in Wysebee, or its agent was deleted."));
}

#[tokio::test]
async fn a_poll_slower_than_the_gap_is_followed_at_once() {
    let (fake, bench) = (fake().await, Bench::new());
    // A quick answer waits out the gap before the next poll; a slow one does not wait more.
    let agent = Agent::open(bench.options(Some(&fake.url), Timing { min_gap: Duration::from_millis(500), ..Bench::timing() }));
    agent.start_pairing("Mac helper").await.unwrap();
    fake.set(|c| c.hold = Duration::from_millis(600));
    let looping = agent.clone();
    let task = tokio::spawn(async move { looping.run().await });
    until("four polls", || fake.get(|c| c.polls.len()) >= 4).await;
    task.abort();
    let times: Vec<_> = fake.get(|c| c.polls.iter().map(|(at, _)| *at).collect());
    let gap = times[3].duration_since(times[2]);
    // Waiting out the gap after a slow poll would make it about 1,100 ms.
    assert!(gap >= Duration::from_millis(600) && gap < Duration::from_millis(1000), "{gap:?}");
    assert_eq!(agent.status().state, "waiting");
}

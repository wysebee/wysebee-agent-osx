//! Developer access and the Android and iPhone toolkits: a shell, background jobs and files anywhere (`dev_shell`,
//! `dev_job`, `dev_file`), adb (`android_dev`) and xcrun (`ios_dev`) — each only while the
//! person has turned on its switch, off by default and forgotten on unpairing. The shells, adb
//! and xcrun here are `sh` scripts on the test PATH.

mod common;

use std::time::Duration;

use agent_core::agent::{ANDROID_OFF, DEV_OFF, IOS_OFF, PAUSED};
use common::{call, fake, pair, Bench};
use serde_json::{json, Value};

#[tokio::test]
async fn nothing_runs_until_the_person_turns_developer_access_on() {
    let (fake, bench) = (fake().await, Bench::new());
    bench.script("zsh", "echo \"$@\"\npwd -P");
    bench.script("bash", "echo \"$@\"\necho warned >&2\nexit 2");
    let agent = bench.agent(&fake);
    pair(&agent, &fake).await;
    for (op, args) in [("dev_shell", json!({"command": "dir"})), ("dev_job", json!({"action": "list"})), ("dev_file", json!({"tool": "list"}))] {
        assert_eq!(call(&agent, &fake, "c0", op, args).await, json!({"ok": false, "error": DEV_OFF}), "{op}");
        fake.set(|c| c.results.clear());
    }
    agent.set_dev_access(true);
    assert!(agent.status().dev_access);

    // zsh (the default, a login shell), in the home folder: the command is one argument after -c.
    let ran = call(&agent, &fake, "c1", "dev_shell", json!({"command": "Get-ChildItem | Select -First 3"})).await;
    let home = std::fs::canonicalize(bench.home()).unwrap();
    assert_eq!(ran["result"], json!({"exit_code": 0, "stdout": format!("-l -c Get-ChildItem | Select -First 3\n{}\n", home.display()), "stderr": ""}));
    // bash, in a folder of the call's.
    std::fs::create_dir(bench.home().join("app")).unwrap();
    let ran = call(&agent, &fake, "c2", "dev_shell", json!({"command": "./gradlew assembleDebug", "shell": "bash", "cwd": "app"})).await;
    assert_eq!(ran["result"]["exit_code"], 2);
    assert_eq!(ran["result"]["stderr"], "warned\n");
    assert!(ran["result"]["stdout"].as_str().unwrap().starts_with("-l -c ./gradlew"));
    // A shell that is not here, a call that cannot be read.
    let none = call(&agent, &fake, "c3", "dev_shell", json!({"command": "x", "shell": "sh"})).await;
    assert_eq!(none["error"], "There is no program 'sh' on this Mac.");
    let bad = call(&agent, &fake, "c4", "dev_shell", json!({"command": ""})).await;
    assert!(bad["error"].as_str().unwrap().starts_with("Give the 'command'"));
    // A file that is not a program: it cannot be started.
    common::script(&bench.bin(), "sh", "");
    std::fs::write(bench.bin().join("sh"), [0xfe, 0xed, 0xfa, 0xce, 0, 0]).unwrap();
    let broken = call(&agent, &fake, "c5", "dev_shell", json!({"command": "x", "shell": "sh"})).await;
    assert!(broken["error"].as_str().unwrap().starts_with("sh could not be started"), "{broken}");

    // Files anywhere: written, read back.
    let wrote = call(&agent, &fake, "c6", "dev_file", json!({"tool": "write", "path": "app/README.md", "content": "# App\n"})).await;
    assert_eq!(wrote["ok"], true);
    let read = call(&agent, &fake, "c7", "dev_file", json!({"tool": "read", "path": "app/README.md"})).await;
    assert_eq!(read["result"]["text"], "     1\t# App");

    // Paused: refused; turned off: refused; unpaired: forgotten.
    agent.set_paused(true).await;
    assert_eq!(call(&agent, &fake, "c8", "dev_file", json!({"tool": "list"})).await, json!({"ok": false, "error": PAUSED}));
    agent.set_paused(false).await;
    agent.set_dev_access(false);
    assert_eq!(call(&agent, &fake, "c9", "dev_job", json!({"action": "list"})).await["error"], DEV_OFF);
    agent.set_dev_access(true);
    agent.unpair().await;
    assert!(!agent.state().dev_access && !agent.status().dev_access);
}

#[tokio::test]
async fn a_long_command_is_stopped_or_runs_as_a_background_job() {
    let (fake, bench) = (fake().await, Bench::new());
    bench.script("zsh", "echo started\nsleep 1\necho finished\nexit 4");
    bench.script("bash", "echo waiting\nexec sleep 30");
    let agent = bench.agent(&fake);
    pair(&agent, &fake).await;
    agent.set_dev_access(true);

    let late = call(&agent, &fake, "c1", "dev_shell", json!({"command": "x", "shell": "bash", "timeout_secs": 1})).await;
    assert!(late["error"].as_str().unwrap().starts_with("The command did not finish within 1 seconds"), "{late}");

    // In the background: answered at once, its output read as it comes, its end kept.
    let started = call(&agent, &fake, "c2", "dev_shell", json!({"command": "gradlew build", "background": true})).await;
    let job = started["result"]["job"].as_str().unwrap().to_string();
    assert_eq!(started["result"]["status"], "running");
    let mut last = Value::Null;
    for n in 0..100 {
        last = call(&agent, &fake, &format!("o{n}"), "dev_job", json!({"job": job})).await["result"].clone();
        if last["status"] == "exited" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!((last["status"].as_str(), last["exit_code"].as_i64()), (Some("exited"), Some(4)), "{last}");
    assert_eq!(last["output"], "started\nfinished\n");
    assert_eq!(last["command"], "gradlew build");
    let tail = call(&agent, &fake, "t1", "dev_job", json!({"job": job, "tail": 9})).await;
    assert_eq!(tail["result"]["output"], "…[8 characters before cut]\nfinished\n");

    // One that runs on: stopped; listed; and stopped all at once when paused.
    let slow = call(&agent, &fake, "c3", "dev_shell", json!({"command": "y", "shell": "bash", "background": true})).await["result"]["job"].as_str().unwrap().to_string();
    let stopped = call(&agent, &fake, "c4", "dev_job", json!({"job": slow, "action": "stop"})).await;
    assert_eq!(stopped["result"], json!({"job": slow, "stopped": true}));
    let mut status = Value::Null;
    for i in 0..100 {
        status = call(&agent, &fake, &format!("s{i}"), "dev_job", json!({"job": slow})).await["result"]["status"].clone();
        if status == "stopped" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(status, "stopped");
    assert_eq!(call(&agent, &fake, "c5", "dev_job", json!({"job": slow, "action": "stop"})).await["result"]["stopped"], false);
    let listed = call(&agent, &fake, "c6", "dev_job", json!({"action": "list"})).await;
    assert_eq!(listed["result"]["jobs"].as_array().unwrap().len(), 2);
    assert!(listed["result"]["jobs"][0].get("output").is_none());
    assert!(call(&agent, &fake, "c7", "dev_job", json!({"job": "job-x"})).await["error"].as_str().unwrap().starts_with("There is no job 'job-x'"));
    assert!(call(&agent, &fake, "c8", "dev_job", json!({"job": slow, "action": "fly"})).await["error"].as_str().unwrap().starts_with("'fly' is not an action"));

    // At most 8 at once; pausing stops them all.
    for i in 0..8 {
        let r = call(&agent, &fake, &format!("m{i}"), "dev_shell", json!({"command": "z", "shell": "bash", "background": true})).await;
        assert_eq!(r["ok"], true, "{r}");
    }
    let ninth = call(&agent, &fake, "m9", "dev_shell", json!({"command": "z", "shell": "bash", "background": true})).await;
    assert!(ninth["error"].as_str().unwrap().starts_with("8 jobs are running already"), "{ninth}");
    agent.set_paused(true).await;
    agent.set_paused(false).await;
    let mut running = 8;
    for i in 0..100 {
        let jobs = call(&agent, &fake, &format!("l{i}"), "dev_job", json!({"action": "list"})).await["result"]["jobs"].clone();
        running = jobs.as_array().unwrap().iter().filter(|j| j["status"] == "running").count();
        if running == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(running, 0);
}

#[tokio::test]
async fn the_android_toolkit_runs_adb_only_while_its_switch_is_on() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    pair(&agent, &fake).await;
    let devices = json!({"tool": "android_devices"});
    assert_eq!(call(&agent, &fake, "c1", "android_dev", devices.clone()).await, json!({"ok": false, "error": ANDROID_OFF}));
    agent.set_android_dev(true);
    assert!(agent.status().android_dev);
    // No adb on this Mac: how to get it.
    let none = call(&agent, &fake, "c2", "android_dev", devices.clone()).await;
    assert_eq!(none["error"], agent_core::android::NO_ADB);
    // adb (the stand-in): the phones it lists.
    bench.script("adb", "printf 'List of devices attached\\nR58M\\tdevice model:Pixel_8\\n'");
    let listed = call(&agent, &fake, "c3", "android_dev", json!({"tool": "android_devices", "args": {}})).await;
    let phones: Value = serde_json::from_str(listed["result"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(phones, json!([{"serial": "R58M", "state": "device", "model": "Pixel 8"}]));
    // An adb that cannot be started.
    let adb = bench.bin().join("adb");
    std::fs::remove_file(&adb).unwrap();
    common::script(&bench.bin(), "adb", "");
    std::fs::write(&adb, [0xfe, 0xed, 0xfa, 0xce, 0, 0]).unwrap();
    let broken = call(&agent, &fake, "c4", "android_dev", devices).await;
    assert!(broken["error"].as_str().unwrap().starts_with("adb could not be run"), "{broken}");
    // Unpaired: forgotten.
    agent.unpair().await;
    assert!(!agent.state().android_dev);
}

#[tokio::test]
async fn the_iphone_toolkit_runs_xcrun_only_while_its_switch_is_on() {
    let (fake, bench) = (fake().await, Bench::new());
    let agent = bench.agent(&fake);
    pair(&agent, &fake).await;
    let devices = json!({"tool": "ios_devices"});
    assert_eq!(call(&agent, &fake, "c1", "ios_dev", devices.clone()).await, json!({"ok": false, "error": IOS_OFF}));
    agent.set_ios_dev(true);
    assert!(agent.status().ios_dev);
    // No xcrun on this Mac: how to get it.
    let none = call(&agent, &fake, "c2", "ios_dev", devices.clone()).await;
    assert_eq!(none["error"], agent_core::ios::NO_XCRUN);
    // xcrun (a script): its simulators; devicectl, which writes no answer here, adds no iPhone.
    bench.script("xcrun", r#"case "$1 $2" in "simctl list") echo '{"devices": {"com.apple.CoreSimulator.SimRuntime.iOS-18-1": [{"udid": "S1", "name": "iPhone 16", "state": "Booted"}]}}';; *) exit 0;; esac"#);
    let listed = call(&agent, &fake, "c3", "ios_dev", json!({"tool": "ios_devices", "args": {}})).await;
    let rows: Value = serde_json::from_str(listed["result"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(rows, json!([{"udid": "S1", "name": "iPhone 16", "os": "iOS 18.1", "kind": "simulator", "state": "Booted"}]));
    // An xcrun that runs too long is stopped; one that cannot be started is said so.
    bench.script("xcrun", "exec sleep 60");
    let slow = agent_core::ios::spawn(bench.bin().join("xcrun"), vec![], Duration::from_millis(300)).await;
    assert_eq!(slow.err().unwrap(), "xcrun took too long and was stopped.");
    let xcrun = bench.bin().join("xcrun");
    std::fs::remove_file(&xcrun).unwrap();
    common::script(&bench.bin(), "xcrun", "");
    std::fs::write(&xcrun, [0xfe, 0xed, 0xfa, 0xce, 0, 0]).unwrap();
    let broken = call(&agent, &fake, "c4", "ios_dev", devices).await;
    assert!(broken["error"].as_str().unwrap().starts_with("xcrun could not be run"), "{broken}");
    // Unpaired: forgotten.
    agent.unpair().await;
    assert!(!agent.state().ios_dev);
}

//! **The iPhone Development Toolkit**: an agent works with this
//! Mac's iOS simulators and the iPhones plugged in to it, through Xcode's `xcrun` — only while
//! the person has turned on "Let Wysebee use iPhones and simulators on this Mac" (`agent.rs`).
//! One switch covers all of it, as for Android.
//!
//! Thirteen tools, one op (`ios_dev` `{tool, args}`): **look** — the simulators and iPhones, a
//! simulator's screenshot, its log, the apps on a simulator or an iPhone, an iPhone's details;
//! **operate** — boot or shut down a simulator, open a link in one, launch or stop an app, set
//! a simulator's light or dark appearance; **change** — install or uninstall an app.
//! Simulators through `simctl`, iPhones through `devicectl` (Xcode 15 or later). Nothing taps a
//! simulator: Screen control does, on the Simulator window. No iPhone screenshot or log:
//! devicectl has none.
//!
//! xcrun is run with arguments (never a shell). How it is run is handed in ([`Run`], Android's
//! seam), so the tests use a fake Xcode. devicectl answers in a JSON file: it is written in
//! `tmp` and removed once read; so is a screenshot.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};

pub use crate::android::{Out, Run};

pub const TIMEOUT: Duration = Duration::from_secs(30);
/// Booting, installing: slow.
pub const LONG: Duration = Duration::from_secs(200);
const MAX_LOG_MINUTES: u64 = 60;
const MAX_OUT: usize = 50_000;

/// The tools, by name.
pub const TOOLS: [&str; 13] = [
    "ios_devices", "ios_screenshot", "ios_logs", "ios_apps", "ios_info",
    "ios_boot", "ios_shutdown", "ios_open_url", "ios_launch", "ios_terminate", "ios_appearance",
    "ios_install", "ios_uninstall",
];

/// No xcrun here: how to get it.
pub const NO_XCRUN: &str = "xcrun is not on this Mac. The person installs Xcode from the App Store, opens it once, and runs `sudo xcode-select -s /Applications/Xcode.app`.";

/// xcrun at `exe` run with `args`, killed after `timeout`; no input.
pub async fn spawn(exe: std::path::PathBuf, args: Vec<String>, timeout: Duration) -> Result<Out, String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let Ok(out) = tokio::time::timeout(timeout, cmd.output()).await else { return Err("xcrun took too long and was stopped.".into()) };
    let out = out.map_err(|e| format!("xcrun could not be run: {e}"))?;
    Ok(Out { stdout: out.stdout, stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(), ok: out.status.success() })
}

/// A simulator or an iPhone.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub udid: String,
    pub name: String,
    /// "iOS 17.2".
    pub os: String,
    pub simulator: bool,
    /// A simulator booted; an iPhone paired and reachable.
    pub ready: bool,
    /// "Booted", "Shutdown"; for an iPhone its connection ("connected", "disconnected").
    pub state: String,
}

impl Device {
    fn row(&self) -> Value {
        json!({"udid": self.udid, "name": self.name, "os": self.os, "kind": if self.simulator { "simulator" } else { "iphone" }, "state": self.state})
    }
}

/// `simctl list devices available -j`: each simulator, its runtime made readable.
pub fn simulators_of(listing: &str) -> Vec<Device> {
    let v: Value = serde_json::from_str(listing).unwrap_or(Value::Null);
    let mut out = vec![];
    for (runtime, list) in v["devices"].as_object().into_iter().flatten() {
        let os = runtime.rsplit('.').next().unwrap_or("").replacen('-', " ", 1).replace('-', ".");
        for d in list.as_array().into_iter().flatten() {
            let state = d["state"].as_str().unwrap_or("").to_string();
            out.push(Device { udid: d["udid"].as_str().unwrap_or("").into(), name: d["name"].as_str().unwrap_or("").into(), os: os.clone(), simulator: true, ready: state == "Booted", state });
        }
    }
    out
}

/// `devicectl list devices --json-output`: each iPhone or iPad (no Mac, no watch).
pub fn iphones_of(listing: &str) -> Vec<Device> {
    let v: Value = serde_json::from_str(listing).unwrap_or(Value::Null);
    v["result"]["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["hardwareProperties"]["platform"] == "iOS")
        .map(|d| {
            let (hw, props, conn) = (&d["hardwareProperties"], &d["deviceProperties"], &d["connectionProperties"]);
            let tunnel = conn["tunnelState"].as_str().unwrap_or("").to_string();
            Device {
                udid: hw["udid"].as_str().or(d["identifier"].as_str()).unwrap_or("").into(),
                name: props["name"].as_str().or(hw["marketingName"].as_str()).unwrap_or("").into(),
                os: format!("iOS {}", props["osVersionNumber"].as_str().unwrap_or("?")),
                simulator: false,
                ready: conn["pairingState"] == "paired" && tunnel != "unavailable",
                state: tunnel,
            }
        })
        .collect()
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| w.to_string()).collect()
}

fn text_of(o: &Out) -> String {
    String::from_utf8_lossy(&o.stdout).trim_end().to_string()
}

fn said(text: String) -> Value {
    json!({"text": crate::dev::cut(&text, MAX_OUT, false)})
}

/// Xcode's answer: what it printed, or why it failed.
fn answer(o: Out, empty: &str) -> Result<Value, String> {
    let text = text_of(&o);
    if !o.ok {
        return Err(format!("Xcode said no: {}", if o.stderr.is_empty() { text } else { o.stderr }));
    }
    Ok(said(if text.is_empty() { empty.to_string() } else { text }))
}

/// A bundle id: letters, digits, dots, hyphens and underscores, with a dot.
pub fn bundle_ok(b: &str) -> bool {
    !b.is_empty() && b.len() <= 255 && b.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)) && b.contains('.')
}

/// devicectl run with `more`, its JSON read from a file in `tmp` (then removed).
async fn devicectl(run: Run<'_>, tmp: &Path, more: &[&str], limit: Duration) -> Result<Value, String> {
    std::fs::create_dir_all(tmp).map_err(|e| format!("A folder for Xcode's answer could not be made: {e}"))?;
    let file = tmp.join(format!("devicectl-{}.json", rand::random::<u64>()));
    let mut args = words(&["devicectl"]);
    args.extend(words(more));
    args.extend(["--json-output".to_string(), file.to_string_lossy().to_string()]);
    let o = run(args, limit).await;
    let read = std::fs::read_to_string(&file);
    let _ = std::fs::remove_file(&file);
    let o = o?;
    if !o.ok {
        return Err(format!("Xcode said no: {}", if o.stderr.is_empty() { text_of(&o) } else { o.stderr }));
    }
    Ok(read.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null))
}

/// Every simulator, and every iPhone devicectl sees (none when it cannot: an older Xcode).
async fn all(run: Run<'_>, tmp: &Path) -> Result<Vec<Device>, String> {
    let sims = run(words(&["simctl", "list", "devices", "available", "-j"]), TIMEOUT).await?;
    if !sims.ok {
        return Err(format!("Xcode's simulators could not be listed: {}", sims.stderr));
    }
    let mut out = simulators_of(&text_of(&sims));
    if let Ok(v) = devicectl(run, tmp, &["list", "devices"], TIMEOUT).await {
        out.extend(iphones_of(&v.to_string()));
    }
    Ok(out)
}

/// The simulator or iPhone a call is for: the `udid` given (or a name), else the one ready.
/// `sim_only`: a simulator; `booted`: it must be booted.
async fn target(run: Run<'_>, tmp: &Path, args: &Value, sim_only: bool, booted: bool) -> Result<Device, String> {
    let list: Vec<Device> = all(run, tmp).await?.into_iter().filter(|d| d.simulator || !sim_only).collect();
    let what = if sim_only { "simulator" } else { "simulator or iPhone" };
    match args["udid"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => {
            let d = list.iter().find(|d| d.udid.eq_ignore_ascii_case(id)).or_else(|| list.iter().find(|d| d.name == id)).ok_or(format!("There is no {what} '{id}' on this Mac: ios_devices lists them."))?;
            if booted && !d.ready {
                return Err(if d.simulator { format!("{} is not booted: boot it with ios_boot first.", d.name) } else { format!("{} is not reachable: it must be unlocked, plugged in (or on the same network) and paired with this Mac.", d.name) });
            }
            Ok(d.clone())
        }
        None => {
            let ready: Vec<&Device> = list.iter().filter(|d| d.ready || !booted).collect();
            match ready.as_slice() {
                [one] => Ok((*one).clone()),
                [] => Err(format!("No {what} is ready on this Mac: boot a simulator with ios_boot (its udid from ios_devices){}.", if sim_only { "" } else { ", or plug in and unlock a paired iPhone" })),
                many => Err(format!("Several are ready: say which with `udid` ({}).", many.iter().map(|d| format!("{} {}", d.udid, d.name)).collect::<Vec<_>>().join(", "))),
            }
        }
    }
}

/// One of the toolkit's tools (`tool`), run with `run` (xcrun's arguments); files Xcode
/// writes go in `tmp`; a path given is read from `home`. Answers `{"text"}`, and `"image"`
/// (`{media_type, data}`) for a screenshot.
pub async fn call(tool: &str, args: &Value, home: &Path, tmp: &Path, run: Run<'_>) -> Result<Value, String> {
    if !TOOLS.contains(&tool) {
        return Err(format!("There is no tool named '{tool}'."));
    }
    let text = |k: &str| args[k].as_str().map(str::trim).unwrap_or("").to_string();
    let bundle = || {
        let b = text("bundle_id");
        if bundle_ok(&b) { Ok(b) } else { Err("Give the app's `bundle_id`, as com.example.app.".to_string()) }
    };
    match tool {
        "ios_devices" => {
            let rows: Vec<Value> = all(run, tmp).await?.iter().map(Device::row).collect();
            Ok(said(if rows.is_empty() { "There is no simulator on this Mac, and no iPhone is paired with it.".into() } else { serde_json::to_string_pretty(&rows).unwrap_or_default() }))
        }
        "ios_screenshot" => {
            let d = target(run, tmp, args, true, true).await?;
            std::fs::create_dir_all(tmp).map_err(|e| format!("A folder for the screenshot could not be made: {e}"))?;
            let file = tmp.join(format!("screen-{}.png", rand::random::<u64>()));
            let o = run(vec!["simctl".into(), "io".into(), d.udid.clone(), "screenshot".into(), "--type=png".into(), file.to_string_lossy().to_string()], TIMEOUT).await;
            let bytes = std::fs::read(&file).unwrap_or_default();
            let _ = std::fs::remove_file(&file);
            let o = o?;
            match crate::android::shrink(&bytes) {
                Some((picture, w, h)) if o.ok => Ok(json!({"text": format!("Screenshot of {} ({}), {w}×{h} pixels. To tap it, use Screen control on the Simulator window.", d.name, d.os), "image": picture})),
                _ => Err(format!("The screenshot could not be taken: {}", if o.stderr.is_empty() { "Xcode wrote no picture" } else { &o.stderr })),
            }
        }
        "ios_logs" => {
            let d = target(run, tmp, args, true, true).await?;
            let minutes = args["minutes"].as_u64().unwrap_or(5).clamp(1, MAX_LOG_MINUTES);
            let process = text("process");
            let mut a = vec!["simctl".into(), "spawn".into(), d.udid, "log".into(), "show".into(), "--style".into(), "compact".into(), "--last".into(), format!("{minutes}m")];
            if !process.is_empty() {
                if !process.chars().all(|c| c.is_ascii_alphanumeric() || " ._-".contains(c)) {
                    return Err("Give `process` as the app's name, as MyApp.".into());
                }
                a.extend(["--predicate".into(), format!("process == \"{process}\"")]);
            }
            let o = run(a, LONG).await?;
            if !o.ok {
                return answer(o, "");
            }
            let contains = text("contains").to_lowercase();
            let all = text_of(&o);
            let kept: Vec<&str> = all.lines().filter(|l| contains.is_empty() || l.to_lowercase().contains(&contains)).collect();
            let tail = kept[kept.len().saturating_sub(2_000)..].join("\n");
            Ok(json!({"text": crate::dev::cut(if kept.is_empty() { "No log lines match." } else { &tail }, MAX_OUT, true)}))
        }
        "ios_apps" => {
            let d = target(run, tmp, args, false, true).await?;
            let contains = text("contains").to_lowercase();
            let mut apps: Vec<String> = if d.simulator {
                let o = run(words(&["simctl", "listapps", &d.udid]), TIMEOUT).await?;
                if !o.ok {
                    return answer(o, "");
                }
                text_of(&o).lines().filter_map(|l| l.trim().strip_prefix("CFBundleIdentifier = ")).map(|b| b.trim_end_matches(';').trim_matches('"').to_string()).collect()
            } else {
                let v = devicectl(run, tmp, &["device", "info", "apps", "--device", &d.udid], TIMEOUT).await?;
                v["result"]["apps"].as_array().into_iter().flatten().map(|a| format!("{}  {}", a["bundleIdentifier"].as_str().unwrap_or(""), a["name"].as_str().unwrap_or(""))).collect()
            };
            apps.retain(|a| contains.is_empty() || a.to_lowercase().contains(&contains));
            apps.sort();
            Ok(said(if apps.is_empty() { "No apps match.".into() } else { apps.join("\n") }))
        }
        "ios_info" => {
            let d = target(run, tmp, args, false, false).await?;
            if d.simulator {
                return Ok(said(serde_json::to_string_pretty(&d.row()).unwrap_or_default()));
            }
            let v = devicectl(run, tmp, &["device", "info", "details", "--device", &d.udid], TIMEOUT).await?;
            Ok(said(serde_json::to_string_pretty(&v["result"]).unwrap_or_default()))
        }
        "ios_boot" | "ios_shutdown" => {
            let d = target(run, tmp, args, true, tool == "ios_shutdown").await?;
            if tool == "ios_boot" && d.ready {
                return Ok(said(format!("{} is booted already.", d.name)));
            }
            let verb = if tool == "ios_boot" { "boot" } else { "shutdown" };
            answer(run(words(&["simctl", verb, &d.udid]), LONG).await?, &format!("{} {}.", d.name, if verb == "boot" { "booted: open Simulator to see it" } else { "shut down" }))
        }
        "ios_open_url" => {
            let url = text("url");
            if url.chars().any(|c| c.is_whitespace() || c.is_control()) || !url.contains(':') {
                return Err("Give `url` as a link, as https://example.com or myapp://path.".into());
            }
            let d = target(run, tmp, args, true, true).await?;
            answer(run(words(&["simctl", "openurl", &d.udid, &url]), TIMEOUT).await?, "Opened it.")
        }
        "ios_launch" => {
            let b = bundle()?;
            let d = target(run, tmp, args, false, true).await?;
            if d.simulator {
                return answer(run(words(&["simctl", "launch", "--terminate-running-process", &d.udid, &b]), TIMEOUT).await?, "Launched.");
            }
            devicectl(run, tmp, &["device", "process", "launch", "--terminate-existing", "--device", &d.udid, &b], LONG).await?;
            Ok(said(format!("Launched {b} on {}.", d.name)))
        }
        "ios_terminate" => {
            let b = bundle()?;
            let d = target(run, tmp, args, true, true).await?;
            answer(run(words(&["simctl", "terminate", &d.udid, &b]), TIMEOUT).await?, "Stopped.")
        }
        "ios_appearance" => {
            let mode = text("mode").to_lowercase();
            if mode != "light" && mode != "dark" {
                return Err("Give `mode`: light or dark.".into());
            }
            let d = target(run, tmp, args, true, true).await?;
            answer(run(words(&["simctl", "ui", &d.udid, "appearance", &mode]), TIMEOUT).await?, &format!("{} is {mode} now.", d.name))
        }
        "ios_install" => {
            let app = crate::dev::path_of(home, &text("app"))?;
            let ext = app.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
            let d = target(run, tmp, args, false, true).await?;
            let path = app.to_string_lossy().to_string();
            if d.simulator {
                if ext != "app" || !app.is_dir() {
                    return Err("Give the path of a simulator build's .app folder on this Mac (xcodebuild's Build/Products/Debug-iphonesimulator).".into());
                }
                return answer(run(words(&["simctl", "install", &d.udid, &path]), LONG).await?, "Installed.");
            }
            if !(ext == "app" && app.is_dir() || ext == "ipa" && app.is_file()) {
                return Err("Give the path of a signed .app folder or .ipa file for iPhone on this Mac.".into());
            }
            devicectl(run, tmp, &["device", "install", "app", "--device", &d.udid, &path], LONG).await?;
            Ok(said(format!("Installed on {}.", d.name)))
        }
        _ => {
            let b = bundle()?;
            let d = target(run, tmp, args, false, true).await?;
            if d.simulator {
                return answer(run(words(&["simctl", "uninstall", &d.udid, &b]), LONG).await?, "Uninstalled.");
            }
            devicectl(run, tmp, &["device", "uninstall", "app", "--device", &d.udid, &b], LONG).await?;
            Ok(said(format!("Uninstalled {b} from {}.", d.name)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const SIMS: &str = r#"{"devices": {
        "com.apple.CoreSimulator.SimRuntime.iOS-17-2": [
            {"udid": "SIM-A", "name": "iPhone 15", "state": "Booted"},
            {"udid": "SIM-B", "name": "iPad Air", "state": "Shutdown"}
        ]}}"#;
    const PHONES: &str = r#"{"result": {"devices": [
        {"identifier": "ID-1", "hardwareProperties": {"udid": "PHONE-1", "platform": "iOS", "marketingName": "iPhone 14"}, "deviceProperties": {"name": "Test iPhone", "osVersionNumber": "17.1"}, "connectionProperties": {"pairingState": "paired", "tunnelState": "disconnected"}},
        {"identifier": "ID-2", "hardwareProperties": {"platform": "watchOS"}, "deviceProperties": {}, "connectionProperties": {}}
    ]}}"#;

    /// A fake Xcode: simctl and devicectl answered by the words a call carries; every call kept.
    struct Xcode {
        sims: Mutex<String>,
        phones: Mutex<Option<String>>,
        /// A call whose words contain this fails, its error this.
        fail: Mutex<Option<(String, String)>>,
        png: Vec<u8>,
        calls: Mutex<Vec<String>>,
        dir: tempfile::TempDir,
    }

    fn png() -> Vec<u8> {
        let mut bytes = vec![];
        image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 8)).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        bytes
    }

    impl Xcode {
        fn new(sims: &str, phones: Option<&str>) -> Arc<Xcode> {
            Arc::new(Xcode { sims: Mutex::new(sims.into()), phones: Mutex::new(phones.map(Into::into)), fail: Mutex::new(None), png: png(), calls: Mutex::new(vec![]), dir: tempfile::tempdir().unwrap() })
        }
        fn failing(&self, words: &str, error: &str) {
            *self.fail.lock().unwrap() = Some((words.into(), error.into()));
        }
        fn calls(&self) -> String {
            self.calls.lock().unwrap().join("\n")
        }
        fn answer(&self, args: &[String]) -> Result<Out, String> {
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            let ok = |s: &str| Ok(Out { stdout: s.as_bytes().to_vec(), stderr: String::new(), ok: true });
            if line.contains("break") || *self.sims.lock().unwrap() == "BREAK" {
                return Err("xcrun could not be run: broken".into());
            }
            if let Some((words, error)) = self.fail.lock().unwrap().clone() {
                if line.contains(&words) {
                    return Ok(Out { stdout: b"printed".to_vec(), stderr: error, ok: false });
                }
            }
            if let Some(i) = args.iter().position(|a| a == "--json-output") {
                let answer = if line.contains("list devices") {
                    match self.phones.lock().unwrap().clone() {
                        Some(p) => p,
                        None => return Ok(Out { stdout: vec![], stderr: "devicectl: no such command".into(), ok: false }),
                    }
                } else if line.contains("info apps") {
                    r#"{"result": {"apps": [{"bundleIdentifier": "com.b.app", "name": "B"}, {"bundleIdentifier": "com.a.app", "name": "A"}]}}"#.into()
                } else if line.contains("info details") {
                    r#"{"result": {"deviceProperties": {"name": "Test iPhone"}}}"#.into()
                } else {
                    "{}".into()
                };
                std::fs::write(&args[i + 1], answer).unwrap();
                return ok("");
            }
            if line.starts_with("simctl list devices") {
                return ok(&self.sims.lock().unwrap().clone());
            }
            if line.contains("screenshot") {
                if !line.contains("noshot") {
                    std::fs::write(args.last().unwrap(), &self.png).unwrap();
                }
                return ok("");
            }
            if line.contains("log show") {
                return ok("10:00 MyApp: hello\n10:01 MyApp: world");
            }
            if line.contains("listapps") {
                return ok("{\n    \"com.apple.mobilesafari\" = {\n        CFBundleIdentifier = \"com.apple.mobilesafari\";\n    };\n    CFBundleIdentifier = \"com.x.app\";\n}");
            }
            ok("")
        }
    }

    async fn run(x: &Arc<Xcode>, tool: &str, args: Value) -> Result<Value, String> {
        let me = x.clone();
        let xcrun = move |a: Vec<String>, _t: Duration| -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Out, String>> + Send>> {
            let me = me.clone();
            Box::pin(async move { me.answer(&a) })
        };
        call(tool, &args, x.dir.path(), &x.dir.path().join("tmp"), &xcrun).await
    }

    fn text(v: Result<Value, String>) -> String {
        match v {
            Ok(v) => v["text"].as_str().unwrap().to_string(),
            Err(e) => e,
        }
    }

    #[test]
    fn listings_are_read() {
        let sims = simulators_of(SIMS);
        assert_eq!(sims[0], Device { udid: "SIM-A".into(), name: "iPhone 15".into(), os: "iOS 17.2".into(), simulator: true, ready: true, state: "Booted".into() });
        assert!(!sims[1].ready);
        assert_eq!(simulators_of("not json"), vec![]);
        let phones = iphones_of(PHONES);
        assert_eq!(phones, vec![Device { udid: "PHONE-1".into(), name: "Test iPhone".into(), os: "iOS 17.1".into(), simulator: false, ready: true, state: "disconnected".into() }]);
        let bare = iphones_of(r#"{"result": {"devices": [{"identifier": "ID-9", "hardwareProperties": {"platform": "iOS", "marketingName": "iPhone 13"}, "deviceProperties": {}, "connectionProperties": {"pairingState": "unpaired"}}]}}"#);
        assert_eq!((bare[0].udid.as_str(), bare[0].name.as_str(), bare[0].os.as_str(), bare[0].ready), ("ID-9", "iPhone 13", "iOS ?", false));
        assert!(bundle_ok("com.example.my-app_2") && !bundle_ok("noddot") && !bundle_ok("com.x;rm") && !bundle_ok(""));
    }

    #[tokio::test]
    async fn the_simulators_and_iphones_are_listed() {
        let x = Xcode::new(SIMS, Some(PHONES));
        let rows: Value = serde_json::from_str(&text(run(&x, "ios_devices", json!({})).await)).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 3);
        assert_eq!(rows[2], json!({"udid": "PHONE-1", "name": "Test iPhone", "os": "iOS 17.1", "kind": "iphone", "state": "disconnected"}));
        assert!(std::fs::read_dir(x.dir.path().join("tmp")).unwrap().next().is_none(), "devicectl's answer is removed");
        // An older Xcode, with no devicectl: the simulators only. None at all: said so.
        let old = Xcode::new(r#"{"devices": {}}"#, None);
        assert_eq!(text(run(&old, "ios_devices", json!({})).await), "There is no simulator on this Mac, and no iPhone is paired with it.");
        old.failing("simctl list", "xcode-select: error: tool 'xcrun' requires Xcode");
        assert!(text(run(&old, "ios_devices", json!({})).await).ends_with("requires Xcode"));
        assert_eq!(text(run(&old, "ios_fly", json!({})).await), "There is no tool named 'ios_fly'.");
        assert_eq!(text(run(&Xcode::new("BREAK", None), "ios_info", json!({})).await), "xcrun could not be run: broken");
    }

    #[tokio::test]
    async fn a_call_finds_its_simulator_or_iphone() {
        let x = Xcode::new(SIMS, Some(PHONES));
        // Two are ready (the booted simulator and the iPhone): say which.
        assert_eq!(text(run(&x, "ios_apps", json!({})).await), "Several are ready: say which with `udid` (SIM-A iPhone 15, PHONE-1 Test iPhone).");
        // A simulator-only tool: the one booted simulator.
        assert!(text(run(&x, "ios_screenshot", json!({})).await).starts_with("Screenshot of iPhone 15 (iOS 17.2), 4×8 pixels."));
        // By udid (any case) or name; one that is not there; one not booted.
        assert!(text(run(&x, "ios_info", json!({"udid": "sim-b"})).await).contains("\"state\": \"Shutdown\""));
        assert!(text(run(&x, "ios_info", json!({"udid": "iPad Air"})).await).contains("SIM-B"));
        assert_eq!(text(run(&x, "ios_screenshot", json!({"udid": "PHONE-1"})).await), "There is no simulator 'PHONE-1' on this Mac: ios_devices lists them.");
        assert_eq!(text(run(&x, "ios_logs", json!({"udid": "SIM-B"})).await), "iPad Air is not booted: boot it with ios_boot first.");
        let far = Xcode::new(SIMS, Some(&PHONES.replace("disconnected", "unavailable")));
        assert!(text(run(&far, "ios_launch", json!({"udid": "PHONE-1", "bundle_id": "com.x.app"})).await).starts_with("Test iPhone is not reachable"));
        // None ready.
        let cold = Xcode::new(&SIMS.replace("Booted", "Shutdown"), None);
        assert!(text(run(&cold, "ios_apps", json!({})).await).ends_with("or plug in and unlock a paired iPhone."));
        assert!(text(run(&cold, "ios_open_url", json!({"url": "https://x.com"})).await).ends_with("(its udid from ios_devices)."));
    }

    #[tokio::test]
    async fn a_simulator_is_looked_at() {
        let x = Xcode::new(SIMS, None);
        let shot = run(&x, "ios_screenshot", json!({})).await.unwrap();
        assert_eq!(shot["image"]["media_type"], "image/jpeg");
        assert!(x.calls().contains("simctl io SIM-A screenshot --type=png "));
        assert!(std::fs::read_dir(x.dir.path().join("tmp")).unwrap().next().is_none(), "the picture is removed");
        assert!(text(run(&x, "ios_screenshot", json!({"udid": "noshot"})).await).starts_with("There is no simulator"));
        *x.sims.lock().unwrap() = SIMS.replace("SIM-A", "noshot");
        assert_eq!(text(run(&x, "ios_screenshot", json!({})).await), "The screenshot could not be taken: Xcode wrote no picture");
        x.failing("io noshot", "Invalid device state");
        assert_eq!(text(run(&x, "ios_screenshot", json!({})).await), "The screenshot could not be taken: Invalid device state");

        let x = Xcode::new(SIMS, None);
        assert_eq!(text(run(&x, "ios_logs", json!({"process": "MyApp", "minutes": 500, "contains": "WORLD"})).await), "10:01 MyApp: world");
        assert!(x.calls().contains("simctl spawn SIM-A log show --style compact --last 60m --predicate process == \"MyApp\""));
        assert_eq!(text(run(&x, "ios_logs", json!({"contains": "nothing"})).await), "No log lines match.");
        assert!(x.calls().contains("--last 5m"));
        assert_eq!(text(run(&x, "ios_logs", json!({"process": "a\"b"})).await), "Give `process` as the app's name, as MyApp.");
        x.failing("log show", "no log");
        assert_eq!(text(run(&x, "ios_logs", json!({})).await), "Xcode said no: no log");

        let x = Xcode::new(SIMS, None);
        assert_eq!(text(run(&x, "ios_apps", json!({})).await), "com.apple.mobilesafari\ncom.x.app");
        assert_eq!(text(run(&x, "ios_apps", json!({"contains": "SAFARI"})).await), "com.apple.mobilesafari");
        assert_eq!(text(run(&x, "ios_apps", json!({"contains": "zzz"})).await), "No apps match.");
        x.failing("listapps", "");
        assert_eq!(text(run(&x, "ios_apps", json!({})).await), "Xcode said no: printed");
    }

    #[tokio::test]
    async fn a_simulator_is_worked() {
        let x = Xcode::new(SIMS, None);
        assert_eq!(text(run(&x, "ios_boot", json!({})).await), "Several are ready: say which with `udid` (SIM-A iPhone 15, SIM-B iPad Air).");
        assert_eq!(text(run(&x, "ios_boot", json!({"udid": "SIM-A"})).await), "iPhone 15 is booted already.");
        assert_eq!(text(run(&x, "ios_boot", json!({"udid": "SIM-B"})).await), "iPad Air booted: open Simulator to see it.");
        assert_eq!(text(run(&x, "ios_shutdown", json!({})).await), "iPhone 15 shut down.");
        assert!(x.calls().contains("simctl boot SIM-B") && x.calls().contains("simctl shutdown SIM-A"));
        assert_eq!(text(run(&x, "ios_open_url", json!({"url": "myapp://home"})).await), "Opened it.");
        for url in ["", "no-colon", "https://a b"] {
            assert!(text(run(&x, "ios_open_url", json!({"url": url})).await).starts_with("Give `url`"), "{url}");
        }
        assert_eq!(text(run(&x, "ios_launch", json!({"bundle_id": "com.x.app"})).await), "Launched.");
        assert!(x.calls().contains("simctl launch --terminate-running-process SIM-A com.x.app"));
        assert_eq!(text(run(&x, "ios_launch", json!({"bundle_id": "nope"})).await), "Give the app's `bundle_id`, as com.example.app.");
        assert_eq!(text(run(&x, "ios_terminate", json!({"bundle_id": "com.x.app"})).await), "Stopped.");
        assert_eq!(text(run(&x, "ios_terminate", json!({})).await), "Give the app's `bundle_id`, as com.example.app.");
        assert_eq!(text(run(&x, "ios_appearance", json!({"mode": "Dark"})).await), "iPhone 15 is dark now.");
        assert_eq!(text(run(&x, "ios_appearance", json!({"mode": "blue"})).await), "Give `mode`: light or dark.");
        assert_eq!(text(run(&x, "ios_uninstall", json!({"bundle_id": "com.x.app"})).await), "Uninstalled.");
        assert_eq!(text(run(&x, "ios_uninstall", json!({})).await), "Give the app's `bundle_id`, as com.example.app.");

        // Installing: a simulator build's .app folder, from the home folder or a full path.
        std::fs::create_dir_all(x.dir.path().join("Build/MyApp.app")).unwrap();
        std::fs::write(x.dir.path().join("MyApp.ipa"), "zip").unwrap();
        assert_eq!(text(run(&x, "ios_install", json!({"app": "Build/MyApp.app"})).await), "Installed.");
        assert!(x.calls().contains(&format!("simctl install SIM-A {}", x.dir.path().join("Build/MyApp.app").display())));
        assert!(text(run(&x, "ios_install", json!({"app": "MyApp.ipa"})).await).starts_with("Give the path of a simulator build's .app folder"));
        assert!(text(run(&x, "ios_install", json!({"app": "Build/Gone.app"})).await).starts_with("Give the path of a simulator build's .app folder"));
        x.failing("simctl install", "Unable to install");
        assert_eq!(text(run(&x, "ios_install", json!({"app": "Build/MyApp.app"})).await), "Xcode said no: Unable to install");
    }

    #[tokio::test]
    async fn an_iphone_is_worked_through_devicectl() {
        let x = Xcode::new(r#"{"devices": {}}"#, Some(PHONES));
        assert_eq!(text(run(&x, "ios_apps", json!({"contains": "a.app"})).await), "com.a.app  A");
        assert!(x.calls().contains("devicectl device info apps --device PHONE-1 --json-output "));
        assert!(text(run(&x, "ios_info", json!({})).await).contains("Test iPhone"));
        assert_eq!(text(run(&x, "ios_launch", json!({"bundle_id": "com.a.app"})).await), "Launched com.a.app on Test iPhone.");
        assert!(x.calls().contains("devicectl device process launch --terminate-existing --device PHONE-1 com.a.app"));
        std::fs::create_dir_all(x.dir.path().join("Signed.app")).unwrap();
        std::fs::write(x.dir.path().join("MyApp.ipa"), "zip").unwrap();
        assert_eq!(text(run(&x, "ios_install", json!({"app": "MyApp.ipa"})).await), "Installed on Test iPhone.");
        assert_eq!(text(run(&x, "ios_install", json!({"app": "Signed.app"})).await), "Installed on Test iPhone.");
        assert!(text(run(&x, "ios_install", json!({"app": "Nothing.ipa"})).await).starts_with("Give the path of a signed .app folder or .ipa file"));
        assert_eq!(text(run(&x, "ios_uninstall", json!({"bundle_id": "com.a.app"})).await), "Uninstalled com.a.app from Test iPhone.");
        // A simulator-only tool finds no simulator here.
        assert!(text(run(&x, "ios_terminate", json!({"bundle_id": "com.a.app"})).await).starts_with("No simulator is ready"));
        // devicectl says no: its error; with nothing on stderr, what it printed.
        x.failing("uninstall", "The app is not installed.");
        assert_eq!(text(run(&x, "ios_uninstall", json!({"bundle_id": "com.a.app"})).await), "Xcode said no: The app is not installed.");
        x.failing("uninstall", "");
        assert_eq!(text(run(&x, "ios_uninstall", json!({"bundle_id": "com.a.app"})).await), "Xcode said no: printed");
        // A folder for Xcode's answer that cannot be made.
        let blocked = Xcode::new(SIMS, Some(PHONES));
        std::fs::write(blocked.dir.path().join("tmp"), "a file").unwrap();
        // (devicectl's listing fails then, and with it every iPhone.)
        assert_eq!(text(run(&blocked, "ios_info", json!({"udid": "PHONE-1"})).await), "There is no simulator or iPhone 'PHONE-1' on this Mac: ios_devices lists them.");
        assert!(text(run(&blocked, "ios_screenshot", json!({})).await).starts_with("A folder for the screenshot could not be made"));
    }
}

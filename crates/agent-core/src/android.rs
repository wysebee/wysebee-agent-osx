//! **The Android Development Toolkit** (ported from Wysebee
//! Desktop): an agent works with the Android
//! phones plugged in to this Mac
//! (or its emulators), through adb — only while the person has turned on "Let Wysebee use
//! Android phones on this Mac" (`agent.rs`). One switch covers all of it (the person's choice);
//! Desktop's ask-each-time is gone.
//!
//! Seventeen tools, one op (`android_dev` `{tool, args}`): **look** — the phones, a
//! screenshot, what is on screen (uiautomator), logcat, the phone's details, its apps, a
//! folder's files, a file downloaded to this Mac (`~/Downloads/Wysebee`); **operate** — tap,
//! swipe, type, a key, open an app or a link; **change** — install or uninstall an app, push a
//! file, an `adb shell` command. Never root, nor rebooting into a bootloader or recovery.
//! adb is run with arguments (never this Mac's shell); every value that reaches the phone's
//! shell is quoted ([`quoted`]).
//!
//! adb is found on the app's `PATH`, to which the shell adds the Android SDK's
//! `platform-tools` (`~/Library/Android/sdk`, `ANDROID_HOME`; Homebrew's folders already). How it is run is handed in
//! ([`Run`]), so the tests use a fake phone.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};

pub const TIMEOUT: Duration = Duration::from_secs(20);
pub const LONG: Duration = Duration::from_secs(200);
const SHELL_TIME: Duration = Duration::from_secs(120);
const MAX_LOG_LINES: u64 = 2_000;
const SHOT_EDGE: u32 = 1280;
const MAX_NODES: usize = 300;
const MAX_TEXT: usize = 500;
const MAX_OUT: usize = 50_000;

/// The tools, by name.
pub const TOOLS: [&str; 17] = [
    "android_devices", "android_screenshot", "android_screen", "android_logcat", "android_info", "android_apps", "android_files", "android_pull",
    "android_tap", "android_swipe", "android_type", "android_key", "android_open",
    "android_install", "android_uninstall", "android_push", "android_shell",
];

/// What adb printed: its output, its errors, whether it succeeded.
pub struct Out {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub ok: bool,
}

/// How adb is run: its arguments and a time limit.
pub type Run<'a> = &'a (dyn Fn(Vec<String>, Duration) -> Pin<Box<dyn Future<Output = Result<Out, String>> + Send>> + Send + Sync);

/// adb at `exe` run with `args`, killed after `timeout`; no input, no window.
pub async fn spawn(exe: PathBuf, args: Vec<String>, timeout: Duration) -> Result<Out, String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let Ok(out) = tokio::time::timeout(timeout, cmd.output()).await else { return Err("adb took too long and was stopped.".into()) };
    let out = out.map_err(|e| format!("adb could not be run: {e}"))?;
    Ok(Out { stdout: out.stdout, stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(), ok: out.status.success() })
}

/// No adb here: how to get it.
pub const NO_ADB: &str = "adb is not on this Mac. The person installs Android's platform-tools (with Android Studio, or `brew install android-platform-tools`) and plugs the phone in with USB debugging on.";

/// A value as the phone's shell reads one word: in single quotes.
pub fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The serials `adb devices` lists as ready ("device"): not "unauthorized" or "offline".
pub fn ready_serials(out: &str) -> Vec<String> {
    out.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some(serial), Some("device")) => Some(serial.to_string()),
                _ => None,
            }
        })
        .collect()
}

/// Each phone `adb devices -l` lists: its serial, state and model.
pub fn phones_of(listing: &str) -> Vec<Value> {
    listing
        .lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut words = line.split_whitespace();
            let serial = words.next().unwrap_or("");
            let state = words.next().unwrap_or("");
            let model = line.split_whitespace().find_map(|w| w.strip_prefix("model:")).unwrap_or("").replace('_', " ");
            json!({"serial": serial, "state": state, "model": model})
        })
        .collect()
}

const KEYS: [(&str, u32); 16] = [
    ("home", 3), ("back", 4), ("recents", 187), ("enter", 66), ("tab", 61), ("delete", 67), ("up", 19), ("down", 20),
    ("left", 21), ("right", 22), ("menu", 82), ("search", 84), ("volume_up", 24), ("volume_down", 25), ("power", 26), ("wakeup", 224),
];

/// The keys `android_key` presses.
pub fn key_names() -> Vec<&'static str> {
    KEYS.iter().map(|(k, _)| *k).collect()
}

fn text_of(o: &Out) -> String {
    String::from_utf8_lossy(&o.stdout).trim_end().to_string()
}

fn said(text: String) -> Value {
    json!({"text": crate::dev::cut(&text, MAX_OUT, false)})
}

/// What a phone that will not take adb's taps, swipes, typing or keys needs.
const INPUT_BLOCKED: &str = "The phone refused this: it does not let adb tap, swipe, type or press keys until a setting is changed on the phone, and trying again will not help. Tell the person to open Settings → Developer options and turn on: on a Xiaomi, Redmi or POCO, \"USB debugging (Security settings)\" (it may need a Mi account signed in or a SIM card); on an OPPO, Realme or OnePlus, \"Disable permission monitoring\"; on another make, anything about USB debugging security or simulating input. Then unplug the phone and plug it back in. A work profile or company device management can also block this.";

fn answer(o: Out, empty: &str) -> Result<Value, String> {
    let text = text_of(&o);
    if !o.ok {
        let why = if o.stderr.is_empty() { text } else { o.stderr };
        if why.contains("INJECT_EVENTS") {
            return Err(INPUT_BLOCKED.into());
        }
        return Err(format!("It failed on the phone: {why}"));
    }
    Ok(said(if text.is_empty() { empty.to_string() } else { text }))
}

/// A package name: letters, digits, dots and underscores, with a dot.
pub fn package_ok(p: &str) -> bool {
    !p.is_empty() && p.len() <= 255 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') && p.contains('.')
}

/// Text as `input text` takes it: spaces as `%s`, the rest quoted for the phone's shell.
pub fn typed(text: &str) -> String {
    quoted(&text.replace('%', "\\%").replace(' ', "%s"))
}

/// A shell command the toolkit will not run: `su`, or a reboot into a bootloader or recovery.
pub fn refused(command: &str) -> Option<&'static str> {
    let c = command.to_lowercase();
    let words: Vec<&str> = c.split(|ch: char| ch.is_whitespace() || ";&|()`$".contains(ch)).filter(|w| !w.is_empty()).collect();
    if words.iter().any(|w| *w == "su" || w.ends_with("/su")) {
        return Some("The toolkit does not run su: no root.");
    }
    let reboot_to = words.windows(2).any(|w| w[0] == "reboot" && ["bootloader", "recovery", "fastboot", "sideload", "download", "edl"].contains(&w[1]));
    if reboot_to {
        return Some("The toolkit does not reboot into a bootloader or recovery.");
    }
    None
}

/// Each element uiautomator dumped that can be read or tapped: where to tap it, and its words.
pub fn screen_lines(xml: &str) -> Vec<String> {
    let attr = |node: &str, name: &str| -> String {
        let key = format!(" {name}=\"");
        node.find(&key).and_then(|i| node[i + key.len()..].find('"').map(|j| node[i + key.len()..i + key.len() + j].to_string())).unwrap_or_default()
    };
    let unescape = |s: String| s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
    let mut out = Vec::new();
    for node in xml.split("<node").skip(1) {
        let (text, id, desc) = (unescape(attr(node, "text")), attr(node, "resource-id"), unescape(attr(node, "content-desc")));
        let clickable = attr(node, "clickable") == "true";
        if text.is_empty() && desc.is_empty() && !clickable {
            continue;
        }
        let nums: Vec<i64> = attr(node, "bounds").split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
        let mut line = if let [x1, y1, x2, y2] = nums[..] { format!("tap ({}, {})", (x1 + x2) / 2, (y1 + y2) / 2) } else { "no bounds".into() };
        if !text.is_empty() {
            line += &format!(" text=\"{}\"", text.chars().take(120).collect::<String>());
        }
        if !desc.is_empty() {
            line += &format!(" desc=\"{}\"", desc.chars().take(120).collect::<String>());
        }
        if !id.is_empty() {
            line += &format!(" id={}", id.rsplit('/').next().unwrap_or(&id));
        }
        if clickable {
            line += " clickable";
        }
        out.push(line);
        if out.len() >= MAX_NODES {
            break;
        }
    }
    out
}

/// A picture scaled to at most [`SHOT_EDGE`], as a JPEG: `{media_type, data}` and its size.
pub(crate) fn shrink(bytes: &[u8]) -> Option<(Value, u32, u32)> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let small = if w.max(h) > SHOT_EDGE { img.resize(SHOT_EDGE, SHOT_EDGE, image::imageops::FilterType::Triangle) } else { img };
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80).encode_image(&small.to_rgb8()).ok()?;
    Some((json!({"media_type": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(jpeg)}), w, h))
}

/// Where a download goes: `~/Downloads/Wysebee/<name>`, never over another file.
pub fn download_path(home: &Path, name: &str) -> PathBuf {
    let dir = home.join("Downloads").join("Wysebee");
    let clean: String = name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() || ":*?\"<>|".contains(c) { '_' } else { c }).collect();
    let clean = if clean.is_empty() || clean == "." || clean == ".." { "file".to_string() } else { clean };
    let first = dir.join(&clean);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (clean.clone(), String::new()),
    };
    (2..10_000).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

fn int(args: &Value, key: &str) -> Option<i64> {
    args[key].as_i64().or_else(|| args[key].as_f64().map(|f| f as i64))
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| w.to_string()).collect()
}

/// The phone a call is for: the `serial` given, else the one phone adb may use.
async fn device(run: Run<'_>, args: &Value) -> Result<String, String> {
    let list = run(words(&["devices"]), TIMEOUT).await?;
    let ready = ready_serials(&text_of(&list));
    let others = || if ready.is_empty() { "none is".to_string() } else { format!("there are {}", ready.join(", ")) };
    match args["serial"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) if ready.iter().any(|r| r == s) => Ok(s.to_string()),
        Some(s) => Err(format!("No phone '{s}' is ready on this Mac: {}.", others())),
        None => match ready.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err("No Android phone is ready on this Mac. The person plugs one in, turns on USB debugging (Settings → Developer options) and allows this Mac on the phone — or starts an emulator.".into()),
            many => Err(format!("Several phones are plugged in: say which with `serial` ({}).", many.join(", "))),
        },
    }
}

/// One of the toolkit's tools (`tool`), run with `run`; a file downloaded goes under `home`.
/// Answers `{"text"}`, and `"image"` (`{media_type, data}`) for a picture.
pub async fn call(tool: &str, args: &Value, home: &Path, run: Run<'_>) -> Result<Value, String> {
    if !TOOLS.contains(&tool) {
        return Err(format!("There is no tool named '{tool}'."));
    }
    if tool == "android_devices" {
        let list = run(words(&["devices", "-l"]), TIMEOUT).await?;
        let rows = phones_of(&text_of(&list));
        if rows.is_empty() {
            return Ok(said("No Android phone is plugged in to this Mac, or USB debugging is off on it.".into()));
        }
        return Ok(said(serde_json::to_string_pretty(&rows).unwrap_or_default()));
    }
    let serial = device(run, args).await?;
    let s = serial.as_str();
    let text = |k: &str| args[k].as_str().map(str::trim).unwrap_or("").to_string();
    let shell = |cmd: String, limit: Duration| run(vec!["-s".into(), serial.clone(), "shell".into(), cmd], limit);
    let with = |more: &[&str]| -> Vec<String> { [vec!["-s".to_string(), serial.clone()], words(more)].concat() };
    match tool {
        "android_screenshot" => {
            let o = run(with(&["exec-out", "screencap", "-p"]), TIMEOUT).await?;
            match shrink(&o.stdout) {
                Some((picture, w, h)) => Ok(json!({"text": format!("Screenshot of {s}. The phone's screen is {w}×{h} pixels: give taps and swipes in those."), "image": picture})),
                None => Err(format!("The screenshot could not be taken: {}", if o.stderr.is_empty() { "the screen may be locked or protected" } else { &o.stderr })),
            }
        }
        "android_screen" => {
            let o = shell("uiautomator dump /data/local/tmp/wysebee_screen.xml >/dev/null && cat /data/local/tmp/wysebee_screen.xml; rm -f /data/local/tmp/wysebee_screen.xml".into(), TIMEOUT).await?;
            let lines = screen_lines(&text_of(&o));
            if lines.is_empty() {
                return Err("Nothing could be read from the screen (it may be locked, or showing something uiautomator cannot see). Try android_screenshot.".into());
            }
            Ok(said(lines.join("\n")))
        }
        "android_logcat" => {
            let lines = (int(args, "lines").unwrap_or(200).max(1) as u64).min(MAX_LOG_LINES);
            let priority = text("priority").to_uppercase();
            let priority = if ["V", "D", "I", "W", "E", "F"].contains(&priority.as_str()) { priority } else { "V".into() };
            let tag = text("tag");
            let mut cmd = format!("logcat -d -v time -t {lines}");
            let package = text("package");
            if !package.is_empty() {
                if !package_ok(&package) {
                    return Err("Give `package` as com.example.app.".into());
                }
                let pid = text_of(&shell(format!("pidof {}", quoted(&package)), TIMEOUT).await?);
                match pid.split_whitespace().next() {
                    Some(p) => cmd += &format!(" --pid={p}"),
                    None => return Err(format!("{package} is not running on the phone, so it has no lines to read: open it first.")),
                }
            }
            cmd += &if tag.is_empty() { format!(" *:{priority}") } else { format!(" {}:{priority} *:S", quoted(&tag)) };
            let all = text_of(&shell(cmd, TIMEOUT).await?);
            let contains = text("contains").to_lowercase();
            let kept: Vec<&str> = all.lines().filter(|l| contains.is_empty() || l.to_lowercase().contains(&contains)).collect();
            Ok(said(if kept.is_empty() { "No log lines match.".into() } else { kept.join("\n") }))
        }
        "android_info" => {
            let cmd = "echo maker=$(getprop ro.product.manufacturer); echo model=$(getprop ro.product.model); echo android=$(getprop ro.build.version.release); echo api=$(getprop ro.build.version.sdk); wm size; wm density; dumpsys battery | grep -E ' level| status| powered'; df -h /data /sdcard 2>/dev/null";
            answer(shell(cmd.into(), TIMEOUT).await?, "Nothing came back.")
        }
        "android_apps" => {
            let all = args["all"].as_bool().unwrap_or(false);
            let o = shell(if all { "pm list packages".into() } else { "pm list packages -3".into() }, TIMEOUT).await?;
            let contains = text("contains").to_lowercase();
            let mut apps: Vec<String> = text_of(&o).lines().filter_map(|l| l.strip_prefix("package:")).map(str::to_string).filter(|p| contains.is_empty() || p.to_lowercase().contains(&contains)).collect();
            apps.sort();
            Ok(said(if apps.is_empty() { "No apps match.".into() } else { apps.join("\n") }))
        }
        "android_files" => {
            let path = text("path");
            let path = if path.is_empty() { "/sdcard".to_string() } else { path };
            answer(shell(format!("ls -la {}", quoted(&path)), TIMEOUT).await?, "The folder is empty.")
        }
        "android_pull" => {
            let path = text("path");
            if path.is_empty() || path.ends_with('/') {
                return Err("Give the file's `path` on the phone, as /sdcard/Download/report.pdf.".into());
            }
            let to = download_path(home, path.rsplit('/').next().unwrap_or("file"));
            std::fs::create_dir_all(to.parent().unwrap_or(home)).map_err(|e| format!("Downloads/Wysebee could not be made: {e}"))?;
            let o = run(with(&["pull", &path, &to.to_string_lossy()]), LONG).await?;
            if !o.ok || !to.is_file() {
                return Err(format!("It could not be downloaded: {}", if o.stderr.is_empty() { text_of(&o) } else { o.stderr }));
            }
            let size = std::fs::metadata(&to).map(|m| m.len()).unwrap_or(0);
            let mut out = json!({"text": format!("Downloaded {path} to {} ({} KB).", to.display(), size.div_ceil(1024))});
            let lower = path.to_lowercase();
            if size <= 20 * 1024 * 1024 && [".jpg", ".jpeg", ".png", ".webp", ".gif"].iter().any(|e| lower.ends_with(e)) {
                if let Some((picture, _, _)) = std::fs::read(&to).ok().and_then(|b| shrink(&b)) {
                    out["image"] = picture;
                }
            }
            Ok(out)
        }
        "android_tap" => match (int(args, "x"), int(args, "y")) {
            (Some(x), Some(y)) if x >= 0 && y >= 0 => answer(shell(format!("input tap {x} {y}"), TIMEOUT).await?, &format!("Tapped ({x}, {y}).")),
            _ => Err("Give `x` and `y`, the point in the phone's pixels.".into()),
        },
        "android_swipe" => match (int(args, "x1"), int(args, "y1"), int(args, "x2"), int(args, "y2")) {
            (Some(a), Some(b), Some(c), Some(d)) if [a, b, c, d].iter().all(|v| *v >= 0) => {
                let ms = int(args, "ms").unwrap_or(300).clamp(50, 5000);
                answer(shell(format!("input swipe {a} {b} {c} {d} {ms}"), TIMEOUT).await?, &format!("Swiped from ({a}, {b}) to ({c}, {d}) in {ms} ms."))
            }
            _ => Err("Give `x1`, `y1`, `x2` and `y2`, in the phone's pixels.".into()),
        },
        "android_type" => {
            let t = args["text"].as_str().unwrap_or("");
            if t.is_empty() || t.chars().count() > MAX_TEXT {
                return Err(format!("Give the `text` to type, at most {MAX_TEXT} characters."));
            }
            if t.chars().any(|c| !c.is_ascii() || c.is_control()) {
                return Err("adb types plain ASCII only: no emoji, accents or new lines.".into());
            }
            answer(shell(format!("input text {}", typed(t)), TIMEOUT).await?, "Typed it.")
        }
        "android_key" => {
            let key = text("key").to_lowercase();
            match KEYS.iter().find(|(k, _)| *k == key) {
                Some((k, code)) => answer(shell(format!("input keyevent {code}"), TIMEOUT).await?, &format!("Pressed {k}.")),
                None => Err(format!("That is not one of the keys android_key presses: {}.", key_names().join(", "))),
            }
        }
        "android_open" => {
            let (package, url) = (text("package"), text("url"));
            let cmd = if !url.is_empty() {
                if url.chars().any(|c| c.is_whitespace() || c.is_control()) || !url.contains(':') {
                    return Err("Give `url` as a link, as https://example.com.".into());
                }
                format!("am start -a android.intent.action.VIEW -d {}", quoted(&url))
            } else if package_ok(&package) {
                format!("monkey -p {} -c android.intent.category.LAUNCHER 1", quoted(&package))
            } else {
                return Err("Give an app's `package` (com.example.app) or a `url`.".into());
            };
            answer(shell(cmd, TIMEOUT).await?, "Opened it.")
        }
        "android_install" => {
            let apk = crate::dev::path_of(home, &text("apk"))?;
            if !apk.is_file() || apk.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() != Some("apk") {
                return Err("Give the path of an .apk file on this Mac.".into());
            }
            answer(run(with(&["install", "-r", &apk.to_string_lossy()]), LONG).await?, "Installed.")
        }
        "android_uninstall" => {
            let package = text("package");
            if !package_ok(&package) {
                return Err("Give the app's `package`, as com.example.app.".into());
            }
            answer(run(with(&["uninstall", &package]), LONG).await?, "Uninstalled.")
        }
        "android_push" => {
            let file = crate::dev::path_of(home, &text("file"))?;
            if !file.is_file() {
                return Err("Give the path of a file on this Mac.".into());
            }
            let to = text("to");
            let to = if to.is_empty() { "/sdcard/Download/".to_string() } else if to.ends_with('/') { to } else { format!("{to}/") };
            answer(run(with(&["push", &file.to_string_lossy(), &to]), LONG).await?, &format!("Copied to {to}."))
        }
        _ => {
            let command = text("command");
            if command.is_empty() {
                return Err("Give the `command`.".into());
            }
            if let Some(why) = refused(&command) {
                return Err(why.into());
            }
            answer(shell(command, SHELL_TIME).await?, "It ran, and printed nothing.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A phone on a fake adb: what it answers, by the words a call carries; every call kept.
    struct Phone {
        devices: Mutex<String>,
        files: Mutex<Vec<(String, Vec<u8>)>>,
        calls: Arc<Mutex<Vec<String>>>,
        home: tempfile::TempDir,
    }

    impl Phone {
        fn new(devices: &str) -> Arc<Phone> {
            Arc::new(Phone { devices: Mutex::new(format!("List of devices attached\n{devices}")), files: Mutex::new(vec![]), calls: Arc::new(Mutex::new(vec![])), home: tempfile::tempdir().unwrap() })
        }
        fn put(&self, name: &str, bytes: &[u8]) {
            self.files.lock().unwrap().push((name.into(), bytes.to_vec()));
        }
        fn file(&self, name: &str) -> Option<Vec<u8>> {
            self.files.lock().unwrap().iter().rev().find(|(n, _)| n == name).map(|(_, b)| b.clone())
        }
        fn calls(&self) -> String {
            self.calls.lock().unwrap().join("\n")
        }
        fn answer(&self, args: &[String]) -> Result<Out, String> {
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            let ok = |s: &str| Ok(Out { stdout: s.as_bytes().to_vec(), stderr: String::new(), ok: true });
            if line.contains("break") || self.devices.lock().unwrap().as_str() == "BREAK" {
                return Err("adb could not be run: broken".into());
            }
            if args.first().map(String::as_str) == Some("devices") {
                return ok(&self.devices.lock().unwrap());
            }
            if line.contains("screencap") {
                return Ok(Out { stdout: self.file("screen.png").unwrap_or_default(), stderr: String::new(), ok: true });
            }
            if line.contains("uiautomator") {
                return ok(&String::from_utf8(self.file("screen.xml").unwrap_or_default()).unwrap());
            }
            if line.contains("pidof") {
                return ok(&String::from_utf8(self.file("pid").unwrap_or_default()).unwrap());
            }
            if line.contains("logcat") {
                return ok("11-03 I/Wysebee: hello\n11-03 W/Other: world");
            }
            if line.contains("pm list packages") {
                return ok("package:com.b.app\npackage:com.a.app");
            }
            if args.get(2).map(String::as_str) == Some("pull") {
                let name = args[3].rsplit('/').next().unwrap().to_string();
                return match self.file(&name) {
                    Some(b) => {
                        std::fs::write(&args[4], b).unwrap();
                        ok("1 file pulled")
                    }
                    None => Ok(Out { stdout: vec![], stderr: format!("adb: error: remote object '{}' does not exist", args[3]), ok: false }),
                };
            }
            if line.contains("install") {
                return ok("Success");
            }
            if line.contains("fail now") {
                return Ok(Out { stdout: vec![], stderr: "it broke".into(), ok: false });
            }
            if line.contains("quietfail") {
                return Ok(Out { stdout: b"half done".to_vec(), stderr: String::new(), ok: false });
            }
            if line.contains("input tap 999") {
                return Ok(Out { stdout: vec![], stderr: "java.lang.SecurityException: Injecting to another application requires INJECT_EVENTS permission".into(), ok: false });
            }
            ok("")
        }
    }

    async fn run(p: &Arc<Phone>, tool: &str, args: Value) -> Result<Value, String> {
        let phone = p.clone();
        let adb = move |args: Vec<String>, _t: Duration| -> Pin<Box<dyn Future<Output = Result<Out, String>> + Send>> {
            let answer = phone.answer(&args);
            Box::pin(async move { answer })
        };
        call(tool, &args, p.home.path(), &adb).await
    }

    fn text(r: Result<Value, String>) -> String {
        match r {
            Ok(v) => v["text"].as_str().unwrap().to_string(),
            Err(e) => e,
        }
    }

    /// A real PNG, as `screencap -p` gives.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([1, 2, 3]));
        let mut png = vec![];
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        png
    }

    const ONE: &str = "R58M\tdevice usb:1-1 product:x model:Pixel_8 device:x\n";

    #[tokio::test]
    async fn the_phone_is_the_one_named_or_the_only_one() {
        let p = Phone::new("");
        assert!(text(run(&p, "android_info", json!({})).await).contains("No Android phone is ready"));
        assert_eq!(text(run(&p, "android_info", json!({"serial": "X1"})).await), "No phone 'X1' is ready on this Mac: none is.");
        assert!(text(run(&p, "android_devices", json!({})).await).contains("No Android phone is plugged in"));
        *p.devices.lock().unwrap() = "List of devices attached\nR58M\tdevice model:Pixel_8\nZY22\tdevice\nQQ1\tunauthorized\n".into();
        assert_eq!(text(run(&p, "android_info", json!({})).await), "Several phones are plugged in: say which with `serial` (R58M, ZY22).");
        assert!(text(run(&p, "android_info", json!({"serial": "X1"})).await).contains("there are R58M, ZY22"));
        assert!(run(&p, "android_info", json!({"serial": "ZY22"})).await.is_ok());
        let listed: Value = serde_json::from_str(&text(run(&p, "android_devices", json!({})).await)).unwrap();
        assert_eq!(listed[0], json!({"serial": "R58M", "state": "device", "model": "Pixel 8"}));
        assert_eq!(listed[2]["state"], "unauthorized");
        assert_eq!(text(run(&p, "android_reboot", json!({})).await), "There is no tool named 'android_reboot'.");
        *p.devices.lock().unwrap() = "BREAK".into();
        assert_eq!(text(run(&p, "android_info", json!({})).await), "adb could not be run: broken");
        assert_eq!(text(run(&p, "android_devices", json!({})).await), "adb could not be run: broken");
    }

    #[tokio::test]
    async fn looking_at_the_phone() {
        let p = Phone::new(ONE);
        p.put("screen.png", &png(100, 200));
        let shot = run(&p, "android_screenshot", json!({})).await.unwrap();
        assert!(shot["text"].as_str().unwrap().contains("100×200") && shot["image"]["media_type"] == "image/jpeg");
        p.put("screen.png", &png(2000, 1000));
        assert!(run(&p, "android_screenshot", json!({})).await.unwrap()["text"].as_str().unwrap().contains("2000×1000"));
        p.put("screen.png", b"locked");
        assert!(text(run(&p, "android_screenshot", json!({})).await).contains("the screen may be locked"));
        assert!(text(run(&p, "android_screen", json!({})).await).contains("Nothing could be read from the screen"));
        p.put("screen.xml", br#"<hierarchy><node text="OK &amp; go" resource-id="a:id/ok" content-desc="Go" clickable="true" bounds="[0,0][10,10]" /><node text="" bounds="[0,0][1,1]" /></hierarchy>"#);
        assert_eq!(text(run(&p, "android_screen", json!({})).await), "tap (5, 5) text=\"OK & go\" desc=\"Go\" id=ok clickable");
        assert!(text(run(&p, "android_logcat", json!({"lines": 5000})).await).contains("hello\n11-03 W/Other: world"));
        assert_eq!(text(run(&p, "android_logcat", json!({"tag": "Wysebee", "priority": "w", "contains": "nothing"})).await), "No log lines match.");
        assert_eq!(text(run(&p, "android_logcat", json!({"priority": "Z"})).await).lines().count(), 2);
        assert_eq!(text(run(&p, "android_logcat", json!({"package": "bad package"})).await), "Give `package` as com.example.app.");
        assert!(text(run(&p, "android_logcat", json!({"package": "com.a.app"})).await).contains("com.a.app is not running"));
        p.put("pid", b"4242\n");
        assert_eq!(text(run(&p, "android_logcat", json!({"package": "com.a.app", "contains": "HELLO"})).await), "11-03 I/Wysebee: hello");
        let calls = p.calls();
        assert!(calls.contains("logcat -d -v time -t 2000 *:V") && calls.contains("'Wysebee':W *:S") && calls.contains("--pid=4242"), "{calls}");
        assert_eq!(text(run(&p, "android_info", json!({})).await), "Nothing came back.");
        assert_eq!(text(run(&p, "android_apps", json!({})).await), "com.a.app\ncom.b.app");
        assert_eq!(text(run(&p, "android_apps", json!({"all": true, "contains": "zzz"})).await), "No apps match.");
        assert_eq!(text(run(&p, "android_files", json!({})).await), "The folder is empty.");
        assert!(p.calls().contains("pm list packages -3") && p.calls().contains("ls -la '/sdcard'"));
    }

    #[tokio::test]
    async fn downloading_a_file() {
        let p = Phone::new(ONE);
        assert!(text(run(&p, "android_pull", json!({"path": "/sdcard/DCIM/"})).await).contains("Give the file's `path`"));
        assert!(text(run(&p, "android_pull", json!({"path": "/sdcard/missing.txt"})).await).contains("It could not be downloaded: adb: error: remote object"));
        p.put("note.txt", b"hi");
        let note = run(&p, "android_pull", json!({"path": "/sdcard/note.txt"})).await.unwrap();
        assert!(note["text"].as_str().unwrap().starts_with("Downloaded /sdcard/note.txt") && note.get("image").is_none());
        assert_eq!(std::fs::read(p.home.path().join("Downloads").join("Wysebee").join("note.txt")).unwrap(), b"hi");
        p.put("pic.PNG", &png(4, 4));
        assert_eq!(run(&p, "android_pull", json!({"path": "/sdcard/pic.PNG"})).await.unwrap()["image"]["media_type"], "image/jpeg");
        // A home whose Downloads is a file: the folder cannot be made.
        let blocked = Phone::new(ONE);
        std::fs::write(blocked.home.path().join("Downloads"), "").unwrap();
        assert!(text(run(&blocked, "android_pull", json!({"path": "/sdcard/note.txt"})).await).contains("could not be made"));
    }

    #[tokio::test]
    async fn operating_the_phone() {
        let p = Phone::new(ONE);
        assert_eq!(text(run(&p, "android_tap", json!({"x": 10.0, "y": 20})).await), "Tapped (10, 20).");
        assert!(run(&p, "android_tap", json!({"x": -1, "y": 20})).await.is_err());
        assert!(text(run(&p, "android_tap", json!({"x": 999, "y": 1})).await).starts_with("The phone refused this"));
        assert_eq!(text(run(&p, "android_swipe", json!({"x1": 1, "y1": 2, "x2": 3, "y2": 4, "ms": 1})).await), "Swiped from (1, 2) to (3, 4) in 50 ms.");
        assert!(run(&p, "android_swipe", json!({"x1": 1})).await.is_err());
        assert_eq!(text(run(&p, "android_type", json!({"text": "hi there 5%"})).await), "Typed it.");
        assert!(text(run(&p, "android_type", json!({"text": ""})).await).contains("at most 500"));
        assert!(text(run(&p, "android_type", json!({"text": "café"})).await).contains("plain ASCII only"));
        assert_eq!(text(run(&p, "android_key", json!({"key": "HOME"})).await), "Pressed home.");
        assert!(text(run(&p, "android_key", json!({"key": "fly"})).await).contains("home, back"));
        assert!(text(run(&p, "android_open", json!({"url": "not a link"})).await).contains("Give `url` as a link"));
        assert_eq!(text(run(&p, "android_open", json!({"url": "https://example.com"})).await), "Opened it.");
        assert_eq!(text(run(&p, "android_open", json!({"package": "com.a.app"})).await), "Opened it.");
        assert!(text(run(&p, "android_open", json!({})).await).contains("Give an app's `package`"));
        let calls = p.calls();
        assert!(calls.contains("input text 'hi%sthere%s5\\%'") && calls.contains("input keyevent 3") && calls.contains("monkey -p 'com.a.app'"), "{calls}");
    }

    #[tokio::test]
    async fn changing_the_phone() {
        let p = Phone::new(ONE);
        assert!(text(run(&p, "android_install", json!({"apk": "nope/a.apk"})).await).contains("Give the path of an .apk"));
        let apk = p.home.path().join("app.APK");
        std::fs::write(&apk, "x").unwrap();
        assert_eq!(text(run(&p, "android_install", json!({"apk": apk.to_string_lossy()})).await), "Success");
        assert!(run(&p, "android_uninstall", json!({"package": "nope"})).await.is_err());
        assert_eq!(text(run(&p, "android_uninstall", json!({"package": "com.a.app"})).await), "Success");
        assert!(text(run(&p, "android_push", json!({"file": "nope/f"})).await).contains("Give the path of a file"));
        for (to, want) in [("", "/sdcard/Download/"), ("/sdcard/x", "/sdcard/x/"), ("/sdcard/y/", "/sdcard/y/")] {
            assert_eq!(text(run(&p, "android_push", json!({"file": "app.APK", "to": to})).await), format!("Copied to {want}."));
        }
        assert_eq!(text(run(&p, "android_shell", json!({"command": " "})).await), "Give the `command`.");
        assert!(text(run(&p, "android_shell", json!({"command": "su"})).await).contains("no root"));
        assert!(text(run(&p, "android_shell", json!({"command": "reboot bootloader"})).await).contains("bootloader"));
        assert_eq!(text(run(&p, "android_shell", json!({"command": "getprop"})).await), "It ran, and printed nothing.");
        assert_eq!(text(run(&p, "android_shell", json!({"command": "fail now"})).await), "It failed on the phone: it broke");
        assert_eq!(text(run(&p, "android_shell", json!({"command": "quietfail"})).await), "It failed on the phone: half done");
    }

    #[test]
    fn the_shell_refuses_root_and_the_bootloader() {
        for bad in ["su -c id", "ls; su", "/system/xbin/su", "reboot bootloader", "reboot  recovery", "echo x && reboot fastboot"] {
            assert!(refused(bad).is_some(), "{bad}");
        }
        for fine in ["dumpsys battery", "pm list packages", "reboot", "am start -n com.a/.Main"] {
            assert!(refused(fine).is_none(), "{fine}");
        }
        assert_eq!(quoted("it's"), "'it'\\''s'");
        assert!(package_ok("com.a_b.c") && !package_ok("nodot") && !package_ok("a b.c") && !package_ok(""));
        assert_eq!(key_names().len(), 16);
    }

    #[test]
    fn the_screen_is_cut_and_a_download_never_overwrites() {
        let xml: String = (0..MAX_NODES + 5).map(|i| format!("<node text=\"n{i}\" bounds=\"\" />")).collect();
        let lines = screen_lines(&xml);
        assert_eq!((lines.len(), lines[0].as_str()), (MAX_NODES, "no bounds text=\"n0\""));
        let home = tempfile::tempdir().unwrap();
        for (name, second) in [("README", "README (2)"), ("a.txt", "a (2).txt"), ("", "file (2)"), ("x:y", "x_y (2)")] {
            let first = download_path(home.path(), name);
            std::fs::create_dir_all(first.parent().unwrap()).unwrap();
            std::fs::write(&first, "x").unwrap();
            assert_eq!(download_path(home.path(), name).file_name().unwrap().to_string_lossy(), second);
        }
    }
}

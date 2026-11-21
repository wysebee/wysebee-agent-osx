//! What a program the AI names itself (`program_run`, Run any program) may not be — a
//! **safeguard, not a sandbox**.
//!
//! The program is the file found (`programs::resolve`), checked by its name without the
//! extension, any case: a tool that erases, partitions or encrypts disks, changes how the Mac
//! starts, deletes backups, changes accounts, permissions, services, scheduled jobs, the
//! system's settings, privacy or security, shuts the Mac down, wipes files, or installs
//! software — and every shell or script host (`zsh`, `bash`, `osascript`, …) and every program
//! that runs another one (`env`, `xargs`, `sudo`, …), which would get round this list. An interpreter is
//! refused when given code inline (`python -c`, `node -e`, …): a script file it runs is the
//! person's. A few everyday tools are refused only with the arguments that destroy data
//! (`git clean`, `git reset --hard`, a forced `git push`, `rsync --delete`).
//!
//! A program the **person** set up and allowed (`local_run`) is never checked here: they chose
//! it. Nothing here limits what a program does once it runs, or what is typed on the screen
//! (`screen_action`) or run through Developer access's shell (`dev_shell`, the person's other
//! switch): a list cannot be complete, which is why Run any program stays off until the person
//! turns it on.

use std::path::Path;

/// Disks, booting, recovery and backups.
const DISK: &[&str] = &[
    "diskutil", "asr", "fdisk", "gpt", "pdisk", "newfs_apfs", "newfs_hfs", "newfs_msdos", "newfs_exfat", "fsck", "fsck_apfs", "fsck_hfs", "hdiutil", "bless", "nvram", "csrutil", "tmutil", "fdesetup", "apfs", "mkfs", "wipefs", "dd",
];
/// Shells, script hosts and programs that run another program: each would run any command,
/// and so get round this list.
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "csh", "tcsh", "fish", "pwsh", "osascript", "osacompile", "automator", "shortcuts", "env", "xargs", "nohup", "nice", "renice", "timeout", "caffeinate", "arch", "script", "expect", "sudo", "su", "doas", "login", "exec", "launchctl", "at", "batch", "crontab", "find", "awk", "gawk", "sed",
];
/// The system's settings: accounts, permissions, services, privacy, security, the network,
/// power, software.
const SYSTEM: &[&str] = &[
    "defaults", "dscl", "dseditgroup", "sysadminctl", "pwpolicy", "security", "systemsetup", "networksetup", "scutil", "pmset", "spctl", "profiles", "tccutil", "softwareupdate", "installer", "pkgutil", "kextload", "kextunload", "kmutil", "chown", "chmod", "chflags", "chgrp", "xattr", "brew", "port", "mas",
];
/// Ending the session.
const POWER: &[&str] = &["shutdown", "reboot", "halt"];
/// Deleting files outright.
const WIPE: &[&str] = &["rm", "rmdir", "unlink", "srm", "shred"];
/// Interpreters, and their flags that take code inline.
const INLINE: &[(&[&str], &[&str])] = &[
    (&["python", "python3", "pythonw", "py"], &["-c"]),
    (&["node", "nodejs", "bun", "deno"], &["-e", "-p", "--eval", "--print", "eval"]),
    (&["ruby"], &["-e"]),
    (&["perl"], &["-e", "-E"]),
    (&["php"], &["-r"]),
    (&["rscript"], &["-e"]),
];

/// Why the AI may not run `exe` with `argv` on its own, or None when it may.
pub fn refusal(exe: &Path, argv: &[String]) -> Option<String> {
    // Its file name after the last `\` or `/` (on every platform), without the extension.
    let file = exe.to_string_lossy().rsplit(['\\', '/']).next().unwrap_or_default().to_lowercase();
    let name = file.rsplit_once('.').map_or(file.as_str(), |(stem, _)| stem).to_string();
    // Without a version at its end too: `python3.12` is `python`, `pwsh7` is `pwsh`.
    let base = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let is = |list: &[&str]| list.contains(&name.as_str()) || list.contains(&base);
    let why = if is(DISK) {
        Some("it works on disks, backups or how the Mac starts")
    } else if is(SHELLS) {
        Some("it is a shell or script host, which would run any command")
    } else if is(SYSTEM) {
        Some("it changes the Mac's settings, accounts, permissions, security or software")
    } else if is(POWER) {
        Some("it would shut down or restart this Mac")
    } else if is(WIPE) {
        Some("it deletes files outright")
    } else if INLINE.iter().any(|(names, flags)| is(names) && argv.iter().any(|a| flags.contains(&a.as_str()))) {
        Some("code given inline to an interpreter is not run; run a script file instead")
    } else {
        destroys_data(&name, argv)
    };
    why.map(|why| format!("Wysebee Agent will not run '{name}' when the AI chooses it: {why}. If the person wants it, they can set it up as a program tool in Wysebee and allow it in Wysebee Agent."))
}

/// An everyday tool given the arguments that destroy data.
fn destroys_data(name: &str, argv: &[String]) -> Option<&'static str> {
    let has = |words: &[&str]| argv.iter().any(|a| words.iter().any(|w| a.eq_ignore_ascii_case(w)));
    let first = argv.first().map(|a| a.to_lowercase()).unwrap_or_default();
    match name {
        "git" if first == "clean" || (first == "reset" && has(&["--hard"])) || (first == "push" && (has(&["-f", "--force", "--force-with-lease", "--mirror", "--delete", "-d"]) || argv.iter().any(|a| a.starts_with('+')))) => Some("those git arguments throw work away"),
        "rsync" if argv.iter().any(|a| a.starts_with("--delete") || a == "--remove-source-files") => Some("those rsync arguments delete files"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn says(exe: &str, argv: &[&str]) -> Option<String> {
        refusal(Path::new(exe), &argv.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn dangerous_programs_are_refused_by_name_in_any_case_and_folder() {
        for exe in ["/usr/sbin/diskutil", "ASR", "tmutil", "csrutil", "nvram", "/usr/bin/hdiutil"] {
            assert!(says(exe, &[]).unwrap().contains("disks, backups or how the Mac starts"), "{exe}");
        }
        for exe in ["/bin/zsh", "bash", "/usr/bin/osascript", "/usr/bin/env", "xargs", "/usr/bin/sudo", "launchctl", "find", "/opt/homebrew/bin/fish"] {
            assert!(says(exe, &[]).unwrap().contains("shell or script host"), "{exe}");
        }
        for exe in ["defaults", "dscl", "security", "/usr/sbin/softwareupdate", "chmod", "/opt/homebrew/bin/brew", "tccutil"] {
            assert!(says(exe, &[]).unwrap().contains("settings, accounts"), "{exe}");
        }
        assert!(says("/sbin/shutdown", &["-h", "now"]).unwrap().contains("shut down"));
        assert!(says("bash5", &[]).unwrap().contains("shell or script host"), "a version at its end");
        assert!(says("/bin/rm", &["-rf", "x"]).unwrap().contains("deletes files outright"));
        let said = says("diskutil", &["eraseDisk"]).unwrap();
        assert!(said.starts_with("Wysebee Agent will not run 'diskutil' when the AI chooses it: ") && said.ends_with("allow it in Wysebee Agent."), "{said}");
    }

    #[test]
    fn interpreters_run_script_files_but_not_inline_code() {
        assert_eq!(says("python", &["tests/run.py", "-v"]), None);
        assert_eq!(says("node", &["server.js"]), None);
        for (exe, argv) in [("python", vec!["-c", "import os"]), ("py", vec!["-3", "-c", "x"]), ("node", vec!["-e", "x"]), ("node", vec!["--eval", "x"]), ("perl", vec!["-E", "x"]), ("php", vec!["-r", "x"]), ("ruby", vec!["-e", "x"]), ("Rscript", vec!["-e", "x"]), ("deno", vec!["eval", "x"]), ("python3.12", vec!["-c", "x"])] {
            assert!(says(exe, &argv).unwrap().contains("inline"), "{exe} {argv:?}");
        }
    }

    #[test]
    fn everyday_tools_are_refused_only_with_the_arguments_that_destroy_data() {
        for argv in [vec!["status"], vec!["log", "-1"], vec!["reset", "HEAD~1"], vec!["push", "origin", "main"], vec![]] {
            assert_eq!(says("git", &argv), None, "{argv:?}");
        }
        for argv in [vec!["clean", "-fdx"], vec!["reset", "--hard"], vec!["push", "--force"], vec!["push", "-f", "origin"], vec!["push", "origin", "+main"], vec!["push", "--delete", "origin", "x"]] {
            assert!(says("git", &argv).unwrap().contains("throw work away"), "{argv:?}");
        }
        assert_eq!(says("rsync", &["-a", "a/", "b/"]), None);
        assert!(says("rsync", &["-a", "--delete-after", "a/", "b/"]).unwrap().contains("delete files"));
        assert!(says("rsync", &["--remove-source-files", "a", "b"]).unwrap().contains("delete files"));
        for exe in ["sw_vers", "system_profiler", "/usr/bin/open", "whoami", "xcodebuild", "", "/usr/bin/git"] {
            assert_eq!(says(exe, &[]), None, "{exe}");
        }
    }
}

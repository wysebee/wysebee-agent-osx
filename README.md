# Wysebee Agent for Mac

A small app in the macOS menu bar that lets **one Wysebee agent run the programs you
allow on this Mac**, and the scripts of the skills you allow, and **find, read and (where
you say so) write files in the folders you pick here**.

Wysebee Desktop is sandboxed (for the Mac App Store) and no longer starts programs itself.
"Run local program" tools run through this app instead: it pairs with Wysebee Cloud, is
handed calls for the programs one agent's settings hold, and runs each **only after you
allowed it here, on the Mac**. It runs no model. It is distributed outside the store
(Developer ID) and is not sandboxed.

It speaks Wysebee's agent-link protocol with Wysebee Cloud.

## How it works

1. Open Wysebee Agent, type a name for a new agent, click **Pair**. It shows a code
   (`XXXX-XXXX`).
2. In Wysebee, under **Devices**, type the code: on the website (the agent then lives in
   Wysebee Cloud, which runs it, so the Mac needs no Wysebee Desktop) or in Wysebee Desktop
   (it runs there). Pairing creates a new agent; if the name is taken by a Mac agent with no Mac now,
   Wysebee offers to reconnect to it instead (nothing is allowed until you click Allow here
   again). Only that agent runs programs on
   this Mac.
3. In Wysebee, on that agent's **Tools** tab, add a "Run local program" tool: a program,
   its fixed arguments and the blanks the agent fills.
4. The program shows up in Wysebee Agent's window with its whole command line, **waiting**.
   Click **Allow**. Change it in Wysebee later and it waits again.
5. The agent can now run it. The switch "Let Wysebee run programs on this Mac" pauses
   everything; **Unpair** ends it.

### Skills

Wysebee Desktop no longer runs a skill's scripts: they run **only for an agent paired with
this app, and only here**. Give the agent a skill in Wysebee (its Skills tab); it shows up
under **Skills** in this window, **waiting**, with the scripts its `SKILL.md` declares (each
command line) and its files. As soon as a skill is given to the agent, or changed in
Wysebee, the app downloads its files from Wysebee Cloud, checks each one against its
SHA-256 and keeps them on this Mac ("Downloaded to this Mac") — nothing of it runs yet.
Click **Allow**: the app puts them in the skill's folder here and finds each script's
program. Change any file of the skill in Wysebee and it is downloaded again at once, and
waits for Allow again; until then the copy you allowed stays as it was, and nothing of the
new one runs.

**Show in Finder** opens the skill's folder on this Mac. Files you put there yourself (a
`.env` the scripts read, say — Wysebee never syncs files that look like secrets) **stay on
this Mac and are never sent**; an Allow replaces only the skill's synced files.

### Folders

Local Folder moved here from Wysebee Desktop (a sandboxed app cannot open a folder path
somebody typed). Under **Folders**, click **Add a folder…**: the Mac's own folder picker opens.
Give it a name (the folder's own name if you leave it empty), choose **Read only** or **Read
and write**, and say what it holds if you like. The agent's folder tools (`folder_find`,
`folder_list`, `folder_read`, `folder_write`) then work in it: text, Word (`.docx`), Excel
(`.xlsx`, `.xlsm`, `.xls`, `.ods`) and PDF are read as text; a picture (`.png`, `.jpg`,
`.jpeg`, `.gif`, `.webp`) is shown to you in Wysebee and never to the model. **Allow writing**
/ **Make read only**, **Edit** and **Remove…** change it later. The folders are numbered here
as the agent sees them. There is nothing to approve: you chose each one here.

### What else Wysebee may do here (Settings)

The window has three tabs: **Programs** (shown when the agent has programs or skills here),
**Folders** and **Settings**, in light or dark (Settings → Appearance: System, Light, Dark).
Under Settings, "What Wysebee may do on this Mac", besides running the programs you allow:

- **Run any program**: the agent names a program and its arguments itself. Disk, backup,
  system-settings and software tools, shells (and anything that runs another program, like
  `sudo` or `env`) and `rm` are refused.
- **See and control the screen**: screenshots, the mouse and the keyboard, with a red ring
  where it clicks. macOS asks you to allow Screen Recording and Accessibility.
- **Work as a developer**: zsh and bash commands, and files anywhere on this Mac, as you —
  builds can run in the background.
- **Use Android phones**: the phones and emulators adb sees (Android Studio's SDK, or
  `brew install android-platform-tools`).
- **Use iPhones and simulators**: through Xcode — boot simulators, take their screenshots,
  read their logs, open links, switch light and dark; install, launch and remove apps on a
  simulator or on an iPhone paired with this Mac (Xcode 15 or later for an iPhone). To tap a
  simulator, the agent uses screen control on the Simulator window.

Each is off until you turn it on (and confirm), and is forgotten when you unpair. The agent
in Wysebee needs the matching tool too (its Tools tab).

## The security model

- **You name the program and its fixed arguments; the model only fills blanks.** A call
  from the cloud carries a program's *name* and the blanks' values, nothing else. What runs
  is this Mac's own approved copy: the program's absolute path as resolved when you clicked
  Allow, with the arguments you saw.
- **Never a shell.** The program is started with an argument list; a value is one
  argument, whatever it holds (`a b; rm -rf / $(x)` is just that text).
- **Approval is local.** It is kept in this app's state file and never taken from the
  cloud. A program is the same while its name, command and arguments (each fixed value and
  each blank's name, in order) are unchanged; its description is not part of it. Anything
  else changed: it waits again and is refused until allowed. Removed in Wysebee: its
  approval is lost.
- **Bounded.** No input, 60 seconds (then it is killed), output and errors cut at 20,000
  characters each, run in your home folder.
- **Skills: only declared scripts.** A `skill_run` call carries a skill's name, a script's
  name and up to 30 arguments. What runs is the command that script has in the approved
  copy's `SKILL.md`, then the arguments, each one argument (never a shell), in the skill's
  folder here, with no input, for at most 60 seconds, output cut as above. A script's
  program is the first word of its command: a path with `/` is a file inside the skill's
  folder (a link leading out, or `..`, is refused, at Allow and again at each run), any
  other word a program on the app's `PATH` plus Homebrew's folders, found when you allow.
  A skill is the same while its synced files (each path and SHA-256) are unchanged; a file
  that does not match its SHA-256 is refused and nothing is allowed. A skill no longer
  given to the agent loses its approval; its folder stays until you unpair.
- **Folders: only the ones you picked, only as you allowed.** A `folder_call` names a tool,
  a folder by the id this app gave it and the model's arguments. A folder this app does not
  have is refused; a path is relative to the folder, never absolute, never `..`, and is
  resolved through links and must still be inside the folder; writing needs a folder you
  made read and write, whatever the call says. Bounded as Wysebee Desktop's tools were:
  files read up to 50 MB (a picture 10 MB), a search over at most 50,000 entries answering
  200, a listing 500 entries, sheets 5,000 rows, a write 1,000,000 characters, every answer
  cut at 50,000 characters; `.git`, `node_modules`, `.Trash` and `__pycache__` are skipped by
  a search. Paused, nothing is answered.
- **Four operations only**: `ping`, `local_run`, `skill_run` and `folder_call`. Anything else
  is refused.
- **The link's secret** (32 random bytes) is made here and kept in the macOS Keychain
  (service `com.wysebee.agent.osx`); the cloud keeps only its SHA-256. It is sent only to
  the cloud the pairing was made with, and is never in the state file or the log.
- **The cloud's address** must be `https://`, or plain `http://` to this computer.
- **What Wysebee Cloud sees**: the program definitions (they are the agent's settings),
  the skills' synced files (it holds them), each call's blank values or script arguments
  and the output, kept until read, an hour at most. It also sees your folders' names, paths,
  permissions and descriptions (it puts them in the agent's settings), and what a folder tool
  read or wrote passes through it like any answer — a picture too. Nothing else is uploaded
  from this Mac.

## Build, run, test

Rust (stable). Nothing else: the window is plain HTML/CSS/JS in `ui/`, with no build step.

```bash
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH   # Homebrew's rustup is keg-only

cargo build --workspace
cargo test --workspace            # agent-core against a fake cloud on 127.0.0.1; nothing real is touched
./scripts/check.sh                # before each commit: clippy, every test, 100% of agent-core's lines
cargo run -p agent-core --example headless -- <state dir> "<agent name>"
                                  # no window: <state dir>/allow stands in for the Allow
                                  # clicks and the folder picker (folder:<path>, folder-write:<path>)
cargo run -p agent-app            # the app: an icon in the menu bar and its window
```

### For development: `scripts/run_dev.sh`

```bash
./scripts/run_dev.sh                 # from source, against Wysebee Cloud
./scripts/run_dev.sh --url <address> # against another cloud
./scripts/run_dev.sh --fresh         # forget this cloud's dev state first
./scripts/run_dev.sh --headless "Dev agent"   # the headless example instead of the window
```

It sets `WYSEBEE_CLOUD_URL` and a state folder of its own per cloud
(`~/Library/Application Support/Wysebee Agent Dev/<prod|custom>/`), so it runs beside an
installed copy, paired as an agent of its own; the log goes to the terminal. The rest of
this section is what the script does by hand.

### Against another cloud

```bash
WYSEBEE_CLOUD_URL=https://cloud.example.com cargo run -p agent-app
```

The window has no way to change the address: a release talks to Wysebee Cloud
(`https://app.wysebee.com`), development sets the variable. Then pair as above, typing the
code in a Wysebee that talks to the same cloud.

To keep a development copy apart from an installed one, move its state folder:

```bash
WYSEBEE_AGENT_STATE_DIR=/tmp/wysebee-agent-dev cargo run -p agent-app
```

(A state folder other than the default has a Keychain item of its own, so the two copies'
pairings never replace each other. A development build is signed differently from an
installed one, so macOS may ask before letting it read its item.)

### Release builds

Wysebee builds, signs, notarizes and packages the downloadable app with its own release
tooling, outside this repository. `cargo build --release -p agent-app --features
custom-protocol` builds the same binary. The app needs no entitlements, and the App
Sandbox is off on purpose.

## Where things live

| What | Where |
| --- | --- |
| State: the pairing (link id, agent, cloud address), the pause switch, the agent's programs as the cloud last sent them, what you allowed, the folders you picked (forgotten when you unpair) | `~/Library/Application Support/Wysebee Agent/state.json` |
| The link's secret | Keychain, generic password, service `com.wysebee.agent.osx`, account `link-secret` (another state folder: `link-secret <16 hex digits of the folder path's SHA-256>`) |
| The approved copy of each skill (its synced files, and any you put there) | `~/Library/Application Support/Wysebee Agent/skills/<name>/` |
| Skill files downloaded and checked, by SHA-256 (shown while a skill waits; never run) | `~/Library/Application Support/Wysebee Agent/skill-cache/` |
| The log (truncated per launch; to the terminal instead when started from one) | `~/Library/Application Support/Wysebee Agent/agent.log` |
| Start at login | a LaunchAgent in `~/Library/LaunchAgents` (written by tauri-plugin-autostart) |

## Layout

```
Cargo.toml          the workspace; the version lives here and only here
crates/agent-core   everything bar the window, and every test: programs.rs, skills.rs,
                    folders.rs, approvals.rs, secrets.rs, cloud.rs, link.rs, guard.rs,
                    screen.rs, dev.rs, android.rs, ios.rs, agent.rs; tests/ against a fake
                    cloud
crates/agent-app    the Tauri 2 shell: main.rs (window, commands), screen.rs (the real
                    screen), tray.rs (menu bar)
ui/                 the window: index.html, style.css, theme.js, app.js; pointer.* (the ring)
scripts/            run_dev.sh (run from source against a cloud), check.sh
```

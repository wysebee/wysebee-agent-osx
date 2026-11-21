// Wysebee Agent's window. Plain JS, no build step.
//
// It asks the shell for `status` each second and draws it; every button is one command.
// Everything that comes from Wysebee Cloud (an agent's name, a program's name,
// description and command line, a skill's name, scripts and file paths) — and every
// folder's name, path and description — is put on the page as text, never as HTML.

'use strict';

const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);

let last = null;
// The tab shown: the person's last choice, kept for the next launch.
let tab = null;
try {
  tab = localStorage.getItem('wysebee-tab');
} catch {
  // No storage: the default below.
}
let drawnPrograms = '';
let drawnSkills = '';
let drawnFolders = '';
// The folder being changed (its id), or added (null with a path); and the one whose
// removal is being confirmed.
let editing = null;
let confirmingRemove = null;

function showError(message) {
  $('error').textContent = message || '';
  $('error').hidden = !message;
}

// One command; its refusal is shown, and the status is read again either way.
async function run(command, args) {
  showError('');
  try {
    await invoke(command, args || {});
  } catch (e) {
    showError(String(e));
  }
  await refresh();
}

function timeLeft(seconds) {
  if (seconds <= 0) return 'The code has run out.';
  const m = Math.floor(seconds / 60);
  const s = String(seconds % 60).padStart(2, '0');
  return `Valid for ${m}:${s}.`;
}

const STATE_TEXT = {
  'approved': 'Allowed',
  'waiting': 'Waiting for you',
  'changed': 'Changed in Wysebee: waiting for you',
  'missing-program': 'Allowed, but the program is gone',
};

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function drawPrograms(programs) {
  const key = JSON.stringify(programs);
  if (key === drawnPrograms) return;
  drawnPrograms = key;
  const list = $('programs');
  list.replaceChildren();
  for (const p of programs) {
    const needs = p.status === 'waiting' || p.status === 'changed';
    const item = el('li', 'program' + (needs ? ' needs' : ''));
    const head = el('div', 'head');
    head.append(el('span', 'name', p.name), el('span', 'state' + (p.status === 'approved' ? ' ok' : ''), STATE_TEXT[p.status] || p.status));
    item.append(head);
    if (p.description) item.append(el('div', 'muted', p.description));
    item.append(el('code', '', p.command_line));
    item.append(el('div', 'where', p.path ? `Program: ${p.path}` : 'This program is not on this Mac.'));
    const actions = el('div', 'actions row');
    if (p.status === 'approved') {
      const revoke = el('button', '', 'Revoke');
      revoke.addEventListener('click', () => run('revoke', { name: p.name }));
      actions.append(revoke);
    } else {
      const allow = el('button', 'primary', p.status === 'missing-program' ? 'Allow again' : 'Allow');
      allow.addEventListener('click', () => run('allow', { name: p.name }));
      actions.append(allow);
      if (p.status !== 'waiting') {
        const revoke = el('button', '', 'Revoke');
        revoke.addEventListener('click', () => run('revoke', { name: p.name }));
        actions.append(revoke);
      }
    }
    item.append(actions);
    list.append(item);
  }
}

const SKILL_STATE_TEXT = {
  'approved': 'Allowed',
  'waiting': 'Waiting for you',
  'changed': 'Changed in Wysebee: waiting for you',
  'missing-program': 'Allowed, but a script\'s program is gone',
};

const CHANGE_TEXT = { added: 'new', changed: 'changed', removed: 'removed' };

function drawSkills(skills) {
  const key = JSON.stringify(skills);
  if (key === drawnSkills) return;
  drawnSkills = key;
  const list = $('skills');
  list.replaceChildren();
  for (const k of skills) {
    const needs = k.status === 'waiting' || k.status === 'changed';
    const item = el('li', 'program' + (needs ? ' needs' : ''));
    const head = el('div', 'head');
    head.append(el('span', 'name', k.name), el('span', 'state' + (k.status === 'approved' ? ' ok' : ''), SKILL_STATE_TEXT[k.status] || k.status));
    item.append(head);
    if (k.note) item.append(el('div', 'muted', k.note));
    if (k.scripts === null) {
      if (!k.note) item.append(el('div', 'muted', 'Reading its SKILL.md…'));
    } else if (k.scripts.length === 0) {
      item.append(el('div', 'muted', 'It declares no scripts: nothing of it runs here.'));
    } else {
      item.append(el('div', 'muted', 'Scripts it runs, each with the arguments the agent adds after:'));
      for (const sc of k.scripts) {
        const line = el('div', 'script');
        line.append(el('span', 'script-name', sc.name));
        line.append(el('code', '', sc.command));
        if (sc.description) line.append(el('div', 'muted', sc.description));
        item.append(line);
      }
    }
    if (needs) {
      // Downloaded at once when it changed in Wysebee (2026-11-13); it runs only once allowed.
      if (k.download_error) item.append(el('div', 'muted', `Not all of it could be downloaded yet: ${k.download_error}`));
      else item.append(el('div', 'muted', k.stored ? 'Downloaded to this Mac. Nothing of it runs until you allow it.' : 'Downloading to this Mac…'));
    }
    if (needs && k.changes.length > 0) {
      item.append(el('div', 'muted', k.status === 'changed' ? 'Files changed since you allowed it:' : 'Its files:'));
      const files = el('ul', 'files');
      for (const c of k.changes) files.append(el('li', '', `${c.path} (${CHANGE_TEXT[c.change] || c.change})`));
      item.append(files);
    }
    const actions = el('div', 'actions row');
    if (k.status === 'approved') {
      const revoke = el('button', '', 'Revoke');
      revoke.addEventListener('click', () => run('revoke_skill', { name: k.name }));
      actions.append(revoke);
    } else {
      const allow = el('button', 'primary', k.status === 'missing-program' ? 'Allow again' : 'Allow');
      allow.addEventListener('click', async () => {
        allow.disabled = true;
        allow.textContent = k.stored ? 'Allowing…' : 'Downloading…';
        await run('allow_skill', { name: k.name });
        drawnSkills = '';
        await refresh();
      });
      actions.append(allow);
      if (k.status !== 'waiting') {
        const revoke = el('button', '', 'Revoke');
        revoke.addEventListener('click', () => run('revoke_skill', { name: k.name }));
        actions.append(revoke);
      }
    }
    const finder = el('button', '', 'Show in Finder');
    finder.addEventListener('click', () => run('show_skill_folder', { name: k.name }));
    actions.append(finder);
    item.append(actions);
    item.append(el('div', 'where', 'Files you put in this folder stay on this Mac and are never sent.'));
    list.append(item);
  }
}

const MODE_TEXT = { read: 'Read only', write: 'Read and write' };

function drawFolders(folders) {
  const key = JSON.stringify([folders, confirmingRemove]);
  if (key === drawnFolders) return;
  drawnFolders = key;
  const list = $('folders');
  list.replaceChildren();
  $('no-folders').hidden = folders.length > 0;
  for (const f of folders) {
    const item = el('li', 'program');
    const head = el('div', 'head');
    head.append(el('span', 'name', `${f.number}. ${f.name}`), el('span', 'state', MODE_TEXT[f.mode] || f.mode));
    item.append(head);
    if (f.description) item.append(el('div', 'muted', f.description));
    item.append(el('code', '', f.path));
    if (!f.present) item.append(el('div', 'warn', 'This folder is not there now: the agent finds nothing in it.'));
    const actions = el('div', 'actions row');
    if (confirmingRemove === f.id) {
      actions.append(el('span', 'muted', 'Remove it? The agent can no longer reach it.'));
      const yes = el('button', 'danger', 'Remove');
      yes.addEventListener('click', async () => {
        confirmingRemove = null;
        await run('remove_folder', { id: f.id });
      });
      const no = el('button', '', 'Keep it');
      no.addEventListener('click', () => { confirmingRemove = null; drawFolders(folders); });
      actions.append(yes, no);
    } else {
      const other = f.mode === 'write' ? 'read' : 'write';
      const toggle = el('button', '', other === 'write' ? 'Allow writing' : 'Make read only');
      toggle.addEventListener('click', () => run('update_folder', { id: f.id, name: f.name, mode: other, description: f.description }));
      const edit = el('button', '', 'Edit');
      edit.addEventListener('click', () => openFolderForm(f));
      const remove = el('button', 'danger', 'Remove…');
      remove.addEventListener('click', () => { confirmingRemove = f.id; drawFolders(folders); });
      actions.append(toggle, edit, remove);
    }
    item.append(actions);
    list.append(item);
  }
}

// The form, to add the folder at `path` (a string) or change the folder `f` (an object).
function openFolderForm(target) {
  const adding = typeof target === 'string';
  editing = adding ? { id: null, path: target } : { id: target.id, path: target.path };
  $('folder-path').textContent = editing.path;
  $('folder-name').value = adding ? '' : target.name;
  $('folder-name').placeholder = adding ? (target.split('/').filter(Boolean).pop() || target) : '';
  $('folder-description').value = adding ? '' : target.description;
  const mode = adding ? 'read' : target.mode;
  for (const r of document.querySelectorAll('input[name="folder-mode"]')) r.checked = r.value === mode;
  $('folder-save').textContent = adding ? 'Add' : 'Save';
  $('folder-form').hidden = false;
  $('folder-add-row').hidden = true;
  $('folder-name').focus();
}

function closeFolderForm() {
  editing = null;
  $('folder-form').hidden = true;
  $('folder-add-row').hidden = false;
}

// Show one tab (programs, folders or settings), and remember it.
function showTab(name) {
  tab = name;
  try {
    localStorage.setItem('wysebee-tab', name);
  } catch {
    // Not kept: fine.
  }
  for (const button of document.querySelectorAll('.tabs [data-tab]')) {
    const on = button.dataset.tab === name;
    button.setAttribute('aria-selected', on ? 'true' : 'false');
    button.tabIndex = on ? 0 : -1;
    $(`panel-${button.dataset.tab}`).hidden = !on;
  }
}

function draw(s) {
  const first = last === null;
  last = s;
  $('mac').textContent = s.mac_name;
  const unpaired = s.state === 'unpaired' || s.state === 'refused';
  $('unpaired').hidden = !unpaired;
  $('waiting').hidden = s.state !== 'waiting';
  $('paired').hidden = s.state !== 'paired';

  if (unpaired) {
    $('note').textContent = s.note || '';
    $('note').hidden = !s.note;
    $('refusal').textContent = s.message || '';
    $('refusal').hidden = !s.message;
  }
  if (s.state === 'waiting') {
    $('code').textContent = s.code || '';
    $('expires').textContent = s.expires_in == null ? '' : timeLeft(s.expires_in);
    $('typed-name').textContent = s.name || '';
  }
  $('agent-chip').hidden = s.state !== 'paired';
  if (s.state === 'paired') {
    $('agent-label').textContent = s.agent.label || s.agent.name;
    $('agent-handle').textContent = '@' + s.agent.name;
    $('connected').textContent = s.connected ? 'Connected' : 'Not connected';
    $('connected').className = 'badge' + (s.connected ? ' on' : '');
    $('run-switch').checked = !s.paused;
    $('paused-note').hidden = !s.paused;
    // While its confirmation shows, a switch stays off until Turn on; one that is on is marked.
    for (const [key, on] of [['any', s.any_program], ['screen', s.screen_control], ['dev', s.dev_access], ['android', s.android_dev], ['ios', s.ios_dev]]) {
      if ($(`${key}-confirm`).hidden) $(`${key}-switch`).checked = !!on;
      $(`row-${key}`).classList.toggle('on', !!on);
    }
    $('autostart').checked = !!s.autostart;
    // The Programs tab only when the agent has programs or skills here; its count, what waits.
    const any = s.programs.length > 0 || s.skills.length > 0;
    $('tab-programs').hidden = !any;
    $('programs-block').hidden = s.programs.length === 0;
    $('skills-block').hidden = s.skills.length === 0;
    $('waiting-count').textContent = String(s.waiting);
    $('waiting-count').hidden = !s.waiting;
    // At launch, what waits for the person comes first; else their last tab.
    let want = first && s.waiting > 0 ? 'programs' : tab || (any ? 'programs' : 'settings');
    if (want === 'programs' && !any) want = 'settings';
    if (want !== tab || $(`panel-${want}`).hidden) showTab(want);
    drawPrograms(s.programs);
    drawSkills(s.skills);
    drawFolders(s.folders);
  } else {
    closeFolderForm();
    confirmingRemove = null;
    $('unpair-confirm').hidden = true;
    $('unpair-row').hidden = false;
  }

  $('version').textContent = `Wysebee Agent ${s.version}`;
}

async function refresh() {
  try {
    draw(await invoke('status'));
  } catch (e) {
    showError(String(e));
  }
}

$('pair-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  $('pair').disabled = true;
  await run('start_pairing', { name: $('agent-name').value });
  $('pair').disabled = false;
});
$('cancel').addEventListener('click', () => run('cancel_pairing'));

// The code's copy button (as the browser extension's): copies the code shown at the click
// (it is refreshed each second), turns to a check for 1.5 s, then back. A copy the web view
// refuses changes nothing.
{
  const button = $('copy-code');
  let timer = 0;
  const show = (copied) => {
    const label = copied ? 'Copied' : 'Copy the code';
    button.classList.toggle('copied', copied);
    button.setAttribute('aria-label', label);
    button.title = label;
  };
  button.addEventListener('click', async () => {
    const code = $('code').textContent.trim();
    if (!code) return;
    try {
      await navigator.clipboard.writeText(code);
    } catch {
      return;
    }
    show(true);
    clearTimeout(timer);
    timer = setTimeout(() => show(false), 1500);
  });
}
for (const button of document.querySelectorAll('.tabs [data-tab]')) {
  button.addEventListener('click', () => showTab(button.dataset.tab));
}

// Appearance: System (macOS' setting), Light or Dark — the page, and the window's own
// title bar (set_theme).
function applyTheme(choice) {
  if (choice === 'light' || choice === 'dark') document.documentElement.dataset.theme = choice;
  else delete document.documentElement.dataset.theme;
  for (const b of document.querySelectorAll('[data-theme-choice]')) b.setAttribute('aria-checked', b.dataset.themeChoice === (choice || 'system') ? 'true' : 'false');
  invoke('set_theme', { theme: choice === 'light' || choice === 'dark' ? choice : null }).catch(() => {});
}
for (const b of document.querySelectorAll('[data-theme-choice]')) {
  b.addEventListener('click', () => {
    const choice = b.dataset.themeChoice === 'system' ? null : b.dataset.themeChoice;
    try {
      if (choice) localStorage.setItem('wysebee-theme', choice);
      else localStorage.removeItem('wysebee-theme');
    } catch {
      // Not kept: this launch only.
    }
    applyTheme(choice);
  });
}
applyTheme(document.documentElement.dataset.theme || null);

$('run-switch').addEventListener('change', (event) => run('set_paused', { paused: !event.target.checked }));
$('autostart').addEventListener('change', (event) => run('set_autostart', { on: event.target.checked }));
// A switch that asks before it turns on: off at once, on only after "Turn on".
function confirmedSwitch(key, command) {
  $(`${key}-switch`).addEventListener('change', (event) => {
    if (!event.target.checked) {
      $(`${key}-confirm`).hidden = true;
      run(command, { on: false });
      return;
    }
    event.target.checked = false;
    $(`${key}-confirm`).hidden = false;
  });
  $(`${key}-on`).addEventListener('click', async () => {
    $(`${key}-confirm`).hidden = true;
    $(`${key}-switch`).checked = true;
    await run(command, { on: true });
  });
  $(`${key}-cancel`).addEventListener('click', () => {
    $(`${key}-confirm`).hidden = true;
    $(`${key}-switch`).checked = false;
  });
}
confirmedSwitch('any', 'set_any_program');
confirmedSwitch('screen', 'set_screen_control');
confirmedSwitch('dev', 'set_dev_access');
confirmedSwitch('android', 'set_android_dev');
confirmedSwitch('ios', 'set_ios_dev');

$('unpair').addEventListener('click', () => {
  $('unpair-confirm').hidden = false;
  $('unpair-row').hidden = true;
});
$('unpair-no').addEventListener('click', () => {
  $('unpair-confirm').hidden = true;
  $('unpair-row').hidden = false;
});
$('unpair-yes').addEventListener('click', async () => {
  await run('unpair');
  drawnPrograms = '';
  drawnSkills = '';
  drawnFolders = '';
});
$('folder-add').addEventListener('click', async () => {
  showError('');
  try {
    const path = await invoke('choose_folder');
    if (path) openFolderForm(path);
  } catch (e) {
    showError(String(e));
  }
});
$('folder-cancel').addEventListener('click', closeFolderForm);
$('folder-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  if (!editing) return;
  const mode = document.querySelector('input[name="folder-mode"]:checked').value;
  const fields = { name: $('folder-name').value, mode, description: $('folder-description').value };
  const command = editing.id ? 'update_folder' : 'add_folder';
  const args = editing.id ? { id: editing.id, ...fields } : { path: editing.path, ...fields };
  showError('');
  try {
    await invoke(command, args);
    closeFolderForm();
  } catch (e) {
    // A refusal keeps the form open, to be corrected.
    showError(String(e));
  }
  await refresh();
});
// No browser context menu (Reload, Inspect…), bar on text fields.
document.addEventListener('contextmenu', (event) => {
  if (!(event.target instanceof HTMLInputElement)) event.preventDefault();
});

refresh();
setInterval(refresh, 1000);

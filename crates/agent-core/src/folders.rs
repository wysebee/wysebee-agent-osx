//! Local folders (the agent-link protocol, "Local folders"): folders on this Mac the person picked
//! in this app's window, which the paired agent may find and read files in — and, where the
//! person said so, write.
//!
//! **Ported from Wysebee Desktop's `folders.rs`** (git HEAD of 2026-11-12), which ran these
//! tools until Local Folder left it: [`resolve`], the search, the listing, the reading of
//! text, Word, Excel, PDF and pictures, the write, every limit and every skipped name are
//! its, so an agent finds them answering as they did. What differs:
//!
//! - **The folders are this app's own** ([`Folder`]: an id this app made, a name, the path,
//!   the permission, a description), picked in its window and kept in its state file; the
//!   cloud and Wysebee Desktop never hold a path this app acts on. A call names a folder by
//!   its id ([`call`]); an id this app does not have is refused.
//! - Where the desktop answered a folder's **number** (a search's results), this app answers
//!   its number in the order it reports its folders (1 = the first), which is the order
//!   Wysebee Desktop numbers them in for the model.
//! - A call answers `{"text", "picture"?}`: what the desktop's tool gave the model, and a
//!   picture it showed the person alone ([`Read::Shown`]).
//!
//! - **Nothing outside the folders**: a path is relative to its folder, never absolute, never
//!   `..`; what it names is resolved through links and must still be inside the folder
//!   ([`resolve`]). Writing refuses a read-only folder, whatever the call says.
//! - **Bounded**: files read up to [`MAX_FILE_BYTES`] (a picture [`MAX_IMAGE_BYTES`]), text
//!   cut to [`MAX_TOOL_RESULT_CHARS`] ([`truncate`]), a search over at most [`MAX_SCANNED`]
//!   entries answering [`MAX_FOUND`], a listing [`MAX_LISTED`], sheets [`MAX_ROWS`] rows, a
//!   write [`MAX_WRITE_CHARS`].

use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// At most this many folders.
pub const MAX_FOLDERS: usize = 20;
/// A folder's name: one line of at most this many characters.
pub const MAX_NAME: usize = 60;
/// A folder's path, as reported: at most this many characters.
pub const MAX_PATH: usize = 1_000;
/// A folder's description: at most this many characters.
pub const MAX_TEXT: usize = 1_000;
/// A file read whole, at most (text, Word, Excel, PDF).
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// A picture shown, at most.
pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
/// What one write may hold.
pub const MAX_WRITE_CHARS: usize = 1_000_000;
pub const MAX_FOUND: usize = 200;
pub const MAX_SCANNED: usize = 50_000;
pub const MAX_LISTED: usize = 500;
/// Rows of a sheet read, at most.
pub const MAX_ROWS: usize = 5_000;
/// A tool's text answer is cut at this many characters (Wysebee Desktop's
/// `tools::MAX_TOOL_RESULT_CHARS`).
pub const MAX_TOOL_RESULT_CHARS: usize = 50_000;

pub const FIND: &str = "folder_find";
pub const LIST: &str = "folder_list";
pub const READ: &str = "folder_read";
pub const WRITE: &str = "folder_write";
const SKIPPED: [&str; 4] = [".git", "node_modules", ".Trash", "__pycache__"];
const PICTURES: [(&str, &str); 5] = [("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("gif", "image/gif"), ("webp", "image/webp")];
const SHEETS: [&str; 4] = ["xlsx", "xlsm", "xls", "ods"];

/// Read only, or read and write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Read,
    Write,
}

impl Mode {
    pub fn parse(text: &str) -> Result<Mode, String> {
        match text.trim() {
            "read" => Ok(Mode::Read),
            "write" => Ok(Mode::Write),
            _ => Err("A folder's permission is read (read only) or write (read and write).".into()),
        }
    }
}

/// A folder the person picked on this Mac.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    /// Made by this app, never reused: what a call names.
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub mode: Mode,
    #[serde(default)]
    pub description: String,
}

impl Folder {
    /// As the poll reports it (the agent-link protocol, "The folders, reported").
    pub fn reported(&self) -> Value {
        json!({"id": self.id, "name": self.name, "path": self.path.display().to_string(), "mode": self.mode, "description": self.description})
    }
}

/// A new folder id: 16 random bytes in hex (32 characters of the protocol's `A-Za-z0-9_-`),
/// so one is never made twice.
pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// A name as typed, tidied: one line of 1 to [`MAX_NAME`] characters; empty: `default`, cut
/// to fit.
pub fn checked_name(typed: &str, default: &str) -> Result<String, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        let cut: String = default.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
        let cut = cut.trim();
        return Ok(if cut.is_empty() { "Folder".into() } else { cut.into() });
    }
    if typed.chars().count() > MAX_NAME || typed.chars().any(char::is_control) {
        return Err(format!("A folder's name is one line of at most {MAX_NAME} characters."));
    }
    Ok(typed.into())
}

/// A description as typed, tidied: at most [`MAX_TEXT`] characters, may be empty.
pub fn checked_description(typed: &str) -> Result<String, String> {
    let typed = typed.trim();
    if typed.chars().count() > MAX_TEXT {
        return Err(format!("Say what the folder holds in at most {MAX_TEXT} characters."));
    }
    Ok(typed.into())
}

/// A folder picked at `path`, checked: a full path to a folder that is there, short enough
/// to report, not one of `existing` already, at most [`MAX_FOLDERS`].
pub fn new_folder(existing: &[Folder], path: &str, name: &str, mode: &str, description: &str) -> Result<Folder, String> {
    if existing.len() >= MAX_FOLDERS {
        return Err(format!("At most {MAX_FOLDERS} folders: remove one first."));
    }
    let path = PathBuf::from(path.trim());
    if !path.is_absolute() {
        return Err("Pick the folder: its full path is needed.".into());
    }
    if !path.is_dir() {
        return Err(format!("There is no folder at {}.", path.display()));
    }
    if path.display().to_string().chars().count() > MAX_PATH {
        return Err(format!("That folder's path is longer than {MAX_PATH} characters."));
    }
    let real = path.canonicalize().map_err(io)?;
    if existing.iter().any(|f| f.path == path || f.path.canonicalize().is_ok_and(|p| p == real)) {
        return Err("That folder is here already.".into());
    }
    let default = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string());
    Ok(Folder { id: new_id(), name: checked_name(name, &default)?, mode: Mode::parse(mode)?, description: checked_description(description)?, path })
}

/// Cut a tool's text answer as Wysebee Desktop's `tools::truncate` did.
pub fn truncate(text: String) -> String {
    if text.chars().count() <= MAX_TOOL_RESULT_CHARS {
        return text;
    }
    let head: String = text.chars().take(MAX_TOOL_RESULT_CHARS).collect();
    format!("{head}\n…[truncated at {MAX_TOOL_RESULT_CHARS} characters]")
}

/// Where `rel` is inside `root`, through links, or why not: never absolute, never `..`, and
/// still inside the folder once resolved. `must_exist` false (a write): the nearest part
/// that exists is checked, and the rest is made below it.
pub fn resolve(root: &Path, rel: &str, must_exist: bool) -> Result<PathBuf, String> {
    let rel = Path::new(rel.trim());
    if rel.is_absolute() || rel.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err("Give a path inside the folder, like reports/q3.pdf: no full paths and no '..'.".into());
    }
    let root = root.canonicalize().map_err(|_| format!("The folder {} is not there now.", root.display()))?;
    let joined = root.join(rel);
    let mut existing = joined.as_path();
    while !existing.exists() {
        if must_exist {
            return Err(format!("There is no {} in that folder.", rel.display()));
        }
        existing = existing.parent().unwrap_or(&root);
    }
    let real = existing.canonicalize().map_err(io)?;
    if !real.starts_with(&root) {
        return Err("That path leads outside the folder.".into());
    }
    // What does not exist yet (a write's new file and folders) goes below what does.
    let rest = joined.strip_prefix(existing).unwrap_or(Path::new(""));
    Ok(if rest.as_os_str().is_empty() { real } else { real.join(rest) })
}

/// A file system error, as the model reads it.
fn io(e: std::io::Error) -> String {
    e.to_string()
}

/// A Word file that could not be read, and why.
fn word_err(e: impl std::fmt::Display) -> String {
    format!("This Word file could not be read: {e}")
}

fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

fn modified(meta: &std::fs::Metadata) -> Value {
    meta.modified().ok().map(|t| json!(chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))).unwrap_or(Value::Null)
}

/// A name against the query: a pattern with `*` matches the whole name; words must all be in it.
fn matches(name: &str, query: &str) -> bool {
    let (name, query) = (name.to_lowercase(), query.trim().to_lowercase());
    if query.contains('*') {
        let pattern = format!("^{}$", query.split('*').map(regex::escape).collect::<Vec<_>>().join(".*"));
        return regex::Regex::new(&pattern).is_ok_and(|r| r.is_match(&name));
    }
    !query.is_empty() && query.split_whitespace().all(|w| name.contains(w))
}

/// Files whose names match `query`, in `folders` (each with its number, from 1).
fn find_files(folders: &[(usize, &Folder)], query: &str, max_scanned: usize) -> Value {
    let mut found = vec![];
    let mut scanned = 0;
    for (number, f) in folders {
        let Ok(root) = f.path.canonicalize() else { continue };
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                scanned += 1;
                let name = entry.file_name().to_string_lossy().to_string();
                let kind = entry.file_type().ok();
                if kind.is_some_and(|k| k.is_dir()) {
                    if !SKIPPED.contains(&name.as_str()) {
                        stack.push(entry.path());
                    }
                } else if kind.is_some_and(|k| k.is_file()) && matches(&name, query) && found.len() < MAX_FOUND {
                    let meta = entry.metadata().ok();
                    let rel = entry.path().strip_prefix(&root).map(|p| p.to_string_lossy().to_string()).unwrap_or(name);
                    found.push(json!({"folder": number, "path": rel, "size": meta.as_ref().map(|m| m.len()), "modified": meta.as_ref().map(modified)}));
                }
                if scanned >= max_scanned {
                    stack.clear();
                    break;
                }
            }
        }
    }
    let more = found.len() >= MAX_FOUND || scanned >= max_scanned;
    json!({"files": found, "incomplete": more})
}

fn list(f: &Folder, args: &Value) -> Result<Value, String> {
    let dir = resolve(&f.path, arg(args, "path"), true)?;
    if !dir.is_dir() {
        return Err("That is a file, not a folder: read it with folder_read.".into());
    }
    let mut entries: Vec<Value> = std::fs::read_dir(&dir)
        .map_err(io)?
        .flatten()
        .map(|e| {
            let meta = e.metadata().ok();
            let is_dir = meta.as_ref().is_some_and(std::fs::Metadata::is_dir);
            json!({"name": e.file_name().to_string_lossy(), "kind": if is_dir { "folder" } else { "file" }, "size": if is_dir { None } else { meta.as_ref().map(|m| m.len()) }})
        })
        .collect();
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let total = entries.len();
    entries.truncate(MAX_LISTED);
    Ok(json!({"entries": entries, "total": total}))
}

/// What a read gives: text for the model, or a picture for the person alone.
#[derive(Debug, PartialEq)]
pub enum Read {
    Text(String),
    Shown { note: String, image: Value },
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn read_file(f: &Folder, args: &Value) -> Result<Read, String> {
    let rel = arg(args, "path");
    let path = resolve(&f.path, rel, true)?;
    if path.is_dir() {
        return Err("That is a folder: list it with folder_list.".into());
    }
    let size = std::fs::metadata(&path).map_err(io)?.len();
    let ext = extension(&path);
    if let Some((_, mime)) = PICTURES.iter().find(|(e, _)| *e == ext) {
        if size > MAX_IMAGE_BYTES {
            return Err(format!("That picture is larger than {} MB.", MAX_IMAGE_BYTES / 1024 / 1024));
        }
        let bytes = std::fs::read(&path).map_err(io)?;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let note = format!("Shown to the person in the chat: {rel}. Pictures from local folders are not shown to you.");
        return Ok(Read::Shown { note, image: json!({"media_type": mime, "data": data}) });
    }
    if size > MAX_FILE_BYTES {
        return Err(format!("That file is larger than {} MB.", MAX_FILE_BYTES / 1024 / 1024));
    }
    let text = match ext.as_str() {
        "docx" => docx_text(&path)?,
        e if SHEETS.contains(&e) => sheets_text(&path, arg(args, "sheet"), MAX_ROWS)?,
        "pdf" => match pdf_extract::extract_text(&path) {
            Ok(t) => t,
            Err(e) => return Err(format!("This PDF could not be read: {e}")),
        },
        _ => {
            let bytes = std::fs::read(&path).map_err(io)?;
            if bytes.iter().take(8000).any(|b| *b == 0) {
                return Err("This kind of file cannot be read as text (it reads text, Word, Excel, PDF and pictures).".into());
            }
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };
    Ok(Read::Text(text))
}

/// A Word document's text: its paragraphs, one per line.
pub fn docx_text(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(word_err)?;
    let mut archive = zip::ZipArchive::new(file).map_err(word_err)?;
    let mut xml = String::new();
    archive.by_name("word/document.xml").map_err(word_err)?.read_to_string(&mut xml).map_err(word_err)?;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let (mut out, mut in_text) = (String::new(), false);
    loop {
        let event = match reader.read_event() {
            Ok(e) => e,
            Err(e) => return Err(word_err(e)),
        };
        match event {
            quick_xml::events::Event::Start(e) if e.name().0 == "w:t" => in_text = true,
            quick_xml::events::Event::End(e) if e.name().0 == "w:t" => in_text = false,
            quick_xml::events::Event::End(e) if e.name().0 == "w:p" => out.push('\n'),
            quick_xml::events::Event::Empty(e) if e.name().0 == "w:tab" => out.push('\t'),
            quick_xml::events::Event::Empty(e) if e.name().0 == "w:br" => out.push('\n'),
            quick_xml::events::Event::Text(t) if in_text => out.push_str(t.as_ref()),
            // &amp; and friends, and &#39;: their characters.
            quick_xml::events::Event::GeneralRef(r) if in_text => match r.resolve_char_ref().ok().flatten() {
                Some(c) => out.push(c),
                None => out.push_str(quick_xml::escape::resolve_predefined_entity(&r).unwrap_or("")),
            },
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// A workbook's sheets (or one), each as tab-separated rows under its name.
pub fn sheets_text(path: &Path, only: &str, max_rows: usize) -> Result<String, String> {
    use calamine::Reader as _;
    let mut book = match calamine::open_workbook_auto(path) {
        Ok(b) => b,
        Err(e) => return Err(format!("This spreadsheet could not be read: {e}")),
    };
    let names: Vec<String> = book.sheet_names().into_iter().filter(|n| only.is_empty() || n == only).collect();
    if names.is_empty() {
        return Err(format!("There is no sheet '{only}'. The sheets: {}.", book.sheet_names().join(", ")));
    }
    let mut out = String::new();
    for name in names {
        let range = match book.worksheet_range(&name) {
            Ok(r) => r,
            Err(e) => return Err(format!("Sheet '{name}' could not be read: {e}")),
        };
        out.push_str(&format!("## {name}\n"));
        for row in range.rows().take(max_rows) {
            out.push_str(&row.iter().map(ToString::to_string).collect::<Vec<_>>().join("\t"));
            out.push('\n');
        }
        if range.height() > max_rows {
            out.push_str(&format!("…[{} more rows]\n", range.height() - max_rows));
        }
    }
    Ok(out)
}

fn write_file(f: &Folder, args: &Value) -> Result<Value, String> {
    if f.mode != Mode::Write {
        return Err(format!("{} is read only: the person did not let Wysebee write there.", f.path.display()));
    }
    let content = arg(args, "content");
    if content.chars().count() > MAX_WRITE_CHARS {
        return Err(format!("A file written here is at most {MAX_WRITE_CHARS} characters."));
    }
    let rel = arg(args, "path");
    let path = resolve(&f.path, rel, false)?;
    if rel.trim().is_empty() || path.is_dir() {
        return Err("Give the file's path inside the folder, like notes/today.md.".into());
    }
    std::fs::create_dir_all(path.parent().unwrap_or(&f.path)).map_err(io)?;
    std::fs::write(&path, content).map_err(io)?;
    Ok(json!({"written": rel, "bytes": content.len()}))
}

/// One `folder_call` on this app's `folders` (blocking file work: call it off the async
/// threads): `tool`, the folder by its id (`None`: every folder, for `folder_find` only) and
/// the model's other arguments. Answers `{"text", "picture"?}`, or why not.
pub fn call(folders: &[Folder], tool: &str, folder: Option<&str>, args: &Value) -> Result<Value, String> {
    if ![FIND, LIST, READ, WRITE].contains(&tool) {
        return Err(format!("There is no folder tool '{tool}'."));
    }
    let picked = match folder {
        None => None,
        Some(id) => Some(folders.iter().position(|f| f.id == id).ok_or(format!("This Mac has no folder '{id}': the person may have removed it in Wysebee Agent."))?),
    };
    let one = || picked.map(|i| &folders[i]).ok_or("Say which folder.".to_string());
    let text = |v: Value| json!({"text": truncate(v.to_string())});
    match tool {
        FIND => {
            let chosen: Vec<(usize, &Folder)> = folders.iter().enumerate().filter(|(i, _)| picked.is_none_or(|p| p == *i)).map(|(i, f)| (i + 1, f)).collect();
            Ok(text(find_files(&chosen, arg(args, "query"), MAX_SCANNED)))
        }
        LIST => list(one()?, args).map(text),
        WRITE => write_file(one()?, args).map(text),
        _ => match read_file(one()?, args)? {
            Read::Text(t) => Ok(json!({"text": truncate(t)})),
            Read::Shown { note, image } => Ok(json!({"text": note, "picture": image})),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn zip_of(path: &Path, files: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    /// A one-page PDF that says `words`, its cross-reference table counted right.
    pub fn pdf_of(path: &Path, words: &str) {
        let stream = format!("BT /F1 12 Tf 72 720 Td ({words}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = vec![];
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
        }
        let xref = out.len();
        out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
        for o in offsets {
            out.push_str(&format!("{o:010} 00000 n \n"));
        }
        out.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1));
        std::fs::write(path, out).unwrap();
    }

    fn xlsx_of(path: &Path) {
        xlsx_with(path, r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Rent</t></is></c><c r="B1"><v>1200</v></c></row></sheetData></worksheet>"#);
    }

    /// A one-sheet workbook ("Budget") whose sheet is `sheet`.
    fn xlsx_with(path: &Path, sheet: &str) {
        zip_of(
            path,
            &[
                ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#),
                ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#),
                ("xl/workbook.xml", r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Budget" sheetId="1" r:id="rId1"/></sheets></workbook>"#),
                ("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#),
                ("xl/worksheets/sheet1.xml", sheet),
            ],
        );
    }

    /// A picture's first bytes: the reader sends a picture as it is, without decoding it.
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    fn folder(id: &str, path: &Path, mode: Mode) -> Folder {
        Folder { id: id.into(), name: id.into(), path: path.into(), mode, description: String::new() }
    }

    fn setup() -> (tempfile::TempDir, Vec<Folder>) {
        let dir = tempfile::tempdir().unwrap();
        let (docs, out) = (dir.path().join("docs"), dir.path().join("out"));
        std::fs::create_dir_all(docs.join("reports/2026")).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(docs.join("notes.txt"), "Buy milk").unwrap();
        std::fs::write(docs.join("blob.bin"), [0u8, 1, 2]).unwrap();
        std::fs::write(docs.join("photo.PNG"), PNG).unwrap();
        zip_of(&docs.join("letter.docx"), &[("word/document.xml", r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>Dear Sam</w:t></w:r></w:p><w:p><w:r><w:t>Tom &amp; Jerry</w:t><w:tab/><w:t>&#65;</w:t><w:br/></w:r></w:p></w:body></w:document>"#)]);
        zip_of(&docs.join("broken.docx"), &[("other.xml", "x")]);
        zip_of(&docs.join("bad-xml.docx"), &[("word/document.xml", "<w:p><w:t>x</w:q>")]);
        std::fs::write(docs.join("not-a-zip.docx"), "plain").unwrap();
        std::fs::write(docs.join("not-a-book.xlsx"), "plain").unwrap();
        std::fs::write(docs.join("not-a.pdf"), "plain").unwrap();
        xlsx_of(&docs.join("reports/2026/budget.xlsx"));
        xlsx_with(&docs.join("bad-sheet.xlsx"), "<worksheet><sheetData><row><c r=\"A1\"><v>1</q></c></row></sheetData></worksheet>");
        pdf_of(&docs.join("reports/q3.pdf"), "Quarter three");
        std::fs::create_dir_all(docs.join("node_modules")).unwrap();
        std::fs::write(docs.join("node_modules/notes-lib.txt"), "x").unwrap();
        let folders = vec![
            Folder { description: "My documents".into(), ..folder("docs", &docs, Mode::Read) },
            folder("out", &out, Mode::Write),
        ];
        (dir, folders)
    }

    /// One call: its text, whether it was refused, and its picture.
    fn run(folders: &[Folder], tool: &str, folder: Option<&str>, args: Value) -> (String, bool, Option<Value>) {
        match call(folders, tool, folder, &args) {
            Ok(v) => (v["text"].as_str().unwrap().to_string(), false, v.get("picture").cloned()),
            Err(e) => (e, true, None),
        }
    }

    #[test]
    fn folders_are_made_checked_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Tax papers");
        std::fs::create_dir_all(&p).unwrap();
        let at = p.to_str().unwrap();
        let made = new_folder(&[], at, "", "read", "  Receipts  ").unwrap();
        assert_eq!((made.name.as_str(), made.mode, made.description.as_str(), made.id.len()), ("Tax papers", Mode::Read, "Receipts", 32));
        assert_ne!(new_id(), new_id());
        assert_eq!(made.reported(), json!({"id": made.id, "name": "Tax papers", "path": at, "mode": "read", "description": "Receipts"}));
        assert_eq!(new_folder(&[], at, " Taxes ", "write", "").unwrap().name, "Taxes");
        // The same folder twice, even by another spelling of its path.
        let again = format!("{}/../Tax papers", dir.path().join("Tax papers").display());
        assert_eq!(new_folder(std::slice::from_ref(&made), &again, "", "read", "").unwrap_err(), "That folder is here already.");
        let refuse = |path: &str, name: &str, mode: &str, description: &str| new_folder(&[], path, name, mode, description).unwrap_err();
        assert!(refuse("relative", "", "read", "").contains("full path"));
        assert!(refuse(dir.path().join("none").to_str().unwrap(), "", "read", "").contains("There is no folder"));
        assert!(refuse(at, "", "all", "").contains("read only"));
        assert!(refuse(at, &"x".repeat(61), "read", "").contains("at most 60"));
        assert!(refuse(at, "two\nlines", "read", "").contains("one line"));
        assert!(refuse(at, "", "read", &"x".repeat(1001)).contains("at most 1000"));
        let full: Vec<Folder> = (0..MAX_FOLDERS).map(|i| folder(&i.to_string(), dir.path(), Mode::Read)).collect();
        assert!(new_folder(&full, at, "", "read", "").unwrap_err().contains("At most 20"));
        // A path too long to report.
        let mut deep = dir.path().to_path_buf();
        // (Just over the limit: macOS refuses a path of 1024 bytes or more.)
        while deep.display().to_string().len() <= MAX_PATH {
            let left = MAX_PATH + 2 - deep.display().to_string().len();
            deep.push("a".repeat(left.min(200)));
        }
        std::fs::create_dir_all(&deep).unwrap();
        assert!(refuse(deep.to_str().unwrap(), "", "read", "").contains("longer than 1000"));
        // A name made from a long folder name is cut; none at all is "Folder"; "/" is "/".
        assert_eq!(checked_name("", &"y".repeat(70)).unwrap().len(), 60);
        assert_eq!(checked_name("", "\n").unwrap(), "Folder");
        assert_eq!(new_folder(&[], "/", "", "read", "").unwrap().name, "/");
        // A state file holds them as they are.
        let text = serde_json::to_string(&made).unwrap();
        assert_eq!(serde_json::from_str::<Folder>(&text).unwrap(), made);
        assert_eq!(serde_json::from_str::<Folder>(r#"{"id":"a","name":"n","path":"/x","mode":"write"}"#).unwrap().description, "");
    }

    #[test]
    fn it_finds_lists_and_reads_every_kind_of_file() {
        let (_dir, folders) = setup();
        let (found, ..) = run(&folders, FIND, None, json!({"query": "NOTES"}));
        let found: Value = serde_json::from_str(&found).unwrap();
        assert_eq!((found["files"].as_array().unwrap().len(), found["files"][0]["path"].as_str(), found["files"][0]["folder"].clone(), found["incomplete"].clone()), (1, Some("notes.txt"), json!(1), json!(false)), "node_modules is skipped");
        assert!(found["files"][0]["modified"].as_str().unwrap().ends_with('Z') && found["files"][0]["size"] == 8);
        let (found, ..) = run(&folders, FIND, Some("docs"), json!({"query": "*.xlsx"}));
        assert!(found.contains("reports/2026/budget.xlsx") && found.contains("\"folder\":1"), "{found}");
        assert!(run(&folders, FIND, Some("out"), json!({"query": "*.xlsx"})).0.contains("\"files\":[]"));
        assert!(run(&folders, FIND, None, json!({"query": "  "})).0.contains("\"files\":[]"));
        // A file in the second folder is found as folder 2.
        std::fs::write(folders[1].path.join("notes-out.txt"), "x").unwrap();
        assert!(run(&folders, FIND, Some("out"), json!({"query": "notes"})).0.contains(r#""folder":2,"path":"notes-out.txt""#));
        assert_eq!(run(&folders, FIND, Some("nine"), json!({"query": "x"})).0, "This Mac has no folder 'nine': the person may have removed it in Wysebee Agent.");

        let (listed, ..) = run(&folders, LIST, Some("docs"), json!({"path": "reports"}));
        assert_eq!(listed, r#"{"entries":[{"name":"2026","kind":"folder","size":null},{"name":"q3.pdf","kind":"file","size":"#.to_string() + &std::fs::metadata(folders[0].path.join("reports/q3.pdf")).unwrap().len().to_string() + "}],\"total\":2}");
        assert_eq!(run(&folders, LIST, Some("docs"), json!({"path": "notes.txt"})).0, "That is a file, not a folder: read it with folder_read.");
        assert_eq!(run(&folders, LIST, None, json!({})).0, "Say which folder.");

        let read = |path: &str| run(&folders, READ, Some("docs"), json!({"path": path}));
        assert_eq!(read("notes.txt").0, "Buy milk");
        assert_eq!(read("letter.docx").0, "Dear Sam\nTom & Jerry\tA\n\n");
        assert!(read("broken.docx").0.starts_with("This Word file could not be read"));
        assert!(read("bad-xml.docx").0.starts_with("This Word file could not be read"));
        assert!(read("not-a-zip.docx").0.starts_with("This Word file could not be read"));
        assert!(read("not-a-book.xlsx").0.starts_with("This spreadsheet could not be read"));
        let (bad, refused, _) = read("bad-sheet.xlsx");
        assert!(refused && bad.starts_with("Sheet 'Budget' could not be read: "), "a workbook whose sheet is broken: {bad}");
        assert!(read("not-a.pdf").0.starts_with("This PDF could not be read"));
        assert_eq!(read("reports/2026/budget.xlsx").0, "## Budget\nRent\t1200\n");
        let book = folders[0].path.join("reports/2026/budget.xlsx");
        assert_eq!(sheets_text(&book, "", 0).unwrap(), "## Budget\n…[1 more rows]\n", "a long sheet is cut");
        let few = find_files(&[(1, &folders[0])], "zzz", 2);
        assert_eq!(few["incomplete"], true, "a search stops after so many entries");
        assert!(run(&folders, READ, Some("docs"), json!({"path": "reports/2026/budget.xlsx", "sheet": "Nope"})).0.contains("The sheets: Budget"));
        assert_eq!(run(&folders, READ, Some("docs"), json!({"path": "reports/2026/budget.xlsx", "sheet": "Budget"})).0, "## Budget\nRent\t1200\n");
        assert!(read("reports/q3.pdf").0.contains("Quarter three"));
        assert!(read("blob.bin").0.starts_with("This kind of file cannot be read"));
        assert_eq!(read("reports").0, "That is a folder: list it with folder_list.");
        let (note, is_error, shown) = read("photo.PNG");
        assert_eq!((note.as_str(), is_error), ("Shown to the person in the chat: photo.PNG. Pictures from local folders are not shown to you.", false));
        assert_eq!(shown.unwrap(), json!({"media_type": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(PNG)}));
        assert_eq!(run(&folders, "folder_delete", Some("docs"), json!({})).0, "There is no folder tool 'folder_delete'.");
    }

    #[test]
    fn nothing_outside_the_folders_and_no_writing_where_it_is_read_only() {
        let (dir, folders) = setup();
        for path in ["../out/x", "/etc/hosts", "reports/../../out"] {
            assert!(run(&folders, READ, Some("docs"), json!({"path": path})).0.contains("no full paths"), "{path}");
        }
        assert_eq!(run(&folders, READ, Some("docs"), json!({"path": "gone.txt"})).0, "There is no gone.txt in that folder.");
        std::os::unix::fs::symlink(dir.path().join("out"), dir.path().join("docs/escape")).unwrap();
        assert_eq!(run(&folders, LIST, Some("docs"), json!({"path": "escape"})).0, "That path leads outside the folder.");
        assert!(run(&folders, WRITE, Some("out"), json!({"path": "../docs/x", "content": "x"})).0.contains("no full paths"));
        // A link out from a folder that may be written: nothing is written through it.
        std::os::unix::fs::symlink(dir.path().join("docs"), dir.path().join("out/back")).unwrap();
        assert_eq!(run(&folders, WRITE, Some("out"), json!({"path": "back/new.txt", "content": "x"})).0, "That path leads outside the folder.");
        assert!(!dir.path().join("docs/new.txt").exists());
        assert!(run(&folders, WRITE, Some("docs"), json!({"path": "x.txt", "content": "x"})).0.contains("is read only"));
        let (wrote, is_error, _) = run(&folders, WRITE, Some("out"), json!({"path": "notes/today.md", "content": "Hello"}));
        assert_eq!((wrote.as_str(), is_error), (r#"{"written":"notes/today.md","bytes":5}"#, false));
        assert_eq!(std::fs::read_to_string(dir.path().join("out/notes/today.md")).unwrap(), "Hello");
        assert!(run(&folders, WRITE, Some("out"), json!({"path": "", "content": "x"})).0.contains("Give the file's path"));
        // Under a file, not a folder: the system's own refusal.
        assert!(run(&folders, WRITE, Some("out"), json!({"path": "notes/today.md/more.md", "content": "x"})).1);
        assert!(run(&folders, WRITE, Some("out"), json!({"path": "big.txt", "content": "x".repeat(MAX_WRITE_CHARS + 1)})).0.contains("at most"));
        // A folder that went away.
        std::fs::remove_dir_all(dir.path().join("out")).unwrap();
        assert!(run(&folders, LIST, Some("out"), json!({})).0.contains("is not there now"));
        assert!(run(&folders, FIND, Some("out"), json!({"query": "x"})).0.contains("\"files\":[]"));
    }

    #[test]
    fn too_large_files_are_refused_long_answers_cut_and_patterns_match_whole_names() {
        assert!(matches("Budget 2026.XLSX", "*.xlsx") && !matches("budget.xlsx.bak", "*.xlsx") && matches("Q3 report.pdf", "report q3"));
        let dir = tempfile::tempdir().unwrap();
        let big = std::fs::File::create(dir.path().join("big.png")).unwrap();
        big.set_len(MAX_IMAGE_BYTES + 1).unwrap();
        let huge = std::fs::File::create(dir.path().join("huge.txt")).unwrap();
        huge.set_len(MAX_FILE_BYTES + 1).unwrap();
        std::fs::write(dir.path().join("long.txt"), "é".repeat(MAX_TOOL_RESULT_CHARS + 1)).unwrap();
        let folders = vec![folder("one", dir.path(), Mode::Read)];
        assert!(run(&folders, READ, Some("one"), json!({"path": "big.png"})).0.contains("larger than 10 MB"));
        assert!(run(&folders, READ, Some("one"), json!({"path": "huge.txt"})).0.contains("larger than 50 MB"));
        let long = run(&folders, READ, Some("one"), json!({"path": "long.txt"})).0;
        assert_eq!(long, format!("{}\n…[truncated at 50000 characters]", "é".repeat(MAX_TOOL_RESULT_CHARS)));
        assert_eq!(truncate("short".into()), "short");
    }
}

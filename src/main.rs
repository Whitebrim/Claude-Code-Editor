#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

use eframe::egui::{
    self, Color32, FontFamily, FontId, Frame, Layout, Margin, RichText, Rounding, ScrollArea,
    Sense, Stroke, TextEdit, Ui,
};
use chrono::{DateTime, Datelike, Local};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

// ---------- palette (Claude Desktop-ish dark) ----------

const BG_APP: Color32 = Color32::from_rgb(20, 20, 22);
const BG_SIDEBAR: Color32 = Color32::from_rgb(24, 24, 27);
const BG_SIDEBAR_ITEM: Color32 = Color32::from_rgb(30, 30, 34);
const BG_SIDEBAR_ITEM_HOVER: Color32 = Color32::from_rgb(40, 40, 46);
const BG_SIDEBAR_ITEM_ACTIVE: Color32 = Color32::from_rgb(56, 44, 60);
const BG_TOOLBAR: Color32 = Color32::from_rgb(28, 28, 32);
const BG_USER: Color32 = Color32::from_rgb(50, 42, 56);
const BG_ASSISTANT: Color32 = Color32::from_rgb(32, 32, 36);
const BG_CODE: Color32 = Color32::from_rgb(14, 14, 16);
const BG_TOOL: Color32 = Color32::from_rgb(26, 30, 34);
const TEXT_PRIMARY: Color32 = Color32::from_rgb(230, 230, 232);
const TEXT_MUTED: Color32 = Color32::from_rgb(140, 140, 148);
const ACCENT_ALICE: Color32 = Color32::from_rgb(217, 145, 90);
const ACCENT_BRIM: Color32 = Color32::from_rgb(140, 170, 230);
const BORDER: Color32 = Color32::from_rgb(50, 50, 56);

// ---------- data ----------

#[derive(Clone)]
struct Msg {
    line_idx: usize,
    role: String,
    display_text: String,
    tool_lines: Vec<String>,
    thinking_text: String,
    is_sidechain: bool,
    editing: bool,
    edit_buf: String,
    original: Value,
    timestamp: Option<String>,
}

#[derive(Clone)]
struct ConversationInfo {
    path: PathBuf,
    filename: String,
    project: String,
    title: String,
    message_count: usize,
    modified: SystemTime,
    branch: Option<String>,
    first_user_uuid: Option<String>,
    is_rewind: bool,
}

struct App {
    projects_root: PathBuf,
    conversations: Vec<ConversationInfo>,
    selected_idx: Option<usize>,
    search: String,
    raw_lines: Vec<String>,
    messages: Vec<Msg>,
    show_sidechain: bool,
    show_tools: bool,
    show_thinking: bool,
    show_rewinds: bool,
    status: String,
    error: Option<String>,
    dirty: bool,
    window_state: WindowState,
    last_saved_state: WindowState,
    last_poll: Instant,
    current_file_mtime: Option<SystemTime>,
    scroll_to_bottom: bool,
    pending_maximize: bool,
    first_frame_done: bool,
    pending_delete_msg: Option<usize>,
    configured_undoer_for: Option<usize>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct WindowState {
    x: f32,
    y: f32,
    // last known windowed inner size (only updated when NOT maximized)
    width: f32,
    height: f32,
    maximized: bool,
    #[serde(default)]
    collapsed_projects: std::collections::BTreeSet<String>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: 100.0,
            y: 100.0,
            width: 1200.0,
            height: 800.0,
            maximized: false,
            collapsed_projects: std::collections::BTreeSet::new(),
        }
    }
}

impl WindowState {
    /// Clamp obviously bogus windowed sizes (e.g. a previous save wrote a
    /// monitor-sized rect thinking it was windowed) back to a sane default.
    fn sanitize(&mut self) {
        if self.width < 400.0 || self.width > 6000.0 {
            self.width = 1200.0;
        }
        if self.height < 300.0 || self.height > 4000.0 {
            self.height = 800.0;
        }
    }
}

fn window_state_path() -> PathBuf {
    let base = std::env::var("APPDATA")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("claude-session-editor").join("window.json")
}

fn load_window_state() -> Option<WindowState> {
    let content = fs::read_to_string(window_state_path()).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_window_state(s: &WindowState) {
    let path = window_state_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(content) = serde_json::to_string_pretty(s) {
        let _ = fs::write(path, content);
    }
}

impl Default for App {
    fn default() -> Self {
        let projects_root = default_projects_root();
        let mut app = Self {
            projects_root: projects_root.clone(),
            conversations: Vec::new(),
            selected_idx: None,
            search: String::new(),
            raw_lines: Vec::new(),
            messages: Vec::new(),
            show_sidechain: false,
            show_tools: true,
            show_thinking: false,
            show_rewinds: false,
            status: format!("Ищу проекты в: {}", projects_root.display()),
            error: None,
            dirty: false,
            window_state: WindowState::default(),
            last_saved_state: WindowState::default(),
            last_poll: Instant::now(),
            current_file_mtime: None,
            scroll_to_bottom: false,
            pending_maximize: false,
            first_frame_done: false,
            pending_delete_msg: None,
            configured_undoer_for: None,
        };
        let mut loaded = load_window_state().unwrap_or_default();
        loaded.sanitize();
        app.pending_maximize = loaded.maximized;
        app.window_state = loaded.clone();
        app.last_saved_state = loaded;
        app.rescan();
        app
    }
}

fn default_projects_root() -> PathBuf {
    for var in ["USERPROFILE", "HOME"] {
        if let Ok(home) = std::env::var(var) {
            let p = PathBuf::from(home).join(".claude").join("projects");
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from(".")
}

fn short_project_name(folder: &str) -> String {
    // Claude Code encodes cwd paths by replacing separators with dashes, so
    // the trailing dash-segment is usually the actual project directory name.
    // "C--Users-name-projects-my-app" → "app"; if that's too short to be
    // meaningful, fall back to the raw folder name.
    let tail = folder.rsplit('-').next().unwrap_or(folder);
    if tail.is_empty() {
        folder.to_string()
    } else {
        tail.to_string()
    }
}

// ---------- parsing ----------

fn extract_blocks(content: &Value) -> (String, String, Vec<String>) {
    let mut text = String::new();
    let mut thinking = String::new();
    let mut tools = Vec::new();
    match content {
        Value::String(s) => text.push_str(s),
        Value::Array(arr) => {
            for block in arr {
                let bt = block.get("type").and_then(Value::as_str).unwrap_or("");
                match bt {
                    "text" => {
                        if let Some(s) = block.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                text.push_str("\n\n");
                            }
                            text.push_str(s);
                        }
                    }
                    "thinking" => {
                        if let Some(s) = block.get("thinking").and_then(Value::as_str) {
                            if !thinking.is_empty() {
                                thinking.push_str("\n\n");
                            }
                            thinking.push_str(s);
                        }
                    }
                    "tool_use" => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let input_short = block
                            .get("input")
                            .map(|v| truncate_chars(&v.to_string(), 240))
                            .unwrap_or_default();
                        tools.push(format!("→ {}  {}", name, input_short));
                    }
                    "tool_result" => {
                        let short = match block.get("content") {
                            Some(Value::String(s)) => summarize(s),
                            Some(Value::Array(arr)) => arr
                                .iter()
                                .filter_map(|b| b.get("text").and_then(Value::as_str))
                                .map(summarize)
                                .collect::<Vec<_>>()
                                .join(" | "),
                            _ => "[result]".into(),
                        };
                        tools.push(format!("← {}", short));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    (text, thinking, tools)
}

fn summarize(s: &str) -> String {
    let one_line: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    truncate_chars(&one_line, 200)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("...");
    out
}

fn scan_conversations(root: &Path) -> Vec<ConversationInfo> {
    let mut results = Vec::new();
    // If `root` looks like a projects root (contains subdirectories that hold
    // .jsonl files), iterate its children. Otherwise treat it as a single
    // project directory itself — this keeps things working if the user picks
    // one project folder via "Change folder".
    let Ok(entries) = fs::read_dir(root) else {
        return results;
    };
    let mut project_dirs: Vec<(PathBuf, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                project_dirs.push((path, name));
            }
        }
    }
    if project_dirs.is_empty() {
        // Fall back to reading `root` directly as a single project.
        let name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        project_dirs.push((root.to_path_buf(), name));
    }

    for (proj_path, project_name) in &project_dirs {
        scan_project_dir(proj_path, project_name, &mut results);
    }

    // Group by first-user-message uuid across everything; within each group
    // the newest is canonical, older siblings are rewind remnants.
    let mut latest_in_group: HashMap<String, SystemTime> = HashMap::new();
    for info in &results {
        if let Some(u) = &info.first_user_uuid {
            let e = latest_in_group.entry(u.clone()).or_insert(info.modified);
            if info.modified > *e {
                *e = info.modified;
            }
        }
    }
    for info in results.iter_mut() {
        if let Some(u) = &info.first_user_uuid {
            if let Some(&latest) = latest_in_group.get(u) {
                info.is_rewind = info.modified < latest;
            }
        }
    }

    results.sort_by(|a, b| b.modified.cmp(&a.modified));
    results
}

fn scan_project_dir(dir: &Path, project: &str, results: &mut Vec<ConversationInfo>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|s| s.to_str()).map(String::from) else {
            continue;
        };
        // Skip sessions that Claude Desktop has released (rewind / delete).
        let released_sidecar = path.with_file_name(format!(
            "{}.desktop-released.json",
            filename.trim_end_matches(".jsonl")
        ));
        if released_sidecar.exists() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };

        let mut title = String::new();
        let mut msg_count = 0usize;
        let mut branch: Option<String> = None;
        let mut first_user_uuid: Option<String> = None;
        let mut last_msg_time: Option<SystemTime> = None;

        for line in content.split('\n') {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let t = v.get("type").and_then(Value::as_str).unwrap_or("");

            if branch.is_none() {
                if let Some(b) = v.get("gitBranch").and_then(Value::as_str) {
                    if !b.is_empty() {
                        branch = Some(b.to_string());
                    }
                }
            }

            if t == "user" || t == "assistant" || t == "attachment" {
                if let Some(ts) = v
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(iso_to_system_time)
                {
                    last_msg_time = Some(last_msg_time.map_or(ts, |cur| cur.max(ts)));
                }
            }

            if t == "user" || t == "assistant" {
                let content_v = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let (text, _, tools) = extract_blocks(&content_v);
                let has_body = !text.trim().is_empty() || !tools.is_empty();
                if has_body {
                    msg_count += 1;
                    if title.is_empty() && t == "user" && !text.trim().is_empty() {
                        title = truncate_chars(text.trim(), 70);
                    }
                    if first_user_uuid.is_none() && t == "user" {
                        if let Some(u) = v.get("uuid").and_then(Value::as_str) {
                            first_user_uuid = Some(u.to_string());
                        }
                    }
                }
            }
        }

        if msg_count == 0 {
            continue;
        }
        if title.is_empty() {
            title = "(без юзерского текста)".into();
        }

        let modified = last_msg_time.unwrap_or(modified);

        results.push(ConversationInfo {
            path,
            filename,
            project: project.to_string(),
            title,
            message_count: msg_count,
            modified,
            branch,
            first_user_uuid,
            is_rewind: false,
        });
    }
}

// ---------- app logic ----------

impl App {
    fn rescan(&mut self) {
        self.conversations = scan_conversations(&self.projects_root);
        let mut projects = std::collections::HashSet::new();
        for c in &self.conversations {
            projects.insert(&c.project);
        }
        self.status = format!(
            "Проектов: {} · диалогов: {} · {}",
            projects.len(),
            self.conversations.len(),
            self.projects_root.display()
        );
    }

    fn load_selected(&mut self, idx: usize) {
        let Some(info) = self.conversations.get(idx).cloned() else {
            return;
        };
        self.selected_idx = Some(idx);
        self.error = None;

        let contents = match fs::read_to_string(&info.path) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(format!("Не смогла прочитать файл: {}", e));
                return;
            }
        };

        let raw_lines: Vec<String> = contents.split('\n').map(String::from).collect();
        let mut messages = Vec::new();

        for (i, line) in raw_lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let t = v.get("type").and_then(Value::as_str).unwrap_or("");
            let is_sidechain = v
                .get("isSidechain")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let timestamp = v
                .get("timestamp")
                .and_then(Value::as_str)
                .map(String::from);

            let (display_text, thinking_text, tool_lines) = if t == "user" || t == "assistant" {
                let content = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .cloned()
                    .unwrap_or(Value::Null);
                extract_blocks(&content)
            } else if t == "attachment" {
                // Attachment records carry system-reminders (env snapshots,
                // command output, etc.) in `rendered[*].content`. Surface them
                // so the editor doesn't hide load-bearing text.
                let mut text = String::new();
                if let Some(Value::Array(arr)) = v.get("rendered") {
                    for block in arr {
                        if let Some(c) = block.get("content").and_then(Value::as_str) {
                            if !text.is_empty() {
                                text.push_str("\n\n");
                            }
                            text.push_str(c);
                        }
                    }
                }
                if text.is_empty() {
                    continue;
                }
                (text, String::new(), Vec::new())
            } else {
                continue;
            };

            if t == "user" && display_text.trim().is_empty() && tool_lines.is_empty() {
                continue;
            }

            messages.push(Msg {
                line_idx: i,
                role: t.to_string(),
                display_text: display_text.clone(),
                tool_lines,
                thinking_text,
                is_sidechain,
                editing: false,
                edit_buf: display_text,
                original: v,
                timestamp,
            });
        }

        self.status = format!("Открыт {} — {} сообщ.", info.filename, messages.len());
        self.raw_lines = raw_lines;
        self.messages = messages;
        self.dirty = false;
        self.scroll_to_bottom = true;
        self.current_file_mtime = fs::metadata(&info.path)
            .and_then(|m| m.modified())
            .ok();
    }

    fn reload_current_if_changed(&mut self) {
        let Some(idx) = self.selected_idx else {
            return;
        };
        let Some(info) = self.conversations.get(idx).cloned() else {
            return;
        };
        let mtime = fs::metadata(&info.path).and_then(|m| m.modified()).ok();
        if mtime == self.current_file_mtime {
            return;
        }
        let any_editing = self.messages.iter().any(|m| m.editing);
        if self.dirty || any_editing {
            return;
        }
        self.load_selected(idx);
    }

    fn rescan_if_changed(&mut self) {
        let fresh = scan_conversations(&self.projects_root);
        let same = fresh.len() == self.conversations.len()
            && fresh
                .iter()
                .zip(self.conversations.iter())
                .all(|(a, b)| {
                    a.filename == b.filename
                        && a.modified == b.modified
                        && a.message_count == b.message_count
                        && a.is_rewind == b.is_rewind
                });
        if !same {
            let previously_selected = self
                .selected_idx
                .and_then(|i| self.conversations.get(i).map(|c| c.filename.clone()));
            self.conversations = fresh;
            self.selected_idx = previously_selected.and_then(|name| {
                self.conversations.iter().position(|c| c.filename == name)
            });
        }
    }

    fn commit_edit(&mut self, msg_idx: usize) {
        let (line_idx, new_text, role) = {
            let m = &mut self.messages[msg_idx];
            (m.line_idx, std::mem::take(&mut m.edit_buf), m.role.clone())
        };

        let mut v = self.messages[msg_idx].original.clone();
        if role == "attachment" {
            // Attachment records store the visible payload in
            // `rendered[i].content`. We joined those with "\n\n" for display,
            // so on write collapse the array to a single block carrying the
            // edited text — no other fields exist on these blocks.
            v["rendered"] = serde_json::json!([{ "content": new_text }]);
        } else if let Some(content) = v.get_mut("message").and_then(|m| m.get_mut("content")) {
            match content {
                Value::String(_) => *content = Value::String(new_text.clone()),
                Value::Array(arr) => {
                    let mut replaced = false;
                    for block in arr.iter_mut() {
                        if block.get("type").and_then(Value::as_str) == Some("text") {
                            block["text"] = Value::String(new_text.clone());
                            replaced = true;
                            break;
                        }
                    }
                    if !replaced {
                        arr.push(serde_json::json!({
                            "type": "text",
                            "text": new_text
                        }));
                    }
                }
                _ => *content = Value::String(new_text.clone()),
            }
        }

        let m = &mut self.messages[msg_idx];
        m.display_text = new_text.clone();
        m.edit_buf = new_text;
        m.editing = false;
        m.original = v.clone();

        if let Ok(serialized) = serde_json::to_string(&v) {
            self.raw_lines[line_idx] = serialized;
            self.dirty = true;
        }
    }

    fn commit_delete(&mut self, msg_idx: usize) {
        if msg_idx >= self.messages.len() {
            return;
        }
        let (line_idx, deleted_uuid, deleted_parent) = {
            let m = &self.messages[msg_idx];
            (
                m.line_idx,
                m.original
                    .get("uuid")
                    .and_then(Value::as_str)
                    .map(String::from),
                m.original.get("parentUuid").cloned(),
            )
        };
        let new_parent = deleted_parent.unwrap_or(Value::Null);

        // Reparent children of the deleted line so the chain stays connected.
        if let Some(ref uuid) = deleted_uuid {
            for (i, line) in self.raw_lines.iter_mut().enumerate() {
                if i == line_idx || line.trim().is_empty() {
                    continue;
                }
                let mut v: Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("parentUuid").and_then(Value::as_str) == Some(uuid.as_str()) {
                    v["parentUuid"] = new_parent.clone();
                    if let Ok(s) = serde_json::to_string(&v) {
                        *line = s;
                    }
                }
            }
            // Keep in-memory Msg.original in sync for anything the UI still shows.
            for m in self.messages.iter_mut() {
                if m.original.get("parentUuid").and_then(Value::as_str) == Some(uuid.as_str()) {
                    m.original["parentUuid"] = new_parent.clone();
                }
            }
        }

        if line_idx < self.raw_lines.len() {
            self.raw_lines.remove(line_idx);
        }
        for m in self.messages.iter_mut() {
            if m.line_idx > line_idx {
                m.line_idx -= 1;
            }
        }
        self.messages.remove(msg_idx);
        self.dirty = true;
    }

    fn insert_attachment_after(&mut self, after_msg_idx: usize) {
        if after_msg_idx >= self.messages.len() {
            return;
        }
        let (after_line_idx, after_uuid, sample) = {
            let m = &self.messages[after_msg_idx];
            (
                m.line_idx,
                m.original
                    .get("uuid")
                    .and_then(Value::as_str)
                    .map(String::from),
                m.original.clone(),
            )
        };

        let new_uuid = generate_uuid_v4();
        let timestamp = current_iso_timestamp();
        let new_uuid_value = Value::String(new_uuid.clone());

        // Rewire existing children of the "after" message to point at the
        // new attachment, so the parent chain stays connected instead of
        // orphaning what was originally the direct child.
        if let Some(ref au) = after_uuid {
            for (i, line) in self.raw_lines.iter_mut().enumerate() {
                if i == after_line_idx || line.trim().is_empty() {
                    continue;
                }
                let mut v: Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v.get("parentUuid").and_then(Value::as_str) == Some(au.as_str()) {
                    v["parentUuid"] = new_uuid_value.clone();
                    if let Ok(s) = serde_json::to_string(&v) {
                        *line = s;
                    }
                }
            }
            for m in self.messages.iter_mut() {
                if m.original.get("parentUuid").and_then(Value::as_str) == Some(au.as_str()) {
                    m.original["parentUuid"] = new_uuid_value.clone();
                }
            }
        }

        let mut new_record = serde_json::json!({
            "parentUuid": after_uuid.map(Value::String).unwrap_or(Value::Null),
            "isSidechain": false,
            "type": "attachment",
            "uuid": new_uuid,
            "timestamp": timestamp,
            "rendered": [{ "content": "" }],
        });
        for key in [
            "sessionId",
            "gitBranch",
            "cwd",
            "version",
            "entrypoint",
            "userType",
        ] {
            if let Some(v) = sample.get(key) {
                new_record[key] = v.clone();
            }
        }

        let insert_line = after_line_idx + 1;
        let new_line_str = match serde_json::to_string(&new_record) {
            Ok(s) => s,
            Err(_) => return,
        };
        self.raw_lines.insert(insert_line, new_line_str);
        for m in self.messages.iter_mut() {
            if m.line_idx >= insert_line {
                m.line_idx += 1;
            }
        }

        let ts = new_record
            .get("timestamp")
            .and_then(Value::as_str)
            .map(String::from);
        let new_msg = Msg {
            line_idx: insert_line,
            role: "attachment".to_string(),
            display_text: String::new(),
            tool_lines: Vec::new(),
            thinking_text: String::new(),
            is_sidechain: false,
            editing: true,
            edit_buf: String::new(),
            original: new_record,
            timestamp: ts,
        };
        self.messages.insert(after_msg_idx + 1, new_msg);
        self.dirty = true;
        self.save();
    }

    fn handle_word_delete(
        &mut self,
        ctx: &egui::Context,
        msg_idx: usize,
        id: egui::Id,
        forward: bool,
    ) {
        if msg_idx >= self.messages.len() {
            return;
        }
        let state = egui::text_edit::TextEditState::load(ctx, id);
        let msg = &mut self.messages[msg_idx];
        let text = msg.edit_buf.clone();

        let (start_bytes, end_bytes, new_cursor_char) =
            if let Some(range) = state.as_ref().and_then(|s| s.cursor.char_range()) {
                let p = range.primary.index;
                let s = range.secondary.index;
                if p != s {
                    let (min_c, max_c) = if p < s { (p, s) } else { (s, p) };
                    let min_b = char_idx_to_byte(&text, min_c);
                    let max_b = char_idx_to_byte(&text, max_c);
                    (min_b, max_b, min_c)
                } else {
                    let cursor_bytes = char_idx_to_byte(&text, p);
                    if forward {
                        let new_pos = word_boundary_right(&text, cursor_bytes);
                        (cursor_bytes, new_pos, p)
                    } else {
                        let new_pos = word_boundary_left(&text, cursor_bytes);
                        let new_char = text[..new_pos].chars().count();
                        (new_pos, cursor_bytes, new_char)
                    }
                }
            } else {
                let end = text.len();
                if forward {
                    (end, end, text.chars().count())
                } else {
                    let new_pos = word_boundary_left(&text, end);
                    let new_char = text[..new_pos].chars().count();
                    (new_pos, end, new_char)
                }
            };

        if start_bytes < end_bytes {
            msg.edit_buf.replace_range(start_bytes..end_bytes, "");
        }

        if let Some(mut st) = state {
            let cc = egui::text::CCursor::new(new_cursor_char);
            st.cursor
                .set_char_range(Some(egui::text_selection::CCursorRange::one(cc)));
            st.store(ctx, id);
        }
    }

    fn handle_ctrl_arrow(
        &mut self,
        ctx: &egui::Context,
        msg_idx: usize,
        id: egui::Id,
        forward: bool,
        extend: bool,
    ) {
        if msg_idx >= self.messages.len() {
            return;
        }
        let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) else {
            return;
        };
        let text = self.messages[msg_idx].edit_buf.clone();

        let (primary_char, secondary_char) = match state.cursor.char_range() {
            Some(r) => (r.primary.index, r.secondary.index),
            None => {
                let end = text.chars().count();
                (end, end)
            }
        };
        let primary_bytes = char_idx_to_byte(&text, primary_char);
        let new_bytes = if forward {
            word_boundary_right(&text, primary_bytes)
        } else {
            word_boundary_left(&text, primary_bytes)
        };
        let new_char = text[..new_bytes].chars().count();

        let new_primary = egui::text::CCursor::new(new_char);
        let new_secondary = if extend {
            egui::text::CCursor::new(secondary_char)
        } else {
            new_primary
        };
        let range = egui::text_selection::CCursorRange {
            primary: new_primary,
            secondary: new_secondary,
        };
        state.cursor.set_char_range(Some(range));
        state.store(ctx, id);
    }

    fn save(&mut self) {
        let Some(idx) = self.selected_idx else {
            return;
        };
        let Some(info) = self.conversations.get(idx).cloned() else {
            return;
        };
        let content = self.raw_lines.join("\n");
        match fs::write(&info.path, content) {
            Ok(_) => {
                self.status = format!("Сохранено: {}", info.filename);
                self.dirty = false;
                // Absorb our own write so reload_current_if_changed doesn't
                // fire and jump the scroll back to the bottom.
                self.current_file_mtime = fs::metadata(&info.path)
                    .and_then(|m| m.modified())
                    .ok();
            }
            Err(e) => self.error = Some(format!("Не смогла сохранить: {}", e)),
        }
    }
}

// ---------- ui ----------

fn setup_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.override_text_color = Some(TEXT_PRIMARY);
    style.visuals.panel_fill = BG_APP;
    style.visuals.window_fill = BG_APP;
    style.visuals.extreme_bg_color = BG_CODE;
    style.visuals.faint_bg_color = BG_ASSISTANT;
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.text_styles.insert(
        egui::TextStyle::Body,
        FontId::new(14.5, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        FontId::new(13.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        FontId::new(13.0, FontFamily::Monospace),
    );
    ctx.set_style(style);
}

const SCROLL_MULTIPLIER: f32 = 2.0;
const POLL_INTERVAL: Duration = Duration::from_millis(1000);

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        setup_style(ctx);

        ctx.input_mut(|i| {
            for event in &mut i.events {
                if let egui::Event::MouseWheel { delta, .. } = event {
                    delta.x *= SCROLL_MULTIPLIER;
                    delta.y *= SCROLL_MULTIPLIER;
                }
            }
            i.raw_scroll_delta *= SCROLL_MULTIPLIER;
            i.smooth_scroll_delta *= SCROLL_MULTIPLIER;
        });

        // Intercept Ctrl-word events before egui's TextEdit consumes them.
        // egui's default splits at every character-class boundary, so
        // punctuation-heavy text jumps character-by-character. Reimplement the
        // Windows semantics where whitespace is the only word separator.
        let editing_idx = self.messages.iter().position(|m| m.editing);
        if let Some(idx) = editing_idx {
            let id = egui::Id::new(("msg_edit", idx));
            let actions = ctx.input_mut(|input| {
                let mut actions: Vec<CtrlWordAction> = Vec::new();
                input.events.retain(|event| {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = event
                    {
                        if modifiers.ctrl && !modifiers.alt && !modifiers.mac_cmd {
                            let shift = modifiers.shift;
                            match key {
                                egui::Key::Backspace if !shift => {
                                    actions.push(CtrlWordAction::DeleteLeft);
                                    return false;
                                }
                                egui::Key::Delete if !shift => {
                                    actions.push(CtrlWordAction::DeleteRight);
                                    return false;
                                }
                                egui::Key::ArrowLeft => {
                                    actions.push(CtrlWordAction::MoveLeft { extend: shift });
                                    return false;
                                }
                                egui::Key::ArrowRight => {
                                    actions.push(CtrlWordAction::MoveRight { extend: shift });
                                    return false;
                                }
                                _ => {}
                            }
                        }
                    }
                    true
                });
                actions
            });
            for action in actions {
                match action {
                    CtrlWordAction::DeleteLeft => self.handle_word_delete(ctx, idx, id, false),
                    CtrlWordAction::DeleteRight => self.handle_word_delete(ctx, idx, id, true),
                    CtrlWordAction::MoveLeft { extend } => {
                        self.handle_ctrl_arrow(ctx, idx, id, false, extend)
                    }
                    CtrlWordAction::MoveRight { extend } => {
                        self.handle_ctrl_arrow(ctx, idx, id, true, extend)
                    }
                }
            }
        }

        // Track window geometry each frame. Position always reflects the current
        // outer_rect (works as a monitor hint even when maximized). Size only
        // updates while windowed. Maximized detection combines the viewport flag
        // (unreliable on Windows) with a size-vs-monitor heuristic.
        let (vp_max_flag, vp_full, vp_outer, vp_inner, vp_monitor) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.maximized.unwrap_or(false),
                v.fullscreen.unwrap_or(false),
                v.outer_rect,
                v.inner_rect,
                v.monitor_size,
            )
        });
        let inferred_max = match (vp_monitor, vp_inner) {
            (Some(m), Some(r)) if m.x > 0.0 && m.y > 0.0 => {
                r.width() / m.x > 0.95 && r.height() / m.y > 0.80
            }
            _ => false,
        };
        let is_max = vp_max_flag || vp_full || inferred_max;

        if let Some(rect) = vp_outer {
            self.window_state.x = rect.min.x;
            self.window_state.y = rect.min.y;
        }
        if !is_max {
            if let Some(rect) = vp_inner {
                self.window_state.width = rect.width();
                self.window_state.height = rect.height();
            }
        }
        self.window_state.maximized = is_max;

        // Apply saved maximized state on the first frame, after position took
        // effect. Doing it via ViewportCommand rather than the ViewportBuilder
        // ensures winit places the window at the requested position first
        // (which selects the monitor), then maximizes on that monitor.
        if !self.first_frame_done {
            self.first_frame_done = true;
            if self.pending_maximize {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                self.pending_maximize = false;
            }
        }

        // Save the moment state changes — no waiting for the poll timer.
        if self.window_state != self.last_saved_state {
            save_window_state(&self.window_state);
            self.last_saved_state = self.window_state.clone();
        }

        // Periodic poll: rescan file list, reload current file if mtime moved.
        if self.last_poll.elapsed() >= POLL_INTERVAL {
            self.last_poll = Instant::now();
            self.rescan_if_changed();
            self.reload_current_if_changed();
        }
        ctx.request_repaint_after(POLL_INTERVAL);

        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            self.save();
        }

        egui::TopBottomPanel::top("toolbar")
            .frame(
                Frame::default()
                    .fill(BG_TOOLBAR)
                    .inner_margin(Margin::symmetric(12.0, 8.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        self.rescan();
                    }
                    if ui.button("Change folder…").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .set_directory(&self.projects_root)
                            .pick_folder()
                        {
                            self.projects_root = p;
                            self.selected_idx = None;
                            self.messages.clear();
                            self.raw_lines.clear();
                            self.dirty = false;
                            self.rescan();
                        }
                    }
                    let can_save = self.selected_idx.is_some() && self.dirty;
                    if ui
                        .add_enabled(can_save, egui::Button::new("Save (Ctrl+S)"))
                        .clicked()
                    {
                        self.save();
                    }
                    ui.separator();
                    ui.checkbox(&mut self.show_rewinds, "Rewinds");
                    ui.checkbox(&mut self.show_sidechain, "Sidechain");
                    ui.checkbox(&mut self.show_tools, "Tool calls");
                    ui.checkbox(&mut self.show_thinking, "Thinking");
                    ui.separator();
                    let status_color = if self.dirty { ACCENT_ALICE } else { TEXT_MUTED };
                    ui.label(RichText::new(&self.status).color(status_color));
                });
            });

        if let Some(err) = self.error.clone() {
            egui::TopBottomPanel::top("error")
                .frame(
                    Frame::default()
                        .fill(Color32::from_rgb(70, 30, 30))
                        .inner_margin(Margin::symmetric(12.0, 6.0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(err).color(Color32::from_rgb(255, 210, 210)));
                        if ui.button("×").clicked() {
                            self.error = None;
                        }
                    });
                });
        }

        egui::SidePanel::left("conversations")
            .resizable(true)
            .default_width(320.0)
            .min_width(240.0)
            .max_width(500.0)
            .frame(
                Frame::default()
                    .fill(BG_SIDEBAR)
                    .inner_margin(Margin::symmetric(10.0, 10.0)),
            )
            .show(ctx, |ui| {
                let shown = self
                    .conversations
                    .iter()
                    .filter(|c| self.show_rewinds || !c.is_rewind)
                    .count();
                let hidden = self.conversations.len() - shown;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Диалоги").strong().size(14.0));
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        let count_text = if hidden > 0 && !self.show_rewinds {
                            format!("{} (+{} rewind)", shown, hidden)
                        } else {
                            format!("{}", shown)
                        };
                        ui.label(RichText::new(count_text).color(TEXT_MUTED).size(12.0));
                    });
                });
                ui.add_space(4.0);
                ui.add(
                    TextEdit::singleline(&mut self.search)
                        .hint_text("поиск…")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                ui.separator();

                let filter = self.search.to_lowercase();
                let show_rewinds = self.show_rewinds;
                let mut to_select: Option<usize> = None;

                // Group conversations by project, keeping order stable
                // (they're already sorted globally by last-message time).
                let mut project_order: Vec<String> = Vec::new();
                let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
                for (i, conv) in self.conversations.iter().enumerate() {
                    if conv.is_rewind && !show_rewinds {
                        continue;
                    }
                    if !filter.is_empty() {
                        let hay = format!(
                            "{} {} {} {}",
                            conv.title.to_lowercase(),
                            conv.filename.to_lowercase(),
                            conv.branch.clone().unwrap_or_default().to_lowercase(),
                            conv.project.to_lowercase()
                        );
                        if !hay.contains(&filter) {
                            continue;
                        }
                    }
                    if !groups.contains_key(&conv.project) {
                        project_order.push(conv.project.clone());
                    }
                    groups.entry(conv.project.clone()).or_default().push(i);
                }

                let mut collapse_toggle: Option<String> = None;

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for project in &project_order {
                            let indices = match groups.get(project) {
                                Some(v) => v,
                                None => continue,
                            };
                            let collapsed =
                                self.window_state.collapsed_projects.contains(project);
                            let sign = if collapsed { "+" } else { "−" };
                            let short = short_project_name(project);
                            let header_text =
                                format!("{}  {}  ({})", sign, short, indices.len());
                            let header_resp = ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(header_text)
                                            .strong()
                                            .color(TEXT_PRIMARY)
                                            .size(13.0),
                                    )
                                    .sense(Sense::click()),
                                )
                                .on_hover_text(project);
                            if header_resp.clicked() {
                                collapse_toggle = Some(project.clone());
                            }
                            ui.add_space(3.0);
                            if !collapsed {
                                for &i in indices {
                                    let conv = &self.conversations[i];
                                    let is_active = self.selected_idx == Some(i);
                                    if render_conv_item(ui, conv, is_active) {
                                        to_select = Some(i);
                                    }
                                    ui.add_space(4.0);
                                }
                            }
                            ui.add_space(6.0);
                            ui.separator();
                        }
                    });

                if let Some(name) = collapse_toggle {
                    if !self.window_state.collapsed_projects.remove(&name) {
                        self.window_state.collapsed_projects.insert(name);
                    }
                }
                if let Some(i) = to_select {
                    self.load_selected(i);
                }
            });

        egui::CentralPanel::default()
            .frame(
                Frame::default()
                    .fill(BG_APP)
                    .inner_margin(Margin::same(20.0)),
            )
            .show(ctx, |ui| {
                if self.selected_idx.is_none() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.label(
                            RichText::new("Выбери диалог слева")
                                .color(TEXT_MUTED)
                                .size(16.0),
                        );
                    });
                    return;
                }

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let show_sc = self.show_sidechain;
                        let show_tools = self.show_tools;
                        let show_thinking = self.show_thinking;

                        let max_w = ui.available_width().min(880.0);
                        let mut edits_to_commit: Vec<usize> = Vec::new();
                        let mut deletes_to_commit: Vec<usize> = Vec::new();
                        let mut attachments_to_add: Vec<usize> = Vec::new();
                        let mut pending_delete = self.pending_delete_msg;

                        ui.vertical_centered(|ui| {
                            ui.set_max_width(max_w);
                            for (i, msg) in self.messages.iter_mut().enumerate() {
                                if msg.is_sidechain && !show_sc {
                                    continue;
                                }
                                render_message(
                                    ui,
                                    i,
                                    msg,
                                    show_tools,
                                    show_thinking,
                                    &mut pending_delete,
                                    &mut edits_to_commit,
                                    &mut deletes_to_commit,
                                    &mut attachments_to_add,
                                );
                                ui.add_space(12.0);
                            }
                        });

                        self.pending_delete_msg = pending_delete;

                        if !edits_to_commit.is_empty() {
                            for idx in edits_to_commit {
                                self.commit_edit(idx);
                            }
                            self.save();
                        }

                        if !deletes_to_commit.is_empty() {
                            deletes_to_commit.sort_unstable_by(|a, b| b.cmp(a));
                            deletes_to_commit.dedup();
                            for idx in deletes_to_commit {
                                self.commit_delete(idx);
                            }
                            self.pending_delete_msg = None;
                            self.save();
                        }

                        if !attachments_to_add.is_empty() {
                            attachments_to_add.sort_unstable_by(|a, b| b.cmp(a));
                            attachments_to_add.dedup();
                            for idx in attachments_to_add {
                                self.insert_attachment_after(idx);
                            }
                        }

                        if self.scroll_to_bottom {
                            ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                            self.scroll_to_bottom = false;
                        }
                    });
            });

        // Tighten egui's undo grouping so Ctrl+Z steps back per short pause
        // instead of erasing the whole burst of typing at once.
        let editing_idx = self.messages.iter().position(|m| m.editing);
        match editing_idx {
            Some(idx) if self.configured_undoer_for != Some(idx) => {
                let id = egui::Id::new(("msg_edit", idx));
                if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) {
                    state.set_undoer(egui::util::undoer::Undoer::with_settings(
                        egui::util::undoer::Settings {
                            max_undos: 200,
                            stable_time: 0.35,
                            auto_save_interval: 1.0,
                        },
                    ));
                    state.store(ctx, id);
                    self.configured_undoer_for = Some(idx);
                }
            }
            None => self.configured_undoer_for = None,
            _ => {}
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        save_window_state(&self.window_state);
    }
}

fn render_conv_item(ui: &mut Ui, conv: &ConversationInfo, is_active: bool) -> bool {
    let bg = if is_active {
        BG_SIDEBAR_ITEM_ACTIVE
    } else {
        BG_SIDEBAR_ITEM
    };

    let mut clicked = false;
    let resp = Frame::default()
        .fill(bg)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::same(10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&conv.title)
                            .color(TEXT_PRIMARY)
                            .size(13.0)
                            .strong(),
                    );
                    if conv.is_rewind {
                        ui.label(
                            RichText::new("rewind")
                                .color(TEXT_MUTED)
                                .italics()
                                .size(10.0),
                        );
                    }
                });
                ui.add_space(3.0);
                let branch = conv.branch.as_deref().unwrap_or("—");
                let meta = format!(
                    "{} сообщ. · {} · {}",
                    conv.message_count,
                    branch,
                    format_time(conv.modified)
                );
                ui.label(RichText::new(meta).color(TEXT_MUTED).size(11.0));
            });
        })
        .response
        .interact(Sense::click());

    if resp.hovered() && !is_active {
        ui.painter().rect_filled(
            resp.rect,
            Rounding::same(6.0),
            BG_SIDEBAR_ITEM_HOVER.linear_multiply(0.3),
        );
    }
    if resp.clicked() {
        clicked = true;
    }
    clicked
}

fn format_msg_time(iso: &str) -> (String, String) {
    let Ok(dt) = DateTime::parse_from_rfc3339(iso) else {
        return (iso.to_string(), iso.to_string());
    };
    let local = dt.with_timezone(&Local);
    let now = Local::now();
    let full = local.format("%d.%m.%Y %H:%M:%S %z").to_string();
    let short = if local.date_naive() == now.date_naive() {
        local.format("%H:%M:%S").to_string()
    } else {
        let one_day = chrono::Duration::days(1);
        if local.date_naive() == (now - one_day).date_naive() {
            format!("вчера {}", local.format("%H:%M"))
        } else if local.year() == now.year() {
            local.format("%d.%m %H:%M").to_string()
        } else {
            local.format("%d.%m.%Y %H:%M").to_string()
        }
    };
    (short, full)
}

fn iso_to_system_time(iso: &str) -> Option<SystemTime> {
    let dt = DateTime::parse_from_rfc3339(iso).ok()?;
    let ms = dt.timestamp_millis();
    if ms >= 0 {
        Some(SystemTime::UNIX_EPOCH + Duration::from_millis(ms as u64))
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(Duration::from_millis((-ms) as u64))
    }
}

fn format_time(t: SystemTime) -> String {
    let dur = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d,
        Err(_) => return "—".into(),
    };
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let ago = now.as_secs().saturating_sub(dur.as_secs());
    if ago < 60 {
        return format!("{}с назад", ago);
    }
    if ago < 3600 {
        return format!("{}мин назад", ago / 60);
    }
    if ago < 86400 {
        return format!("{}ч назад", ago / 3600);
    }
    let days = ago / 86400;
    if days < 30 {
        return format!("{}д назад", days);
    }
    let secs = dur.as_secs();
    let (y, m, d) = epoch_to_ymd(secs);
    format!("{:02}.{:02}.{:04}", d, m, y)
}

fn epoch_to_ymd(secs: u64) -> (i32, u32, u32) {
    let days = (secs / 86400) as i64;
    let (y, m, d) = civil_from_days(days);
    (y as i32, m, d)
}

// Howard Hinnant's algorithm
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

enum CtrlWordAction {
    DeleteLeft,
    DeleteRight,
    MoveLeft { extend: bool },
    MoveRight { extend: bool },
}

fn char_idx_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

// Pseudo-random UUID v4 without pulling in a rand crate: mix nanosecond
// timestamp, PID and a stack address through DefaultHasher twice for the two
// 64-bit halves, then set the version/variant bits. Uniqueness is enough for
// tagging JSONL records we insert here.
fn generate_uuid_v4() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let stack_probe = 0u64;
    let stack_addr = &stack_probe as *const _ as usize;
    let mut hasher = DefaultHasher::new();
    now.as_nanos().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    stack_addr.hash(&mut hasher);
    let h1 = hasher.finish();
    let mut hasher = DefaultHasher::new();
    h1.hash(&mut hasher);
    now.subsec_nanos().hash(&mut hasher);
    stack_addr.rotate_left(17).hash(&mut hasher);
    let h2 = hasher.finish();

    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&h1.to_le_bytes());
    bytes[8..].copy_from_slice(&h2.to_le_bytes());
    bytes[6] = (bytes[6] & 0x0F) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // RFC 4122 variant

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

fn current_iso_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// Windows-style word navigation: whitespace is the only word separator.
// Punctuation stays part of the word, matching Notepad and most edit controls,
// instead of egui's default that treats every character-class boundary as a
// break. `left` = previous word start (Ctrl+Backspace, Ctrl+Left).
fn word_boundary_left(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    if cursor == 0 {
        return 0;
    }
    let head = &text[..cursor];
    let trimmed = head.trim_end();
    match trimmed.rfind(char::is_whitespace) {
        Some(idx) => {
            let ch_len = trimmed[idx..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            idx + ch_len
        }
        None => 0,
    }
}

// Windows-style double-click select: a "word" is a run of alphanumeric+`_`.
// Clicking on any non-word char (punctuation, whitespace) picks just that
// single char, matching Notepad and most other native controls.
fn word_range_at(text: &str, cursor_byte: usize) -> (usize, usize) {
    if text.is_empty() {
        return (0, 0);
    }
    let cursor = cursor_byte.min(text.len());
    let (peek_pos, peek_char) = if cursor < text.len() {
        let ch = text[cursor..].chars().next().unwrap();
        (cursor, ch)
    } else {
        let (i, ch) = text.char_indices().next_back().unwrap();
        (i, ch)
    };
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if !is_word(peek_char) {
        return (peek_pos, peek_pos + peek_char.len_utf8());
    }
    let mut end = peek_pos;
    for (_, c) in text[peek_pos..].char_indices() {
        if is_word(c) {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    let mut start = peek_pos;
    for (i, c) in text[..peek_pos].char_indices().rev() {
        if is_word(c) {
            start = i;
        } else {
            break;
        }
    }
    (start, end)
}

// `right` = next word start (Ctrl+Delete, Ctrl+Right): skip the rest of the
// current run of non-whitespace, then skip trailing whitespace.
fn word_boundary_right(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    let len = text.len();
    if cursor >= len {
        return len;
    }
    let mut iter = text[cursor..].char_indices().peekable();
    while let Some(&(_, ch)) = iter.peek() {
        if ch.is_whitespace() {
            break;
        }
        iter.next();
    }
    while let Some(&(_, ch)) = iter.peek() {
        if !ch.is_whitespace() {
            break;
        }
        iter.next();
    }
    match iter.peek() {
        Some(&(offset, _)) => cursor + offset,
        None => len,
    }
}

fn render_message(
    ui: &mut Ui,
    idx: usize,
    msg: &mut Msg,
    show_tools: bool,
    show_thinking: bool,
    pending_delete: &mut Option<usize>,
    edits_to_commit: &mut Vec<usize>,
    deletes_to_commit: &mut Vec<usize>,
    attachments_to_add: &mut Vec<usize>,
) {
    let is_user = msg.role == "user";
    let is_attachment = msg.role == "attachment";
    let bg = if is_user {
        BG_USER
    } else if is_attachment {
        BG_TOOL
    } else {
        BG_ASSISTANT
    };
    let (label, label_color) = if is_user {
        ("BRIM", ACCENT_BRIM)
    } else if is_attachment {
        ("attachment", TEXT_MUTED)
    } else {
        ("ALICE", ACCENT_ALICE)
    };
    // Every role is editable. Assistant/user write back to `message.content`
    // (its first text block, or a new one if the array had none), attachments
    // rewrite `rendered` to a single content block.
    let can_edit = true;

    Frame::default()
        .fill(bg)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::same(16.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(label_color).strong().size(13.0));
                ui.label(
                    RichText::new(format!("#{}", idx + 1))
                        .color(TEXT_MUTED)
                        .monospace()
                        .size(11.5),
                );
                if let Some(ref ts) = msg.timestamp {
                    let (short, full) = format_msg_time(ts);
                    ui.label(RichText::new(short).color(TEXT_MUTED).size(11.5))
                        .on_hover_text(full);
                }
                if msg.is_sidechain {
                    ui.label(
                        RichText::new("sidechain")
                            .color(TEXT_MUTED)
                            .italics()
                            .size(11.0),
                    );
                }
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    // right_to_left draws first-added rightmost, so button
                    // order on screen (left→right) is Edit, Copy, Delete.
                    if *pending_delete == Some(idx) {
                        let confirm = egui::Button::new(
                            RichText::new("Confirm delete").color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(140, 40, 40));
                        if ui.add(confirm).clicked() {
                            deletes_to_commit.push(idx);
                        }
                        if ui.button("Cancel").clicked() {
                            *pending_delete = None;
                        }
                    } else {
                        if !msg.editing {
                            let del = egui::Button::new(
                                RichText::new("Delete").color(Color32::from_rgb(220, 130, 130)),
                            );
                            if ui.add(del).clicked() {
                                *pending_delete = Some(idx);
                            }
                            if ui.button("Copy").clicked() {
                                ui.ctx().copy_text(msg.display_text.clone());
                            }
                        }
                        if can_edit {
                            if msg.editing {
                                if ui.button("Save edit").clicked() {
                                    edits_to_commit.push(idx);
                                }
                                if ui.button("Cancel").clicked() {
                                    msg.editing = false;
                                    msg.edit_buf = msg.display_text.clone();
                                }
                            } else if ui.button("Edit").clicked() {
                                msg.editing = true;
                                msg.edit_buf = msg.display_text.clone();
                            }
                        }
                        if !msg.editing {
                            let add_btn =
                                egui::Button::new(RichText::new("+").monospace().strong().size(13.0));
                            if ui
                                .add(add_btn)
                                .on_hover_text("Insert attachment below")
                                .clicked()
                            {
                                attachments_to_add.push(idx);
                            }
                        }
                    }
                });
            });

            ui.add_space(6.0);

            if show_thinking && !msg.thinking_text.is_empty() {
                Frame::default()
                    .fill(BG_TOOL)
                    .rounding(Rounding::same(6.0))
                    .inner_margin(Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("thinking")
                                .color(TEXT_MUTED)
                                .italics()
                                .size(11.0),
                        );
                        ui.label(RichText::new(&msg.thinking_text).color(TEXT_MUTED).size(12.5));
                    });
                ui.add_space(6.0);
            }

            if msg.editing {
                let te_id = egui::Id::new(("msg_edit", idx));
                // Snapshot the cursor BEFORE egui processes this frame's
                // input. If a double-click lands, that snapshot still holds
                // the single-click position (set on the previous frame),
                // which is where the pointer actually is. Reading the state
                // after `ui.add` would already show the widened selection.
                let pre_state = egui::text_edit::TextEditState::load(ui.ctx(), te_id);
                let te_response = ui.add(
                    TextEdit::multiline(&mut msg.edit_buf)
                        .id(te_id)
                        .font(FontId::new(14.0, FontFamily::Proportional))
                        .desired_width(f32::INFINITY)
                        .desired_rows(8),
                );
                if te_response.double_clicked() {
                    let click_char = pre_state
                        .as_ref()
                        .and_then(|s| s.cursor.char_range())
                        .map(|r| r.primary.index);
                    if let Some(cc) = click_char {
                        let ctx = ui.ctx();
                        if let Some(mut state) =
                            egui::text_edit::TextEditState::load(ctx, te_id)
                        {
                            let text = &msg.edit_buf;
                            let click_byte = char_idx_to_byte(text, cc);
                            let (sb, eb) = word_range_at(text, click_byte);
                            let sc = text[..sb].chars().count();
                            let ec = text[..eb].chars().count();
                            let new_range = egui::text_selection::CCursorRange {
                                primary: egui::text::CCursor::new(ec),
                                secondary: egui::text::CCursor::new(sc),
                            };
                            state.cursor.set_char_range(Some(new_range));
                            state.store(ctx, te_id);
                        }
                    }
                }
            } else if msg.display_text.is_empty() {
                ui.label(RichText::new("(нет текста)").color(TEXT_MUTED).italics());
            } else {
                render_body(ui, &msg.display_text);
            }

            if show_tools && !msg.tool_lines.is_empty() {
                ui.add_space(8.0);
                Frame::default()
                    .fill(BG_TOOL)
                    .rounding(Rounding::same(6.0))
                    .inner_margin(Margin::same(8.0))
                    .show(ui, |ui| {
                        for t in &msg.tool_lines {
                            ui.label(RichText::new(t).monospace().color(TEXT_MUTED).size(12.0));
                        }
                    });
            }
        });
}

fn render_body(ui: &mut Ui, text: &str) {
    let mut in_code = false;
    let mut buf = String::new();

    for line in text.split('\n') {
        if line.trim_start().starts_with("```") {
            flush(ui, &mut buf, in_code);
            in_code = !in_code;
            continue;
        }
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    flush(ui, &mut buf, in_code);
}

fn flush(ui: &mut Ui, buf: &mut String, in_code: bool) {
    if buf.is_empty() {
        return;
    }
    let text = std::mem::take(buf);
    if in_code {
        Frame::default()
            .fill(BG_CODE)
            .rounding(Rounding::same(6.0))
            .inner_margin(Margin::same(10.0))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(&text)
                        .monospace()
                        .color(Color32::from_rgb(210, 210, 215))
                        .size(13.0),
                );
            });
    } else {
        for section in parse_inline_sections(&text) {
            match section {
                InlineSection::Plain(s) => {
                    if !s.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(RichText::new(&s).color(TEXT_PRIMARY).size(14.5));
                        });
                    }
                }
                InlineSection::Hidden { name, content } => {
                    render_hidden_tag(ui, &name, &content);
                }
            }
        }
    }
}

fn render_hidden_tag(ui: &mut Ui, name: &str, content: &str) {
    Frame::default()
        .fill(BG_TOOL)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::same(8.0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("<{}>", name))
                    .monospace()
                    .italics()
                    .color(TEXT_MUTED)
                    .size(11.0),
            );
            ui.add_space(2.0);
            let trimmed = content.trim_matches(|c: char| c == '\n');
            ui.label(RichText::new(trimmed).color(TEXT_MUTED).size(12.5));
        });
}

enum InlineSection {
    Plain(String),
    Hidden { name: String, content: String },
}

// Split a plain-text paragraph into runs of ordinary text and blocks wrapped in
// kebab-case XML-like tags (`<system-reminder>...</system-reminder>`,
// `<command-name>...</command-name>`, `<local-command-stdout>...`, etc.). Only
// names containing a hyphen count, so ordinary sentences with `<` are left
// alone and things like `<p>` or `<abc>` don't trigger.
fn parse_inline_sections(text: &str) -> Vec<InlineSection> {
    let mut out: Vec<InlineSection> = Vec::new();
    let mut cursor = 0usize;
    let bytes = text.as_bytes();

    let push_plain = |out: &mut Vec<InlineSection>, s: &str| {
        if s.is_empty() {
            return;
        }
        if let Some(InlineSection::Plain(prev)) = out.last_mut() {
            prev.push_str(s);
        } else {
            out.push(InlineSection::Plain(s.to_string()));
        }
    };

    while cursor < text.len() {
        let rel = match text[cursor..].find('<') {
            Some(r) => r,
            None => {
                push_plain(&mut out, &text[cursor..]);
                break;
            }
        };
        let open = cursor + rel;

        // Try to parse an opening tag at `open`.
        let mut name_end = open + 1;
        while name_end < text.len() {
            let b = bytes[name_end];
            if b == b'>' {
                break;
            }
            let is_name = matches!(b,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_');
            if !is_name {
                name_end = open;
                break;
            }
            name_end += 1;
        }

        let is_valid_tag = name_end > open + 1
            && name_end < text.len()
            && bytes[name_end] == b'>'
            && {
                let name = &text[open + 1..name_end];
                name.contains('-')
                    && name
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_alphabetic())
                        .unwrap_or(false)
            };

        if is_valid_tag {
            let name = &text[open + 1..name_end];
            let content_start = name_end + 1;
            let close = format!("</{}>", name);
            if let Some(close_rel) = text[content_start..].find(&close) {
                let content_end = content_start + close_rel;
                let close_end = content_end + close.len();
                push_plain(&mut out, &text[cursor..open]);
                out.push(InlineSection::Hidden {
                    name: name.to_string(),
                    content: text[content_start..content_end].to_string(),
                });
                cursor = close_end;
                continue;
            }
        }

        // Not a recognized tag — emit up to and including this `<`, keep going.
        let end = open + 1;
        push_plain(&mut out, &text[cursor..end]);
        cursor = end;
    }

    out
}

fn main() -> eframe::Result<()> {
    let mut saved = load_window_state();
    if let Some(s) = saved.as_mut() {
        s.sanitize();
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_min_inner_size([700.0, 500.0])
        .with_title("Claude Session Editor");

    match &saved {
        Some(s) => {
            // Position + windowed size come from persisted state. Maximization
            // is applied later via ViewportCommand on the first frame so winit
            // honors the position and picks the correct monitor before max.
            viewport = viewport
                .with_inner_size([s.width, s.height])
                .with_position([s.x, s.y]);
        }
        None => {
            viewport = viewport.with_inner_size([1200.0, 800.0]);
        }
    }

    let opts = eframe::NativeOptions {
        viewport,
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "claude-session-editor",
        opts,
        Box::new(|_cc| Ok(Box::<App>::default())),
    )
}

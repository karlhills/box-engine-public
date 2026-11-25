//! The Script pane: Luau files edited inside the editor.
//!
//! - **Open** a `.luau` file by double-clicking it in the Assets pane, with
//!   **Edit** on a Script component, or by double-clicking a Console line
//!   that names `scripts/x.luau:12` (it opens at that line). Each file is a
//!   tab of the pane.
//! - **Save** with Cmd+S while the text has focus (the scene's Cmd+S
//!   otherwise). Saving writes the file; the asset server sees it change and
//!   running scripts hot-reload, during Play too.
//! - **Errors**: warnings and errors in the log that name a line of the open
//!   file (`scripts/x.luau:12: ...`) mark that line in the margin, the
//!   message on hover.
//! - **Disk changes** (another program, an agent's `write_asset`): a file
//!   without edits reloads; one with edits shows a bar to reload or keep.
//! - **Open in Code Editor**: VS Code when installed (at the line), else the
//!   system's app for the file.
//!
//! Unsaved scripts join the unsaved-changes prompt when the project closes.
//! Cost: nothing while the pane is hidden; while shown, one layout of the
//! visible file per frame (egui caches it while the text is unchanged) and a
//! file timestamp check every half second.

pub mod highlight;
mod pane;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::state::EditorState;

pub(crate) use pane::script_ui;

/// Lines of a file named by log problems: (path, log sequence, (line,
/// message) pairs).
pub(crate) type ProblemCache = (String, u64, Vec<(u32, String)>);

/// One open file.
#[derive(Clone, Debug)]
pub struct ScriptTab {
    /// Relative to `assets/`, forward slashes (`scripts/bear.luau`).
    pub path: String,
    pub text: String,
    /// The text as last read or saved.
    pub saved: String,
    stamp: Option<SystemTime>,
    /// The file changed on disk while this tab has edits.
    pub changed_on_disk: bool,
}

impl ScriptTab {
    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }

    /// The file name (`bear.luau`).
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// The pane's state (on [`EditorState`]).
#[derive(Debug, Default)]
pub struct ScriptEditor {
    pub tabs: Vec<ScriptTab>,
    pub active: usize,
    /// The text had keyboard focus last frame (Cmd+S saves the script).
    pub focused: bool,
    /// Bring the Script tab to the front (read by the editor's dock).
    pub(crate) want_focus: bool,
    /// Put the cursor on this line (1-based) of the active tab and show it.
    pub(crate) goto: Option<u32>,
    /// The find bar's text, while it is open.
    pub find: Option<String>,
    pub(crate) find_focus: bool,
    /// The match the find bar is on.
    pub(crate) find_index: usize,
    /// A close of this tab waits for Save / Don't Save.
    pub(crate) confirm_close: Option<usize>,
    last_disk_check: Option<Instant>,
    /// The last margin's problems, to reuse while the log is unchanged.
    pub(crate) problems: Option<ProblemCache>,
}

impl EditorState {
    fn assets_dir(&self) -> Option<PathBuf> {
        self.project.as_ref().map(|p| p.root().join("assets"))
    }

    /// The Script pane's state.
    pub fn script_editor(&self) -> &ScriptEditor {
        &self.scripts
    }

    /// The Script pane's state, to change (tests, tools).
    pub fn script_editor_mut(&mut self) -> &mut ScriptEditor {
        &mut self.scripts
    }

    /// Opens `path` (relative to `assets/`, or starting with `assets/`) in
    /// the Script pane, at `line` (1-based) when given, and brings the pane
    /// to the front.
    pub fn open_script(&mut self, path: &str, line: Option<u32>) -> Result<(), String> {
        let rel = normalize(path);
        if let Some(i) = self.scripts.tabs.iter().position(|t| t.path == rel) {
            self.scripts.active = i;
        } else {
            let dir = self
                .assets_dir()
                .ok_or("open a project to edit its scripts")?;
            let file = dir.join(&rel);
            let bytes =
                box_engine::box_assets::vfs::read(&file).map_err(|e| format!("{rel}: {e}"))?;
            let text = String::from_utf8(bytes).map_err(|_| format!("{rel} is not UTF-8 text"))?;
            self.scripts.tabs.push(ScriptTab {
                path: rel,
                saved: text.clone(),
                text,
                stamp: stamp(&file),
                changed_on_disk: false,
            });
            self.scripts.active = self.scripts.tabs.len() - 1;
        }
        self.scripts.want_focus = true;
        self.scripts.goto = line;
        Ok(())
    }

    /// Writes tab `i` to its file.
    pub fn save_script(&mut self, i: usize) -> Result<(), String> {
        self.ensure_writable().map_err(|e| format!("{e:#}"))?;
        let dir = self.assets_dir().ok_or("no project is open")?;
        let tab = self.scripts.tabs.get_mut(i).ok_or("no such script tab")?;
        let file = dir.join(&tab.path);
        let tmp = file.with_extension("luau.saving");
        std::fs::write(&tmp, &tab.text)
            .and_then(|()| std::fs::rename(&tmp, &file))
            .map_err(|e| format!("could not save {}: {e}", tab.path))?;
        tab.saved = tab.text.clone();
        tab.stamp = stamp(&file);
        tab.changed_on_disk = false;
        tracing::info!("saved {}", tab.path);
        Ok(())
    }

    /// Saves every script with edits; returns how many.
    pub fn save_all_scripts(&mut self) -> Result<usize, String> {
        let dirty: Vec<usize> = (0..self.scripts.tabs.len())
            .filter(|i| self.scripts.tabs[*i].dirty())
            .collect();
        for &i in &dirty {
            self.save_script(i)?;
        }
        Ok(dirty.len())
    }

    /// Scripts with unsaved edits.
    pub fn dirty_scripts(&self) -> Vec<&str> {
        self.scripts
            .tabs
            .iter()
            .filter(|t| t.dirty())
            .map(|t| t.path.as_str())
            .collect()
    }

    /// Closes tab `i`, dropping unsaved edits.
    pub fn close_script(&mut self, i: usize) {
        if i < self.scripts.tabs.len() {
            self.scripts.tabs.remove(i);
            if self.scripts.active >= self.scripts.tabs.len() {
                self.scripts.active = self.scripts.tabs.len().saturating_sub(1);
            }
            self.scripts.confirm_close = None;
        }
    }

    /// Reads tab `i` again from disk (dropping its edits).
    pub fn reload_script(&mut self, i: usize) -> Result<(), String> {
        let dir = self.assets_dir().ok_or("no project is open")?;
        let tab = self.scripts.tabs.get_mut(i).ok_or("no such script tab")?;
        let file = dir.join(&tab.path);
        let bytes =
            box_engine::box_assets::vfs::read(&file).map_err(|e| format!("{}: {e}", tab.path))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        tab.saved = text.clone();
        tab.text = text;
        tab.stamp = stamp(&file);
        tab.changed_on_disk = false;
        Ok(())
    }

    /// Notices files changed on disk (at most every half second): a tab
    /// without edits reloads, one with edits is flagged.
    pub fn check_scripts_on_disk(&mut self) {
        let now = Instant::now();
        if self
            .scripts
            .last_disk_check
            .is_some_and(|t| now - t < Duration::from_millis(500))
        {
            return;
        }
        self.scripts.last_disk_check = Some(now);
        let Some(dir) = self.assets_dir() else {
            return;
        };
        let mut reload = Vec::new();
        for (i, tab) in self.scripts.tabs.iter_mut().enumerate() {
            let s = stamp(&dir.join(&tab.path));
            if s != tab.stamp && s.is_some() {
                if tab.dirty() {
                    tab.changed_on_disk = true;
                    tab.stamp = s;
                } else {
                    reload.push(i);
                }
            }
        }
        for i in reload {
            let _ = self.reload_script(i);
        }
    }

    /// Lines of `path` that warnings and errors in the log name, with the
    /// latest message for each.
    pub fn script_problems(&self, path: &str) -> Vec<(u32, String)> {
        let mut out: Vec<(u32, String)> = Vec::new();
        let log = self.console.log.lock();
        for r in log.records() {
            if r.level > tracing::Level::WARN {
                continue;
            }
            if let Some((p, line)) = script_location(&r.message)
                && p == path
            {
                match out.iter_mut().find(|(l, _)| *l == line) {
                    Some(slot) => slot.1 = r.message.clone(),
                    None => out.push((line, r.message.clone())),
                }
            }
        }
        out
    }

    /// Opens `path` (relative to `assets/`) at `line` in the user's code
    /// editor: VS Code when installed, else the system's app for the file.
    pub fn open_in_code_editor(&self, path: &str, line: Option<u32>) {
        let Some(dir) = self.assets_dir() else {
            return;
        };
        let file = dir.join(normalize(path));
        open_in_code_editor(&file, line);
    }
}

/// The file's modification time.
fn stamp(file: &Path) -> Option<SystemTime> {
    std::fs::metadata(file).and_then(|m| m.modified()).ok()
}

/// `assets/scripts\a.luau` → `scripts/a.luau`.
pub fn normalize(path: &str) -> String {
    let p = path.trim().replace('\\', "/");
    p.strip_prefix("assets/").unwrap_or(&p).to_string()
}

/// The first `<file>.luau:<line>` a message names, as (path relative to
/// `assets/`, line).
pub fn script_location(message: &str) -> Option<(String, u32)> {
    let strip = |c: char| "`'\"()[]{},;<>@".contains(c);
    for raw in message.split_whitespace().take(64) {
        let token = raw.trim_matches(strip);
        let Some(at) = token.find(".luau:") else {
            continue;
        };
        let path = &token[..at + ".luau".len()];
        let digits: String = token[at + ".luau:".len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(line) = digits.parse::<u32>() {
            return Some((normalize(path), line));
        }
    }
    None
}

/// VS Code (at `line`) when installed, else the system's default app.
pub fn open_in_code_editor(file: &Path, line: Option<u32>) {
    let vscode = cfg!(target_os = "macos")
        && ["/Applications/Visual Studio Code.app"]
            .iter()
            .map(PathBuf::from)
            .chain(
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join("Applications/Visual Studio Code.app")),
            )
            .any(|p| p.is_dir());
    let result = if vscode {
        let url = format!("vscode://file{}:{}", file.display(), line.unwrap_or(1));
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-t")
            .arg(file)
            .spawn()
    } else if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(file)
            .spawn()
    } else {
        std::process::Command::new("xdg-open").arg(file).spawn()
    };
    if let Err(e) = result {
        tracing::warn!("could not open {} in a code editor: {e}", file.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_script_lines_in_messages() {
        assert_eq!(
            script_location("scripts/bear.luau:12: attempt to index nil with 'x'"),
            Some(("scripts/bear.luau".into(), 12))
        );
        assert_eq!(
            script_location("error in `assets/scripts/a.luau:3:5`"),
            Some(("scripts/a.luau".into(), 3))
        );
        assert_eq!(script_location("loaded scripts/a.luau"), None);
        assert_eq!(normalize("assets\\scripts\\x.luau"), "scripts/x.luau");
    }
}

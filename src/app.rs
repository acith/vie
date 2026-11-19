use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ratatui::layout::Size;
use ratatui_image::picker::ProtocolType;

use crate::fs::{self, FileEntry, FilterMode};
use crate::fs_ops;
use crate::image_loader::{ImageLoader, LoadResult, LoadedImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browser,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    Copy,
    Cut,
}

#[derive(Debug, Clone)]
pub struct Clipboard {
    pub mode: ClipboardMode,
    pub paths: HashSet<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum InputKind {
    Create,
    Rename(PathBuf),
    Search,
}

#[derive(Debug, Clone)]
pub enum ConfirmKind {
    Trash(Vec<PathBuf>),
    Delete(Vec<PathBuf>),
}

#[derive(Debug, Clone)]
pub enum Popup {
    None,
    Input {
        kind: InputKind,
        title: String,
        value: String,
        /// Character index (UTF-8 safe)
        cursor: usize,
    },
    Confirm {
        kind: ConfirmKind,
        title: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    EditText(PathBuf),
    PlayAudio(PathBuf),
    PlayVideo(PathBuf),
}

#[derive(Clone)]
pub enum ImageState {
    Empty,
    Loading,
    Loaded(LoadedImage),
    Error { message: String },
}

pub struct App {
    pub mode: Mode,
    pub should_quit: bool,
    pub needs_terminal_clear: bool,
    pub protocol_type: ProtocolType,
    pub pending_action: Option<PendingAction>,

    // Browser state
    pub current_dir: PathBuf,
    pub entries: Vec<FileEntry>,
    pub selected: usize,
    pub parent_entries: Vec<FileEntry>,
    pub parent_selected: Option<usize>,
    pub preview_dir_entries: Vec<FileEntry>,
    pub text_preview: Option<Vec<String>>,
    pub filter_mode: FilterMode,
    pub show_hidden: bool,
    pub dir_history: HashMap<PathBuf, PathBuf>,

    // File management (Yazi-style)
    pub selected_paths: HashSet<PathBuf>,
    pub clipboard: Option<Clipboard>,
    pub popup: Popup,
    pub status_message: Option<(String, bool)>,
    pub last_search: Option<String>,

    // Viewer state
    pub viewer_images: Vec<FileEntry>,
    pub viewer_index: usize,
    pub show_viewer_bar: bool,

    // Image loading state
    pub loader: ImageLoader,
    pub image_state: ImageState,
    pub active_request_id: u64,
    pub requested_key: Option<(PathBuf, Size)>,
    pub last_render_size: Option<Size>,

    // Key chord helper (for `gg`)
    pub pending_g: bool,
}

impl App {
    pub fn new(start_path: PathBuf, loader: ImageLoader, protocol_type: ProtocolType) -> Self {
        let canonical = std::fs::canonicalize(&start_path).unwrap_or(start_path);

        let (current_dir, initial_file) = if canonical.is_file() {
            let parent = canonical
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            (parent, Some(canonical))
        } else {
            (canonical, None)
        };

        let mut app = Self {
            mode: Mode::Browser,
            should_quit: false,
            needs_terminal_clear: false,
            protocol_type,
            pending_action: None,
            current_dir,
            entries: Vec::new(),
            selected: 0,
            parent_entries: Vec::new(),
            parent_selected: None,
            preview_dir_entries: Vec::new(),
            text_preview: None,
            filter_mode: FilterMode::AllFiles,
            show_hidden: false,
            dir_history: HashMap::new(),
            selected_paths: HashSet::new(),
            clipboard: None,
            popup: Popup::None,
            status_message: None,
            last_search: None,
            viewer_images: Vec::new(),
            viewer_index: 0,
            show_viewer_bar: true,
            loader,
            image_state: ImageState::Empty,
            active_request_id: 0,
            requested_key: None,
            last_render_size: None,
            pending_g: false,
        };

        app.reload_directories(initial_file.as_deref());

        if let Some(ref file) = initial_file {
            if fs::is_supported_image(file) {
                app.enter_viewer_for_path(file);
            }
        }

        app
    }

    pub fn has_popup(&self) -> bool {
        !matches!(self.popup, Popup::None)
    }

    pub fn set_info(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), false));
    }

    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), true));
    }

    pub fn reload_directories(&mut self, select_path: Option<&Path>) {
        // Parent directory
        if let Some(parent) = self.current_dir.parent() {
            self.parent_entries = fs::read_directory(parent, self.filter_mode, self.show_hidden);
            self.parent_selected = self
                .parent_entries
                .iter()
                .position(|e| e.path == self.current_dir);
        } else {
            self.parent_entries.clear();
            self.parent_selected = None;
        }

        // Current directory
        self.entries = fs::read_directory(&self.current_dir, self.filter_mode, self.show_hidden);

        let desired = select_path
            .map(Path::to_path_buf)
            .or_else(|| self.dir_history.get(&self.current_dir).cloned());

        self.selected = desired
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .unwrap_or_else(|| self.selected.min(self.entries.len().saturating_sub(1)));

        if !self.entries.is_empty() && self.selected >= self.entries.len() {
            self.selected = self.entries.len() - 1;
        }

        self.on_selection_changed();
    }

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.selected)
    }

    pub fn current_viewer_entry(&self) -> Option<&FileEntry> {
        self.viewer_images.get(self.viewer_index)
    }

    pub fn active_image_path(&self) -> Option<&Path> {
        match self.mode {
            Mode::Browser => self
                .selected_entry()
                .filter(|e| e.is_image)
                .map(|e| e.path.as_path()),
            Mode::Viewer => self.current_viewer_entry().map(|e| e.path.as_path()),
        }
    }

    fn record_history(&mut self) {
        if let Some(entry) = self.selected_entry() {
            self.dir_history
                .insert(self.current_dir.clone(), entry.path.clone());
        }
    }

    fn on_selection_changed(&mut self) {
        self.record_history();
        self.preview_dir_entries.clear();
        self.text_preview = None;

        if let Some(entry) = self.selected_entry().cloned() {
            if entry.is_dir {
                self.preview_dir_entries =
                    fs::read_directory(&entry.path, self.filter_mode, self.show_hidden);
                if !matches!(self.image_state, ImageState::Empty) {
                    self.image_state = ImageState::Empty;
                    self.requested_key = None;
                    self.needs_terminal_clear = true;
                }
            } else if entry.is_image {
                if let Some(size) = self.last_render_size {
                    self.ensure_image_loaded(size);
                }
            } else {
                if !matches!(self.image_state, ImageState::Empty) {
                    self.image_state = ImageState::Empty;
                    self.requested_key = None;
                    self.needs_terminal_clear = true;
                }
                if !entry.is_audio && !entry.is_video {
                    self.text_preview = fs::read_text_preview(&entry.path, 80, 16384);
                }
            }
        } else if !matches!(self.image_state, ImageState::Empty) {
            self.image_state = ImageState::Empty;
            self.requested_key = None;
            self.needs_terminal_clear = true;
        }
    }

    // --- Browser navigation ---

    pub fn move_down(&mut self, step: usize) {
        if self.entries.is_empty() {
            return;
        }
        let next = (self.selected + step).min(self.entries.len() - 1);
        if next != self.selected {
            self.selected = next;
            self.on_selection_changed();
        }
    }

    pub fn move_up(&mut self, step: usize) {
        if self.entries.is_empty() {
            return;
        }
        let next = self.selected.saturating_sub(step);
        if next != self.selected {
            self.selected = next;
            self.on_selection_changed();
        }
    }

    pub fn jump_top(&mut self) {
        if !self.entries.is_empty() && self.selected != 0 {
            self.selected = 0;
            self.on_selection_changed();
        }
    }

    pub fn jump_bottom(&mut self) {
        if !self.entries.is_empty() {
            let last = self.entries.len() - 1;
            if self.selected != last {
                self.selected = last;
                self.on_selection_changed();
            }
        }
    }

    pub fn go_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent().map(Path::to_path_buf) {
            let previous = self.current_dir.clone();
            self.record_history();
            self.current_dir = parent;
            self.reload_directories(Some(&previous));
        }
    }

    pub fn enter_selected(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };

        if entry.is_dir {
            self.record_history();
            self.current_dir = entry.path;
            self.reload_directories(None);
        } else if entry.is_image {
            self.enter_viewer_for_path(&entry.path);
        } else if entry.is_audio {
            self.pending_action = Some(PendingAction::PlayAudio(entry.path));
        } else if entry.is_video {
            self.pending_action = Some(PendingAction::PlayVideo(entry.path));
        } else if fs::is_text_file(&entry.path) {
            self.pending_action = Some(PendingAction::EditText(entry.path));
        } else {
            self.set_info("Binary file (not an image, text, audio, or video)");
        }
    }

    pub fn toggle_filter(&mut self) {
        let current_path = self.selected_entry().map(|e| e.path.clone());
        self.filter_mode = self.filter_mode.next();
        self.reload_directories(current_path.as_deref());
    }

    pub fn toggle_hidden(&mut self) {
        let current_path = self.selected_entry().map(|e| e.path.clone());
        self.show_hidden = !self.show_hidden;
        self.reload_directories(current_path.as_deref());
    }

    // --- Yazi-style selection & file management ---

    pub fn toggle_select_and_down(&mut self) {
        if let Some(entry) = self.selected_entry().cloned() {
            if !self.selected_paths.remove(&entry.path) {
                self.selected_paths.insert(entry.path);
            }
            self.move_down(1);
        }
    }

    pub fn select_all(&mut self) {
        for entry in &self.entries {
            self.selected_paths.insert(entry.path.clone());
        }
        let count = self.selected_paths.len();
        self.set_info(format!("Selected {count} item(s)"));
    }

    pub fn invert_selection(&mut self) {
        for entry in &self.entries {
            if !self.selected_paths.remove(&entry.path) {
                self.selected_paths.insert(entry.path.clone());
            }
        }
    }

    pub fn clear_selection_and_clipboard(&mut self) {
        let had_something = !self.selected_paths.is_empty() || self.clipboard.is_some();
        self.selected_paths.clear();
        self.clipboard = None;
        if had_something {
            self.set_info("Cleared selection & yank");
        }
    }

    /// Targets for yank/cut/delete: selected items if any, otherwise hovered item.
    fn action_targets(&self) -> Vec<PathBuf> {
        if !self.selected_paths.is_empty() {
            self.selected_paths.iter().cloned().collect()
        } else if let Some(entry) = self.selected_entry() {
            vec![entry.path.clone()]
        } else {
            Vec::new()
        }
    }

    pub fn yank_selected(&mut self, mode: ClipboardMode) {
        let targets = self.action_targets();
        if targets.is_empty() {
            return;
        }
        let count = targets.len();
        self.clipboard = Some(Clipboard {
            mode,
            paths: targets.into_iter().collect(),
        });
        self.selected_paths.clear();

        let verb = match mode {
            ClipboardMode::Copy => "Yanked (copy)",
            ClipboardMode::Cut => "Cut (move)",
        };
        self.set_info(format!("{verb} {count} item(s) — press 'p' to paste"));
    }

    pub fn paste(&mut self, overwrite: bool) {
        let Some(clip) = self.clipboard.clone() else {
            self.set_info("Nothing to paste (press 'y' to copy or 'x' to cut)");
            return;
        };

        let mut last_dest: Option<PathBuf> = None;
        let mut ok_count = 0usize;
        let mut errors: Vec<String> = Vec::new();

        for src in &clip.paths {
            let res = match clip.mode {
                ClipboardMode::Copy => fs_ops::copy_entry(src, &self.current_dir, overwrite),
                ClipboardMode::Cut => fs_ops::move_entry(src, &self.current_dir, overwrite),
            };
            match res {
                Ok(dest) => {
                    last_dest = Some(dest);
                    ok_count += 1;
                }
                Err(err) => errors.push(err.to_string()),
            }
        }

        if clip.mode == ClipboardMode::Cut && errors.is_empty() {
            self.clipboard = None;
        }

        self.reload_directories(last_dest.as_deref());

        if errors.is_empty() {
            let verb = match clip.mode {
                ClipboardMode::Copy => "Copied",
                ClipboardMode::Cut => "Moved",
            };
            self.set_info(format!("{verb} {ok_count} item(s)"));
        } else {
            self.set_error(format!(
                "Completed {ok_count}, failed {}: {}",
                errors.len(),
                errors[0]
            ));
        }
    }

    pub fn open_create_popup(&mut self) {
        self.open_popup(Popup::Input {
            kind: InputKind::Create,
            title: " Create (end with / for folder) ".to_string(),
            value: String::new(),
            cursor: 0,
        });
    }

    pub fn open_rename_popup(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        let char_len = entry.name.chars().count();
        self.open_popup(Popup::Input {
            kind: InputKind::Rename(entry.path),
            title: " Rename ".to_string(),
            value: entry.name,
            cursor: char_len,
        });
    }

    pub fn open_search_popup(&mut self) {
        self.open_popup(Popup::Input {
            kind: InputKind::Search,
            title: " Find in folder ".to_string(),
            value: String::new(),
            cursor: 0,
        });
    }

    pub fn open_delete_popup(&mut self, permanent: bool) {
        let targets = self.action_targets();
        if targets.is_empty() {
            return;
        }
        let count = targets.len();
        let preview = if count == 1 {
            targets[0]
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| targets[0].display().to_string())
        } else {
            format!("{count} selected items")
        };

        if permanent {
            self.open_popup(Popup::Confirm {
                kind: ConfirmKind::Delete(targets),
                title: " Permanent Delete ".to_string(),
                message: format!("Permanently delete {preview}? (y/N)"),
            });
        } else {
            self.open_popup(Popup::Confirm {
                kind: ConfirmKind::Trash(targets),
                title: " Move to Trash ".to_string(),
                message: format!("Move {preview} to Trash? (y/N)"),
            });
        }
    }

    fn open_popup(&mut self, popup: Popup) {
        self.popup = popup;
        self.needs_terminal_clear = true;
    }

    pub fn close_popup(&mut self) {
        self.popup = Popup::None;
        self.needs_terminal_clear = true;
    }

    pub fn submit_popup(&mut self) {
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        self.needs_terminal_clear = true;

        match popup {
            Popup::Input { kind, value, .. } => match kind {
                InputKind::Create => match fs_ops::create_entry(&self.current_dir, &value) {
                    Ok(created) => {
                        let name = created
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.reload_directories(Some(&created));
                        self.set_info(format!("Created: {name}"));
                    }
                    Err(e) => self.set_error(e.to_string()),
                },
                InputKind::Rename(src) => match fs_ops::rename_entry(&src, &value) {
                    Ok(dest) => {
                        self.selected_paths.remove(&src);
                        if let Some(ref mut clip) = self.clipboard {
                            if clip.paths.remove(&src) {
                                clip.paths.insert(dest.clone());
                            }
                        }
                        self.reload_directories(Some(&dest));
                        self.set_info(format!("Renamed to: {}", value.trim()));
                    }
                    Err(e) => self.set_error(e.to_string()),
                },
                InputKind::Search => {
                    let query = value.trim().to_string();
                    if !query.is_empty() {
                        self.last_search = Some(query);
                        self.search_next(true);
                    }
                }
            },
            Popup::Confirm { kind, .. } => match kind {
                ConfirmKind::Trash(paths) => match fs_ops::trash_entries(&paths) {
                    Ok(n) => {
                        for p in &paths {
                            self.selected_paths.remove(p);
                        }
                        self.reload_directories(None);
                        self.set_info(format!("Moved {n} item(s) to Trash"));
                    }
                    Err(e) => self.set_error(e.to_string()),
                },
                ConfirmKind::Delete(paths) => match fs_ops::delete_permanent(&paths) {
                    Ok(n) => {
                        for p in &paths {
                            self.selected_paths.remove(p);
                        }
                        self.reload_directories(None);
                        self.set_info(format!("Permanently deleted {n} item(s)"));
                    }
                    Err(e) => self.set_error(e.to_string()),
                },
            },
            Popup::None => {}
        }
    }

    pub fn search_next(&mut self, forward: bool) {
        let Some(query) = self.last_search.as_ref().map(|q| q.to_lowercase()) else {
            return;
        };
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len();
        for offset in 1..=len {
            let idx = if forward {
                (self.selected + offset) % len
            } else {
                (self.selected + len - offset) % len
            };
            if self.entries[idx].name.to_lowercase().contains(&query) {
                self.selected = idx;
                self.on_selection_changed();
                self.set_info(format!("Match: {}", self.entries[idx].name));
                return;
            }
        }
        self.set_error(format!("No match for '{query}'"));
    }

    // --- Viewer navigation ---

    pub fn enter_viewer_for_path(&mut self, target: &Path) {
        let all_in_dir =
            fs::read_directory(&self.current_dir, FilterMode::ImagesAndDirs, self.show_hidden);
        self.viewer_images = all_in_dir.into_iter().filter(|e| e.is_image).collect();

        if self.viewer_images.is_empty() {
            return;
        }

        self.viewer_index = self
            .viewer_images
            .iter()
            .position(|e| e.path == target)
            .unwrap_or(0);

        self.mode = Mode::Viewer;
        self.requested_key = None;
        self.last_render_size = None;
        self.needs_terminal_clear = true;
    }

    pub fn exit_viewer(&mut self) {
        let viewed_path = self.current_viewer_entry().map(|e| e.path.clone());
        self.mode = Mode::Browser;
        self.requested_key = None;
        self.last_render_size = None;
        self.needs_terminal_clear = true;

        if let Some(ref path) = viewed_path {
            if let Some(idx) = self.entries.iter().position(|e| &e.path == path) {
                self.selected = idx;
            }
        }
        self.on_selection_changed();
    }

    pub fn viewer_next(&mut self) {
        if self.viewer_images.is_empty() {
            return;
        }
        if self.viewer_index + 1 < self.viewer_images.len() {
            self.viewer_index += 1;
            self.on_viewer_image_changed();
        }
    }

    pub fn viewer_prev(&mut self) {
        if self.viewer_images.is_empty() {
            return;
        }
        if self.viewer_index > 0 {
            self.viewer_index -= 1;
            self.on_viewer_image_changed();
        }
    }

    pub fn viewer_first(&mut self) {
        if !self.viewer_images.is_empty() && self.viewer_index != 0 {
            self.viewer_index = 0;
            self.on_viewer_image_changed();
        }
    }

    pub fn viewer_last(&mut self) {
        if !self.viewer_images.is_empty() {
            let last = self.viewer_images.len() - 1;
            if self.viewer_index != last {
                self.viewer_index = last;
                self.on_viewer_image_changed();
            }
        }
    }

    pub fn toggle_viewer_bar(&mut self) {
        self.show_viewer_bar = !self.show_viewer_bar;
        self.requested_key = None;
        self.last_render_size = None;
        self.needs_terminal_clear = true;
    }

    fn on_viewer_image_changed(&mut self) {
        self.requested_key = None;
        if let Some(size) = self.last_render_size {
            self.ensure_image_loaded(size);
        }
    }

    // --- Image loading & preloading ---

    pub fn ensure_image_loaded(&mut self, target_size: Size) {
        if target_size.width == 0 || target_size.height == 0 {
            return;
        }

        self.last_render_size = Some(target_size);

        let Some(path) = self.active_image_path().map(Path::to_path_buf) else {
            return;
        };

        let key = (path.clone(), target_size);
        if self.requested_key.as_ref() == Some(&key) {
            return;
        }

        self.requested_key = Some(key);
        self.active_request_id = self.loader.request(path.clone(), target_size);

        let same_file_loaded =
            matches!(&self.image_state, ImageState::Loaded(img) if img.path == path);
        if !same_file_loaded {
            self.image_state = ImageState::Loading;
        }

        self.preload_neighbors(target_size);
    }

    fn preload_neighbors(&self, target_size: Size) {
        match self.mode {
            Mode::Viewer => {
                if let Some(next) = self.viewer_images.get(self.viewer_index + 1) {
                    self.loader.preload(next.path.clone(), target_size);
                }
                if self.viewer_index > 0 {
                    if let Some(prev) = self.viewer_images.get(self.viewer_index - 1) {
                        self.loader.preload(prev.path.clone(), target_size);
                    }
                }
            }
            Mode::Browser => {
                if let Some(next) = self
                    .entries
                    .get(self.selected + 1)
                    .filter(|e| e.is_image)
                {
                    self.loader.preload(next.path.clone(), target_size);
                }
                if self.selected > 0 {
                    if let Some(prev) = self
                        .entries
                        .get(self.selected - 1)
                        .filter(|e| e.is_image)
                    {
                        self.loader.preload(prev.path.clone(), target_size);
                    }
                }
            }
        }
    }

    /// Poll background worker results. Returns true if UI should redraw.
    pub fn poll_loader(&mut self) -> bool {
        let mut updated = false;
        while let Some(res) = self.loader.try_recv() {
            match res {
                LoadResult::Ready { request_id, image } => {
                    if request_id == self.active_request_id {
                        self.image_state = ImageState::Loaded(image);
                        self.needs_terminal_clear = true;
                        updated = true;
                    }
                }
                LoadResult::Error {
                    request_id,
                    message,
                } => {
                    if request_id == self.active_request_id {
                        self.image_state = ImageState::Error { message };
                        self.needs_terminal_clear = true;
                        updated = true;
                    }
                }
            }
        }
        updated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_image::picker::Picker;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn browser_viewer_navigation_and_filter_cycle() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!("vie_test_{nanos}"));
        fs::create_dir_all(tmp.join("sub_folder")).unwrap();
        fs::write(tmp.join("1.webp"), b"fake").unwrap();
        fs::write(tmp.join("2.jpg"), b"fake").unwrap();
        fs::write(tmp.join("10.avif"), b"fake").unwrap();
        fs::write(tmp.join("notes.txt"), b"hello").unwrap();

        let picker = Picker::halfblocks();
        let loader = ImageLoader::new(picker.clone());
        let mut app = App::new(tmp.clone(), loader, picker.protocol_type());

        assert_eq!(app.filter_mode, FilterMode::AllFiles);
        assert_eq!(app.entries.len(), 5);

        app.toggle_filter();
        assert_eq!(app.filter_mode, FilterMode::ImagesAndDirs);
        let names: Vec<&str> = app.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["sub_folder", "1.webp", "2.jpg", "10.avif"]);

        app.toggle_filter();
        assert_eq!(app.filter_mode, FilterMode::DirsOnly);
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].name, "sub_folder");

        app.toggle_filter();
        assert_eq!(app.filter_mode, FilterMode::AllFiles);

        app.toggle_filter();
        assert_eq!(app.filter_mode, FilterMode::ImagesAndDirs);

        app.move_down(1);
        assert_eq!(app.selected_entry().unwrap().name, "1.webp");
        app.enter_selected();
        assert_eq!(app.mode, Mode::Viewer);
        assert_eq!(app.current_viewer_entry().unwrap().name, "1.webp");

        app.viewer_next();
        assert_eq!(app.current_viewer_entry().unwrap().name, "2.jpg");
        app.viewer_next();
        assert_eq!(app.current_viewer_entry().unwrap().name, "10.avif");

        app.exit_viewer();
        assert_eq!(app.mode, Mode::Browser);
        assert_eq!(app.selected_entry().unwrap().name, "10.avif");

        // Test Yazi-style yank & paste into sub_folder
        app.yank_selected(ClipboardMode::Copy);
        app.jump_top();
        assert_eq!(app.selected_entry().unwrap().name, "sub_folder");
        app.enter_selected();
        app.paste(false);
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].name, "10.avif");

        let _ = fs::remove_dir_all(&tmp);
    }
}

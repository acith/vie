use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// All files and directories (default)
    AllFiles,
    /// Directories + supported image files
    ImagesAndDirs,
    /// Directories only
    DirsOnly,
}

impl FilterMode {
    pub fn next(self) -> Self {
        match self {
            FilterMode::AllFiles => FilterMode::ImagesAndDirs,
            FilterMode::ImagesAndDirs => FilterMode::DirsOnly,
            FilterMode::DirsOnly => FilterMode::AllFiles,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FilterMode::AllFiles => "All Files",
            FilterMode::ImagesAndDirs => "Dirs+Images",
            FilterMode::DirsOnly => "Dirs Only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Natural,
    Alphabetical,
    Modified,
    Size,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortConfig {
    pub by: SortBy,
    pub reverse: bool,
    pub dirs_first: bool,
}

impl Default for SortConfig {
    fn default() -> Self {
        Self {
            by: SortBy::Natural,
            reverse: false,
            dirs_first: true,
        }
    }
}

impl SortConfig {
    pub fn label(self) -> String {
        let name = match self.by {
            SortBy::Natural => "Natural",
            SortBy::Alphabetical => "Alpha",
            SortBy::Modified => "Modified",
            SortBy::Size => "Size",
            SortBy::Extension => "Ext",
        };
        let arrow = if self.reverse { "↓" } else { "↑" };
        if self.dirs_first {
            format!("{name} {arrow}")
        } else {
            format!("{name} {arrow} mix")
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_image: bool,
    pub is_audio: bool,
    pub is_video: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: PathBuf,
    pub rel_path: String,
    pub is_dir: bool,
    pub is_image: bool,
    pub is_audio: bool,
    pub is_video: bool,
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "webp", "jpg", "jpeg", "png", "gif", "avif", "bmp", "ico", "tiff", "tif", "tga", "pnm", "pbm",
    "pgm", "ppm", "qoi", "hdr", "dds", "ff",
];

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "flac", "ogg", "m4a", "aac", "opus", "wma", "aiff",
];

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "m4v", "flv", "wmv", "mpg", "mpeg", "3gp",
];

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rs", "toml", "yaml", "yml", "json", "jsonl", "sh", "bash", "zsh",
    "fish", "py", "js", "mjs", "cjs", "ts", "tsx", "jsx", "html", "htm", "css", "scss", "less",
    "c", "h", "cpp", "hpp", "cc", "cxx", "go", "java", "kt", "kts", "swift", "rb", "php", "lua",
    "sql", "xml", "svg", "csv", "tsv", "ini", "conf", "cfg", "log", "env", "lock", "vim", "el",
    "clj", "ex", "exs", "erl", "hs", "ml", "zig", "nim", "v", "r", "pl", "pm", "gradle", "cmake",
];

pub fn is_supported_image(path: &Path) -> bool {
    has_extension(path, IMAGE_EXTENSIONS)
}

pub fn is_supported_audio(path: &Path) -> bool {
    has_extension(path, AUDIO_EXTENSIONS)
}

pub fn is_supported_video(path: &Path) -> bool {
    has_extension(path, VIDEO_EXTENSIONS)
}

fn has_extension(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            list.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// Smart-case substring match (like Yazi / ripgrep):
/// case-insensitive if `needle` has no uppercase characters, case-sensitive otherwise.
pub fn smart_case_contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let has_upper = needle.chars().any(|c| c.is_uppercase());
    if has_upper {
        haystack.contains(needle)
    } else {
        haystack.to_lowercase().contains(&needle.to_lowercase())
    }
}

/// Checks whether a file is a human-editable text file.
pub fn is_text_file(path: &Path) -> bool {
    if is_supported_image(path) || is_supported_audio(path) || is_supported_video(path) {
        return false;
    }
    if has_extension(path, TEXT_EXTENSIONS) {
        return true;
    }

    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 8192];
    let Ok(n) = file.read(&mut buf) else {
        return false;
    };
    if n == 0 {
        return true;
    }
    let slice = &buf[..n];
    if slice.contains(&0) {
        return false;
    }
    match std::str::from_utf8(slice) {
        Ok(_) => true,
        Err(err) => err.error_len().is_none(),
    }
}

/// Reads the first `max_lines` lines (up to `max_bytes`) of a text file for previewing.
pub fn read_text_preview(path: &Path, max_lines: usize, max_bytes: usize) -> Option<Vec<String>> {
    if !is_text_file(path) {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let mut buf = vec![0u8; max_bytes];
    let n = file.read(&mut buf).ok()?;
    buf.truncate(n);
    if buf.contains(&0) {
        return None;
    }

    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<String> = text
        .lines()
        .take(max_lines)
        .map(|line| line.replace('\t', "    "))
        .collect();
    Some(lines)
}

pub fn sort_entries(entries: &mut [FileEntry], sort: SortConfig) {
    entries.sort_by(|a, b| {
        if sort.dirs_first {
            match (a.is_dir, b.is_dir) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
        }

        let ord = match sort.by {
            SortBy::Natural => natord::compare_ignore_case(&a.name, &b.name),
            SortBy::Alphabetical => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortBy::Modified => a
                .modified
                .cmp(&b.modified)
                .then_with(|| natord::compare_ignore_case(&a.name, &b.name)),
            SortBy::Size => a
                .size
                .cmp(&b.size)
                .then_with(|| natord::compare_ignore_case(&a.name, &b.name)),
            SortBy::Extension => {
                let ext_a = a
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default();
                let ext_b = b
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .unwrap_or_default();
                ext_a
                    .cmp(&ext_b)
                    .then_with(|| natord::compare_ignore_case(&a.name, &b.name))
            }
        };

        if sort.reverse {
            ord.reverse()
        } else {
            ord
        }
    });
}

pub fn read_directory(
    dir: &Path,
    filter: FilterMode,
    show_hidden: bool,
    sort: SortConfig,
) -> Vec<FileEntry> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut entries: Vec<FileEntry> = read_dir
        .filter_map(|res| res.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();

            if !show_hidden && name.starts_with('.') {
                return None;
            }

            let metadata = fs::metadata(&path).ok();
            let is_dir = metadata
                .as_ref()
                .map(|m| m.is_dir())
                .unwrap_or_else(|| entry.file_type().map(|t| t.is_dir()).unwrap_or(false));
            let is_image = !is_dir && is_supported_image(&path);
            let is_audio = !is_dir && !is_image && is_supported_audio(&path);
            let is_video = !is_dir && !is_image && !is_audio && is_supported_video(&path);
            let size = if is_dir {
                0
            } else {
                metadata.as_ref().map(|m| m.len()).unwrap_or(0)
            };
            let modified = metadata.as_ref().and_then(|m| m.modified().ok());

            let keep = match filter {
                FilterMode::AllFiles => true,
                FilterMode::ImagesAndDirs => is_dir || is_image,
                FilterMode::DirsOnly => is_dir,
            };

            if !keep {
                return None;
            }

            Some(FileEntry {
                path,
                name,
                is_dir,
                is_image,
                is_audio,
                is_video,
                size,
                modified,
            })
        })
        .collect();

    sort_entries(&mut entries, sort);
    entries
}

/// Breadth-first recursive search under `root` matching `query` against relative paths.
pub fn search_recursive(
    root: &Path,
    query: &str,
    filter: FilterMode,
    show_hidden: bool,
    max_results: usize,
) -> Vec<SearchHit> {
    let trimmed = query.trim();
    let mut results = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back(root.to_path_buf());

    let mut visited_dirs = 0usize;
    const MAX_DIRS: usize = 4000;

    while let Some(dir) = queue.pop_front() {
        if visited_dirs >= MAX_DIRS || results.len() >= max_results {
            break;
        }
        visited_dirs += 1;

        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };

        let mut children: Vec<_> = read_dir.flatten().collect();
        children.sort_by(|a, b| {
            natord::compare_ignore_case(
                &a.file_name().to_string_lossy(),
                &b.file_name().to_string_lossy(),
            )
        });

        for entry in children {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }

            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            // Avoid following directory symlinks to prevent cycles
            if ft.is_symlink() {
                continue;
            }

            let is_dir = ft.is_dir();
            if is_dir {
                queue.push_back(path.clone());
            }

            let is_image = !is_dir && is_supported_image(&path);
            let is_audio = !is_dir && !is_image && is_supported_audio(&path);
            let is_video = !is_dir && !is_image && !is_audio && is_supported_video(&path);

            let keep = match filter {
                FilterMode::AllFiles => true,
                FilterMode::ImagesAndDirs => is_dir || is_image,
                FilterMode::DirsOnly => is_dir,
            };
            if !keep {
                continue;
            }

            let rel_path = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| name.clone());

            if smart_case_contains(&rel_path, trimmed) {
                results.push(SearchHit {
                    path,
                    rel_path,
                    is_dir,
                    is_image,
                    is_audio,
                    is_video,
                });
                if results.len() >= max_results {
                    break;
                }
            }
        }
    }

    results
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

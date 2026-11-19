use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_image: bool,
    pub is_audio: bool,
    pub is_video: bool,
    pub size: u64,
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
        // Empty file is safe to edit in vi
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

pub fn read_directory(dir: &Path, filter: FilterMode, show_hidden: bool) -> Vec<FileEntry> {
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
                metadata.map(|m| m.len()).unwrap_or(0)
            };

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
            })
        })
        .collect();

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => natord::compare_ignore_case(&a.name, &b.name),
    });

    entries
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

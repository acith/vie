use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// Directories + supported image files (default)
    ImagesAndDirs,
    /// Directories only
    DirsOnly,
    /// All files and directories
    AllFiles,
}

impl FilterMode {
    pub fn next(self) -> Self {
        match self {
            FilterMode::ImagesAndDirs => FilterMode::DirsOnly,
            FilterMode::DirsOnly => FilterMode::AllFiles,
            FilterMode::AllFiles => FilterMode::ImagesAndDirs,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FilterMode::ImagesAndDirs => "Dirs+Images",
            FilterMode::DirsOnly => "Dirs Only",
            FilterMode::AllFiles => "All Files",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_image: bool,
    pub size: u64,
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "webp", "jpg", "jpeg", "png", "gif", "avif", "bmp", "ico", "tiff", "tif", "tga", "pnm", "pbm",
    "pgm", "ppm", "qoi", "hdr", "dds", "ff",
];

pub fn is_supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            IMAGE_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
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
            let size = if is_dir {
                0
            } else {
                metadata.map(|m| m.len()).unwrap_or(0)
            };

            let keep = match filter {
                FilterMode::ImagesAndDirs => is_dir || is_image,
                FilterMode::DirsOnly => is_dir,
                FilterMode::AllFiles => true,
            };

            if !keep {
                return None;
            }

            Some(FileEntry {
                path,
                name,
                is_dir,
                is_image,
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

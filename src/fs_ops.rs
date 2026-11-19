use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

/// Create a file or directory (trailing `/` creates a directory, Yazi-style).
pub fn create_entry(base_dir: &Path, raw_input: &str) -> Result<PathBuf> {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() {
        bail!("Name cannot be empty");
    }

    let is_dir = trimmed.ends_with('/') || trimmed.ends_with(std::path::MAIN_SEPARATOR);
    let clean = trimmed.trim_end_matches(['/', '\\']);
    if clean.is_empty() {
        bail!("Invalid name");
    }

    let target = base_dir.join(clean);
    if target.exists() {
        bail!("Already exists: {}", clean);
    }

    if is_dir {
        fs::create_dir_all(&target)
            .with_context(|| format!("Failed to create directory: {}", target.display()))?;
    } else {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create parent directory: {}", parent.display()))?;
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .with_context(|| format!("Failed to create file: {}", target.display()))?;
    }

    Ok(target)
}

/// Rename a file or directory in place.
pub fn rename_entry(src: &Path, new_name: &str) -> Result<PathBuf> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        bail!("New name cannot be empty");
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        bail!("Name cannot contain path separators");
    }

    let parent = src
        .parent()
        .ok_or_else(|| anyhow!("Cannot determine parent directory"))?;
    let dest = parent.join(trimmed);

    if dest == src {
        return Ok(dest);
    }
    if dest.exists() {
        bail!("Target already exists: {}", trimmed);
    }

    fs::rename(src, &dest)
        .with_context(|| format!("Failed to rename {} to {}", src.display(), dest.display()))?;
    Ok(dest)
}

/// Copy a file or directory recursively into `dest_dir`.
pub fn copy_entry(src: &Path, dest_dir: &Path, overwrite: bool) -> Result<PathBuf> {
    let file_name = src
        .file_name()
        .ok_or_else(|| anyhow!("Invalid source path"))?;
    let mut dest = dest_dir.join(file_name);

    if src.is_dir() && (dest == src || dest.starts_with(src)) {
        if dest == src && !overwrite {
            dest = unique_destination(dest_dir, file_name);
        } else {
            bail!("Cannot copy a directory into itself");
        }
    } else if dest.exists() {
        if overwrite {
            if dest == src {
                return Ok(dest);
            }
            remove_path(&dest)?;
        } else {
            dest = unique_destination(dest_dir, file_name);
        }
    }

    if src.is_dir() {
        copy_dir_recursive(src, &dest)?;
    } else {
        fs::copy(src, &dest)
            .with_context(|| format!("Failed to copy {} to {}", src.display(), dest.display()))?;
    }

    Ok(dest)
}

/// Move a file or directory into `dest_dir` (falls back to copy+remove across filesystems).
pub fn move_entry(src: &Path, dest_dir: &Path, overwrite: bool) -> Result<PathBuf> {
    let file_name = src
        .file_name()
        .ok_or_else(|| anyhow!("Invalid source path"))?;
    let mut dest = dest_dir.join(file_name);

    if dest == src {
        return Ok(dest);
    }
    if src.is_dir() && dest.starts_with(src) {
        bail!("Cannot move a directory into itself");
    }

    if dest.exists() {
        if overwrite {
            remove_path(&dest)?;
        } else {
            dest = unique_destination(dest_dir, file_name);
        }
    }

    match fs::rename(src, &dest) {
        Ok(()) => Ok(dest),
        Err(_) => {
            // Cross-device fallback
            if src.is_dir() {
                copy_dir_recursive(src, &dest)?;
            } else {
                fs::copy(src, &dest).with_context(|| {
                    format!("Failed to copy {} to {}", src.display(), dest.display())
                })?;
            }
            remove_path(src)?;
            Ok(dest)
        }
    }
}

/// Move paths to the OS Trash (freedesktop trash on Linux).
pub fn trash_entries(paths: &[PathBuf]) -> Result<usize> {
    if paths.is_empty() {
        return Ok(0);
    }
    trash::delete_all(paths).context("Failed to move item(s) to trash")?;
    Ok(paths.len())
}

/// Permanently delete paths.
pub fn delete_permanent(paths: &[PathBuf]) -> Result<usize> {
    let mut count = 0;
    for path in paths {
        remove_path(path)?;
        count += 1;
    }
    Ok(count)
}

fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() && !path.is_symlink() {
        fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove directory: {}", path.display()))?;
    } else {
        fs::remove_file(path)
            .with_context(|| format!("Failed to remove file: {}", path.display()))?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)
        .with_context(|| format!("Failed to create directory: {}", dest.display()))?;

    for entry in fs::read_dir(src)
        .with_context(|| format!("Failed to read directory: {}", src.display()))?
    {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            fs::copy(&src_path, &dest_path).with_context(|| {
                format!(
                    "Failed to copy {} to {}",
                    src_path.display(),
                    dest_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn unique_destination(dest_dir: &Path, original_name: &std::ffi::OsStr) -> PathBuf {
    let orig_path = Path::new(original_name);
    let stem = orig_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| original_name.to_string_lossy().into_owned());
    let ext = orig_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    for i in 1..10_000 {
        let candidate = dest_dir.join(format!("{stem}_{i}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dest_dir.join(original_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("vie_fsops_{label}_{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_create_rename_copy_move_delete() {
        let root = make_temp_dir("crud");

        // Create folder & file
        let folder = create_entry(&root, "album/").unwrap();
        assert!(folder.is_dir());

        let file = create_entry(&root, "photo.webp").unwrap();
        assert!(file.is_file());
        fs::write(&file, b"imgdata").unwrap();

        // Rename file
        let renamed = rename_entry(&file, "cover.webp").unwrap();
        assert!(renamed.is_file());
        assert!(!file.exists());

        // Copy into album (twice -> auto-rename second copy)
        let copy1 = copy_entry(&renamed, &folder, false).unwrap();
        let copy2 = copy_entry(&renamed, &folder, false).unwrap();
        assert_eq!(copy1.file_name().unwrap(), "cover.webp");
        assert_eq!(copy2.file_name().unwrap(), "cover_1.webp");

        // Move renamed into album (auto-rename -> cover_2.webp)
        let moved = move_entry(&renamed, &folder, false).unwrap();
        assert_eq!(moved.file_name().unwrap(), "cover_2.webp");
        assert!(!renamed.exists());

        // Permanent delete
        let deleted = delete_permanent(&[folder.clone()]).unwrap();
        assert_eq!(deleted, 1);
        assert!(!folder.exists());

        let _ = fs::remove_dir_all(&root);
    }
}

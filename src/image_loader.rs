use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use ratatui::layout::Size;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;

const DECODED_CACHE_CAP: usize = 24;
const PROTOCOL_CACHE_CAP: usize = 24;

#[derive(Clone)]
pub struct LoadedImage {
    pub path: PathBuf,
    pub protocol: Protocol,
    pub width: u32,
    pub height: u32,
    pub format_label: String,
}

pub enum LoadResult {
    Ready {
        request_id: u64,
        image: LoadedImage,
    },
    Error {
        request_id: u64,
        message: String,
    },
}

struct LoadRequest {
    request_id: u64,
    path: PathBuf,
    target_size: Size,
    is_preload: bool,
}

#[derive(Clone)]
struct DecodedImage {
    img: Arc<DynamicImage>,
    width: u32,
    height: u32,
    format_label: String,
}

pub struct ImageLoader {
    tx: Sender<LoadRequest>,
    rx: Receiver<LoadResult>,
    latest_id: Arc<AtomicU64>,
}

impl ImageLoader {
    pub fn new(picker: Picker) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<LoadRequest>();
        let (res_tx, res_rx) = mpsc::channel::<LoadResult>();
        let latest_id = Arc::new(AtomicU64::new(0));
        let worker_latest = Arc::clone(&latest_id);

        thread::spawn(move || {
            worker_loop(picker, req_rx, res_tx, worker_latest);
        });

        Self {
            tx: req_tx,
            rx: res_rx,
            latest_id,
        }
    }

    /// Request an image to be loaded & encoded for the given terminal cell size.
    /// Returns the monotonically increasing `request_id`.
    pub fn request(&self, path: PathBuf, target_size: Size) -> u64 {
        let id = self.latest_id.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.tx.send(LoadRequest {
            request_id: id,
            path,
            target_size,
            is_preload: false,
        });
        id
    }

    /// Queue a background preload for an adjacent image (low priority).
    pub fn preload(&self, path: PathBuf, target_size: Size) {
        let current_id = self.latest_id.load(Ordering::SeqCst);
        let _ = self.tx.send(LoadRequest {
            request_id: current_id,
            path,
            target_size,
            is_preload: true,
        });
    }

    /// Non-blocking poll for completed image loads.
    pub fn try_recv(&self) -> Option<LoadResult> {
        self.rx.try_recv().ok()
    }
}

fn worker_loop(
    picker: Picker,
    req_rx: Receiver<LoadRequest>,
    res_tx: Sender<LoadResult>,
    latest_id: Arc<AtomicU64>,
) {
    let mut decoded_cache: VecDeque<(PathBuf, DecodedImage)> = VecDeque::new();
    let mut protocol_cache: VecDeque<((PathBuf, Size), LoadedImage)> = VecDeque::new();
    let mut pending_preloads: VecDeque<LoadRequest> = VecDeque::new();

    loop {
        let first = if pending_preloads.is_empty() {
            match req_rx.recv() {
                Ok(req) => req,
                Err(_) => break,
            }
        } else {
            match req_rx.try_recv() {
                Ok(req) => req,
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some(preload_req) = pending_preloads.pop_front() {
                        process_preload(
                            &picker,
                            preload_req,
                            &latest_id,
                            &mut decoded_cache,
                            &mut protocol_cache,
                        );
                    }
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        };

        // Drain channel so rapid scrolling skips stale intermediate requests.
        let mut latest_primary: Option<LoadRequest> = None;
        if first.is_preload {
            push_preload(&mut pending_preloads, first);
        } else {
            latest_primary = Some(first);
        }

        while let Ok(next) = req_rx.try_recv() {
            if next.is_preload {
                push_preload(&mut pending_preloads, next);
            } else {
                latest_primary = Some(next);
                pending_preloads.clear();
            }
        }

        if let Some(req) = latest_primary {
            if req.request_id < latest_id.load(Ordering::SeqCst) {
                continue;
            }
            if req.target_size.width == 0 || req.target_size.height == 0 {
                continue;
            }

            let result = load_and_encode(
                &picker,
                &req.path,
                req.target_size,
                &mut decoded_cache,
                &mut protocol_cache,
            );

            // Check again before sending result
            if req.request_id < latest_id.load(Ordering::SeqCst) {
                continue;
            }

            let msg = match result {
                Ok(image) => LoadResult::Ready {
                    request_id: req.request_id,
                    image,
                },
                Err(err) => LoadResult::Error {
                    request_id: req.request_id,
                    message: err,
                },
            };

            if res_tx.send(msg).is_err() {
                break;
            }
        }
    }
}

fn push_preload(queue: &mut VecDeque<LoadRequest>, req: LoadRequest) {
    if queue.len() >= 4 {
        queue.pop_front();
    }
    queue.push_back(req);
}

fn process_preload(
    picker: &Picker,
    req: LoadRequest,
    latest_id: &Arc<AtomicU64>,
    decoded_cache: &mut VecDeque<(PathBuf, DecodedImage)>,
    protocol_cache: &mut VecDeque<((PathBuf, Size), LoadedImage)>,
) {
    if req.request_id < latest_id.load(Ordering::SeqCst) {
        return;
    }
    if req.target_size.width == 0 || req.target_size.height == 0 {
        return;
    }
    let _ = load_and_encode(
        picker,
        &req.path,
        req.target_size,
        decoded_cache,
        protocol_cache,
    );
}

fn load_and_encode(
    picker: &Picker,
    path: &Path,
    target_size: Size,
    decoded_cache: &mut VecDeque<(PathBuf, DecodedImage)>,
    protocol_cache: &mut VecDeque<((PathBuf, Size), LoadedImage)>,
) -> Result<LoadedImage, String> {
    let key = (path.to_path_buf(), target_size);
    if let Some(pos) = protocol_cache.iter().position(|(k, _)| k == &key) {
        let (_, item) = protocol_cache.remove(pos).unwrap();
        protocol_cache.push_back((key, item.clone()));
        return Ok(item);
    }

    let decoded = if let Some(pos) = decoded_cache.iter().position(|(p, _)| p == path) {
        let (p, img) = decoded_cache.remove(pos).unwrap();
        decoded_cache.push_back((p, img.clone()));
        img
    } else {
        let img = decode_image(path)?;
        if decoded_cache.len() >= DECODED_CACHE_CAP {
            decoded_cache.pop_front();
        }
        decoded_cache.push_back((path.to_path_buf(), img.clone()));
        img
    };

    let protocol = picker
        .new_protocol((*decoded.img).clone(), target_size, Resize::Scale(None))
        .map_err(|e| format!("Protocol encode error: {e}"))?;

    let loaded = LoadedImage {
        path: path.to_path_buf(),
        protocol,
        width: decoded.width,
        height: decoded.height,
        format_label: decoded.format_label,
    };

    if protocol_cache.len() >= PROTOCOL_CACHE_CAP {
        protocol_cache.pop_front();
    }
    protocol_cache.push_back((key, loaded.clone()));

    Ok(loaded)
}

fn decode_image(path: &Path) -> Result<DecodedImage, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    // AVIF is decoded via ImageMagick (`magick`) when libdav1d is not compiled in
    if ext == "avif" {
        return decode_via_magick(path, "AVIF");
    }

    match decode_native(path) {
        Ok(img) => Ok(img),
        Err(native_err) => {
            let label = if ext.is_empty() {
                "IMG".to_string()
            } else {
                ext.to_ascii_uppercase()
            };
            decode_via_magick(path, &label).map_err(|_| native_err)
        }
    }
}

fn decode_native(path: &Path) -> Result<DecodedImage, String> {
    let reader = ImageReader::open(path)
        .map_err(|e| format!("Failed to open file: {e}"))?
        .with_guessed_format()
        .map_err(|e| format!("Failed to guess format: {e}"))?;

    let format_label = reader
        .format()
        .map(|f| format!("{f:?}").to_ascii_uppercase())
        .unwrap_or_else(|| "IMG".to_string());

    let mut decoder = reader
        .into_decoder()
        .map_err(|e| format!("Decoder error: {e}"))?;

    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| format!("Decode error: {e}"))?;
    if let Some(orient) = orientation {
        img.apply_orientation(orient);
    }

    let (width, height) = (img.width(), img.height());
    Ok(DecodedImage {
        img: Arc::new(img),
        width,
        height,
        format_label,
    })
}

fn decode_via_magick(path: &Path, format_label: &str) -> Result<DecodedImage, String> {
    let input_arg = format!("{}[0]", path.display());
    let output = Command::new("magick")
        .arg(&input_arg)
        .arg("-auto-orient")
        .arg("png:-")
        .output()
        .map_err(|e| format!("ImageMagick (`magick`) unavailable: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ImageMagick decode failed: {}", stderr.trim()));
    }

    let img = image::load_from_memory_with_format(&output.stdout, ImageFormat::Png)
        .map_err(|e| format!("Failed to parse PNG from magick: {e}"))?;
    let (width, height) = (img.width(), img.height());

    Ok(DecodedImage {
        img: Arc::new(img),
        width,
        height,
        format_label: format_label.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_image::picker::ProtocolType;
    use std::fs;

    fn find_sample_with_ext(root: &Path, target_ext: &str) -> Option<PathBuf> {
        let mut stack = vec![root.to_path_buf()];
        let mut visited_dirs = 0usize;
        while let Some(dir) = stack.pop() {
            let Ok(rd) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case(target_ext))
                    {
                        return Some(p);
                    }
                } else if p.is_dir() && visited_dirs < 300 {
                    visited_dirs += 1;
                    stack.push(p);
                }
            }
        }
        None
    }

    #[test]
    fn decode_and_encode_sample_formats_for_kitty_and_sixel() {
        let sample_root = Path::new("/home/tong/sourceCode/nodeJs/imgViewer/sample_images");
        if !sample_root.exists() {
            return;
        }

        let mut kitty_picker = Picker::halfblocks();
        kitty_picker.set_protocol_type(ProtocolType::Kitty);

        let mut sixel_picker = Picker::halfblocks();
        sixel_picker.set_protocol_type(ProtocolType::Sixel);

        for ext in ["webp", "jpg", "jpeg", "gif", "png", "avif"] {
            if let Some(sample_path) = find_sample_with_ext(sample_root, ext) {
                let decoded = decode_image(&sample_path)
                    .unwrap_or_else(|e| panic!("Failed to decode {ext} ({sample_path:?}): {e}"));
                assert!(decoded.width > 0 && decoded.height > 0);

                let kitty_proto = kitty_picker
                    .new_protocol((*decoded.img).clone(), Size::new(60, 30), Resize::Scale(None))
                    .unwrap_or_else(|e| panic!("Kitty encode failed for {ext}: {e}"));
                assert!(kitty_proto.size().width > 0 && kitty_proto.size().height > 0);

                let sixel_proto = sixel_picker
                    .new_protocol((*decoded.img).clone(), Size::new(60, 30), Resize::Scale(None))
                    .unwrap_or_else(|e| panic!("Sixel encode failed for {ext}: {e}"));
                assert!(sixel_proto.size().width > 0 && sixel_proto.size().height > 0);
            }
        }
    }
}

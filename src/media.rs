use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use image::imageops::FilterType;
use image::{DynamicImage, RgbImage};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph};
use ratatui::Terminal;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};

use crate::fs;
use crate::ui::center_rect;

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn new(child: Option<Child>) -> Self {
        Self(child)
    }

    fn is_running(&mut self) -> bool {
        if let Some(ref mut c) = self.0 {
            matches!(c.try_wait(), Ok(None))
        } else {
            false
        }
    }

    fn kill(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Detects MP4/M4A files that have an ID3v2 header prepended (common with `.mp3` downloads).
/// FFmpeg's `mov/mp4` demuxer miscalculates chunk offsets on such files, producing silence.
/// Wrapping them with FFmpeg's `subfile` protocol skips the ID3 header cleanly without modifying the file.
pub fn ffmpeg_input_arg(path: &Path) -> String {
    if let Some((offset, file_size)) = detect_id3_wrapped_mp4(path) {
        return format!(
            "subfile,,start,{offset},end,{file_size},,:{path_str}",
            path_str = path.to_string_lossy()
        );
    }
    path.to_string_lossy().into_owned()
}

fn detect_id3_wrapped_mp4(path: &Path) -> Option<(u64, u64)> {
    let mut file = File::open(path).ok()?;
    let file_size = file.metadata().ok()?.len();
    if file_size < 24 {
        return None;
    }

    let mut hdr = [0u8; 10];
    file.read_exact(&mut hdr).ok()?;
    if &hdr[0..3] != b"ID3" {
        return None;
    }

    // ID3v2 synchsafe 28-bit integer size
    let id3_body_size = ((hdr[6] as u64 & 0x7f) << 21)
        | ((hdr[7] as u64 & 0x7f) << 14)
        | ((hdr[8] as u64 & 0x7f) << 7)
        | (hdr[9] as u64 & 0x7f);
    let has_footer = (hdr[5] & 0x10) != 0;
    let offset = 10 + id3_body_size + if has_footer { 10 } else { 0 };

    if offset + 12 > file_size {
        return None;
    }

    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut box_hdr = [0u8; 12];
    file.read_exact(&mut box_hdr).ok()?;
    if &box_hdr[4..8] == b"ftyp" {
        Some((offset, file_size))
    } else {
        None
    }
}

fn probe_duration_secs(path: &Path) -> Option<f64> {
    let input = ffmpeg_input_arg(path);
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(&input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse::<f64>().ok().filter(|d| *d > 0.0)
}

fn probe_video_dimensions(path: &Path) -> Option<(u32, u32)> {
    let input = ffmpeg_input_arg(path);
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=s=x:p=0",
        ])
        .arg(&input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout);
    let line = s.lines().next()?.trim();
    let mut parts = line.split('x');
    let w = parts.next()?.parse::<u32>().ok()?;
    let h = parts.next()?.parse::<u32>().ok()?;
    if w > 0 && h > 0 {
        Some((w, h))
    } else {
        None
    }
}

fn spawn_audio_process(path: &Path, video_disabled: bool) -> Result<Child, String> {
    let input = ffmpeg_input_arg(path);
    let mut ffplay = Command::new("ffplay");
    ffplay
        .env("PULSE_LATENCY_MSEC", "150")
        .args(["-nodisp", "-autoexit", "-loglevel", "quiet"]);
    if video_disabled {
        ffplay.arg("-vn");
    }
    ffplay
        .arg("-i")
        .arg(&input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Ok(child) = ffplay.spawn() {
        return Ok(child);
    }

    let mut mpv = Command::new("mpv");
    mpv.env("PULSE_LATENCY_MSEC", "150")
        .args(["--no-video", "--really-quiet"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Ok(child) = mpv.spawn() {
        return Ok(child);
    }

    let mut cvlc = Command::new("cvlc");
    cvlc.env("PULSE_LATENCY_MSEC", "150")
        .args(["--play-and-exit", "--no-video", "--quiet"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Ok(child) = cvlc.spawn() {
        return Ok(child);
    }

    Err("No terminal audio player found (install ffmpeg/ffplay, mpv, or vlc)".to_string())
}

fn format_mm_ss(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let mins = total / 60;
    let rem = total % 60;
    format!("{mins:02}:{rem:02}")
}

fn next_prng(state: &mut u64) -> u64 {
    let mut x = *state;
    if x == 0 {
        x = 0x9e37_79b9_7f4a_7c15;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn pick_next_track(current: usize, len: usize, shuffle: bool, rng: &mut u64) -> usize {
    if len <= 1 {
        return 0;
    }
    if shuffle {
        let step = (next_prng(rng) as usize % (len - 1)) + 1;
        (current + step) % len
    } else {
        (current + 1) % len
    }
}

/// Plays an audio playlist inside the terminal with auto-next, shuffle toggle, and prev/next controls.
/// Returns the `PathBuf` of the last track played.
pub fn play_audio(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    playlist: &[PathBuf],
    start_index: usize,
) -> Result<PathBuf, String> {
    if playlist.is_empty() {
        return Err("No audio files to play".to_string());
    }

    let mut current_idx = start_index.min(playlist.len() - 1);
    let mut shuffle = false;
    let mut history: Vec<usize> = Vec::new();
    let mut rng_state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x1234_5678_9abc_def0);

    let eq_heights: [&[&str]; 4] = [
        &[
            "  ▂▄▆█▆▄▂   ▄▆█▄▂▄▆█▆▄▂   ▂▄▆█▄▂  ",
            "  ████████  ████████████  ██████  ",
        ],
        &[
            "  ▄▆█▄▂▄▆█  ▂▄▆█▆▄▂▄▆█▄   ▆█▆▄▂▄  ",
            "  ████████  ████████████  ██████  ",
        ],
        &[
            "  ▆█▄▂▄▆█▆  ▆█▄▂▄▆█▄▂▄▆   ▂▄▆█▆▄  ",
            "  ████████  ████████████  ██████  ",
        ],
        &[
            "  ▂▄▆█▆▄▂▄  ▄▆█▆▄▂▄▆█▆▄   ▄▆█▄▂▄  ",
            "  ████████  ████████████  ██████  ",
        ],
    ];

    let _ = terminal.clear();

    'playlist_loop: loop {
        let path = &playlist[current_idx];
        let child = spawn_audio_process(path, true)?;
        let mut guard = ChildGuard::new(Some(child));

        let duration = probe_duration_secs(path);
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let ext_label = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_uppercase())
            .unwrap_or_else(|| "AUDIO".to_string());

        let start = Instant::now();
        let mut last_draw = Instant::now() - Duration::from_secs(1);
        let mut needs_redraw = true;
        let mut tick: usize = 0;

        loop {
            if needs_redraw || last_draw.elapsed() >= Duration::from_millis(500) {
                let elapsed = start.elapsed().as_secs_f64();
                let ratio = duration
                    .map(|d| (elapsed / d).clamp(0.0, 1.0))
                    .unwrap_or(0.0);
                let time_label = match duration {
                    Some(d) => format!("{} / {}", format_mm_ss(elapsed.min(d)), format_mm_ss(d)),
                    None => format!("{} / --:--", format_mm_ss(elapsed)),
                };

                let eq = eq_heights[tick % eq_heights.len()];
                tick = tick.wrapping_add(1);

                let track_pos = format!("{}/{}", current_idx + 1, playlist.len());
                let (mode_label, mode_color) = if shuffle {
                    ("Shuffle", Color::Magenta)
                } else {
                    ("Sequential", Color::Green)
                };

                terminal
                    .draw(|frame| {
                        let area = frame.area();
                        let box_w = area.width.min(72).max(40);
                        let box_h = 12.min(area.height);
                        let rect = center_rect(area, Size::new(box_w, box_h));

                        let block = Block::default()
                            .title(" 🎵 Terminal Audio Player ")
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(Color::Cyan));
                        let inner = block.inner(rect);
                        frame.render_widget(block, rect);

                        let rows = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([
                                Constraint::Length(1),
                                Constraint::Length(1),
                                Constraint::Length(2),
                                Constraint::Length(2),
                                Constraint::Length(1),
                                Constraint::Min(1),
                            ])
                            .split(inner);

                        let meta_line = Paragraph::new(Line::from(vec![
                            Span::styled(
                                format!(" Track {track_pos} "),
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw("  "),
                            Span::styled(
                                format!(" {mode_label} "),
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(mode_color)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  [{ext_label} • {}]", fs::format_size(file_size)),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]))
                        .alignment(Alignment::Center);
                        frame.render_widget(meta_line, rows[0]);

                        let title = Paragraph::new(Line::from(Span::styled(
                            file_name.as_str(),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )))
                        .alignment(Alignment::Center);
                        frame.render_widget(title, rows[1]);

                        let visualizer = Paragraph::new(vec![
                            Line::from(Span::styled(eq[0], Style::default().fg(Color::Cyan))),
                            Line::from(Span::styled(eq[1], Style::default().fg(Color::Blue))),
                        ])
                        .alignment(Alignment::Center);
                        frame.render_widget(visualizer, rows[2]);

                        let gauge = Gauge::default()
                            .block(Block::default())
                            .gauge_style(
                                Style::default()
                                    .fg(Color::Green)
                                    .bg(Color::DarkGray)
                                    .add_modifier(Modifier::BOLD),
                            )
                            .ratio(ratio)
                            .label(time_label.as_str());
                        frame.render_widget(gauge, rows[3]);

                        let hint = Paragraph::new(Line::from(Span::styled(
                            "n/→:next  p/←:prev  s:shuffle/seq  q/Esc:stop & return",
                            Style::default().fg(Color::DarkGray),
                        )))
                        .alignment(Alignment::Center);
                        frame.render_widget(hint, rows[5]);
                    })
                    .map_err(|e| e.to_string())?;

                last_draw = Instant::now();
                needs_redraw = false;
            }

            // Track finished naturally -> advance to next track automatically
            if !guard.is_running() {
                if playlist.len() <= 1 {
                    break 'playlist_loop;
                }
                history.push(current_idx);
                current_idx = pick_next_track(current_idx, playlist.len(), shuffle, &mut rng_state);
                continue 'playlist_loop;
            }

            if event::poll(Duration::from_millis(80)).map_err(|e| e.to_string())? {
                if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                    if key.kind != KeyEventKind::Release {
                        let is_ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('c');
                        if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) || is_ctrl_c {
                            guard.kill();
                            break 'playlist_loop;
                        }
                        match key.code {
                            KeyCode::Char('n')
                            | KeyCode::Char('j')
                            | KeyCode::Char('l')
                            | KeyCode::Right
                            | KeyCode::Down => {
                                guard.kill();
                                history.push(current_idx);
                                current_idx = pick_next_track(
                                    current_idx,
                                    playlist.len(),
                                    shuffle,
                                    &mut rng_state,
                                );
                                continue 'playlist_loop;
                            }
                            KeyCode::Char('p')
                            | KeyCode::Char('k')
                            | KeyCode::Char('h')
                            | KeyCode::Left
                            | KeyCode::Up => {
                                guard.kill();
                                current_idx = history.pop().unwrap_or_else(|| {
                                    (current_idx + playlist.len() - 1) % playlist.len()
                                });
                                continue 'playlist_loop;
                            }
                            KeyCode::Char('s') => {
                                shuffle = !shuffle;
                                needs_redraw = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    Ok(playlist[current_idx].clone())
}

/// Plays a video file (e.g. `.mp4`) directly inside the terminal screen with synchronized audio.
pub fn play_video(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    picker: &Picker,
    path: &Path,
    lite_mode: bool,
) -> Result<(), String> {
    let input = ffmpeg_input_arg(path);
    let (src_w, src_h) = probe_video_dimensions(path).unwrap_or((640, 360));
    let duration = probe_duration_secs(path);
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    // Scale video frame resolution & FPS for smooth playback even on low-spec CPUs
    let (max_w, max_h, fps): (u32, u32, &str) = if lite_mode {
        (240, 136, "8")
    } else {
        (320, 180, "10")
    };
    let scale = (max_w as f64 / src_w as f64)
        .min(max_h as f64 / src_h as f64)
        .min(1.0);
    let out_w = ((src_w as f64 * scale).round() as u32).max(16) & !1;
    let out_h = ((src_h as f64 * scale).round() as u32).max(16) & !1;

    let term_size = terminal.size().map_err(|e| e.to_string())?;
    let render_target = Size::new(
        term_size.width.saturating_sub(2).max(10),
        term_size.height.saturating_sub(2).max(6),
    );

    let mut ffmpeg = Command::new("ffmpeg")
        .args(["-loglevel", "quiet", "-flags2", "fast", "-re", "-i"])
        .arg(&input)
        .args([
            "-sws_flags",
            "fast_bilinear",
            "-vf",
            &format!("scale={out_w}:{out_h}"),
            "-r",
            fps,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start ffmpeg for terminal video: {e}"))?;

    let stdout = ffmpeg
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture ffmpeg video stream".to_string())?;
    let mut ffmpeg_guard = ChildGuard::new(Some(ffmpeg));

    // Spawn background audio track (no window, with generous buffer via PULSE_LATENCY_MSEC)
    let audio_child = spawn_audio_process(path, true).ok();
    let mut audio_guard = ChildGuard::new(audio_child);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop_flag);
    let fast_halfblocks = Arc::new(AtomicBool::new(lite_mode));
    let worker_fast = Arc::clone(&fast_halfblocks);
    let hd_picker = picker.clone();
    let lite_picker = Picker::halfblocks();

    let (frame_tx, frame_rx) = mpsc::sync_channel::<Protocol>(1);

    thread::spawn(move || {
        let mut reader = stdout;
        let frame_bytes = (out_w as usize) * (out_h as usize) * 3;
        let mut buf = vec![0u8; frame_bytes];

        while !worker_stop.load(Ordering::Relaxed) {
            if reader.read_exact(&mut buf).is_err() {
                break;
            }
            if let Some(rgb) = RgbImage::from_raw(out_w, out_h, buf.clone()) {
                let dyn_img = DynamicImage::ImageRgb8(rgb);
                let active_picker = if worker_fast.load(Ordering::Relaxed) {
                    &lite_picker
                } else {
                    &hd_picker
                };
                if let Ok(proto) = active_picker.new_protocol(
                    dyn_img,
                    render_target,
                    Resize::Scale(Some(FilterType::Nearest)),
                ) {
                    let _ = frame_tx.try_send(proto);
                }
            }
        }
    });

    let start = Instant::now();
    let mut current_frame: Option<Protocol> = None;
    let mut show_bar = true;
    let _ = terminal.clear();

    loop {
        let mut got_new_frame = false;
        while let Ok(proto) = frame_rx.try_recv() {
            current_frame = Some(proto);
            got_new_frame = true;
        }

        let elapsed = start.elapsed().as_secs_f64();
        let time_str = match duration {
            Some(d) => format!("{} / {}", format_mm_ss(elapsed.min(d)), format_mm_ss(d)),
            None => format_mm_ss(elapsed),
        };
        let mode_tag = if fast_halfblocks.load(Ordering::Relaxed) {
            "Fast"
        } else {
            "HD"
        };

        if got_new_frame || current_frame.is_none() {
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    let (vid_area, bar_area) = if show_bar && area.height > 2 {
                        let chunks = Layout::default()
                            .direction(Direction::Vertical)
                            .constraints([Constraint::Min(1), Constraint::Length(1)])
                            .split(area);
                        (chunks[0], Some(chunks[1]))
                    } else {
                        (area, None)
                    };

                    if let Some(ref proto) = current_frame {
                        let centered = center_rect(vid_area, proto.size());
                        frame.render_widget(Image::new(proto), centered);
                    } else {
                        let loading = Paragraph::new(format!("Loading video {file_name}..."))
                            .style(Style::default().fg(Color::DarkGray))
                            .alignment(Alignment::Center);
                        frame.render_widget(loading, vid_area);
                    }

                    if let Some(bar_rect) = bar_area {
                        let left = Line::from(vec![
                            Span::styled(
                                " VIDEO ",
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::Magenta)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(" "),
                            Span::styled(
                                file_name.as_str(),
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  [{time_str}]  [{mode_tag}]"),
                                Style::default().fg(Color::Cyan),
                            ),
                        ]);

                        let right = Line::from(Span::styled(
                            "m:Fast/HD  i:bar  q/Esc:stop ",
                            Style::default().fg(Color::DarkGray),
                        ));

                        let bar = Layout::default()
                            .direction(Direction::Horizontal)
                            .constraints([Constraint::Min(10), Constraint::Length(30)])
                            .split(bar_rect);

                        frame.render_widget(Paragraph::new(left), bar[0]);
                        frame.render_widget(
                            Paragraph::new(right).alignment(Alignment::Right),
                            bar[1],
                        );
                    }
                })
                .map_err(|e| e.to_string())?;
        }

        if event::poll(Duration::from_millis(40)).map_err(|e| e.to_string())? {
            if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                if key.kind != KeyEventKind::Release {
                    let is_ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c');
                    if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) || is_ctrl_c {
                        break;
                    } else if key.code == KeyCode::Char('i') {
                        show_bar = !show_bar;
                        let _ = terminal.clear();
                    } else if key.code == KeyCode::Char('m') {
                        let prev = fast_halfblocks.load(Ordering::Relaxed);
                        fast_halfblocks.store(!prev, Ordering::Relaxed);
                        let _ = terminal.clear();
                    }
                }
            }
        }

        if !ffmpeg_guard.is_running() && !audio_guard.is_running() {
            break;
        }
    }

    stop_flag.store(true, Ordering::Relaxed);
    ffmpeg_guard.kill();
    audio_guard.kill();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_id3_wrapped_mp4_subfile_arg() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp_mp4 = std::env::temp_dir().join(format!("vie_id3_mp4_{nanos}.mp3"));
        let tmp_mp3 = std::env::temp_dir().join(format!("vie_plain_{nanos}.mp3"));

        // ID3v2 header with body size 16 (synchsafe = 0,0,0,16), followed by MP4 `ftypisom`
        let mut data = vec![b'I', b'D', b'3', 3, 0, 0, 0, 0, 0, 16];
        data.extend_from_slice(&[0u8; 16]);
        data.extend_from_slice(&[0, 0, 0, 20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm']);
        fs::write(&tmp_mp4, &data).unwrap();

        let arg = ffmpeg_input_arg(&tmp_mp4);
        assert!(arg.starts_with(&format!("subfile,,start,26,end,{},,:{}", data.len(), tmp_mp4.display())));

        // Plain MP3 (ID3v2 followed by MPEG sync bytes 0xFF 0xFB)
        let mut plain = vec![b'I', b'D', b'3', 3, 0, 0, 0, 0, 0, 16];
        plain.extend_from_slice(&[0u8; 16]);
        plain.extend_from_slice(&[0xff, 0xfb, 0x90, 0x00, 0, 0, 0, 0, 0, 0, 0, 0]);
        fs::write(&tmp_mp3, &plain).unwrap();

        assert_eq!(ffmpeg_input_arg(&tmp_mp3), tmp_mp3.to_string_lossy());

        let _ = fs::remove_file(&tmp_mp4);
        let _ = fs::remove_file(&tmp_mp3);
    }

    #[test]
    fn playlist_next_sequential_and_shuffle() {
        let mut rng = 42u64;
        assert_eq!(pick_next_track(0, 3, false, &mut rng), 1);
        assert_eq!(pick_next_track(2, 3, false, &mut rng), 0);

        for cur in 0..5 {
            let nxt = pick_next_track(cur, 5, true, &mut rng);
            assert_ne!(nxt, cur);
            assert!(nxt < 5);
        }
    }
}

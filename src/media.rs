use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
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

fn probe_duration_secs(path: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse::<f64>().ok().filter(|d| *d > 0.0)
}

fn probe_video_dimensions(path: &Path) -> Option<(u32, u32)> {
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
        .arg(path)
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
    let mut ffplay = Command::new("ffplay");
    ffplay.args(["-nodisp", "-autoexit", "-loglevel", "quiet"]);
    if video_disabled {
        ffplay.arg("-vn");
    }
    ffplay
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Ok(child) = ffplay.spawn() {
        return Ok(child);
    }

    let mut mpv = Command::new("mpv");
    mpv.args(["--no-video", "--really-quiet"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Ok(child) = mpv.spawn() {
        return Ok(child);
    }

    let mut cvlc = Command::new("cvlc");
    cvlc.args(["--play-and-exit", "--no-video", "--quiet"])
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

/// Plays an audio file (e.g. `.mp3`) inside the terminal with a TUI progress & visualizer screen.
pub fn play_audio(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    path: &Path,
) -> Result<(), String> {
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
    let _ = terminal.clear();

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
    let mut tick: usize = 0;

    loop {
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

        terminal
            .draw(|frame| {
                let area = frame.area();
                let box_w = area.width.min(68).max(36);
                let box_h = 11.min(area.height);
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
                        Constraint::Length(2),
                        Constraint::Length(2),
                        Constraint::Length(2),
                        Constraint::Min(1),
                    ])
                    .split(inner);

                let title = Paragraph::new(Line::from(vec![
                    Span::styled(
                        file_name.as_str(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  [{ext_label} • {}]", fs::format_size(file_size)),
                        Style::default().fg(Color::Yellow),
                    ),
                ]))
                .alignment(Alignment::Center);
                frame.render_widget(title, rows[0]);

                let visualizer = Paragraph::new(vec![
                    Line::from(Span::styled(eq[0], Style::default().fg(Color::Cyan))),
                    Line::from(Span::styled(eq[1], Style::default().fg(Color::Blue))),
                ])
                .alignment(Alignment::Center);
                frame.render_widget(visualizer, rows[1]);

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
                frame.render_widget(gauge, rows[2]);

                let hint = Paragraph::new(Line::from(Span::styled(
                    "Press 'q', Esc, or Enter to stop and return to Explorer",
                    Style::default().fg(Color::DarkGray),
                )))
                .alignment(Alignment::Center);
                frame.render_widget(hint, rows[4]);
            })
            .map_err(|e| e.to_string())?;

        if !guard.is_running() {
            break;
        }

        if event::poll(Duration::from_millis(120)).map_err(|e| e.to_string())? {
            if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                if key.kind != KeyEventKind::Release {
                    let is_ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c');
                    if matches!(
                        key.code,
                        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ')
                    ) || is_ctrl_c
                    {
                        break;
                    }
                }
            }
        }
    }

    guard.kill();
    Ok(())
}

/// Plays a video file (e.g. `.mp4`) directly inside the terminal screen with synchronized audio.
pub fn play_video(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    picker: &Picker,
    path: &Path,
) -> Result<(), String> {
    let (src_w, src_h) = probe_video_dimensions(path).unwrap_or((640, 360));
    let duration = probe_duration_secs(path);
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    // Scale video frame resolution for responsive real-time terminal encoding
    let max_w: u32 = 420;
    let max_h: u32 = 240;
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
        .args(["-loglevel", "quiet", "-re", "-i"])
        .arg(path)
        .args([
            "-vf",
            &format!("scale={out_w}:{out_h}"),
            "-r",
            "12",
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

    // Spawn background audio track (no window)
    let audio_child = spawn_audio_process(path, true).ok();
    let mut audio_guard = ChildGuard::new(audio_child);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop_flag);
    let worker_picker = picker.clone();

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
                if let Ok(proto) =
                    worker_picker.new_protocol(dyn_img, render_target, Resize::Scale(None))
                {
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
                                format!("  [{time_str}]  ({src_w}x{src_h})"),
                                Style::default().fg(Color::Cyan),
                            ),
                        ]);

                        let right = Line::from(Span::styled(
                            "i:toggle bar  q/Esc:stop & return ",
                            Style::default().fg(Color::DarkGray),
                        ));

                        let bar = Layout::default()
                            .direction(Direction::Horizontal)
                            .constraints([Constraint::Min(10), Constraint::Length(35)])
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

        if event::poll(Duration::from_millis(16)).map_err(|e| e.to_string())? {
            if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                if key.kind != KeyEventKind::Release {
                    let is_ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c');
                    if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) || is_ctrl_c {
                        break;
                    } else if key.code == KeyCode::Char('i') {
                        show_bar = !show_bar;
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

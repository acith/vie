use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_image::picker::ProtocolType;
use ratatui_image::Image;

use crate::app::{App, ClipboardMode, ConfirmKind, ImageState, Popup};
use crate::fs::{self, FileEntry, FilterMode};
use crate::ui::center_rect;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header
            Constraint::Min(3),    // 3-column browser
            Constraint::Length(1), // Status bar
        ])
        .split(area);

    render_header(frame, app, vertical[0]);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(35),
            Constraint::Percentage(45),
        ])
        .split(vertical[1]);

    render_parent_pane(frame, app, columns[0]);
    render_current_pane(frame, app, columns[1]);
    render_preview_pane(frame, app, columns[2]);
    render_status_bar(frame, app, vertical[2]);

    if app.has_popup() {
        render_popup(frame, app, area);
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let proto_name = match app.protocol_type {
        ProtocolType::Kitty => "Kitty",
        ProtocolType::Sixel => "Sixel",
        ProtocolType::Iterm2 => "iTerm2",
        ProtocolType::Halfblocks => "Halfblocks",
    };

    let dir_str = app.current_dir.to_string_lossy();
    let hidden_badge = if app.show_hidden { " [Hidden]" } else { "" };

    let mut spans = vec![
        Span::styled(
            " vie ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            dir_str.as_ref(),
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  [{}]{hidden_badge}", app.filter_mode.label()),
            Style::default().fg(Color::Yellow),
        ),
    ];

    if !app.selected_paths.is_empty() {
        spans.push(Span::styled(
            format!("  [{} selected]", app.selected_paths.len()),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    if let Some(ref clip) = app.clipboard {
        let (label, color) = match clip.mode {
            ClipboardMode::Copy => ("Copy", Color::Green),
            ClipboardMode::Cut => ("Cut", Color::Red),
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!(" {label}: {} ", clip.paths.len()),
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ));
    }

    spans.push(Span::styled(
        format!("  ({proto_name})"),
        Style::default().fg(Color::DarkGray),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_parent_pane(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.parent_entries.is_empty() {
        return;
    }

    let items: Vec<ListItem> = app
        .parent_entries
        .iter()
        .map(|e| simple_entry_item(e, true))
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(app.parent_selected);
    frame.render_stateful_widget(list, inner, &mut state);
}

fn render_current_pane(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.entries.is_empty() {
        let hint = match app.filter_mode {
            FilterMode::DirsOnly => "No folders (press 'f' to show images)",
            FilterMode::ImagesAndDirs => "No folders or images (press 'f' for all files)",
            FilterMode::AllFiles => "Empty directory (press 'a' to create)",
        };
        let empty = Paragraph::new(hint)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(empty, inner);
        return;
    }

    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|e| current_entry_item(e, app))
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, inner, &mut state);
}

fn render_preview_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    // Avoid emitting Kitty/Sixel image escape sequences while a modal popup is open
    if app.has_popup() {
        return;
    }

    let Some(entry) = app.selected_entry().cloned() else {
        return;
    };

    if entry.is_dir {
        if app.preview_dir_entries.is_empty() {
            let empty = Paragraph::new("Empty directory")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(empty, area);
            return;
        }
        let items: Vec<ListItem> = app
            .preview_dir_entries
            .iter()
            .map(|e| simple_entry_item(e, true))
            .collect();
        frame.render_widget(List::new(items), area);
        return;
    }

    if !entry.is_image {
        let info = Paragraph::new(vec![
            Line::from(Span::styled(
                entry.name.as_str(),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("Size: {}", fs::format_size(entry.size))),
            Line::from(Span::styled(
                "Not a supported image format",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        frame.render_widget(info, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let img_area = chunks[0];
    let meta_area = chunks[1];

    let padded = Rect {
        x: img_area.x.saturating_add(1),
        y: img_area.y,
        width: img_area.width.saturating_sub(2),
        height: img_area.height,
    };

    if padded.width > 0 && padded.height > 0 {
        app.ensure_image_loaded(Size::new(padded.width, padded.height));
    }

    let mut meta_text = format!(" {} ", fs::format_size(entry.size));

    match &app.image_state {
        ImageState::Loaded(loaded) if loaded.path == entry.path => {
            let centered = center_rect(padded, loaded.protocol.size());
            frame.render_widget(Image::new(&loaded.protocol), centered);
            meta_text = format!(
                " {}x{}  {}  {} ",
                loaded.width,
                loaded.height,
                loaded.format_label,
                fs::format_size(entry.size)
            );
        }
        ImageState::Loading | ImageState::Loaded(_) => {
            let loading = Paragraph::new("Loading preview...")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(loading, padded);
        }
        ImageState::Error { message } => {
            let err = Paragraph::new(format!("Cannot preview image:\n{message}"))
                .style(Style::default().fg(Color::Red))
                .alignment(Alignment::Center);
            frame.render_widget(err, padded);
        }
        ImageState::Empty => {}
    }

    let meta = Paragraph::new(meta_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(meta, meta_area);
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    if let Some((ref msg, is_error)) = app.status_message {
        let style = if is_error {
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        };
        let line = Line::from(Span::styled(format!(" {msg} "), style));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let total = app.entries.len();
    let pos = if total == 0 { 0 } else { app.selected + 1 };

    let selected_info = app
        .selected_entry()
        .map(|e| {
            if e.is_dir {
                format!("{}/", e.name)
            } else {
                format!("{} ({})", e.name, fs::format_size(e.size))
            }
        })
        .unwrap_or_default();

    let left = Line::from(vec![
        Span::styled(
            " NORMAL ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{pos}/{total} "),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(selected_info),
    ]);

    let hints = "a:add r:ren y/x/p:cp/mv d/D:del Spc:sel /:find f:flt q:quit ";
    let right = Line::from(Span::styled(hints, Style::default().fg(Color::DarkGray)));

    let hint_len = hints.len() as u16;
    let bar = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(hint_len)])
        .split(area);

    frame.render_widget(Paragraph::new(left), bar[0]);
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), bar[1]);
}

fn render_popup(frame: &mut Frame, app: &App, area: Rect) {
    match &app.popup {
        Popup::Input {
            title,
            value,
            cursor,
            ..
        } => {
            let popup_w = area.width.min(64).max(30);
            let popup_h = 3;
            let rect = center_rect(area, Size::new(popup_w, popup_h));

            frame.render_widget(Clear, rect);
            let block = Block::default()
                .title(title.as_str())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);

            let para = Paragraph::new(value.as_str()).style(Style::default().fg(Color::White));
            frame.render_widget(para, inner);

            let cursor_x = inner.x + (*cursor as u16).min(inner.width.saturating_sub(1));
            frame.set_cursor_position(Position::new(cursor_x, inner.y));
        }
        Popup::Confirm {
            kind,
            title,
            message,
        } => {
            let popup_w = area.width.min(64).max(36);
            let popup_h = 5;
            let rect = center_rect(area, Size::new(popup_w, popup_h));

            let border_color = match kind {
                ConfirmKind::Delete(_) => Color::Red,
                ConfirmKind::Trash(_) => Color::Yellow,
            };

            frame.render_widget(Clear, rect);
            let block = Block::default()
                .title(title.as_str())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);

            let text = Paragraph::new(vec![
                Line::from(Span::styled(
                    message.as_str(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "Press 'y'/Enter to confirm, 'n'/Esc to cancel",
                    Style::default().fg(Color::DarkGray),
                )),
            ])
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });

            frame.render_widget(text, inner);
        }
        Popup::None => {}
    }
}

fn current_entry_item(entry: &FileEntry, app: &App) -> ListItem<'static> {
    let is_selected = app.selected_paths.contains(&entry.path);
    let clip_state = app.clipboard.as_ref().and_then(|c| {
        if c.paths.contains(&entry.path) {
            Some(c.mode)
        } else {
            None
        }
    });

    let mut spans: Vec<Span<'static>> = Vec::new();

    // Marker column (selection / yank / cut)
    if is_selected {
        spans.push(Span::styled(
            "▎",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    } else if let Some(mode) = clip_state {
        match mode {
            ClipboardMode::Copy => spans.push(Span::styled(
                "+",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )),
            ClipboardMode::Cut => spans.push(Span::styled(
                "-",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
        }
    } else {
        spans.push(Span::raw(" "));
    }

    let (suffix, base_style) = if entry.is_dir {
        (
            "/",
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        )
    } else if entry.is_image {
        ("", Style::default().fg(Color::White))
    } else {
        ("", Style::default().fg(Color::DarkGray))
    };

    let name_style = if is_selected {
        base_style.fg(Color::Yellow)
    } else {
        base_style
    };

    spans.push(Span::styled(format!("{}{suffix}", entry.name), name_style));

    ListItem::new(Line::from(spans))
}

fn simple_entry_item(entry: &FileEntry, dimmed: bool) -> ListItem<'static> {
    let (prefix, suffix, style) = if entry.is_dir {
        let color = if dimmed { Color::Blue } else { Color::LightBlue };
        (
            " ",
            "/",
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )
    } else if entry.is_image {
        let color = if dimmed { Color::DarkGray } else { Color::White };
        (" ", "", Style::default().fg(color))
    } else {
        (" ", "", Style::default().fg(Color::DarkGray))
    };

    ListItem::new(Line::from(Span::styled(
        format!("{prefix}{}{suffix}", entry.name),
        style,
    )))
}

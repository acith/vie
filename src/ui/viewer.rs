use ratatui::layout::{Alignment, Constraint, Direction, Layout, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use ratatui_image::Image;

use crate::app::{App, ImageState};
use crate::fs;
use crate::ui::center_rect;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let (img_area, bar_area) = if app.show_viewer_bar && area.height > 2 {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    if img_area.width > 0 && img_area.height > 0 {
        app.ensure_image_loaded(Size::new(img_area.width, img_area.height));
    }

    let Some(entry) = app.current_viewer_entry().cloned() else {
        let msg = Paragraph::new("No images to display (press 'q' to return)")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(msg, img_area);
        return;
    };

    let mut dims_text = String::new();

    match &app.image_state {
        ImageState::Loaded(loaded) if loaded.path == entry.path => {
            let centered = center_rect(img_area, loaded.protocol.size());
            frame.render_widget(Image::new(&loaded.protocol), centered);
            dims_text = format!(" {}x{} {} ", loaded.width, loaded.height, loaded.format_label);
        }
        ImageState::Loading | ImageState::Loaded(_) => {
            let loading = Paragraph::new(format!("Loading {}...", entry.name))
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            frame.render_widget(loading, img_area);
        }
        ImageState::Error { message } => {
            let err = Paragraph::new(format!("Failed to load {}:\n{message}", entry.name))
                .style(Style::default().fg(Color::Red))
                .alignment(Alignment::Center);
            frame.render_widget(err, img_area);
        }
        ImageState::Empty => {}
    }

    if let Some(bar_rect) = bar_area {
        let total = app.viewer_images.len();
        let pos = app.viewer_index + 1;

        let left = Line::from(vec![
            Span::styled(
                " VIEW ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{pos}/{total} "),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(entry.name.as_str(), Style::default().fg(Color::White)),
            Span::styled(dims_text, Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("({})", fs::format_size(entry.size)),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        let right = Line::from(Span::styled(
            "j/k/h/l/Space:nav  gg/G:first/last  i:bar  q:back ",
            Style::default().fg(Color::DarkGray),
        ));

        let bar = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(10), Constraint::Length(50)])
            .split(bar_rect);

        frame.render_widget(Paragraph::new(left), bar[0]);
        frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), bar[1]);
    }
}

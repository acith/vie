pub mod browser;
pub mod viewer;

use ratatui::layout::{Rect, Size};
use ratatui::Frame;

use crate::app::{App, Mode};

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.mode {
        Mode::Browser => browser::render(frame, app),
        Mode::Viewer => viewer::render(frame, app),
    }
}

pub fn center_rect(outer: Rect, img_size: Size) -> Rect {
    let w = img_size.width.min(outer.width);
    let h = img_size.height.min(outer.height);
    let x = outer.x + (outer.width.saturating_sub(w)) / 2;
    let y = outer.y + (outer.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

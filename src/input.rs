use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{App, ClipboardMode, Mode, Popup};

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if key.kind == KeyEventKind::Release {
        return;
    }

    // Ctrl+C quits from anywhere
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    // If a popup is open, route keys exclusively to the popup
    if app.has_popup() {
        handle_popup_key(app, key);
        return;
    }

    // Clear previous transient status message on next user action
    app.status_message = None;

    match app.mode {
        Mode::Browser => handle_browser_key(app, key),
        Mode::Viewer => handle_viewer_key(app, key),
    }
}

fn handle_popup_key(app: &mut App, key: KeyEvent) {
    match &mut app.popup {
        Popup::Input { value, cursor, .. } => match key.code {
            KeyCode::Esc => app.close_popup(),
            KeyCode::Enter => app.submit_popup(),
            KeyCode::Left => {
                *cursor = cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                let max = value.chars().count();
                if *cursor < max {
                    *cursor += 1;
                }
            }
            KeyCode::Home => *cursor = 0,
            KeyCode::End => *cursor = value.chars().count(),
            KeyCode::Backspace => {
                if *cursor > 0 {
                    let remove_idx = *cursor - 1;
                    if let Some((byte_idx, _)) = value.char_indices().nth(remove_idx) {
                        value.remove(byte_idx);
                        *cursor -= 1;
                    }
                }
            }
            KeyCode::Delete => {
                if let Some((byte_idx, _)) = value.char_indices().nth(*cursor) {
                    value.remove(byte_idx);
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let byte_idx = value
                    .char_indices()
                    .nth(*cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(value.len());
                value.insert(byte_idx, c);
                *cursor += 1;
            }
            _ => {}
        },
        Popup::Confirm { .. } => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.submit_popup(),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.close_popup(),
            _ => {}
        },
        Popup::None => {}
    }
}

fn handle_browser_key(app: &mut App, key: KeyEvent) {
    if app.pending_g {
        app.pending_g = false;
        if key.code == KeyCode::Char('g') {
            app.jump_top();
            return;
        }
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('d') => app.move_down(15),
            KeyCode::Char('u') => app.move_up(15),
            KeyCode::Char('a') => app.select_all(),
            KeyCode::Char('r') => app.invert_selection(),
            _ => {}
        }
        return;
    }

    match key.code {
        // Quit & navigation
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => app.move_down(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_up(1),
        KeyCode::PageDown => app.move_down(15),
        KeyCode::PageUp => app.move_up(15),
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => app.go_parent(),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => app.enter_selected(),
        KeyCode::Char('g') => app.pending_g = true,
        KeyCode::Home => app.jump_top(),
        KeyCode::Char('G') | KeyCode::End => app.jump_bottom(),

        // Filter & hidden
        KeyCode::Char('f') => app.toggle_filter(),
        KeyCode::Char('.') => app.toggle_hidden(),

        // Yazi-style selection & file/folder management
        KeyCode::Char(' ') => app.toggle_select_and_down(),
        KeyCode::Esc => app.clear_selection_and_clipboard(),
        KeyCode::Char('y') => app.yank_selected(ClipboardMode::Copy),
        KeyCode::Char('x') => app.yank_selected(ClipboardMode::Cut),
        KeyCode::Char('p') => app.paste(false),
        KeyCode::Char('P') => app.paste(true),
        KeyCode::Char('a') => app.open_create_popup(),
        KeyCode::Char('r') => app.open_rename_popup(),
        KeyCode::Char('d') => app.open_delete_popup(false),
        KeyCode::Char('D') => app.open_delete_popup(true),

        // Search in current directory
        KeyCode::Char('/') => app.open_search_popup(),
        KeyCode::Char('n') => app.search_next(true),
        KeyCode::Char('N') => app.search_next(false),

        _ => {}
    }
}

fn handle_viewer_key(app: &mut App, key: KeyEvent) {
    if app.pending_g {
        app.pending_g = false;
        if key.code == KeyCode::Char('g') {
            app.viewer_first();
            return;
        }
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.exit_viewer(),
        KeyCode::Char('j')
        | KeyCode::Char('l')
        | KeyCode::Down
        | KeyCode::Right
        | KeyCode::Char(' ')
        | KeyCode::PageDown => app.viewer_next(),
        KeyCode::Char('k')
        | KeyCode::Char('h')
        | KeyCode::Up
        | KeyCode::Left
        | KeyCode::Backspace
        | KeyCode::PageUp => app.viewer_prev(),
        KeyCode::Char('g') => app.pending_g = true,
        KeyCode::Home => app.viewer_first(),
        KeyCode::Char('G') | KeyCode::End => app.viewer_last(),
        KeyCode::Char('i') => app.toggle_viewer_bar(),
        _ => {}
    }
}

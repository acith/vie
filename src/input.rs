use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::{App, ClipboardMode, InputKind, Mode, Popup};
use crate::fs::SortBy;

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
    let mut trigger_live_filter = false;
    let mut trigger_recursive_update = false;

    match &mut app.popup {
        Popup::Input {
            kind,
            value,
            cursor,
            ..
        } => {
            let is_filter = *kind == InputKind::Filter;
            match key.code {
                KeyCode::Esc => {
                    app.close_popup();
                    return;
                }
                KeyCode::Enter => {
                    app.submit_popup();
                    return;
                }
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
                            trigger_live_filter = is_filter;
                        }
                    }
                }
                KeyCode::Delete => {
                    if let Some((byte_idx, _)) = value.char_indices().nth(*cursor) {
                        value.remove(byte_idx);
                        trigger_live_filter = is_filter;
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
                    trigger_live_filter = is_filter;
                }
                _ => {}
            }
        }
        Popup::Confirm { .. } => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.submit_popup(),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.close_popup(),
            _ => {}
        },
        Popup::RecursiveSearch { query, cursor, .. } => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                match key.code {
                    KeyCode::Char('n') | KeyCode::Char('j') => app.recursive_search_move(1),
                    KeyCode::Char('p') | KeyCode::Char('k') => app.recursive_search_move(-1),
                    _ => {}
                }
                return;
            }
            match key.code {
                KeyCode::Esc => {
                    app.close_popup();
                    return;
                }
                KeyCode::Enter => {
                    app.submit_popup();
                    return;
                }
                KeyCode::Down | KeyCode::Tab => {
                    app.recursive_search_move(1);
                    return;
                }
                KeyCode::Up | KeyCode::BackTab => {
                    app.recursive_search_move(-1);
                    return;
                }
                KeyCode::PageDown => {
                    app.recursive_search_move(10);
                    return;
                }
                KeyCode::PageUp => {
                    app.recursive_search_move(-10);
                    return;
                }
                KeyCode::Left => {
                    *cursor = cursor.saturating_sub(1);
                }
                KeyCode::Right => {
                    let max = query.chars().count();
                    if *cursor < max {
                        *cursor += 1;
                    }
                }
                KeyCode::Home => *cursor = 0,
                KeyCode::End => *cursor = query.chars().count(),
                KeyCode::Backspace => {
                    if *cursor > 0 {
                        let remove_idx = *cursor - 1;
                        if let Some((byte_idx, _)) = query.char_indices().nth(remove_idx) {
                            query.remove(byte_idx);
                            *cursor -= 1;
                            trigger_recursive_update = true;
                        }
                    }
                }
                KeyCode::Delete => {
                    if let Some((byte_idx, _)) = query.char_indices().nth(*cursor) {
                        query.remove(byte_idx);
                        trigger_recursive_update = true;
                    }
                }
                KeyCode::Char(c) => {
                    let byte_idx = query
                        .char_indices()
                        .nth(*cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(query.len());
                    query.insert(byte_idx, c);
                    *cursor += 1;
                    trigger_recursive_update = true;
                }
                _ => {}
            }
        }
        Popup::None => {}
    }

    if trigger_live_filter {
        app.apply_live_filter_from_popup();
    }
    if trigger_recursive_update {
        app.update_recursive_search();
    }
}

fn handle_browser_key(app: &mut App, key: KeyEvent) {
    if app.pending_comma {
        app.pending_comma = false;
        match key.code {
            KeyCode::Char('n') => app.set_sort(SortBy::Natural, false),
            KeyCode::Char('N') => app.set_sort(SortBy::Natural, true),
            KeyCode::Char('a') => app.set_sort(SortBy::Alphabetical, false),
            KeyCode::Char('A') => app.set_sort(SortBy::Alphabetical, true),
            KeyCode::Char('m') => app.set_sort(SortBy::Modified, true),
            KeyCode::Char('M') => app.set_sort(SortBy::Modified, false),
            KeyCode::Char('s') => app.set_sort(SortBy::Size, true),
            KeyCode::Char('S') => app.set_sort(SortBy::Size, false),
            KeyCode::Char('e') => app.set_sort(SortBy::Extension, false),
            KeyCode::Char('E') => app.set_sort(SortBy::Extension, true),
            KeyCode::Char('d') => app.toggle_dirs_first(),
            _ => {}
        }
        return;
    }

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

        // Yazi-style sort menu, type filter, live name filter, find & recursive search
        KeyCode::Char(',') => app.pending_comma = true,
        KeyCode::Char('t') => app.toggle_filter(),
        KeyCode::Char('f') => app.open_filter_popup(),
        KeyCode::Char('/') => app.open_find_popup(true),
        KeyCode::Char('?') => app.open_find_popup(false),
        KeyCode::Char('n') => app.search_next(true),
        KeyCode::Char('N') => app.search_next(false),
        KeyCode::Char('s') => app.open_recursive_search_popup(),
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

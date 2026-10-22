//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

/// Settings: one row each, Enter changes.
/// Settings in sections, the value beside each label, coloured when it is
/// a state worth noticing: off, on, failed.
pub(super) fn draw_settings(frame: &mut Frame, area: Rect, app: &App) {
    use crate::tui::app::SettingKey;
    let block = pane_focused(trs!("Settings", "Настройки"), app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let label_width = SettingKey::ALL.iter().map(|k| k.label().chars().count()).max().unwrap_or(0) + 4;
    let home = std::env::var("HOME").unwrap_or_default();
    let mut lines = Vec::new();
    let mut cursor_line = 0;
    for (index, key) in SettingKey::ALL.iter().enumerate() {
        if index == 0 || SettingKey::ALL[index - 1].section() != key.section() {
            if index > 0 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::from(Span::styled(key.section().to_string(), theme::heading())));
        }
        let selected = index == app.cursor();
        if selected {
            cursor_line = lines.len();
        }
        let mut value = app.setting_value(*key);
        // Paths as people type them: the home folder is «~».
        if !home.is_empty() && value.starts_with(&home) {
            value = format!("~{}", &value[home.len()..]);
        }
        let tone = match key {
            SettingKey::Cloud if app.cloud_error.is_some() => theme::error(),
            SettingKey::Cloud if value.contains("potok cloud login") => theme::label(),
            SettingKey::Cloud => theme::ok(),
            SettingKey::Device if app.read_only.is_some() => theme::attention(),
            SettingKey::ScreenLock if app.lock_hash().is_some() => theme::ok(),
            SettingKey::ScreenLock | SettingKey::LockAfter => theme::label(),
            SettingKey::Rates if app.rates.iter().any(|r| r.rate.is_none()) => theme::attention(),
            _ => theme::value(),
        };
        let pick = |base: Style| if selected { theme::on_cursor(base) } else { base };
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<w$}", key.label(), w = label_width - 2), pick(theme::label())),
            Span::styled(format!("{:<w$}", fit(&value, width.saturating_sub(label_width)), w = width.saturating_sub(label_width)), pick(tone)),
        ]));
    }
    let (from, to) = window(cursor_line, lines.len(), inner.height as usize);
    rows_of(frame, inner, lines[from..to].to_vec());
}

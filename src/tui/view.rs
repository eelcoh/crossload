use super::model::Model;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Wrap},
    Frame,
};
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
pub(super) fn draw(model: &Model, frame: &mut Frame<'_>) {
    let area = frame.area();
    if area.width < 20 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("Crossload: enlarge the terminal (q to quit)."),
            area,
        );
        return;
    }
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(3),
    ])
    .split(area);
    let source = if model.local {
        model.options.browse.display().to_string()
    } else {
        model
            .options
            .device
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    };
    frame.render_widget(
        Paragraph::new(clean(&format!(
            "CROSSLOAD | {} | {source}",
            if model.local { "Local" } else { "Kobo" }
        ))),
        areas[0],
    );
    let destination = model
        .options
        .send_to
        .clone()
        .or_else(|| {
            model
                .options
                .copy_to
                .as_ref()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| "local import only".into());
    frame.render_widget(
        Paragraph::new(clean(&format!(
            "Output: {} | Destination: {destination}",
            model.options.output.display()
        ))),
        areas[1],
    );
    frame.render_widget(
        Paragraph::new(clean(&format!(
            "{}Filter: {}",
            if model.search { "/ " } else { "" },
            model.query
        ))),
        areas[2],
    );
    let entries = model.filtered();
    let offset = model
        .selected
        .saturating_sub(areas[3].height.saturating_sub(1) as usize);
    let lines: Vec<Line<'_>> = entries
        .iter()
        .enumerate()
        .skip(offset)
        .take(areas[3].height as usize)
        .map(|(i, entry)| {
            let style = if i == model.selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::styled(
                clean(&format!(
                    "{} {:7} {}  {}",
                    if i == model.selected { ">" } else { " " },
                    entry.kind,
                    entry.title,
                    entry.author
                )),
                style,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), areas[3]);
    frame.render_widget(Paragraph::new(format!("{} entries | Enter: open/import/transfer | Tab: Local/Kobo\n↑↓/j k: select | /: search | Backspace: parent | o: output | r: refresh | q: quit", entries.len())), areas[4]);
    let prefix = if model.pending_quit {
        "Finishing before quit"
    } else if model.busy.is_some() || model.loading.is_some() {
        ["Working .", "Working ..", "Working ..."][model.tick % 3]
    } else {
        "Status"
    };
    frame.render_widget(
        Paragraph::new(clean(&format!("{prefix}: {}", model.status))).wrap(Wrap { trim: false }),
        areas[5],
    );
}

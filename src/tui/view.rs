use super::model::Model;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    widgets::{Cell, Paragraph, Row, Table, Wrap},
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
        Constraint::Length(if area.height >= 16 { 3 } else { 0 }),
        Constraint::Length(2),
        Constraint::Length(if area.height >= 16 { 3 } else { 1 }),
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
        .saturating_sub(areas[3].height.saturating_sub(2) as usize);
    let rows = entries
        .iter()
        .enumerate()
        .skip(offset)
        .take(areas[3].height.saturating_sub(1) as usize)
        .map(|(i, entry)| {
            let presence = if entry.preview
                || matches!(entry.source, super::Source::Directory(_))
                || entry.kind == "ACSM"
                || (model.options.send_to.is_none() && model.options.copy_to.is_none())
            {
                "—"
            } else if model.checking.is_some() {
                "Checking"
            } else {
                model
                    .presence
                    .iter()
                    .find(|(s, _)| s == &entry.source)
                    .map(|(_, status)| status.as_str())
                    .unwrap_or("Unknown")
            };
            let mut cells = vec![Cell::from(clean(&entry.title)), Cell::from(presence)];
            if area.width >= 70 {
                cells.push(Cell::from(clean(&entry.author)));
                cells.push(Cell::from(entry.kind));
            }
            Row::new(cells).style(if i == model.selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            })
        });
    let (headers, widths) = if area.width >= 70 {
        (
            vec!["TITLE", "READER", "AUTHOR", "TYPE"],
            vec![
                Constraint::Percentage(50),
                Constraint::Length(8),
                Constraint::Percentage(30),
                Constraint::Length(7),
            ],
        )
    } else {
        (
            vec!["TITLE", "READER"],
            vec![Constraint::Min(8), Constraint::Length(8)],
        )
    };
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(if model.loading.is_some() {
                "Loading books…"
            } else if model.query.is_empty() {
                "No books in this location. Tab switches source; o opens output."
            } else {
                "No matching books. Press / to edit the search."
            }),
            areas[3],
        );
    } else {
        frame.render_widget(
            Table::new(rows, widths)
                .column_spacing(2)
                .header(Row::new(headers).style(Style::default().add_modifier(Modifier::BOLD))),
            areas[3],
        );
    }
    if let Some(entry) = entries.get(model.selected) {
        let source = match &entry.source {
            super::Source::Kobo(id) => id.clone(),
            super::Source::Local(p) | super::Source::Directory(p) => p.display().to_string(),
        };
        frame.render_widget(
            Paragraph::new(clean(&format!(
                "{} — {}\n{}: {source}",
                entry.title, entry.author, entry.kind
            )))
            .wrap(Wrap { trim: false }),
            areas[4],
        );
    }
    let action = if entries
        .get(model.selected)
        .is_some_and(|e| matches!(e.source, super::Source::Directory(_)))
    {
        "open folder"
    } else if model.options.send_to.is_some() || model.options.copy_to.is_some() {
        "import/transfer"
    } else {
        "import"
    };
    frame.render_widget(Paragraph::new(format!("{}/{} | Enter: {action} | /: search | Tab: source | r: refresh\n↑↓/j k: select | PgUp/PgDn: 10 rows | Home/End | o: output | Backspace: parent | q: quit", if entries.is_empty() { 0 } else { model.selected + 1 }, entries.len())), areas[5]);
    let prefix = if model.pending_quit {
        "Finishing before quit"
    } else if model.busy.is_some() || model.loading.is_some() || model.checking.is_some() {
        ["Working .", "Working ..", "Working ..."][model.tick % 3]
    } else {
        "Status"
    };
    frame.render_widget(
        Paragraph::new(clean(&format!("{prefix}: {}", model.status))).wrap(Wrap { trim: false }),
        areas[6],
    );
}

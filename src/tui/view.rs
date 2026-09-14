use super::{model::Model, Source};
use crate::books::Place;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    widgets::{Cell, Paragraph, Row, Table, Wrap},
    Frame,
};
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
pub(super) fn draw(model: &Model, frame: &mut Frame<'_>) {
    let area = frame.area();
    if area.width < 25 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("Crossload: enlarge terminal (q quits)."),
            area,
        );
        return;
    }
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(if area.height >= 18 { 4 } else { 0 }),
        Constraint::Length(1),
        Constraint::Length(3),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new("CROSSLOAD | Books").style(Style::default().add_modifier(Modifier::BOLD)),
        areas[0],
    );
    let statuses = [Place::Local, Place::Kobo, Place::Xteink]
        .iter()
        .map(|place| {
            format!(
                "{}: {}",
                place.label(),
                model
                    .catalog
                    .status
                    .iter()
                    .find(|(p, _)| p == place)
                    .map(|(_, s)| clean(s))
                    .unwrap_or_else(|| "Checking".into())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(Paragraph::new(statuses), areas[1]);
    frame.render_widget(
        Paragraph::new(format!(
            "{}Search: {}",
            if model.search { "/ " } else { "" },
            clean(&model.query)
        )),
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
        .map(|(i, e)| {
            let locations = match &e.source {
                Source::Book(book, _) => [Place::Local, Place::Kobo, Place::Xteink]
                    .iter()
                    .filter(|p| book.has(**p))
                    .map(|p| p.label())
                    .collect::<Vec<_>>()
                    .join(" / "),
                Source::Local(_) => "Local ACSM".into(),
            };
            let mut cells = vec![Cell::from(clean(&e.title)), Cell::from(locations)];
            if area.width >= 80 {
                cells.push(Cell::from(clean(&e.author)));
            }
            Row::new(cells).style(if i == model.selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            })
        });
    let (headers, widths) = if area.width >= 80 {
        (
            vec!["TITLE", "LOCATIONS", "AUTHOR"],
            vec![
                Constraint::Percentage(45),
                Constraint::Length(23),
                Constraint::Min(10),
            ],
        )
    } else {
        (
            vec!["TITLE", "LOCATIONS"],
            vec![Constraint::Min(8), Constraint::Length(23)],
        )
    };
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(if model.loading.is_some() {
                "Discovering books; available locations appear as they finish…"
            } else {
                "No books found. Check location status above or edit the search."
            })
            .wrap(Wrap { trim: false }),
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
    if let Some(entry) = model
        .action
        .as_ref()
        .or_else(|| entries.get(model.selected).copied())
    {
        let details = match &entry.source {
            Source::Book(book, _) => book
                .preferred()
                .map(|c| {
                    format!(
                        "Preferred source: {}{}\n{}",
                        c.place.label(),
                        if c.optimized || c.place == Place::Xteink {
                            " (device copy; reduced quality possible)"
                        } else {
                            " (original)"
                        },
                        clean(&c.path)
                    )
                })
                .unwrap_or_default(),
            Source::Local(p) => format!("ACSM: {}", clean(&p.display().to_string())),
        };
        frame.render_widget(
            Paragraph::new(format!(
                "{} — {}\n{details}",
                clean(&entry.title),
                clean(&entry.author)
            ))
            .wrap(Wrap { trim: false }),
            areas[4],
        );
    }
    frame.render_widget(Paragraph::new(format!("{}/{} | Enter: copy actions | /: search | r: refresh | ↑↓ PgUp/PgDn Home/End | q: quit",if entries.is_empty(){0}else{model.selected+1},entries.len())),areas[5]);
    let status = if model.action.is_some() {
        format!(
            "Copy to: 1 Local | 2 Kobo | 3 Xteink | Esc cancel\n{}",
            clean(&model.status)
        )
    } else {
        format!(
            "{}: {}",
            if model.busy.is_some() || model.loading.is_some() {
                ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][model.tick % 10]
            } else {
                "Status"
            },
            clean(&model.status)
        )
    };
    frame.render_widget(Paragraph::new(status).wrap(Wrap { trim: false }), areas[6]);
}

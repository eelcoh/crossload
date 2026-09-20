use super::{
    model::{Destination, Model, Removal},
    Entry, Source,
};
use crate::books::{Book, Place};
use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Cell, Clear, Padding, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, Wrap,
    },
    Frame,
};
const PLACES: [Place; 3] = [Place::Local, Place::Kobo, Place::CrossPoint];
/// Original copy, device/optimized copy, absent, unreadable.
const PRESENT: &str = "●";
const DEVICE: &str = "◐";
const ABSENT: &str = "·";
const BROKEN: &str = "✗";
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
/// Colour carries no information on its own: every state also has a glyph or
/// word, so NO_COLOR and monochrome terminals lose nothing.
fn colored() -> bool {
    static ALLOW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALLOW.get_or_init(|| std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()))
}
fn accent() -> Style {
    fg(Color::Cyan).add_modifier(Modifier::BOLD)
}
fn fg(color: Color) -> Style {
    if colored() {
        Style::default().fg(color)
    } else {
        Style::default()
    }
}
/// Terminal themes map grey (and DIM) unpredictably, often to near-invisible
/// against their own background, so secondary text keeps the default
/// foreground. Hierarchy comes from weight, position and the state colours.
fn plain() -> Style {
    Style::default()
}
fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
/// Truncate to a printed width, not a character count: one CJK glyph occupies
/// two cells, and a table column is measured in cells.
pub(super) fn shorten(text: &str, limit: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let text = clean(text);
    if text.width() <= limit {
        return text;
    }
    let mut result = String::new();
    let mut width = 0;
    for c in text.chars() {
        let next = width + c.width().unwrap_or(0);
        if next > limit.saturating_sub(1) {
            break;
        }
        width = next;
        result.push(c);
    }
    result.push('…');
    result
}
fn width(text: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    text.width()
}
/// Keep the end of a path: a file is identified by its name, not by the
/// directories above it.
fn tail(path: &str, limit: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let path = clean(path);
    if path.width() <= limit {
        return path;
    }
    let mut kept = String::new();
    for c in path.chars().rev() {
        if kept.width() + c.len_utf8() > limit.saturating_sub(1) {
            break;
        }
        kept.push(c);
    }
    format!("…{}", kept.chars().rev().collect::<String>())
}
/// A location pill: state glyph, name and the shortest useful detail.
fn pill(model: &Model, place: Place) -> Vec<Span<'static>> {
    let status = model
        .catalog
        .status
        .iter()
        .find(|(p, _)| *p == place)
        .map(|(_, s)| s.as_str())
        .unwrap_or("Checking");
    let name = place.label();
    if status.starts_with("Ready") {
        let count = model.catalog.books.iter().filter(|b| b.has(place)).count();
        let unreadable = model
            .catalog
            .books
            .iter()
            .filter(|b| {
                b.copies
                    .iter()
                    .any(|c| c.place == place && c.sha.is_empty())
            })
            .count();
        let mut spans = vec![
            Span::styled(format!("{PRESENT} "), fg(Color::Green)),
            Span::styled(name, bold()),
            Span::styled(format!(" {count}"), plain()),
        ];
        if unreadable > 0 {
            spans.push(Span::styled(format!(" ⚠{unreadable}"), fg(Color::Yellow)));
        }
        return spans;
    }
    if status.starts_with("Checking") {
        // Setup holds discovery back, so nothing is being checked yet: a
        // spinner that cannot finish is worse than saying so.
        if model.options.first_run {
            return vec![
                Span::styled(format!("{ABSENT} "), plain()),
                Span::styled(name, plain()),
                Span::styled(" not scanned", plain()),
            ];
        }
        return vec![
            Span::styled("◌ ", fg(Color::Cyan)),
            Span::styled(name, bold()),
            Span::styled(" checking", plain()),
        ];
    }
    let reason = status.strip_prefix("Unavailable: ").unwrap_or(status);
    if reason == "Not configured" {
        return vec![
            Span::styled(format!("{ABSENT} "), plain()),
            Span::styled(name, plain()),
            Span::styled(" not configured", plain()),
        ];
    }
    vec![
        Span::styled("⚠ ", fg(Color::Yellow)),
        Span::styled(name, bold()),
        Span::styled(format!(" {}", shorten(reason, 28)), fg(Color::Yellow)),
    ]
}
/// Title and location pills on one row where they fit, wrapped when they do not.
fn header(model: &Model, width: u16) -> Vec<Line<'static>> {
    let title = vec![
        Span::styled("Crossload", bold()),
        Span::styled(" · Books", plain()),
    ];
    let pills: Vec<Vec<Span<'static>>> = PLACES.iter().map(|p| pill(model, *p)).collect();
    let gap = "   ";
    let joined: Vec<Span<'static>> = pills
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            let mut spans = if i == 0 { vec![] } else { vec![Span::raw(gap)] };
            spans.extend(p.iter().cloned());
            spans
        })
        .collect();
    let used = Line::from(title.clone()).width() + Line::from(joined.clone()).width();
    if used + gap.len() <= width as usize {
        let mut spans = title;
        spans.push(Span::raw(" ".repeat(width as usize - used)));
        spans.extend(joined);
        return vec![Line::from(spans)];
    }
    let mut lines = vec![Line::from(title)];
    let mut current: Vec<Span<'static>> = vec![];
    for spans in pills {
        let mut candidate = current.clone();
        if !candidate.is_empty() {
            candidate.push(Span::raw(gap));
        }
        candidate.extend(spans.iter().cloned());
        if !current.is_empty() && Line::from(candidate.clone()).width() > width as usize {
            lines.push(Line::from(std::mem::take(&mut current)));
            current = spans;
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(Line::from(current));
    }
    lines
}
/// The list offset that keeps `selected` visible, moving as little as possible
/// and holding a two-row margin away from the edges where the viewport allows.
pub(super) fn offset(current: usize, selected: usize, len: usize, visible: usize) -> usize {
    if visible == 0 || len <= visible {
        return 0;
    }
    let max = len - visible;
    let margin = ((visible - 1) / 2).min(2);
    let mut offset = current.min(max);
    if selected < offset + margin {
        offset = selected.saturating_sub(margin);
    }
    if selected + margin >= offset + visible {
        offset = (selected + margin + 1).saturating_sub(visible);
    }
    offset.min(max)
}
/// One cell per location, so presence reads as a fixed column, not a sentence.
fn presence(book: &Book, selected: bool) -> Line<'static> {
    let mut spans = vec![];
    for (i, place) in PLACES.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let paint = |style: Style| if selected { plain() } else { style };
        spans.push(match book.copies.iter().find(|c| c.place == *place) {
            Some(copy) if copy.sha.is_empty() => Span::styled(BROKEN, paint(fg(Color::Red))),
            Some(copy) if copy.optimized => Span::styled(DEVICE, paint(fg(Color::Yellow))),
            Some(_) => Span::styled(PRESENT, paint(fg(Color::Green))),
            None => Span::raw(ABSENT),
        });
    }
    Line::from(spans)
}
fn unreadable(book: &Book) -> bool {
    !book.copies.is_empty() && book.copies.iter().all(|c| c.sha.is_empty())
}
fn details(
    model: &Model,
    entries: &[&Entry],
    width: usize,
) -> Option<(String, Vec<Line<'static>>)> {
    let entry = model
        .action
        .first()
        .or_else(|| entries.get(model.selected).copied())?;
    let label = |text: &str| Span::styled(format!("{text:<8}"), bold());
    let lines = match &entry.source {
        Source::Book(book) => {
            let copy = book.preferred();
            vec![
                Line::from(vec![
                    label("Source"),
                    match copy {
                        Some(c) if c.optimized => Span::styled(
                            format!(
                                "{} · {} · device copy, images may be optimized",
                                c.place.label(),
                                c.format.label()
                            ),
                            fg(Color::Yellow),
                        ),
                        Some(c) => Span::styled(
                            format!("{} · {} · original", c.place.label(), c.format.label()),
                            fg(Color::Green),
                        ),
                        None => Span::styled("No usable copy", fg(Color::Red)),
                    },
                    Span::styled(
                        copy.map(|c| format!(" · {}", size(c.size)))
                            .unwrap_or_default(),
                        plain(),
                    ),
                ]),
                Line::from(vec![
                    label("Path"),
                    Span::raw(
                        copy.map(|c| tail(&c.path, width.saturating_sub(10)))
                            .unwrap_or_default(),
                    ),
                ]),
                Line::from(match &book.series {
                    Some(series) => vec![
                        label("Series"),
                        Span::raw(match book.series_index {
                            // A whole number reads as one: book 2, not book 2.0.
                            Some(index) if index.fract() == 0.0 => {
                                format!("{series}, book {}", index as i64)
                            }
                            Some(index) => format!("{series}, book {index}"),
                            None => series.clone(),
                        }),
                    ],
                    None => vec![],
                }),
            ]
            .into_iter()
            // A book in no series says nothing, rather than leaving a gap.
            .filter(|line: &Line| !line.spans.is_empty())
            .collect()
        }
        Source::Local(path) => vec![
            Line::from(vec![
                label("Source"),
                Span::styled("ACSM import request", fg(Color::Yellow)),
                Span::styled(" · choose Local to fulfil it", plain()),
            ]),
            Line::from(vec![
                label("Path"),
                Span::raw(tail(&path.display().to_string(), width.saturating_sub(10))),
            ]),
        ],
    };
    let title = if model.action.len() > 1 {
        format!(" {} selected ", crate::books::books(model.action.len()))
    } else if entry.author.is_empty() {
        format!(" {} ", clean(&entry.title))
    } else {
        format!(" {} · {} ", clean(&entry.title), clean(&entry.author))
    };
    Some((title, lines))
}
fn frame_block(title: String) -> Block<'static> {
    Block::bordered()
        .padding(Padding::horizontal(1))
        .title(Line::from(Span::styled(title, bold())))
}
pub(super) fn draw(model: &Model, frame: &mut Frame<'_>) {
    let area = frame.area();
    if area.width < 25 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("Crossload: enlarge terminal (q quits).").wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    model.width.set(area.width);
    let header = header(model, area.width);
    let entries = model.filtered();
    let detail = (area.height >= 18)
        .then(|| details(model, &entries, area.width as usize))
        .flatten();
    let areas = Layout::vertical([
        Constraint::Length(header.len() as u16),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(detail.as_ref().map_or(0, |(_, l)| l.len() as u16 + 2)),
        Constraint::Length(1),
        Constraint::Length(if area.height >= 24 { 3 } else { 2 }),
    ])
    .split(area);
    frame.render_widget(Paragraph::new(header), areas[0]);
    frame.render_widget(
        Paragraph::new(if model.search {
            Line::from(vec![
                Span::styled("/", accent()),
                Span::raw(clean(&model.query)),
                Span::styled("▏", accent()),
            ])
        } else {
            let mut spans = vec![Span::styled(model.filter.label(), bold())];
            if !model.query.is_empty() {
                spans.push(Span::raw(format!(" · matching {}", clean(&model.query))));
            }
            spans.push(Span::raw(format!(
                "  {} of {}",
                entries.len(),
                model.entries.len()
            )));
            spans.push(Span::raw(model.sort.label()));
            if !model.marked.is_empty() {
                spans.push(Span::styled(
                    format!("  ✓ {} marked", model.marked.len()),
                    fg(Color::Green),
                ));
            }
            Line::from(spans)
        }),
        areas[1],
    );
    draw_list(model, frame, areas[2], &entries);
    if !model.action.is_empty() {
        draw_action(model, frame, areas[2]);
    }
    if let Some(entry) = &model.copies {
        draw_copies(model, frame, areas[2], entry);
    }
    if let Some((title, lines)) = detail.filter(|_| areas[3].height > 0) {
        let block = frame_block(title);
        let inner = block.inner(areas[3]);
        frame.render_widget(block, areas[3]);
        frame.render_widget(Paragraph::new(lines), inner);
    }
    let keys: &[(&str, &str)] = if model.history.is_some() {
        &[("esc", "close")]
    } else if model.ask.is_some() {
        &[("y", "convert and copy"), ("esc", "cancel")]
    } else if model.confirm.is_some() {
        &[("y", "delete"), ("esc", "keep")]
    } else if model.copies.is_some() {
        &[("1-9", "delete a copy"), ("esc", "close")]
    } else if !model.action.is_empty() {
        &[("1 2 3", "destination"), ("esc", "cancel")]
    } else if model.busy.is_some() {
        &[("esc", "stop after this book"), ("q", "quit when finished")]
    } else {
        &[
            ("?", "help"),
            ("enter", "copy"),
            ("space", "mark"),
            ("f", "filter"),
            ("s", "sort"),
            (",", "settings"),
            ("d", "copies"),
            ("/", "search"),
            ("r", "refresh"),
            ("q", "quit"),
        ]
    };
    let mut hints: Vec<Span<'static>> = vec![];
    for (key, action) in keys {
        let group = vec![
            Span::raw(if hints.is_empty() { "" } else { "   " }),
            Span::styled(*key, accent()),
            Span::raw(" "),
            Span::styled(*action, plain()),
        ];
        let mut candidate = hints.clone();
        candidate.extend(group);
        // A hint that would run off the edge is dropped, not truncated.
        if Line::from(candidate.clone()).width() > areas[4].width as usize {
            continue;
        }
        hints = candidate;
    }
    frame.render_widget(Paragraph::new(Line::from(hints)), areas[4]);
    frame.render_widget(
        Paragraph::new(status_lines(model)).wrap(Wrap { trim: false }),
        areas[5],
    );
    if let Some(entries) = &model.history {
        let area = overlay(frame, " Copied lately · Esc close ", 88, 20);
        let lines: Vec<Line<'static>> = if entries.is_empty() {
            vec![Line::raw(
                "Nothing copied yet, or the record has not been kept.",
            )]
        } else {
            entries
                .iter()
                .rev()
                .take(area.height as usize)
                .map(|entry| {
                    Line::from(vec![
                        Span::styled(format!("{}  ", crate::history::stamp(entry.at)), plain()),
                        Span::styled(format!("{:<34}", shorten(&entry.title, 33)), bold()),
                        Span::styled(format!("{:<11}", entry.to.label()), plain()),
                        if entry.ok {
                            Span::styled("copied", fg(Color::Green))
                        } else {
                            Span::styled(
                                shorten(&format!("failed: {}", entry.detail), 24),
                                fg(Color::Red),
                            )
                        },
                    ])
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), area);
    }
    if model.help {
        let area = overlay(frame, " Help · ↑/↓ scroll · ?/Esc close ", 86, 23);
        frame.render_widget(
            Paragraph::new(super::model::HELP[model.help_scroll as usize..].join("\n"))
                .wrap(Wrap { trim: false }),
            area,
        );
    }
    if let Some(selected) = model.filter_menu {
        let area = overlay(frame, " Filter · ↑/↓ Enter · Esc cancel ", 52, 9);
        let lines: Vec<_> = super::model::Filter::CYCLE
            .iter()
            .enumerate()
            .map(|(i, filter)| {
                Line::styled(
                    format!(
                        "{}  {}{}",
                        i + 1,
                        filter.label(),
                        if *filter == model.filter { " ✓" } else { "" }
                    ),
                    if selected == i {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        plain()
                    },
                )
            })
            .collect();
        let scroll = selected.saturating_sub(area.height.saturating_sub(1) as usize) as u16;
        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
    }
    if let Some(panel) = &model.settings {
        draw_settings(panel, model.configuring.is_some(), frame);
    }
}
fn overlay(frame: &mut Frame<'_>, title: &str, width: u16, height: u16) -> Rect {
    let bounds = frame.area();
    let width = width.min(bounds.width);
    let height = height.min(bounds.height);
    let area = Rect::new(
        bounds.x + (bounds.width - width) / 2,
        bounds.y + (bounds.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let block = frame_block(title.to_owned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}
/// How far a value sits inside the label above it.
const VALUE: usize = 4;
fn draw_settings(panel: &super::settings::Panel, working: bool, frame: &mut Frame<'_>) {
    let area = overlay(
        frame,
        if panel.first_run {
            " Welcome to Crossload · Setup "
        } else {
            " Settings "
        },
        88,
        if panel.first_run { 25 } else { 22 },
    );
    let parts = Layout::vertical([
        Constraint::Length(if panel.first_run { 3 } else { 0 }),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(2),
    ])
    .split(area);
    if panel.first_run {
        // Nobody arrives here knowing what an import folder is; say what the
        // program does before asking where its things are.
        frame.render_widget(
            Paragraph::new(
                "Crossload copies books between this computer, a Kobo and your reader. \
                 Tell it where they live. Nothing is saved until you choose Save and rescan, \
                 and you can change all of it later with , in the library.",
            )
            .wrap(Wrap { trim: false }),
            parts[0],
        );
    }
    let list = parts[1];
    let selected = panel.device_selected.unwrap_or(panel.selected);
    let lines: Vec<Line<'static>> = if panel.device_selected.is_some() {
        panel
            .devices
            .iter()
            .enumerate()
            .map(|(i, path)| {
                Line::styled(
                    shorten(&path.display().to_string(), list.width as usize),
                    if selected == i {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        plain()
                    },
                )
            })
            .collect()
    } else {
        super::settings::LABELS
            .iter()
            .enumerate()
            .flat_map(|(i, label)| {
                let selected = i == panel.selected;
                // A row is a heading and the thing it names, so the name keeps
                // the weight and the value keeps the indent: which is which
                // stays visible when the selection is somewhere else.
                let mut lines = vec![Line::styled(
                    format!("{} {label}", if selected { "›" } else { " " }),
                    if selected { accent() } else { bold() },
                )];
                if let Some(value) = panel.values.get(i) {
                    let editing = selected && panel.edit.is_some();
                    // A value being typed owns the whole row: its mark is about
                    // to change anyway, and the cursor must stay in view.
                    let (glyph, word) = if editing {
                        ("", "")
                    } else {
                        panel.marks[i].parts()
                    };
                    let reserved = if word.is_empty() {
                        0
                    } else {
                        width(word) + width(glyph) + 3
                    };
                    let room = (list.width as usize).saturating_sub(VALUE + reserved);
                    let text = if value.is_empty() {
                        "(not configured)".to_owned()
                    } else if let (true, Some((_, cursor))) = (editing, &panel.edit) {
                        // Keep the cursor visible even when a path is wider than the dialog.
                        let prefix = tail(&value[..*cursor], room.saturating_sub(1));
                        format!("{prefix}▏{}", &value[*cursor..])
                    } else if i == 3 || i == 5 {
                        shorten(value, room)
                    } else {
                        // Paths differ at their end, so that is the end to keep.
                        tail(value, room)
                    };
                    let mut spans = vec![Span::styled(
                        format!("{:VALUE$}{text}", ""),
                        if selected {
                            Style::default().add_modifier(Modifier::REVERSED)
                        } else {
                            plain()
                        },
                    )];
                    if !word.is_empty() {
                        spans.push(Span::styled(
                            format!("  {glyph} {word}"),
                            match panel.marks[i] {
                                super::settings::Mark::Good(_) => fg(Color::Green),
                                super::settings::Mark::Warn(_) => fg(Color::Yellow),
                                super::settings::Mark::Bad(_) => fg(Color::Red),
                                _ => plain(),
                            },
                        ));
                    }
                    lines.push(Line::from(spans));
                }
                lines
            })
            .collect()
    };
    let selected_line = if panel.device_selected.is_some() {
        selected
    } else {
        selected + selected.min(6) + usize::from(selected < 6)
    };
    let scroll = selected_line.saturating_sub(list.height.saturating_sub(1) as usize);
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), list);
    frame.render_widget(
        Paragraph::new(clean(&panel.hint())).wrap(Wrap { trim: false }),
        parts[2],
    );
    let keys = if working {
        "Working… Ctrl+C quits after completion"
    } else if panel.device_selected.is_some() {
        "↑/↓ choose · Enter select · Esc cancel"
    } else if panel.edit.is_some() {
        "←/→ Home/End move · Ctrl+U clear · Enter accept · Ctrl+S save · Esc undo"
    } else if panel.first_run {
        "↑/↓ Tab select · Enter edit/run · Ctrl+S save · Esc skip setup for now"
    } else {
        "↑/↓ Tab select · Enter edit/run · Ctrl+S save · Esc discard and close"
    };
    frame.render_widget(Paragraph::new(keys).wrap(Wrap { trim: false }), parts[3]);
}
fn draw_list(model: &Model, frame: &mut Frame<'_>, area: Rect, entries: &[&Entry]) {
    let total = model.entries.len();
    let block = frame_block(if entries.len() == total {
        format!(" Library · {total} ")
    } else {
        format!(" Library · {} of {total} ", entries.len())
    });
    let block = block.title_bottom(
        Line::from(Span::styled(
            format!(
                " {}/{} ",
                if entries.is_empty() {
                    0
                } else {
                    model.selected + 1
                },
                entries.len()
            ),
            plain(),
        ))
        .right_aligned(),
    );
    let block = if area.width >= 72 {
        let mut legend = vec![
            Span::styled(PRESENT, fg(Color::Green)),
            Span::raw(" original  "),
            Span::styled(DEVICE, fg(Color::Yellow)),
            Span::raw(" device copy  "),
            Span::raw(format!("{ABSENT} none ")),
        ];
        if area.width >= 100 {
            legend.insert(4, Span::raw(" unreadable  "));
            legend.insert(4, Span::styled(BROKEN, fg(Color::Red)));
        }
        block.title(Line::from(legend).right_aligned())
    } else {
        block
    };
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if entries.is_empty() {
        frame.render_widget(
            Paragraph::new(if model.loading.is_some() {
                "Discovering books; available locations appear as they finish…"
            } else if model.options.first_run {
                "Setup is open. Choose your folders, or press Esc to browse without saving."
            } else if model.skipped {
                "No books found. Press , to tell Crossload where your books live."
            } else {
                "No books found. Check location status above or edit the search."
            })
            .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    // Columns are earned by width: presence and title always, then the author,
    // then the size of the copy a transfer would read from.
    let mut headers = vec!["", "L K C", "TITLE"];
    let mut widths = vec![
        Constraint::Length(1),
        Constraint::Length(5),
        Constraint::Percentage(45),
    ];
    let author = inner.width >= 56;
    let size_column = inner.width >= 92;
    if author {
        headers.push("AUTHOR");
        widths.push(Constraint::Min(16));
    }
    if size_column {
        headers.push("SIZE");
        widths.push(Constraint::Length(9));
    }
    if !author {
        widths[2] = Constraint::Min(8);
    }
    let columns = Layout::horizontal(widths.clone()).spacing(1).split(inner);
    let visible = inner.height.saturating_sub(1) as usize;
    let start = offset(model.scroll.get(), model.selected, entries.len(), visible);
    model.scroll.set(start);
    let rows = entries
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(i, e)| {
            let selected = i == model.selected;
            // The selected row inverts as a whole, so its own colours would
            // read as patches: on that row the glyph shapes carry the state.
            let paint = |style: Style| if selected { plain() } else { style };
            let (marker, title, detail) = match &e.source {
                Source::Book(book) => (
                    presence(book, selected),
                    Span::styled(
                        shorten(&e.title, columns[2].width as usize),
                        paint(if unreadable(book) {
                            fg(Color::Red)
                        } else {
                            Style::default()
                        }),
                    ),
                    book.preferred().map(|c| size(c.size)).unwrap_or_default(),
                ),
                Source::Local(_) => (
                    Line::from(Span::styled("  ⇩  ", paint(fg(Color::Yellow)))),
                    Span::styled(
                        shorten(&e.title, columns[2].width as usize),
                        paint(fg(Color::Yellow)),
                    ),
                    e.kind.to_string(),
                ),
            };
            let mut cells = vec![
                Cell::from(Line::from(if model.is_marked(e) {
                    Span::styled("✓", paint(fg(Color::Green)))
                } else {
                    Span::raw(" ")
                })),
                Cell::from(marker),
                Cell::from(Line::from(title)),
            ];
            if author {
                cells.push(Cell::from(Line::from(Span::styled(
                    shorten(&e.author, columns[3].width as usize),
                    plain(),
                ))));
            }
            if size_column {
                cells.push(Cell::from(
                    Line::from(Span::styled(detail, plain())).right_aligned(),
                ));
            }
            Row::new(cells).style(if selected {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default()
            })
        });
    frame.render_widget(
        Table::new(rows, widths)
            .column_spacing(1)
            .header(Row::new(headers).style(bold())),
        inner,
    );
    if entries.len() > visible {
        let mut state = ScrollbarState::new(entries.len())
            .viewport_content_length(visible)
            .position(start);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None),
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut state,
        );
    }
}
/// Copy destinations as a dialog over the list: availability is decided before
/// the keypress, by the same rules the model enforces afterwards.
/// Break text into lines that fit, on word boundaries where it can.
fn wrapped(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(8);
    let mut lines = vec![];
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if width(&candidate) > limit && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line = word.to_owned();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}
/// The most a set of books could take at its destination: what its sources
/// weigh. Optimizing and converting only ever make a book smaller.
fn wanted(entries: &[Entry]) -> u64 {
    entries
        .iter()
        .filter_map(|entry| match &entry.source {
            Source::Book(book) => book.preferred().map(|copy| copy.size),
            Source::Local(_) => None,
        })
        .sum()
}
fn draw_action(model: &Model, frame: &mut Frame<'_>, area: Rect) {
    if let Some((place, reason)) = &model.ask {
        let width = area.width.min(52);
        let mut lines: Vec<Line<'static>> =
            wrapped(&clean(reason), width.saturating_sub(2) as usize)
                .into_iter()
                .map(|line| Line::styled(line, fg(Color::Yellow)))
                .collect();
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("y", accent()),
            Span::styled(format!(" convert and copy to {}", place.label()), plain()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("esc", accent()),
            Span::styled(" cancel", plain()),
        ]));
        let height = (lines.len() as u16 + 2).min(area.height);
        let popup = Rect {
            x: area.x + area.width.saturating_sub(width) / 2,
            y: area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        };
        let block = frame_block(" Convert? ".to_owned()).border_style(fg(Color::Yellow));
        let inner = block.inner(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(block, popup);
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }
    let entries = &model.action;
    let single = entries.len() == 1;
    let mut lines = vec![Line::from(match entries.first().map(|e| &e.source) {
        Some(Source::Book(book)) if single => match book.preferred() {
            Some(copy) => Span::styled(
                format!("from {} · {}", copy.place.label(), size(copy.size)),
                plain(),
            ),
            None => Span::styled("no usable copy", fg(Color::Red)),
        },
        Some(Source::Local(_)) if single => Span::styled("from a local ACSM request", plain()),
        _ => Span::styled(
            format!(
                "{} marked · {}",
                crate::books::books(entries.len()),
                size(wanted(entries))
            ),
            plain(),
        ),
    })];
    lines.push(Line::default());
    for (key, place) in [
        ("1", Place::Local),
        ("2", Place::Kobo),
        ("3", Place::CrossPoint),
    ] {
        // For a set, the dialog counts what would actually be copied and why
        // the rest would not; the first reason stands for the remainder.
        let mut ready = 0;
        let mut asking = false;
        let mut hint = String::new();
        let mut blocked = String::new();
        for entry in entries {
            match model.destination(entry, place) {
                Destination::Ready(reason) => {
                    ready += 1;
                    if hint.is_empty() {
                        hint = reason.to_owned();
                    }
                }
                // Offered, but it will ask before it does anything.
                Destination::Ask(reason, _) => {
                    ready += 1;
                    asking = true;
                    if hint.is_empty() {
                        hint = reason.to_owned();
                    }
                }
                Destination::Blocked(reason, _) if blocked.is_empty() => {
                    blocked = reason.to_owned();
                }
                Destination::Blocked(_, _) => {}
            }
        }
        let text = if ready == 0 {
            blocked
        } else if single {
            hint
        } else if ready == entries.len() {
            format!("copy all {ready}")
        } else {
            format!("copy {ready} of {}, rest {blocked}", entries.len())
        };
        // What the destination has left, and whether the set would fit in it.
        // A conversion shrinks a PDF a long way, so the source size is the most
        // that could be needed rather than what will be.
        let room = model
            .room
            .iter()
            .find(|(p, _)| *p == place)
            .map(|(_, room)| *room);
        let space = match (ready, room) {
            (0, _) => Span::raw(String::new()),
            (_, Some(Some(free))) if free < wanted(entries) => {
                Span::styled(format!("  ✗ {} free", size(free)), fg(Color::Red))
            }
            (_, Some(Some(free))) => Span::styled(format!("  {} free", size(free)), plain()),
            // Only a mounted card can be measured; CrossPoint reports its free
            // memory, which is not its storage, so nothing here pretends.
            (_, Some(None)) => Span::styled("  free unknown", plain()),
            (_, None) => Span::raw(String::new()),
        };
        let (label, style, detail) = if ready > 0 {
            (
                Span::styled(key, accent()),
                bold(),
                Span::styled(
                    text,
                    if asking {
                        fg(Color::Yellow)
                    } else {
                        fg(Color::Green)
                    },
                ),
            )
        } else {
            (
                Span::styled(key, plain()),
                plain(),
                Span::styled(text, fg(Color::Red)),
            )
        };
        lines.push(Line::from(vec![
            label,
            Span::raw("  "),
            Span::styled(format!("{:<11}", place.label()), style),
            detail,
            space,
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("esc", accent()),
        Span::styled(" cancel", plain()),
    ]));
    // Never wider or taller than the list it covers, so a small terminal clips
    // the dialog's own content instead of drawing outside the frame.
    let width = area.width.min(56);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let title = match entries.first() {
        Some(entry) if single => format!(" Copy to · {} ", shorten(&entry.title, 24)),
        _ => format!(" Copy {} to ", crate::books::books(entries.len())),
    };
    let block = frame_block(title).border_style(accent());
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines), inner);
}
/// A bar that says how far a set has come, because each book's own progress
/// replaces the text beneath it many times over.
fn progress_line(done: usize, total: usize, width: u16) -> Line<'static> {
    let label = "Copying ";
    let counted = format!(" {done} of {total} books");
    let cells = usize::from(width)
        .saturating_sub(label.len() + counted.len() + 2)
        .clamp(0, 32);
    let filled = cells.saturating_mul(done).checked_div(total).unwrap_or(0);
    Line::from(vec![
        Span::styled(label, bold()),
        Span::styled("█".repeat(filled), fg(Color::Green)),
        Span::styled("░".repeat(cells - filled), plain()),
        Span::styled(counted, bold()),
    ])
}
/// Every copy of one book, which is also the only place a duplicate becomes
/// visible: two files of the same book share a single row in the library.
fn draw_copies(model: &Model, frame: &mut Frame<'_>, area: Rect, entry: &Entry) {
    let Source::Book(book) = &entry.source else {
        return;
    };
    let width = area.width.min(72);
    let mut lines = vec![];
    for (index, copy) in book.copies.iter().enumerate().take(9) {
        let blocked = match model.removal(book, copy) {
            Removal::Ready => None,
            Removal::Blocked(reason, _) => Some(reason),
        };
        let key = format!("{}", index + 1);
        lines.push(Line::from(vec![
            if blocked.is_some() {
                Span::raw(key)
            } else {
                Span::styled(key, accent())
            },
            Span::raw("  "),
            Span::styled(format!("{:<7}", copy.place.label()), bold()),
            Span::styled(format!("{:>9}  ", size(copy.size)), plain()),
            Span::styled(
                format!(
                    "{:<12}",
                    if copy.sha.is_empty() {
                        "unreadable"
                    } else if copy.optimized {
                        "device copy"
                    } else {
                        "original"
                    }
                ),
                if copy.optimized {
                    fg(Color::Yellow)
                } else {
                    fg(Color::Green)
                },
            ),
            Span::styled(blocked.unwrap_or("").to_string(), plain()),
        ]));
        lines.push(Line::from(Span::styled(
            format!(
                "     {}",
                tail(&copy.path, width.saturating_sub(8) as usize)
            ),
            plain(),
        )));
    }
    lines.push(Line::default());
    match &model.confirm {
        Some((place, path)) => {
            lines.push(Line::from(vec![
                Span::styled("Delete this ", fg(Color::Red)),
                Span::styled(place.label(), bold()),
                Span::styled(" copy? It cannot be undone.", fg(Color::Red)),
            ]));
            lines.push(Line::from(Span::raw(format!(
                "  {}",
                tail(path, width.saturating_sub(5) as usize)
            ))));
            lines.push(Line::from(vec![
                Span::styled("  y", accent()),
                Span::styled(" deletes it · ", plain()),
                Span::styled("esc", accent()),
                Span::styled(" keeps it", plain()),
            ]));
        }
        None => lines.push(Line::from(vec![
            Span::styled("1-9", accent()),
            Span::styled(" delete that copy · ", plain()),
            Span::styled("esc", accent()),
            Span::styled(" close", plain()),
        ])),
    }
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let block = frame_block(format!(" Copies · {} ", shorten(&entry.title, 30))).border_style(
        if model.confirm.is_some() {
            fg(Color::Red)
        } else {
            accent()
        },
    );
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines), inner);
}
fn status_lines(model: &Model) -> Vec<Line<'static>> {
    let text = clean(&model.status);
    let busy = model.busy.is_some() || model.loading.is_some();
    let (mark, style) = if busy {
        (
            ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][model.tick % 10].to_string(),
            fg(Color::Cyan),
        )
    } else if text.starts_with("Error:") || text.contains("error") {
        ("✗".into(), fg(Color::Red))
    } else if text.contains("verified") {
        ("✓".into(), fg(Color::Green))
    } else {
        ("·".into(), plain())
    };
    let status = Line::from(vec![
        Span::styled(format!("{mark} "), style),
        Span::styled(text, if busy { Style::default() } else { style }),
    ]);
    match model.step {
        Some((done, total)) => vec![progress_line(done, total, model.width.get()), status],
        None => vec![status],
    }
}

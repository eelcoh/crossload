//! Turning placed text back into prose.
//!
//! A PDF has no paragraphs, only lines that happen to sit under one another.
//! Rebuilding them is guesswork with rules: a line that ends short ends a
//! paragraph, an indent starts one, a larger face is a heading, and a word
//! broken across a line break was never two words.
use super::text::Piece;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Block {
    Heading(String),
    Paragraph(String),
    /// A picture, by its place in the book's own list of them.
    Figure(usize),
}

/// One line of a page: its text, where it begins and ends, and how big it is.
#[derive(Clone, Debug)]
pub(super) struct Line {
    pub y: f32,
    pub left: f32,
    pub right: f32,
    pub size: f32,
    pub text: String,
}

/// Lines within this fraction of a font size of each other are one line.
const SAME_LINE: f32 = 0.4;
/// A face this much larger than the body is a heading rather than emphasis.
const HEADING: f32 = 1.15;
/// A line ending this many ems short of the right margin has ended a
/// paragraph, which is what a ragged last line means.
const SHORT_OF_MARGIN: f32 = 1.5;
/// An indent worth this fraction of the font size starts a paragraph.
const INDENT: f32 = 0.6;
/// Text is justified when most lines end within this many ems of the margin.
/// Only then does a short line mean anything: in ragged text every line ends
/// somewhere different and none of it is a paragraph ending.
const JUSTIFIED_WITHIN: f32 = 2.0;
const MOSTLY: f32 = 0.6;

/// A word as it would be looked up: no case, no punctuation around it.
fn word(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric() && c != '-')
        .to_lowercase()
}
/// Every word the document uses, for deciding what a hyphen at a line break
/// meant. A document is its own dictionary: a compound it hyphenates will be
/// hyphenated somewhere it did not have to break, and a word it broke will
/// appear whole elsewhere. The fragment before a break is not a word and is
/// left out.
fn vocabulary(lines: &[&Line]) -> std::collections::BTreeSet<String> {
    let mut words = std::collections::BTreeSet::new();
    for line in lines {
        let broken = line.text.ends_with('-');
        let mut parts = line.text.split_whitespace().peekable();
        while let Some(part) = parts.next() {
            if broken && parts.peek().is_none() {
                continue;
            }
            let part = word(part);
            if !part.is_empty() {
                words.insert(part);
            }
        }
    }
    words
}
/// What to do with a hyphen a line break left behind: keep it, or close it up.
fn keeps_hyphen(stem: &str, rest: &str, words: &std::collections::BTreeSet<String>) -> bool {
    let (Some(before), Some(after)) = (
        stem.split_whitespace().next_back(),
        rest.split_whitespace().next(),
    ) else {
        return false;
    };
    let (before, after) = (word(before), word(after));
    if before.is_empty() || after.is_empty() {
        return false;
    }
    // Ask the document which form it uses when it is not out of room. A
    // hyphenated compound wins over the closed-up form, because a break can
    // only ever remove a hyphen, never invent one.
    if words.contains(&format!("{before}-{after}")) {
        return true;
    }
    if words.contains(&format!("{before}{after}")) {
        return false;
    }
    // It says neither. Closing up is what a break usually meant.
    false
}
/// A printed table of contents leads to a page number an EPUB does not have.
/// It is not a heading, whatever size it is set in.
fn dot_leader(text: &str) -> bool {
    text.contains("....")
}
/// Collapse the leader dots that a table of contents is drawn with.
fn tidy(text: &str) -> String {
    if !dot_leader(text) {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut dots = 0;
    for c in text.chars() {
        if c == '.' {
            dots += 1;
            continue;
        }
        if dots > 3 {
            out.push_str(" … ");
        } else {
            out.extend(std::iter::repeat_n('.', dots));
        }
        dots = 0;
        out.push(c);
    }
    if dots > 3 {
        out.push_str(" …");
    }
    out.trim().to_owned()
}

/// Where a run of text ends, from the advance its own font declares.
fn end_of(piece: &Piece) -> f32 {
    piece.x + piece.advance
}

/// The size most of the document's text is set in.
pub(super) fn body_size(pages: &[Vec<Piece>]) -> f32 {
    let mut weights: std::collections::BTreeMap<i32, usize> = Default::default();
    for piece in pages.iter().flatten() {
        *weights.entry(piece.size.round() as i32).or_default() += piece.text.len();
    }
    weights
        .into_iter()
        .max_by_key(|(_, weight)| *weight)
        .map_or(11.0, |(size, _)| size as f32)
}

/// A page's pieces gathered into lines, top to bottom.
pub(super) fn lines(pieces: &[Piece]) -> Vec<Line> {
    let mut ordered: Vec<&Piece> = pieces.iter().collect();
    ordered.sort_by(|a, b| b.y.total_cmp(&a.y));
    let mut lines: Vec<Vec<&Piece>> = vec![];
    for piece in ordered {
        match lines.last_mut() {
            Some(line) if (line[0].y - piece.y).abs() <= piece.size.max(1.0) * SAME_LINE => {
                line.push(piece)
            }
            _ => lines.push(vec![piece]),
        }
    }
    lines
        .into_iter()
        .map(|mut line| {
            line.sort_by(|a, b| a.x.total_cmp(&b.x));
            let mut text = String::new();
            let mut end = f32::NEG_INFINITY;
            for piece in &line {
                let gap = piece.x - end;
                if !text.is_empty()
                    && gap > piece.size * 0.18
                    && !text.ends_with(' ')
                    && !piece.text.starts_with(' ')
                {
                    text.push(' ');
                }
                text.push_str(&piece.text);
                end = end_of(piece);
            }
            Line {
                y: line[0].y,
                left: line.iter().map(|p| p.x).fold(f32::INFINITY, f32::min),
                right: end,
                // The size the line is mostly set in, not its largest glyph.
                size: line
                    .iter()
                    .max_by_key(|p| p.text.len())
                    .map_or(11.0, |p| p.size),
                text: text.trim().to_owned(),
            }
        })
        .filter(|line| !line.text.is_empty())
        .collect()
}

/// A page of a book has text on it; a running head and a page number do not
/// make a page.
const TEXT_ON_A_PAGE: usize = 120;

/// Whether a page's text starts in two separated bands, which is what a second
/// column looks like: a left margin, a second margin past the middle, and a
/// gutter between them with nothing in it.
pub(super) fn two_columns(pieces: &[Piece], width: f32) -> bool {
    let total: usize = pieces.iter().map(|piece| piece.text.len()).sum();
    if total < TEXT_ON_A_PAGE {
        return false;
    }
    let share = |from: f32, to: f32| {
        pieces
            .iter()
            .filter(|piece| piece.x >= width * from && piece.x < width * to)
            .map(|piece| piece.text.len())
            .sum::<usize>() as f32
            / total as f32
    };
    // One column with an indented quotation fails the third test.
    share(0.0, 0.25) > 0.2 && share(0.45, 0.70) > 0.2 && share(0.25, 0.45) < 0.1
}
/// Where one column ends and the next begins. The gutter is empty by the test
/// above, so anywhere inside it separates the two.
const GUTTER: f32 = 0.42;

/// A page's lines in the order they are meant to be read. Reading a
/// two-column page straight across weaves the columns into each other, which
/// is the single worst thing a conversion can do to a book.
pub(super) fn page_lines(pieces: &[Piece], width: f32) -> Vec<Line> {
    if !two_columns(pieces, width) {
        return lines(pieces);
    }
    let (left, mut right): (Vec<Piece>, Vec<Piece>) = pieces
        .iter()
        .cloned()
        .partition(|piece| piece.x < width * GUTTER);
    // Slide the second column onto the first's margin. Everything downstream
    // measures indents and line endings against one margin, and a column that
    // starts half a page in would otherwise look like one long indent.
    let margin = |column: &[Piece]| {
        column
            .iter()
            .map(|piece| piece.x)
            .fold(f32::INFINITY, f32::min)
    };
    let offset = margin(&right) - margin(&left);
    if offset.is_finite() {
        for piece in &mut right {
            piece.x -= offset;
        }
    }
    let mut ordered = lines(&left);
    ordered.extend(lines(&right));
    ordered
}

/// A running head, a footer or a page number: text that repeats in the same
/// place page after page and belongs to the paper, not to the book.
pub(super) fn furniture(pages: &[Vec<Line>]) -> std::collections::BTreeSet<String> {
    let key = |text: &str| {
        text.chars()
            .filter(|c| !c.is_ascii_digit())
            .collect::<String>()
            .trim()
            .to_lowercase()
    };
    // Furniture is repetition from page to page, so a single page cannot have
    // any: its one line would otherwise be counted as both its head and its
    // foot and thrown away as a running head.
    if pages.len() < 2 {
        return std::collections::BTreeSet::new();
    }
    let mut seen: std::collections::BTreeMap<String, usize> = Default::default();
    for page in pages {
        // Only the outermost lines can be furniture; body text repeats too.
        // A page of one line offers it once, not once at each end.
        let ends = [page.first(), page.last().filter(|_| page.len() > 1)];
        for line in ends.into_iter().flatten() {
            *seen.entry(key(&line.text)).or_default() += 1;
        }
    }
    let enough = (pages.len() / 3).max(2);
    seen.into_iter()
        .filter(|(text, count)| *count >= enough && !text.is_empty())
        .map(|(text, _)| text)
        .chain(std::iter::once(String::new()))
        .collect()
}

/// Join what the page break split: a page number between two halves of a
/// sentence is not a full stop.
/// A picture on a page: how far down it sits, and which picture it is.
pub(super) type Placed = (f32, usize);

/// One thing a page puts in the flow, in reading order.
enum Item<'a> {
    Line(&'a Line),
    Figure(usize),
}

pub(super) fn blocks(pages: &[Vec<Line>], figures: &[Vec<Placed>], body: f32) -> Vec<Block> {
    let furniture = furniture(pages);
    let key = |text: &str| {
        text.chars()
            .filter(|c| !c.is_ascii_digit())
            .collect::<String>()
            .trim()
            .to_lowercase()
    };
    let kept: Vec<&Line> = pages
        .iter()
        .flatten()
        .filter(|line| !furniture.contains(&key(&line.text)))
        .collect();
    // Figures take their place among the lines of their own page, by where
    // they sat on it, so a picture keeps the paragraphs it stood between.
    let flow: Vec<Item> = pages
        .iter()
        .enumerate()
        .flat_map(|(number, lines)| {
            let mut items: Vec<(f32, Item)> = lines
                .iter()
                .filter(|line| !furniture.contains(&key(&line.text)))
                .map(|line| (line.y, Item::Line(line)))
                .collect();
            items.extend(
                figures
                    .get(number)
                    .into_iter()
                    .flatten()
                    .map(|(top, index)| (*top, Item::Figure(*index))),
            );
            // Down the page is decreasing y.
            items.sort_by(|a, b| b.0.total_cmp(&a.0));
            items.into_iter().map(|(_, item)| item)
        })
        .collect();
    // Where a full line ends. Taken near the top of the spread rather than at
    // the very top, so one wide table row cannot redefine the margin and make
    // every ordinary line look short.
    let mut rights: Vec<f32> = kept.iter().map(|line| line.right).collect();
    rights.sort_by(f32::total_cmp);
    let margin_right = rights
        .get(rights.len() * 85 / 100)
        .copied()
        .unwrap_or(f32::INFINITY);
    let justified = !kept.is_empty()
        && kept
            .iter()
            .filter(|line| line.right > margin_right - line.size * JUSTIFIED_WITHIN)
            .count() as f32
            / kept.len() as f32
            > MOSTLY;
    let margin = kept
        .iter()
        .map(|line| line.left)
        .fold(f32::INFINITY, f32::min);
    let words = vocabulary(&kept);
    let mut blocks: Vec<Block> = vec![];
    let mut previous: Option<&Line> = None;
    for item in flow {
        let line = match item {
            Item::Figure(index) => {
                blocks.push(Block::Figure(index));
                // Whatever follows a picture starts a paragraph of its own.
                previous = None;
                continue;
            }
            Item::Line(line) => line,
        };
        let heading = line.size > body * HEADING && !dot_leader(&line.text);
        let starts_paragraph = previous.is_none_or(|last| {
            last.size > body * HEADING
                || line.size > body * HEADING
                || line.left > margin + line.size * INDENT
                || (justified && last.right < margin_right - last.size * SHORT_OF_MARGIN)
                // A gap wider than a line is a break, unless it is a page turn,
                // where y jumps back up the page.
                || (last.y > line.y && last.y - line.y > line.size * 1.8)
        });
        match blocks.last_mut() {
            Some(Block::Paragraph(text)) if !starts_paragraph && !heading => {
                if let Some(stem) = text.strip_suffix('-') {
                    // A word broken across lines was usually never two words,
                    // but a compound that carries its own hyphen still is one.
                    let broken = !stem.ends_with(char::is_whitespace)
                        && line.text.starts_with(char::is_lowercase);
                    if broken && !keeps_hyphen(stem, &line.text, &words) {
                        *text = stem.to_owned();
                    } else if !broken {
                        text.push(' ');
                    }
                } else {
                    text.push(' ');
                }
                text.push_str(&tidy(&line.text));
            }
            _ if heading => blocks.push(Block::Heading(tidy(&line.text))),
            _ => blocks.push(Block::Paragraph(tidy(&line.text))),
        }
        previous = Some(line);
    }
    blocks
}

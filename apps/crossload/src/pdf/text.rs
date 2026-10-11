//! Where text sits on a PDF page, and what it says.
//!
//! One walk of the content stream serves both judging a PDF and converting it,
//! so the two can never disagree about what is on a page.
//!
//! Three things here are easy to get wrong and quietly ruin everything
//! downstream. `MediaBox` is usually an indirect reference, so a page's width
//! has to be dereferenced or every measurement is against the wrong scale.
//! `Td`, `TD` and `T*` move in text space, so their offsets are in units of the
//! text matrix's own scale: a jump of 20 at a scale of 11 is 220 points, and
//! added raw it collapses to 20 and hides the column it jumped to. And the
//! numbers inside a `TJ` array are how most producers write a space at all.
use lopdf::{content::Content, Document, Object, ObjectId};

/// A run of text placed on a page, in page points from the lower-left corner.
#[derive(Clone, Debug)]
pub(super) struct Piece {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    /// How far this run advances the pen, so the next run's distance from it
    /// says whether a space was drawn by moving rather than by writing one.
    pub advance: f32,
    pub text: String,
}

/// Something drawn on a page by name, rather than written out: a picture, or
/// a form that may hold one. Where it sits is taken from the transformation in
/// force, which maps the unit square onto the page.
#[derive(Clone, Debug)]
pub(super) struct Drawn {
    pub name: Vec<u8>,
    /// The top edge in page points, so a figure can be put back into reading
    /// order among the lines it sat between.
    pub top: f32,
    pub height: f32,
    pub width: f32,
}

/// A font stream cannot be trusted to be small; decoding one is bounded.
const FONT_LIMIT: usize = 4 * 1024 * 1024;
/// A gap this wide, as a fraction of the font size, is a space the producer
/// drew by moving rather than by writing one.
const GAP_IS_A_SPACE: f32 = 0.18;
/// What a glyph is assumed to advance when there is no font to ask at all.
const ASSUMED_WIDTH: f32 = 0.5;

/// What is needed to read a font's text and know how wide it is.
struct Font<'a> {
    encoding: Option<lopdf::Encoding<'a>>,
    /// Advance per character code, in thousandths of an em.
    widths: std::collections::BTreeMap<u32, f32>,
    /// What a code not listed above advances.
    default: f32,
    /// A composite font codes each glyph in two bytes; a simple font in one.
    bytes_per_code: usize,
}
impl Font<'_> {
    /// The advance of one run of codes, in ems.
    fn advance(&self, bytes: &[u8]) -> f32 {
        bytes
            .chunks(self.bytes_per_code)
            .map(|code| {
                let code = code.iter().fold(0u32, |value, b| value << 8 | *b as u32);
                self.widths.get(&code).copied().unwrap_or(self.default) / 1000.0
            })
            .sum()
    }
}
/// Widths of a composite font, from the `W` array of its descendant. Entries
/// are either a first code and a list, or a range of codes and one width.
/// Codes are taken as CIDs, which holds for the Identity encodings that almost
/// every producer uses.
fn composite_widths(array: &[Object], widths: &mut std::collections::BTreeMap<u32, f32>) {
    let mut index = 0;
    while index < array.len() {
        let first = number(array.get(index)) as u32;
        match array.get(index + 1) {
            Some(Object::Array(listed)) => {
                for (offset, width) in listed.iter().enumerate() {
                    widths.insert(first + offset as u32, number_of(width));
                }
                index += 2;
            }
            Some(_) => {
                let last = number(array.get(index + 1)) as u32;
                let width = number(array.get(index + 2));
                // A malformed range must not become a loop of millions.
                for code in first..=last.min(first.saturating_add(65_535)) {
                    widths.insert(code, width);
                }
                index += 3;
            }
            None => break,
        }
    }
}
fn font<'a>(document: &'a Document, dictionary: &'a lopdf::Dictionary) -> Font<'a> {
    let resolve = |dictionary: &lopdf::Dictionary, key: &[u8]| {
        dictionary
            .get(key)
            .ok()
            .and_then(|object| document.dereference(object).ok())
            .map(|(_, object)| object.to_owned())
    };
    let composite = dictionary
        .get(b"Subtype")
        .and_then(|subtype| subtype.as_name())
        .is_ok_and(|name| name == b"Type0");
    let mut widths = std::collections::BTreeMap::new();
    let mut default = 500.0;
    if composite {
        // The widths of a composite font belong to the font it descends to.
        let descendant = resolve(dictionary, b"DescendantFonts")
            .and_then(|object| object.as_array().ok().and_then(|a| a.first()).cloned())
            .and_then(|object| {
                document
                    .dereference(&object)
                    .ok()
                    .map(|(_, o)| o.to_owned())
            })
            .and_then(|object| object.as_dict().ok().cloned());
        if let Some(descendant) = descendant {
            default = resolve(&descendant, b"DW").map_or(1000.0, |o| number_of(&o));
            if let Some(array) = resolve(&descendant, b"W").and_then(|o| o.as_array().ok().cloned())
            {
                composite_widths(&array, &mut widths);
            }
        } else {
            default = 1000.0;
        }
    } else {
        let first = resolve(dictionary, b"FirstChar").map_or(0, |o| number_of(&o) as u32);
        if let Some(array) = resolve(dictionary, b"Widths").and_then(|o| o.as_array().ok().cloned())
        {
            for (offset, width) in array.iter().enumerate() {
                widths.insert(first + offset as u32, number_of(width));
            }
        }
    }
    Font {
        encoding: dictionary
            .get_font_encoding_with_limit(document, FONT_LIMIT)
            .ok(),
        widths,
        default,
        bytes_per_code: if composite { 2 } else { 1 },
    }
}

pub(super) fn number(operand: Option<&Object>) -> f32 {
    match operand {
        Some(Object::Real(value)) => *value,
        Some(Object::Integer(value)) => *value as f32,
        _ => 0.0,
    }
}

/// The page's width in its own units, inherited from the page tree when the
/// page itself does not say.
pub(super) fn width(document: &Document, page: ObjectId) -> f32 {
    size(document, page).0
}

/// A page's width and height in points, from the box it declares.
pub(super) fn size(document: &Document, page: ObjectId) -> (f32, f32) {
    let mut node = document.get_dictionary(page).ok();
    for _ in 0..32 {
        let Some(dictionary) = node else { break };
        // Both the box and its numbers may be written as references.
        let resolved = |object| document.dereference(object).ok().map(|(_, value)| value);
        if let Some(box_) = dictionary
            .get(b"MediaBox")
            .ok()
            .and_then(resolved)
            .and_then(|object| object.as_array().ok())
        {
            let edge = |index: usize| number(box_.get(index).and_then(&resolved));
            let (left, right) = (edge(0), edge(2));
            let (bottom, top) = (edge(1), edge(3));
            if right - left > 1.0 {
                return (right - left, (top - bottom).max(1.0));
            }
        }
        node = dictionary
            .get(b"Parent")
            .and_then(|parent| document.dereference(parent))
            .ok()
            .and_then(|(_, object)| object.as_dict().ok());
    }
    // A4 is the likeliest default and only sets the scale for column bands.
    (595.0, 842.0)
}

/// The horizontal part of a transformation: everything this needs to know
/// about placement, without modelling rotation or skew.
#[derive(Clone, Copy)]
struct Axis {
    scale: f32,
    origin: f32,
}
impl Axis {
    const IDENTITY: Self = Self {
        scale: 1.0,
        origin: 0.0,
    };
    fn at(self, value: f32) -> f32 {
        self.origin + value * self.scale
    }
}

/// Decode one shown string with the font in use, turning the kerning numbers
/// of a `TJ` array into the spaces they stand in for.
fn shown(operands: &[Object], font: Option<&Font>) -> (String, f32) {
    let decode = |bytes: &[u8]| match font.and_then(|font| font.encoding.as_ref()) {
        Some(encoding) => Document::decode_text(encoding, bytes).unwrap_or_default(),
        // Without an encoding the bytes are most often Latin-1 already.
        None => bytes.iter().map(|b| *b as char).collect(),
    };
    let run = |bytes: &[u8], decoded: &str| -> f32 {
        match font {
            Some(font) => font.advance(bytes),
            None => decoded.chars().count() as f32 * ASSUMED_WIDTH,
        }
    };
    let (mut text, mut advance) = (String::new(), 0.0);
    for operand in operands {
        match operand {
            Object::String(bytes, _) => {
                let decoded = decode(bytes);
                advance += run(bytes, &decoded);
                text.push_str(&decoded);
            }
            Object::Array(items) => {
                for item in items {
                    match item {
                        Object::String(bytes, _) => {
                            let decoded = decode(bytes);
                            advance += run(bytes, &decoded);
                            text.push_str(&decoded);
                        }
                        number => {
                            // Thousandths of text space, and negative moves right.
                            let gap = -number_of(number) / 1000.0;
                            advance += gap;
                            if gap > GAP_IS_A_SPACE && !text.ends_with(' ') {
                                text.push(' ');
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    (text, advance)
}
fn number_of(object: &Object) -> f32 {
    number(Some(object))
}

/// What a page draws: its text, and the things it draws by name.
///
/// Both come from one walk, so a figure's place among the lines is measured
/// against the same transformations the lines were.
pub(super) fn contents(document: &Document, page: ObjectId) -> (Vec<Piece>, Vec<Drawn>) {
    let mut found = Found::default();
    let resources = document
        .get_page_resources(page)
        .ok()
        .and_then(|(direct, streams)| {
            direct.cloned().or_else(|| {
                streams
                    .iter()
                    .find_map(|id| document.get_dictionary(*id).ok().cloned())
            })
        })
        .unwrap_or_default();
    walk(
        document,
        &document.get_page_content(page),
        &resources,
        (Axis::IDENTITY, Axis::IDENTITY),
        0,
        &mut found,
    );
    (found.pieces, found.drawn)
}

/// What one walk of a content stream turned up.
#[derive(Default)]
struct Found {
    pieces: Vec<Piece>,
    drawn: Vec<Drawn>,
}

/// How deep a form may be nested before this stops following.
const FORMS_DEEP: usize = 6;

/// Walk one content stream, following the forms it draws.
///
/// A producer may put the whole page inside a form XObject and leave one `Do`
/// on the page itself. Not following that reads the page as empty, which
/// refuses a perfectly ordinary document as a scan.
fn walk(
    document: &Document,
    stream: &[u8],
    resources: &lopdf::Dictionary,
    base: (Axis, Axis),
    depth: usize,
    found: &mut Found,
) {
    let Ok(content) = Content::decode(stream) else {
        return;
    };
    let fonts: std::collections::BTreeMap<Vec<u8>, Font> = resources
        .get(b"Font")
        .ok()
        .and_then(|entry| document.dereference(entry).ok())
        .and_then(|(_, object)| object.as_dict().ok())
        .into_iter()
        .flat_map(|table| table.iter())
        .filter_map(|(name, entry)| {
            let dictionary = document
                .dereference(entry)
                .ok()
                .and_then(|(_, object)| object.as_dict().ok())?;
            Some((name.clone(), font(document, dictionary)))
        })
        .collect();
    let (mut horizontal, mut vertical) = base;
    let mut saved: Vec<(Axis, Axis)> = vec![];
    // Text, line and font state, all reset by BT.
    let (mut x, mut y, mut line_x, mut line_y) = (0.0, 0.0, 0.0, 0.0);
    let (mut scale, mut font_size, mut leading) = (1.0_f32, 0.0_f32, 0.0_f32);
    let mut font: Option<Vec<u8>> = None;
    for operation in &content.operations {
        let operands = &operation.operands;
        match operation.operator.as_str() {
            "q" => saved.push((horizontal, vertical)),
            "Q" => {
                if let Some((h, v)) = saved.pop() {
                    horizontal = h;
                    vertical = v;
                }
            }
            "cm" => {
                horizontal.origin += number(operands.get(4)) * horizontal.scale;
                vertical.origin += number(operands.get(5)) * vertical.scale;
                let (a, d) = (number(operands.first()), number(operands.get(3)));
                if a != 0.0 {
                    horizontal.scale *= a;
                }
                if d != 0.0 {
                    vertical.scale *= d;
                }
            }
            "BT" => {
                x = 0.0;
                y = 0.0;
                line_x = 0.0;
                line_y = 0.0;
                scale = 1.0;
            }
            "Tf" => {
                font = operands
                    .first()
                    .and_then(|o| o.as_name().ok())
                    .map(Vec::from);
                font_size = number(operands.get(1));
            }
            "TL" => leading = number(operands.first()),
            "Tm" => {
                let a = number(operands.first());
                scale = if a == 0.0 { 1.0 } else { a };
                x = number(operands.get(4));
                y = number(operands.get(5));
                line_x = x;
                line_y = y;
            }
            "Td" | "TD" => {
                if operation.operator == "TD" {
                    leading = -number(operands.get(1));
                }
                line_x += number(operands.first()) * scale;
                line_y += number(operands.get(1)) * scale;
                x = line_x;
                y = line_y;
            }
            "T*" => {
                line_y -= leading * scale;
                x = line_x;
                y = line_y;
            }
            "Tj" | "TJ" | "'" | "\"" => {
                if matches!(operation.operator.as_str(), "'" | "\"") {
                    line_y -= leading * scale;
                    x = line_x;
                    y = line_y;
                }
                let size = font_size * scale * vertical.scale.abs();
                let (text, advance) = shown(operands, font.as_ref().and_then(|n| fonts.get(n)));
                // A space drawn as its own run is still a space. Dropping it
                // leaves the words either side to be told apart by a gap that
                // the space itself was holding open.
                if !text.is_empty() {
                    found.pieces.push(Piece {
                        x: horizontal.at(x),
                        y: vertical.at(y),
                        size,
                        advance: advance * font_size * scale * horizontal.scale.abs(),
                        text,
                    });
                }
            }
            "Do" => {
                let Some(name) = operands.first().and_then(|o| o.as_name().ok()) else {
                    continue;
                };
                let object = resources
                    .get(b"XObject")
                    .ok()
                    .and_then(|entry| document.dereference(entry).ok())
                    .and_then(|(_, object)| object.as_dict().ok())
                    .and_then(|table| table.get(name).ok())
                    .and_then(|entry| document.dereference(entry).ok())
                    .and_then(|(_, object)| object.as_stream().ok());
                let kind = object
                    .and_then(|stream| stream.dict.get(b"Subtype").ok())
                    .and_then(|subtype| subtype.as_name().ok());
                match kind {
                    Some(b"Form") if depth < FORMS_DEEP => {
                        // A form draws in its own space, on top of the
                        // transformation in force where it was called.
                        if let Some(stream) = resources
                            .get(b"XObject")
                            .ok()
                            .and_then(|entry| document.dereference(entry).ok())
                            .and_then(|(_, object)| object.as_dict().ok())
                            .and_then(|table| table.get(name).ok())
                            .and_then(|entry| document.dereference(entry).ok())
                            .and_then(|(_, object)| object.as_stream().ok())
                        {
                            let inner = stream
                                .dict
                                .get(b"Resources")
                                .ok()
                                .and_then(|entry| document.dereference(entry).ok())
                                .and_then(|(_, object)| object.as_dict().ok())
                                .cloned()
                                // A form without resources of its own inherits
                                // the ones in force where it was drawn.
                                .unwrap_or_else(|| resources.clone());
                            if let Ok(bytes) = stream.decompressed_content() {
                                walk(
                                    document,
                                    &bytes,
                                    &inner,
                                    (horizontal, vertical),
                                    depth + 1,
                                    found,
                                );
                            }
                        }
                    }
                    _ => {
                        // The transformation maps the unit square onto the
                        // page, so the two edges are its image of 0 and 1.
                        // Which is the top depends on the sign of the scale.
                        let (a, b) = (vertical.at(0.0), vertical.at(1.0));
                        found.drawn.push(Drawn {
                            name: name.to_vec(),
                            top: a.max(b),
                            height: (b - a).abs(),
                            width: (horizontal.at(1.0) - horizontal.at(0.0)).abs(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

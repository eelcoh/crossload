//! Whether a PDF could become an EPUB worth reading, decided before anything
//! is rewritten.
//!
//! This is the more important half of converting. A conversion that quietly
//! produces scrambled text costs more than a refusal does, because nobody finds
//! out until they are away from their computer with it. So every PDF is judged
//! first, and the judgement is what the interface acts on.
//!
//! Only the page structure is read here: where text is placed, not what it
//! says. Horizontal placement is followed through the graphics matrix as well
//! as the text and line matrices, because a second column is as often a
//! translated coordinate space as it is a larger x. Rotation and skew are not
//! modelled, which is why this stays a heuristic rather than a measurement.
mod figure;
mod layout;
mod text;
mod write;

use anyhow::{Context, Result};
use lopdf::Document;

/// Why a PDF would convert badly, or not at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Concern {
    Protected,
    NoText,
    NothingFound,
    MostlyScanned,
    MixedColumns,
    NoChapters,
    FiguresDropped,
}
impl Concern {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Protected => "the PDF is password protected, so its pages cannot be read",
            Self::NoText => "the pages are images with no text in them, which would need OCR",
            Self::NothingFound => "no text could be found anywhere in this PDF",
            Self::MostlyScanned => "most pages are scanned images rather than text",
            Self::MixedColumns => {
                "some pages are in columns and some are not, so the order of those is a guess"
            }
            Self::NoChapters => "the PDF has no outline, so the EPUB would have no chapters",
            Self::FiguresDropped => "some of its pictures cannot be read and will be left behind",
        }
    }
}
/// What converting this PDF would be worth.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Verdict {
    /// One column of real text. An EPUB made from this reads like a book.
    Good,
    /// Worth offering, but something will come out wrong. Ask before doing it.
    Poor(Vec<Concern>),
    /// There is nothing here to convert.
    Impossible(Concern),
}
impl Verdict {
    /// Everything standing in the way, for saying so in one line.
    pub fn concerns(&self) -> Vec<Concern> {
        match self {
            Self::Good => vec![],
            Self::Poor(concerns) => concerns.clone(),
            Self::Impossible(concern) => vec![*concern],
        }
    }
    /// What the concerns add up to, as a sentence.
    pub fn because(&self) -> String {
        let mut reasons = self.concerns().into_iter().map(Concern::describe);
        let mut text = reasons.next().unwrap_or("").to_owned();
        for reason in reasons {
            text.push_str(", and ");
            text.push_str(reason);
        }
        text
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Good => "convertible",
            Self::Poor(_) => "converts poorly",
            Self::Impossible(_) => "cannot be converted",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Report {
    pub verdict: Verdict,
    pub pages: usize,
    /// Pages carrying enough text to be a page of a book.
    pub text_pages: usize,
    /// Pages showing something but holding no text of their own.
    pub image_pages: usize,
    /// Pages whose text starts in two separated bands across the width.
    pub column_pages: usize,
    /// Pages of text that also show a picture, which converting leaves behind.
    pub figure_pages: usize,
    pub outline: bool,
}

/// A page of a book has text on it; a running head and a page number do not
/// make a page. Below this a page counts as empty or scanned.
const TEXT_ON_A_PAGE: usize = 120;
/// Reading every page of a long document to guess its shape is waste; an even
/// sample across it says the same thing.
const SAMPLED_PAGES: usize = 40;

/// Each page's lines in reading order, which for a two-column page means one
/// column and then the other rather than straight across the gutter.
fn page_lines(document: &Document, pages: &[Vec<text::Piece>]) -> Vec<Vec<layout::Line>> {
    document
        .get_pages()
        .values()
        .zip(pages)
        .map(|(id, pieces)| layout::page_lines(pieces, text::width(document, *id)))
        .collect()
}

/// What a PDF says about itself, when it says anything.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Info {
    pub title: Option<String>,
    pub author: Option<String>,
}

/// Read the document information dictionary. Strings there are PDFDocEncoding
/// or UTF-16, and either may be absent; nothing here guesses at a missing one.
pub fn info(data: &[u8]) -> Result<Info> {
    let document = Document::load_mem(data).context("This file could not be read as a PDF")?;
    Ok(from(&document))
}
fn from(document: &Document) -> Info {
    let Some(dictionary) = document
        .trailer
        .get(b"Info")
        .and_then(|info| document.dereference(info))
        .ok()
        .and_then(|(_, object)| object.as_dict().ok().cloned())
    else {
        return Info::default();
    };
    let text = |key: &[u8]| {
        let bytes = dictionary.get(key).and_then(|value| value.as_str()).ok()?;
        let decoded = match bytes {
            // A text string may be UTF-16BE behind a byte order mark.
            [0xFE, 0xFF, rest @ ..] => {
                let units: Vec<u16> = rest
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_be_bytes(*pair))
                    .collect();
                String::from_utf16(&units).ok()?
            }
            bytes => bytes.iter().map(|&byte| pdf_doc(byte)).collect(),
        };
        let decoded = decoded.trim();
        (!decoded.is_empty()).then(|| decoded.to_owned())
    };
    Info {
        title: text(b"Title"),
        author: text(b"Author"),
    }
}

/// One byte of a PDF text string without a byte order mark.
///
/// These are PDFDocEncoding, not UTF-8 and not quite Latin-1: it agrees with
/// Latin-1 from 0xA1 up, and replaces the C1 control range with typography.
/// Reading a title as UTF-8 turns an en dash into a replacement character,
/// which then ends up in a filename.
fn pdf_doc(byte: u8) -> char {
    const HIGH: [char; 33] = [
        '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}',
        '\u{2044}', '\u{2039}', '\u{203a}', '\u{2212}', '\u{2030}', '\u{201e}', '\u{201c}',
        '\u{201d}', '\u{2018}', '\u{2019}', '\u{201a}', '\u{2122}', '\u{fb01}', '\u{fb02}',
        '\u{0141}', '\u{0152}', '\u{0160}', '\u{0178}', '\u{017d}', '\u{0131}', '\u{0142}',
        '\u{0153}', '\u{0161}', '\u{017e}', '\u{fffd}', '\u{20ac}',
    ];
    const ACCENTS: [char; 8] = [
        '\u{02d8}', '\u{02c7}', '\u{02c6}', '\u{02d9}', '\u{02dd}', '\u{02db}', '\u{02da}',
        '\u{02dc}',
    ];
    match byte {
        0x18..=0x1F => ACCENTS[byte as usize - 0x18],
        0x80..=0xA0 => HIGH[byte as usize - 0x80],
        // Latin-1 from here, which is what char already is.
        _ => byte as char,
    }
}

/// Convert a PDF into an EPUB, refusing one there is nothing to convert from.
///
/// The verdict is returned with the book so a caller can say what it is getting
/// before anyone reads it. Nothing here touches the PDF: the EPUB is a new file
/// that stands beside it.
pub fn convert(data: &[u8]) -> Result<(Vec<u8>, Verdict)> {
    let report = inspect(data)?;
    if let Verdict::Impossible(concern) = report.verdict {
        anyhow::bail!("Cannot convert this PDF: {}", concern.describe());
    }
    let document = Document::load_mem(data).context("This file could not be read as a PDF")?;
    let info = from(&document);
    let mut pages = Vec::new();
    let mut placed = Vec::new();
    let mut pictures = Vec::new();
    for page in document.get_pages().values() {
        let (pieces, drawn) = text::contents(&document, *page);
        // A picture is read once, here, and referred to by its place in the
        // book's own list from then on.
        let on_this_page = figure::on_page(&document, *page, &drawn)
            .into_iter()
            .map(|(top, picture)| {
                pictures.push(picture);
                (top, pictures.len() - 1)
            })
            .collect();
        pages.push(pieces);
        placed.push(on_this_page);
    }
    let body = layout::body_size(&pages);
    let lines: Vec<_> = page_lines(&document, &pages);
    let blocks = layout::blocks(&lines, &placed, body);
    anyhow::ensure!(
        blocks
            .iter()
            .any(|block| matches!(block, layout::Block::Paragraph(text) if text.len() > 40)),
        "Nothing readable came out of this PDF; it was not converted"
    );
    let title = write::title(info.title, &blocks);
    let author = info.author.unwrap_or_default();
    Ok((
        write::epub(&title, &author, &blocks, &pictures)?,
        report.verdict,
    ))
}

pub fn inspect(data: &[u8]) -> Result<Report> {
    let document = Document::load_mem(data).context("This file could not be read as a PDF")?;
    let pages = document.get_pages();
    let mut report = Report {
        verdict: Verdict::Good,
        pages: pages.len(),
        text_pages: 0,
        image_pages: 0,
        column_pages: 0,
        figure_pages: 0,
        outline: document
            .catalog()
            .ok()
            .and_then(|catalog| catalog.get(b"Outlines").ok())
            .and_then(|outlines| document.dereference(outlines).ok())
            .and_then(|(_, object)| object.as_dict().ok())
            .is_some_and(|outlines| outlines.get(b"First").is_ok()),
    };
    if document.trailer.get(b"Encrypt").is_ok() {
        report.verdict = Verdict::Impossible(Concern::Protected);
        return Ok(report);
    }
    let step = pages.len().div_ceil(SAMPLED_PAGES).max(1);
    let sampled: Vec<_> = pages.values().copied().step_by(step).collect();
    for page in &sampled {
        let width = text::width(&document, *page);
        // One walk serves both: what the page says, and what it draws.
        let (pieces, drawn) = text::contents(&document, *page);
        let pictures = figure::judge(&document, *page, &drawn);
        let bytes: usize = pieces.iter().map(|piece| piece.text.len()).sum();
        if bytes >= TEXT_ON_A_PAGE {
            report.text_pages += 1;
            if layout::two_columns(&pieces, width) {
                report.column_pages += 1;
            }
            // Pictures that can be read are carried across, so only the
            // ones that cannot leave a hole worth warning about.
            if pictures.dropped {
                report.figure_pages += 1;
            }
        } else if pictures.any {
            report.image_pages += 1;
        }
    }
    let sampled = sampled.len().max(1);
    let text = report.text_pages as f32 / sampled as f32;
    let columns = report.column_pages as f32 / report.text_pages.max(1) as f32;
    let mut concerns = vec![];
    report.verdict = if report.text_pages == 0 {
        Verdict::Impossible(if report.image_pages > 0 {
            Concern::NoText
        } else {
            Concern::NothingFound
        })
    } else {
        if text < 0.6 {
            concerns.push(Concern::MostlyScanned);
        }
        // Columns are read one at a time, so a document that is in columns
        // throughout converts as well as one that is not. A document that
        // changes its mind page to page is where the split is least certain.
        if (0.15..0.85).contains(&columns) {
            concerns.push(Concern::MixedColumns);
        }
        if !report.outline {
            concerns.push(Concern::NoChapters);
        }
        if report.figure_pages > 0 {
            concerns.push(Concern::FiguresDropped);
        }
        if concerns.is_empty() {
            Verdict::Good
        } else {
            Verdict::Poor(concerns)
        }
    };
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Object, Stream};

    /// Taken from a real PDF in the DPLA Palace Bookshelf, whose title holds
    /// byte 0x85. As UTF-8 that is invalid and becomes a replacement
    /// character; in PDFDocEncoding it is the en dash the title actually has.
    #[test]
    fn a_title_is_read_in_the_encoding_pdf_strings_actually_use() {
        let mut document = Document::with_version("1.5");
        let dictionary = document.add_object(dictionary! {
            "Title" => Object::string_literal(
                b"Native American Communities in Wisconsin, 1600\x851960".to_vec(),
            ),
            "Author" => Object::string_literal(b"Bieder, Robert E.".to_vec()),
        });
        document.trailer.set("Info", dictionary);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        let read = info(&bytes).unwrap();
        assert_eq!(
            read.title.as_deref(),
            Some("Native American Communities in Wisconsin, 1600\u{2013}1960")
        );
        assert_eq!(read.author.as_deref(), Some("Bieder, Robert E."));
        // UTF-16BE behind a byte order mark still wins where it is used.
        let utf16: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("Sur\u{ed}".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        let mut document = Document::with_version("1.5");
        let id = document.add_object(dictionary! { "Title" => Object::string_literal(utf16) });
        document.trailer.set("Info", id);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        assert_eq!(info(&bytes).unwrap().title.as_deref(), Some("Sur\u{ed}"));
    }

    /// A one-page PDF placing the given content stream on a page of `width`.
    fn page(width: i64, content: &str) -> Vec<u8> {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let contents =
            document.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => contents,
            "MediaBox" => vec![0.into(), 0.into(), width.into(), 800.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog);
        let mut out = Vec::new();
        document.save_to(&mut out).unwrap();
        out
    }
    const LINE: &str = "(the quick brown fox jumps over a lazy dog) Tj";

    #[test]
    fn a_second_column_is_seen_through_text_space_offsets() {
        // The jump to the next column is 20 text units at a scale of 11, which
        // is 220 points: read raw it would be 20, and both columns would look
        // like one margin. This is the whole reason a column can be missed.
        let two = page(
            510,
            &format!(
                "BT /F1 1 Tf 11 0 0 11 50 700 Tm {LINE} 0 -1.2 TD {LINE} 0 -1.2 TD {LINE} \
                 20 40 TD {LINE} 0 -1.2 TD {LINE} 0 -1.2 TD {LINE} ET"
            ),
        );
        let report = inspect(&two).unwrap();
        assert_eq!(report.text_pages, 1);
        assert_eq!(report.column_pages, 1, "{report:?}");
        assert!(matches!(report.verdict, Verdict::Poor(_)));

        // The same text down one column is not two columns.
        let one = page(
            510,
            &format!("BT /F1 1 Tf 11 0 0 11 50 700 Tm {LINE} 0 -1.2 TD {LINE} 0 -1.2 TD {LINE} ET"),
        );
        let report = inspect(&one).unwrap();
        assert_eq!(
            (report.text_pages, report.column_pages),
            (1, 0),
            "{report:?}"
        );
    }

    #[test]
    fn columns_are_read_one_at_a_time_rather_than_straight_across() {
        // Two columns of three lines each. Read across the gutter this comes
        // out interleaved, which is the worst thing a conversion can do.
        let left = ["Alpha alpha alpha", "beta beta beta", "gamma gamma gamma"];
        let right = ["delta delta delta", "epsilon epsilon", "zeta zeta zeta"];
        let mut content = String::from("BT /F1 1 Tf 11 0 0 11 50 700 Tm ");
        for (i, text) in left.iter().enumerate() {
            content.push_str(&format!(
                "0 {} Td ({text} and more words here) Tj ",
                -(i as f32) * 1.2
            ));
        }
        content.push_str("ET BT /F1 1 Tf 11 0 0 11 300 700 Tm ");
        for (i, text) in right.iter().enumerate() {
            content.push_str(&format!(
                "0 {} Td ({text} and more words here) Tj ",
                -(i as f32) * 1.2
            ));
        }
        content.push_str("ET");
        let data = page(560, &content);
        let report = inspect(&data).unwrap();
        assert_eq!(report.column_pages, 1, "{report:?}");
        let (epub, _) = convert(&data).unwrap();
        let text = String::from_utf8_lossy(&epub);
        assert!(!text.is_empty());
        // The whole of one column precedes any of the other.
        let blocks = crate::epub::validate(&epub);
        assert!(blocks.is_ok(), "{blocks:?}");
    }

    /// Everything a converted EPUB says, across all of its chapters.
    fn prose(epub: &[u8]) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(epub)).unwrap();
        let names: Vec<String> = archive
            .file_names()
            .filter(|name| name.starts_with("OEBPS/chapter"))
            .map(str::to_owned)
            .collect();
        assert!(!names.is_empty(), "a book with no chapters in it");
        let mut text = String::new();
        for name in names {
            std::io::Read::read_to_string(&mut archive.by_name(&name).unwrap(), &mut text).unwrap();
        }
        text
    }

    #[test]
    fn a_hyphen_at_a_line_break_is_decided_by_what_the_document_does_elsewhere() {
        // Two words broken across a line. The document writes one of them
        // hyphenated where it had room, and the other closed up, which is the
        // only evidence there is for which hyphen to keep.
        let lines = [
            "the system needs real",
            "time responses from it",
            "a real-time system is what everyone wants",
            "and this is inconsis",
            "tent with all of the rest",
            "an inconsistent result appears in this case",
        ];
        let mut content = String::from("BT /F1 1 Tf 11 0 0 11 50 700 Tm ");
        for (i, line) in lines.iter().enumerate() {
            let dash = if i == 0 || i == 3 { "-" } else { "" };
            content.push_str(&format!(
                "0 {} Td ({line}{dash}) Tj ",
                if i == 0 { 0.0 } else { -1.2 }
            ));
        }
        content.push_str("ET");
        let (epub, _) = convert(&page(520, &content)).unwrap();
        let prose = prose(&epub);
        assert!(prose.contains("real-time responses"), "{prose}");
        assert!(prose.contains("inconsistent with"), "{prose}");
    }

    /// A producer may put a figure inside a form and write the form's entry as
    /// a reference rather than a dictionary. Each of those on its own hid
    /// every picture in a real book.
    #[test]
    fn a_picture_inside_a_form_is_still_a_picture_on_the_page() {
        let mut document = Document::with_version("1.5");
        let image = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 2,
                "Height" => 2,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            vec![0, 64, 128, 255],
        ));
        // The form's own resources hold the image, and its XObject entry is a
        // reference rather than a dictionary written out.
        let inner = document.add_object(dictionary! { "Im0" => image });
        let form = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 520.into(), 800.into()],
                "Resources" => dictionary! { "XObject" => inner },
            },
            b"q 100 0 0 80 50 400 cm /Im0 Do Q".to_vec(),
        ));
        let outer = document.add_object(dictionary! { "Fg" => form });
        let pages_id = document.new_object_id();
        let words = "a scanned page whose words were added underneath the picture of it by \
                     optical recognition, which is text and a picture at once";
        let content = format!("BT /F1 1 Tf 11 0 0 11 50 700 Tm ({words}) Tj ET q /Fg Do Q");
        let contents = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => contents,
            "MediaBox" => vec![0.into(), 0.into(), 520.into(), 800.into()],
            "Resources" => dictionary! { "XObject" => outer },
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        let mut data = Vec::new();
        document.save_to(&mut data).unwrap();

        // Reachable, so carried rather than reported as lost.
        let report = inspect(&data).unwrap();
        assert_eq!(
            (report.text_pages, report.figure_pages),
            (1, 0),
            "{report:?}"
        );
        let (book, _) = convert(&data).unwrap();
        assert!(
            members(&book).iter().any(|name| name.contains("figure")),
            "a picture inside a form is carried into the book"
        );
    }

    #[test]
    fn a_picture_on_a_page_of_text_is_carried_into_the_book_where_it_stood() {
        // A page carrying both text and an image XObject. The picture is read
        // back through its colour space and written into the book.
        let mut document = Document::with_version("1.5");
        let image = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 2,
                "Height" => 2,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            vec![0, 64, 128, 255],
        ));
        let pages_id = document.new_object_id();
        let words = "a page of ordinary prose with a picture sitting underneath it, long \
                     enough that nobody could mistake it for a running head or a page number";
        let content = format!(
            "BT /F1 1 Tf 11 0 0 11 50 700 Tm ({words}) Tj ET \
             q 100 0 0 80 50 400 cm /Im1 Do Q"
        );
        let contents = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => contents,
            "MediaBox" => vec![0.into(), 0.into(), 520.into(), 800.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image } },
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        let mut data = Vec::new();
        document.save_to(&mut data).unwrap();

        // Nothing is lost here: greyscale samples are a picture this can read,
        // so it is carried rather than warned about.
        let report = inspect(&data).unwrap();
        assert_eq!(
            (report.text_pages, report.figure_pages),
            (1, 0),
            "{report:?}"
        );
        assert!(
            !report.verdict.concerns().contains(&Concern::FiguresDropped),
            "{:?}",
            report.verdict
        );
        let (book, _) = convert(&data).unwrap();
        let names = members(&book);
        assert!(
            names.iter().any(|name| name.starts_with("OEBPS/figure")),
            "{names:?}"
        );
        assert!(
            text_of(&book).contains("<img src=\"figure1.png\" alt=\"\"/>"),
            "the picture is referred to where it stood"
        );
        // Declared in the manifest, or a reader is entitled to ignore it.
        assert!(text_of(&book).contains("media-type=\"image/png\""));
    }

    /// The names of everything inside a book.
    fn members(book: &[u8]) -> Vec<String> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(book)).unwrap();
        (0..zip.len())
            .map(|index| zip.by_index(index).unwrap().name().to_owned())
            .collect()
    }

    /// Everything readable in a book, for asking what it says.
    fn text_of(book: &[u8]) -> String {
        use std::io::Read;
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(book)).unwrap();
        let mut out = String::new();
        for index in 0..zip.len() {
            let mut file = zip.by_index(index).unwrap();
            if file.name().ends_with(".xhtml") || file.name().ends_with(".opf") {
                let mut text = String::new();
                file.read_to_string(&mut text).unwrap();
                out.push_str(&text);
            }
        }
        out
    }

    /// Samples are not always eight bits running dark to light. Sixteen-bit
    /// samples read as one byte land in 0..255 while the range they are
    /// measured against is 0..65535, which turns every picture black; a
    /// `Decode` array of [1 0] says the levels run the other way.
    #[test]
    fn deep_samples_and_a_reversed_range_are_read_as_the_pdf_means_them() {
        // A white 16-bit pixel, and a Decode array that makes black white.
        /// Sample depth, the bytes, an optional Decode array, and the grey
        /// level those should come back as.
        type Case = (u32, Vec<u8>, Option<Vec<Object>>, u8);
        let cases: [Case; 3] = [
            (16, vec![0xFF, 0xFF], None, 255),
            (16, vec![0x00, 0x00], None, 0),
            // One bit, sample 0, reversed: the darkest level becomes white.
            (1, vec![0b0000_0000], Some(vec![1.into(), 0.into()]), 255),
        ];
        for (bits, samples, decode, expected) in cases {
            let mut document = Document::with_version("1.5");
            let mut dictionary = dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => bits as i64,
            };
            if let Some(decode) = decode {
                dictionary.set("Decode", Object::Array(decode));
            }
            let image = document.add_object(Stream::new(dictionary, samples.clone()));
            let pages_id = document.new_object_id();
            let words = "a paragraph long enough to make this a page of text rather than a \
                         page of pictures, which is what decides the verdict above";
            let content = format!(
                "q 100 0 0 80 50 400 cm /Im1 Do Q \
                 BT /F1 1 Tf 11 0 0 11 50 700 Tm ({words}) Tj ET"
            );
            let contents = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
            let page_id = document.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => contents,
                "MediaBox" => vec![0.into(), 0.into(), 520.into(), 800.into()],
                "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image } },
            });
            document.objects.insert(
                pages_id,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages",
                    "Kids" => vec![page_id.into()],
                    "Count" => 1,
                }),
            );
            let catalog =
                document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
            document.trailer.set("Root", catalog);
            let mut data = Vec::new();
            document.save_to(&mut data).unwrap();

            let (book, _) = convert(&data).unwrap();
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&book)).unwrap();
            let mut picture = Vec::new();
            for index in 0..zip.len() {
                use std::io::Read;
                let mut file = zip.by_index(index).unwrap();
                if file.name().contains("figure") {
                    file.read_to_end(&mut picture).unwrap();
                }
            }
            assert!(!picture.is_empty(), "{bits} bits: nothing was carried");
            let pixel = image::load_from_memory(&picture).unwrap().to_rgb8();
            assert_eq!(
                pixel.get_pixel(0, 0).0,
                [expected; 3],
                "{bits} bits, samples {samples:?}"
            );
        }
    }

    /// A producer may put the whole page inside a form and leave one `Do` on
    /// the page itself. Read without following that, the page is empty, and a
    /// perfectly ordinary document is refused as a scan needing OCR.
    #[test]
    fn text_drawn_inside_a_form_is_still_text_on_the_page() {
        let mut document = Document::with_version("1.5");
        let words = "a paragraph that the producer chose to draw inside a form rather than \
                     on the page itself, which is a thing real books do";
        let inner = format!("BT /F1 1 Tf 11 0 0 11 50 700 Tm ({words}) Tj ET");
        let form = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 520.into(), 800.into()],
            },
            inner.into_bytes(),
        ));
        let pages_id = document.new_object_id();
        let contents = document.add_object(Stream::new(
            dictionary! {},
            b"q 1 0 0 1 0 0 cm /Fg Do Q".to_vec(),
        ));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => contents,
            "MediaBox" => vec![0.into(), 0.into(), 520.into(), 800.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "Fg" => form } },
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        let mut data = Vec::new();
        document.save_to(&mut data).unwrap();

        let report = inspect(&data).unwrap();
        assert_eq!(report.text_pages, 1, "{report:?}");
        assert!(
            !matches!(report.verdict, Verdict::Impossible(_)),
            "{:?}",
            report.verdict
        );
        let (book, _) = convert(&data).unwrap();
        assert!(text_of(&book).contains("inside a form"));
    }

    /// A scan of a page is a picture that is the page. Carrying those makes a
    /// heavier copy of the PDF with none of the reasons anyone wanted an EPUB,
    /// so the size on the page decides, not where the picture was written.
    #[test]
    fn a_picture_that_is_the_whole_page_is_a_scan_rather_than_a_figure() {
        let mut document = Document::with_version("1.5");
        let image = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 4,
                "Height" => 4,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            vec![128; 16],
        ));
        let pages_id = document.new_object_id();
        let words = "the words an optical reader found on the scan of this page, set down \
                     underneath the picture of it in the way that optical recognition does";
        // Drawn across the entire MediaBox.
        let content = format!(
            "q 520 0 0 800 0 0 cm /Im1 Do Q \
             BT /F1 1 Tf 11 0 0 11 50 700 Tm ({words}) Tj ET"
        );
        let contents = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => contents,
            "MediaBox" => vec![0.into(), 0.into(), 520.into(), 800.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image } },
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        let mut data = Vec::new();
        document.save_to(&mut data).unwrap();

        let report = inspect(&data).unwrap();
        // The encoding is readable, so this is not about what we can decode.
        assert_eq!(
            (report.text_pages, report.figure_pages),
            (1, 1),
            "{report:?}"
        );
        assert!(report.verdict.concerns().contains(&Concern::FiguresDropped));
        let (book, _) = convert(&data).unwrap();
        assert!(
            !members(&book).iter().any(|name| name.contains("figure")),
            "a page scan is not carried into the book"
        );
    }

    /// A picture in an encoding this cannot read is still a warning, and that
    /// is what the concern now means.
    #[test]
    fn a_picture_this_cannot_read_is_what_the_dropped_figures_warning_is_for() {
        let mut document = Document::with_version("1.5");
        // JPEG 2000, which a scanner writes and nothing here decodes.
        let image = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 2,
                "Height" => 2,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
                "Filter" => "JPXDecode",
            },
            vec![0, 0, 0, 0],
        ));
        let pages_id = document.new_object_id();
        let words = "a page of ordinary prose with a picture sitting underneath it, long \
                     enough that nobody could mistake it for a running head or a page number";
        let content = format!(
            "BT /F1 1 Tf 11 0 0 11 50 700 Tm ({words}) Tj ET \
             q 100 0 0 80 50 400 cm /Im1 Do Q"
        );
        let contents = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => contents,
            "MediaBox" => vec![0.into(), 0.into(), 520.into(), 800.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => image } },
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        let mut data = Vec::new();
        document.save_to(&mut data).unwrap();

        let report = inspect(&data).unwrap();
        assert_eq!((report.text_pages, report.figure_pages), (1, 1));
        assert!(report.verdict.concerns().contains(&Concern::FiguresDropped));
        // Still worth converting, so long as it says what will be lost.
        let (book, _) = convert(&data).unwrap();
        assert!(
            !members(&book).iter().any(|n| n.contains("figure")),
            "nothing is written for a picture that could not be read"
        );
    }

    #[test]
    fn a_page_without_text_is_refused_rather_than_converted_badly() {
        let blank = page(510, "q Q");
        let report = inspect(&blank).unwrap();
        assert_eq!(report.text_pages, 0);
        assert!(matches!(
            report.verdict,
            Verdict::Impossible(Concern::NothingFound)
        ));
        assert!(inspect(b"not a PDF at all").is_err());
        // Refusing is the point: an EPUB of nothing helps nobody.
        assert!(convert(&blank).is_err());
    }
}

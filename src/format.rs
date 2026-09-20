//! What kind of book a file is.
//!
//! EPUB is the format Crossload works on: it is unpacked, checked, optimized
//! for the reader's screen and repacked. PDF and CBZ travel byte for byte. They
//! are discovered, identified, copied and verified like any other book, and
//! nothing here ever rewrites one.
use crate::epub;
use anyhow::{ensure, Context, Result};
use std::{io::Cursor, path::Path};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Epub,
    Pdf,
    Cbz,
}
impl Format {
    /// Every format Crossload will pick up, in the order a reader meets them.
    pub const ALL: [Self; 3] = [Self::Epub, Self::Pdf, Self::Cbz];
    pub fn label(self) -> &'static str {
        match self {
            Self::Epub => "EPUB",
            Self::Pdf => "PDF",
            Self::Cbz => "CBZ",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Pdf => "pdf",
            Self::Cbz => "cbz",
        }
    }
    /// Whether Crossload rebuilds this format. Only an EPUB is ever taken apart
    /// and put back together; the rest are copied exactly as they were found,
    /// so image optimization and author-folder renaming do not apply to them.
    pub fn rewritten(self) -> bool {
        self == Self::Epub
    }
    /// Whether the reader's library will list a file of this format.
    ///
    /// CrossPoint answers this itself: every entry in `/api/files` carries an
    /// `isEpub` flag, and it is false for everything else. A PDF copied to an
    /// X4 sits on its storage and is never shown, so putting one there is not
    /// a transfer, it is litter.
    pub fn shown_on_reader(self) -> bool {
        self == Self::Epub
    }
    /// The format a name claims. The contents still have to agree: `validate`
    /// is what decides whether the claim holds.
    pub fn of(name: &str) -> Option<Self> {
        let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
        Self::ALL.into_iter().find(|f| f.extension() == extension)
    }
    pub fn of_path(path: &Path) -> Option<Self> {
        Self::of(path.file_name()?.to_str()?)
    }
}

fn image(name: &str) -> bool {
    matches!(
        name.rsplit_once('.')
            .unwrap_or_default()
            .1
            .to_ascii_lowercase()
            .as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "avif"
    )
}

/// Confirm a file really is what its name claims. An EPUB gets the structural
/// validation it always had, because it is about to be taken apart; the others
/// only have to be whole, because nothing will touch their insides.
pub fn validate(format: Format, data: &[u8]) -> Result<()> {
    match format {
        Format::Epub => {
            epub::validate(data)?;
            epub::font_metadata(data)?;
        }
        Format::Pdf => {
            // A header is allowed to sit behind a preamble, and the trailer at
            // the very end is how a half-finished download gives itself away.
            let head = &data[..data.len().min(1024)];
            ensure!(
                head.windows(5).any(|w| w == b"%PDF-"),
                "Not a PDF: no PDF header in the first kilobyte"
            );
            let tail = &data[data.len().saturating_sub(4096)..];
            ensure!(
                tail.windows(5).any(|w| w == b"%%EOF"),
                "PDF is incomplete: it ends without an end-of-file marker"
            );
        }
        Format::Cbz => {
            epub::inspect(data).context("Not a readable CBZ")?;
            let archive = zip::ZipArchive::new(Cursor::new(data))?;
            ensure!(
                archive.file_names().any(image),
                "CBZ holds no pages: no images inside it"
            );
        }
    }
    Ok(())
}

/// The title to show for a book whose format carries no metadata: what the file
/// is called, which is the only honest answer available without guessing.
pub fn name_title(source: &str) -> String {
    let name = source
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source)
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem).trim();
    if stem.is_empty() {
        "Untitled".to_owned()
    } else {
        stem.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zipped(names: &[&str]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for name in names {
            writer
                .start_file::<_, ()>(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"page").unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn a_name_claims_a_format_and_the_contents_have_to_agree() {
        assert_eq!(Format::of("Dune.EPUB"), Some(Format::Epub));
        assert_eq!(Format::of("paper.v2.pdf"), Some(Format::Pdf));
        assert_eq!(Format::of("comic.cbz"), Some(Format::Cbz));
        // A format we do not carry, and a file with nothing to claim.
        assert_eq!(Format::of("comic.cbr"), None);
        assert_eq!(Format::of("README"), None);

        let mut pdf = b"%PDF-1.7\nbody".to_vec();
        assert!(validate(Format::Pdf, &pdf).is_err(), "truncated");
        pdf.extend_from_slice(b"\ntrailer\n%%EOF\n");
        assert!(validate(Format::Pdf, &pdf).is_ok());
        assert!(validate(Format::Pdf, &zipped(&["001.jpg"])).is_err());

        assert!(validate(Format::Cbz, &zipped(&["001.jpg", "002.png"])).is_ok());
        // A ZIP of anything else is not a comic, and neither is a PDF.
        assert!(validate(Format::Cbz, &zipped(&["notes.txt"])).is_err());
        assert!(validate(Format::Cbz, &pdf).is_err());
    }

    #[test]
    fn a_format_without_metadata_is_titled_by_its_file() {
        assert_eq!(name_title("/books/Some Paper.pdf"), "Some Paper");
        assert_eq!(name_title("Comics/Vol 1.cbz"), "Vol 1");
        assert_eq!(name_title("/x/archive.tar.gz"), "archive.tar");
        assert_eq!(name_title("/x/.pdf"), "Untitled");
    }
}

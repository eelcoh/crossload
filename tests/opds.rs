//! Reading OPDS feeds, against the shapes real catalogues actually serve.
//!
//! The fixtures here are trimmed from live feeds: the DPLA Palace Bookshelf,
//! Boston Public Library through the Palace Project, and Project Gutenberg.
//! Nothing in this file reaches the network.
use crossload::format::Format;
use crossload::opds::{self, Delivery, Kind};
use std::io::{Cursor, Write};
use url::Url;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

fn base() -> Url {
    Url::parse("https://dpla.thepalaceproject.org/bookshelf/").unwrap()
}

fn feed(entries: &str) -> String {
    format!(
        r#"<feed xmlns="http://www.w3.org/2005/Atom"
                xmlns:opds="http://opds-spec.org/2010/catalog"
                xmlns:dcterms="http://purl.org/dc/terms/">
             <title>Palace Bookshelf</title>
             <link rel="search" type="application/opensearchdescription+xml" href="search/"/>
             <link rel="next" href="groups/?after=10"/>
             {entries}
           </feed>"#
    )
}

/// Palace Bookshelf: open access, straight to a file, no card and no DRM.
#[test]
fn an_open_access_entry_is_a_book_that_can_be_taken_as_it_is() {
    let parsed = opds::parse(
        &feed(
            r#"<entry>
                 <title>The Last Egyptian : A Romance of the Nile</title>
                 <id>urn:uuid:123</id>
                 <author><name>L. Frank (Lyman Frank) Baum</name></author>
                 <dcterms:language>en</dcterms:language>
                 <link href="works/35897/fulfill/15" rel="http://opds-spec.org/acquisition/open-access"
                       type="text/html;profile=http://librarysimplified.org/terms/profiles/streaming-media"/>
                 <link href="works/35897/fulfill/7" rel="http://opds-spec.org/acquisition/open-access"
                       type="application/epub+zip">
                   <opds:availability status="available"/>
                 </link>
               </entry>"#,
        ),
        &base(),
    )
    .unwrap();
    assert_eq!(parsed.title, "Palace Bookshelf");
    let entry = parsed.books().next().unwrap();
    assert_eq!(entry.author(), "L. Frank (Lyman Frank) Baum");
    // The streaming link comes first in the feed and is still not the offer
    // chosen, because it is not a file.
    let offer = entry.best().unwrap();
    assert_eq!(offer.kind, Kind::OpenAccess);
    assert_eq!(offer.delivery(), Delivery::Book(Format::Epub));
    assert_eq!(offer.availability.as_deref(), Some("available"));
    // Relative hrefs resolve against the feed they came from.
    assert_eq!(
        offer.href,
        "https://dpla.thepalaceproject.org/bookshelf/works/35897/fulfill/7"
    );
    assert!(entry.deliverable());
    assert_eq!(
        parsed.next.as_deref(),
        Some("https://dpla.thepalaceproject.org/bookshelf/groups/?after=10")
    );
    assert_eq!(
        parsed.search.as_deref(),
        Some("https://dpla.thepalaceproject.org/bookshelf/search/")
    );
}

/// A lending library declares the DRM chain before the loan, and that is the
/// only place it says an ACSM is coming.
#[test]
fn a_borrow_link_is_read_through_its_chain_to_the_acsm_underneath() {
    let parsed = opds::parse(
        &feed(
            r#"<entry>
                 <title>Running Away: A Memoir</title>
                 <author><name>Robert Andrew Powell</name></author>
                 <link href="works/ISBN/9781477867617/borrow"
                       rel="http://opds-spec.org/acquisition/borrow"
                       type="application/atom+xml;type=entry;profile=opds-catalog">
                   <opds:indirectAcquisition type="application/vnd.adobe.adept+xml">
                     <opds:indirectAcquisition type="application/epub+zip"/>
                   </opds:indirectAcquisition>
                   <opds:indirectAcquisition type="application/atom+xml;type=entry;profile=opds-catalog">
                     <opds:indirectAcquisition type="text/html;profile=&quot;streaming-media&quot;"/>
                   </opds:indirectAcquisition>
                   <opds:availability status="available"/>
                 </link>
               </entry>"#,
        ),
        &base(),
    )
    .unwrap();
    let offer = parsed.books().next().unwrap().best().unwrap();
    assert!(offer.borrowable());
    // The link's own type is an entry document. Reading that as the delivery
    // would call every loan in every library an OPDS file.
    assert_eq!(offer.delivery(), Delivery::Acsm(Format::Epub));
    assert!(offer.delivery().available());
    assert_eq!(offer.delivery().label(), "EPUB via Adobe DRM");
}

/// Half of what a US library lends is something this program cannot open.
/// Each is refused by name rather than hidden or attempted.
#[test]
fn what_cannot_be_delivered_is_named_rather_than_guessed_at() {
    let cases = [
        (
            r#"<opds:indirectAcquisition type="application/vnd.readium.lcp.license.v1.0+json">
                 <opds:indirectAcquisition type="application/epub+zip"/>
               </opds:indirectAcquisition>"#,
            "Readium LCP, which this program cannot open",
        ),
        (
            r#"<opds:indirectAcquisition type="application/vnd.overdrive.circulation.api+json;profile=audiobook"/>"#,
            "an audiobook",
        ),
        (
            r#"<opds:indirectAcquisition type="application/atom+xml;type=entry;profile=opds-catalog">
                 <opds:indirectAcquisition type="text/html;profile=&quot;streaming-media&quot;"/>
               </opds:indirectAcquisition>"#,
            "a reader in a browser, not a file",
        ),
        (
            r#"<opds:indirectAcquisition type="application/vnd.librarysimplified.bearer-token+json"/>"#,
            "a bearer-token handover",
        ),
    ];
    for (chain, reason) in cases {
        let parsed = opds::parse(
            &feed(&format!(
                r#"<entry><title>Lent</title>
                     <link href="borrow" rel="http://opds-spec.org/acquisition/borrow"
                           type="application/atom+xml;type=entry;profile=opds-catalog">
                       {chain}
                     </link>
                   </entry>"#
            )),
            &base(),
        )
        .unwrap();
        let entry = parsed.books().next().unwrap();
        assert_eq!(
            entry.best().unwrap().delivery(),
            Delivery::Unsupported(reason),
            "chain {chain}"
        );
        assert!(!entry.deliverable());
    }
}

/// An LCP loan and an Adobe loan of the same book: take the one we can open.
#[test]
fn a_deliverable_offer_is_preferred_over_one_that_would_only_be_refused() {
    let parsed = opds::parse(
        &feed(
            r#"<entry><title>Two ways</title>
                 <link href="lcp" rel="http://opds-spec.org/acquisition/borrow" type="application/vnd.readium.lcp.license.v1.0+json"/>
                 <link href="free" rel="http://opds-spec.org/acquisition/open-access" type="application/epub+zip"/>
               </entry>"#,
        ),
        &base(),
    )
    .unwrap();
    let offer = parsed.books().next().unwrap().best().unwrap();
    assert_eq!(offer.kind, Kind::OpenAccess);
    assert_eq!(offer.delivery(), Delivery::Book(Format::Epub));
}

/// Gutenberg answers a search with navigation entries, not books. A client
/// that assumed otherwise would report an empty catalogue.
#[test]
fn navigation_entries_are_kept_apart_from_books_and_carry_their_own_feed() {
    let parsed = opds::parse(
        r#"<feed xmlns="http://www.w3.org/2005/Atom">
             <title>Search results</title>
             <entry>
               <title>Pride and Prejudice</title>
               <link href="/ebooks/1342.opds" rel="subsection"
                     type="application/atom+xml;profile=opds-catalog"/>
             </entry>
             <entry>
               <title>EPUB (no images, older E-readers)</title>
               <link href="/ebooks/1342.epub3.images" rel="http://opds-spec.org/acquisition"
                     type="application/epub+zip" length="558381"/>
             </entry>
           </feed>"#,
        &Url::parse("https://www.gutenberg.org/ebooks/search/?query=austen").unwrap(),
    )
    .unwrap();
    assert_eq!(parsed.books().count(), 1);
    let nav = parsed.navigation().next().unwrap();
    assert_eq!(nav.title, "Pride and Prejudice");
    assert_eq!(
        nav.subsection.as_deref(),
        Some("https://www.gutenberg.org/ebooks/1342.opds")
    );
    // Gutenberg gives a length and the Palace Project does not, so it can be
    // used when present and never required.
    let offer = parsed.books().next().unwrap().best().unwrap();
    assert_eq!(offer.kind, Kind::Direct);
    assert_eq!(offer.length, Some(558_381));
}

/// Asking a catalogue's website for a feed is the ordinary mistake, because
/// the two addresses differ and the website answers with a page.
#[test]
fn a_web_page_where_a_feed_was_expected_says_so_rather_than_quoting_an_xml_parser() {
    let gutenberg = Url::parse("https://www.gutenberg.org/ebooks/search/?query=austen").unwrap();
    for page in [
        "<!DOCTYPE html>\n<html lang=\"en\"><head><title>Search</title></head></html>",
        "\u{feff}\n  <HTML><BODY>Sign in</BODY></HTML>",
    ] {
        let error = opds::parse(page, &gutenberg).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("is a web page, not an OPDS feed"),
            "{message}"
        );
        assert!(message.contains("search.opds"), "{message}");
    }
}

/// XML that is not a feed is a different mistake and gets a different answer.
#[test]
fn a_document_that_is_not_a_feed_is_refused_by_saying_what_it_was() {
    let error = opds::parse(
        "<opensearchdescription><ShortName>Search</ShortName></opensearchdescription>",
        &base(),
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("Not an OPDS feed"), "{message}");
    assert!(message.contains("opensearchdescription"), "{message}");
}

fn epub(title: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();
    let options = SimpleFileOptions::default();
    zip.start_file("META-INF/container.xml", options).unwrap();
    zip.write_all(
        br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
              <rootfiles><rootfile full-path="content.opf"
                media-type="application/oebps-package+xml"/></rootfiles>
            </container>"#,
    )
    .unwrap();
    zip.start_file("content.opf", options).unwrap();
    zip.write_all(
        format!(
            r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="i">
                 <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                   <dc:identifier id="i">urn:uuid:1</dc:identifier>
                   <dc:title>{title}</dc:title>
                   <dc:creator>A. Writer</dc:creator>
                   <dc:language>en</dc:language>
                 </metadata>
                 <manifest><item id="t" href="t.xhtml" media-type="application/xhtml+xml"/></manifest>
                 <spine><itemref idref="t"/></spine>
               </package>"#
        )
        .as_bytes(),
    )
    .unwrap();
    zip.start_file("t.xhtml", options).unwrap();
    zip.write_all(b"<html xmlns='http://www.w3.org/1999/xhtml'><body><p>Text</p></body></html>")
        .unwrap();
    zip.finish().unwrap().into_inner()
}

/// A downloaded book is named from what it says about itself, not from the
/// catalogue's own spelling, because that is what the rest of the program
/// will read back.
#[test]
fn a_download_is_named_by_its_own_metadata_and_downloading_twice_is_not_an_error() {
    let output = tempfile::tempdir().unwrap();
    let data = epub("A Romance of the Nile");
    let first = opds::publish(&data, Format::Epub, output.path()).unwrap();
    let name = first.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("A Romance of the Nile ["), "{name}");
    assert!(name.ends_with(".epub"), "{name}");
    // The digest is in the name, so the same book is the same file.
    let again = opds::publish(&data, Format::Epub, output.path()).unwrap();
    assert_eq!(first, again);
    assert_eq!(std::fs::read(&first).unwrap(), data);
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 1);
    // A different book does not collide with it.
    let other = opds::publish(&epub("A Romance of the Nile"), Format::Epub, output.path());
    assert_eq!(other.unwrap(), first);
}

/// A catalogue that promises an EPUB and sends a web page is caught here
/// rather than saved into the library.
#[test]
fn something_that_is_not_the_promised_book_is_never_published() {
    let output = tempfile::tempdir().unwrap();
    let error = opds::publish(b"<html>Sign in</html>", Format::Epub, output.path()).unwrap_err();
    assert!(
        format!("{error:#}").contains("did not deliver a readable EPUB"),
        "{error:#}"
    );
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
}

//! OPDS 1.2 catalogue client.
//!
//! A catalogue is not a fourth `Place`: nothing syncs to it, and nothing here
//! is ever written back. Books arrive by being downloaded into the import
//! folder, where the rest of the program already finds them.
//!
//! Only reading is implemented. What a feed offers is reported exactly as the
//! feed states it, including the parts we cannot open, because a catalogue
//! full of Readium LCP is a catalogue this program cannot deliver and saying
//! so is more useful than hiding the entry.
use crate::format::Format;
use anyhow::{ensure, Context, Result};
use curl::easy::{Easy, List};
use std::time::Duration;
use url::Url;

/// The Digital Public Library of America's open collection, served through the
/// Palace Project. It needs no library card, and every book in it is open
/// access, which makes it the one catalogue that can be used as it is found.
pub const BOOKSHELF: &str = "https://dpla.thepalaceproject.org/bookshelf/";

const ATOM: &str = "http://www.w3.org/2005/Atom";
const OPDS: &str = "http://opds-spec.org/2010/catalog";
const DCTERMS: &str = "http://purl.org/dc/terms/";
const OPENSEARCH: &str = "http://a9.com/-/spec/opensearch/1.1/";
const ACQUISITION: &str = "http://opds-spec.org/acquisition";

/// Feeds are text and stay small; a book does not go through this path.
const MAX_FEED: usize = 8 * 1024 * 1024;

/// What a catalogue is willing to do with an entry. OPDS spells these as
/// `rel` suffixes of the acquisition relation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Free to take, with no loan and no DRM.
    OpenAccess,
    /// A loan. Needs a card, and delivers through whatever DRM follows.
    Borrow,
    Buy,
    Sample,
    Subscribe,
    /// `rel="…/acquisition"` with no suffix: a plain download.
    Direct,
}

impl Kind {
    fn of(rel: &str) -> Option<Self> {
        match rel.strip_prefix(ACQUISITION)? {
            "" => Some(Self::Direct),
            "/open-access" => Some(Self::OpenAccess),
            "/borrow" => Some(Self::Borrow),
            "/buy" => Some(Self::Buy),
            "/sample" => Some(Self::Sample),
            "/subscribe" => Some(Self::Subscribe),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAccess => "free",
            Self::Borrow => "borrow",
            Self::Buy => "buy",
            Self::Sample => "sample",
            Self::Subscribe => "subscribe",
            Self::Direct => "download",
        }
    }
}

/// What actually arrives if an offer is followed to its end.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Delivery {
    /// A file this program reads, straight from the link.
    Book(Format),
    /// An Adobe fulfillment token wrapping a book. `adobe.rs` turns one into
    /// an EPUB with an existing activation, so this is deliverable too.
    Acsm(Format),
    /// Understood, and refused with the reason rather than attempted.
    Unsupported(&'static str),
}

impl Delivery {
    pub fn available(&self) -> bool {
        !matches!(self, Self::Unsupported(_))
    }
    pub fn label(&self) -> String {
        match self {
            Self::Book(format) => format.label().to_owned(),
            Self::Acsm(format) => format!("{} via Adobe DRM", format.label()),
            Self::Unsupported(why) => (*why).to_owned(),
        }
    }
}

/// Media types named in a feed, mapped to what this program can do with them.
/// Anything not listed is reported by its own media type rather than guessed
/// at.
fn readable(media_type: &str) -> Option<Format> {
    match base_type(media_type) {
        "application/epub+zip" => Some(Format::Epub),
        "application/pdf" => Some(Format::Pdf),
        "application/vnd.comicbook+zip" | "application/x-cbz" => Some(Format::Cbz),
        _ => None,
    }
}

/// Media types carry parameters, and the ones here matter only for telling
/// streaming HTML apart from HTML. Comparison is on the type itself.
fn base_type(media_type: &str) -> &str {
    media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .trim_end_matches('/')
}

fn refusal(media_type: &str) -> &'static str {
    match base_type(media_type) {
        "application/vnd.readium.lcp.license.v1.0+json" | "application/audiobook+lcp" => {
            "Readium LCP, which this program cannot open"
        }
        "application/vnd.overdrive.circulation.api+json" | "application/audiobook+json" => {
            "an audiobook"
        }
        "text/html" => "a reader in a browser, not a file",
        "application/vnd.librarysimplified.bearer-token+json" => "a bearer-token handover",
        _ => "a format this program does not read",
    }
}

/// One step of a DRM chain, as `opds:indirectAcquisition` nests them. A
/// borrow link says nothing about its own media type; the chain underneath it
/// is where a library declares that borrowing yields an ACSM yielding an EPUB.
#[derive(Clone, Debug)]
pub struct Indirect {
    pub media_type: String,
    pub inner: Vec<Indirect>,
}

#[derive(Clone, Debug)]
pub struct Offer {
    pub kind: Kind,
    pub href: String,
    pub media_type: String,
    /// Free text distinguishing several links of one media type. Catalogues
    /// word it their own way, so it is shown and never matched on.
    pub title: Option<String>,
    /// Only some catalogues give it. Gutenberg does, the Palace Project does
    /// not, so a size can never be required before downloading.
    pub length: Option<u64>,
    pub indirect: Vec<Indirect>,
    pub availability: Option<String>,
}

impl Offer {
    /// A direct link is followed as it stands. Otherwise the chain decides,
    /// and only a chain we can walk to the end counts as deliverable.
    pub fn delivery(&self) -> Delivery {
        if let Some(format) = readable(&self.media_type) {
            return Delivery::Book(format);
        }
        if base_type(&self.media_type) == "application/vnd.adobe.adept+xml" {
            return Delivery::Acsm(self.inner_format().unwrap_or(Format::Epub));
        }
        best(&self.indirect).unwrap_or_else(|| {
            // Name what is actually on offer at the end of the chain. A borrow
            // link is typed as the entry document that answers it, so
            // reporting its own type would describe every refusal as OPDS.
            Delivery::Unsupported(refusal(leaf(&self.indirect).unwrap_or(&self.media_type)))
        })
    }

    fn inner_format(&self) -> Option<Format> {
        self.indirect
            .iter()
            .find_map(|step| readable(&step.media_type))
    }

    pub fn borrowable(&self) -> bool {
        self.kind == Kind::Borrow
    }
}

/// Walk the declared chain and take the best ending it reaches. An ACSM is
/// preferred over nothing but never over a file we can take directly.
fn best(steps: &[Indirect]) -> Option<Delivery> {
    let mut acsm = None;
    for step in steps {
        if let Some(format) = readable(&step.media_type) {
            return Some(Delivery::Book(format));
        }
        if base_type(&step.media_type) == "application/vnd.adobe.adept+xml" {
            let format = step
                .inner
                .iter()
                .find_map(|inner| readable(&inner.media_type))
                .unwrap_or(Format::Epub);
            acsm.get_or_insert(Delivery::Acsm(format));
            continue;
        }
        // An entry document is the borrow step itself, so keep descending.
        if base_type(&step.media_type) == "application/atom+xml" {
            match best(&step.inner) {
                Some(Delivery::Book(format)) => return Some(Delivery::Book(format)),
                Some(found @ Delivery::Acsm(_)) => {
                    acsm.get_or_insert(found);
                }
                _ => {}
            }
        }
    }
    acsm
}

/// The first type at the end of a chain, skipping the entry documents that
/// only carry the chain onwards.
fn leaf(steps: &[Indirect]) -> Option<&str> {
    steps.iter().find_map(|step| {
        if base_type(&step.media_type) == "application/atom+xml" {
            leaf(&step.inner)
        } else {
            Some(step.media_type.as_str())
        }
    })
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub summary: Option<String>,
    pub publisher: Option<String>,
    pub language: Option<String>,
    pub offers: Vec<Offer>,
    /// A navigation entry points at a feed rather than carrying offers.
    /// Gutenberg's search results are all of these.
    pub subsection: Option<String>,
}

impl Entry {
    /// One line, in the spelling the library uses. An absent author is left
    /// to the importer, which has its own answer for that.
    pub fn author(&self) -> String {
        match self.authors.len() {
            0 => "Unknown author".to_owned(),
            1 => self.authors[0].clone(),
            _ => format!("{} and others", self.authors[0]),
        }
    }

    /// The offer to act on. Free beats borrowed, and anything deliverable
    /// beats an offer we would only have to refuse afterwards.
    pub fn best(&self) -> Option<&Offer> {
        let rank = |offer: &Offer| {
            let delivery = match offer.delivery() {
                Delivery::Book(_) => 0,
                Delivery::Acsm(_) => 1,
                Delivery::Unsupported(_) => 2,
            };
            let kind = match offer.kind {
                Kind::OpenAccess | Kind::Direct => 0,
                Kind::Borrow => 1,
                Kind::Sample => 2,
                Kind::Buy | Kind::Subscribe => 3,
            };
            (delivery, kind)
        };
        self.offers.iter().min_by_key(|offer| rank(offer))
    }

    pub fn deliverable(&self) -> bool {
        self.best()
            .is_some_and(|offer| offer.delivery().available())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Feed {
    pub title: String,
    pub entries: Vec<Entry>,
    /// Paging, as `rel="next"`.
    pub next: Option<String>,
    /// An OpenSearch description document, not a query.
    pub search: Option<String>,
    /// Present when the catalogue expects a card. Its absence is what makes
    /// Palace Bookshelf usable without one.
    pub authentication: Option<String>,
}

impl Feed {
    pub fn books(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.offers.is_empty())
    }
    pub fn navigation(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(|e| e.offers.is_empty() && e.subsection.is_some())
    }
}

/// Fetch and parse one feed.
pub fn fetch(url: &str) -> Result<Feed> {
    let base = Url::parse(url).with_context(|| format!("Invalid catalogue address {url}"))?;
    ensure!(
        matches!(base.scheme(), "http" | "https"),
        "A catalogue address must be http or https"
    );
    let body = get(
        base.as_str(),
        "application/atom+xml;profile=opds-catalog, application/xml;q=0.9, */*;q=0.1",
        MAX_FEED,
    )?;
    let text = std::str::from_utf8(&body).context("Catalogue did not return text")?;
    parse(text, &base)
}

fn looks_like_a_web_page(text: &str) -> bool {
    let start = text
        .trim_start()
        .trim_start_matches('\u{feff}')
        .trim_start();
    let head: String = start.chars().take(200).collect::<String>().to_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

/// Parse a feed whose bytes are already in hand, resolving relative links
/// against the address it came from.
pub fn parse(text: &str, base: &Url) -> Result<Feed> {
    // A catalogue's web address and its feed address are usually different, and
    // asking for the wrong one returns a page rather than an error. Saying so
    // is more use than handing the HTML to an XML parser and repeating its
    // complaint about a <head> tag.
    ensure!(
        !looks_like_a_web_page(text),
        "{base} is a web page, not an OPDS feed. Catalogues usually serve the feed \
         at a different address; Gutenberg, for one, uses /ebooks/search.opds/ \
         where its site uses /ebooks/search/"
    );
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .context("Catalogue did not return a parsable Atom feed")?;
    let root = document.root_element();
    ensure!(
        root.has_tag_name((ATOM, "feed")),
        "Not an OPDS feed: the document is <{}>, not an Atom <feed>",
        root.tag_name().name()
    );
    let resolve = |href: &str| base.join(href).map(String::from).ok();
    let mut feed = Feed {
        title: text_of(root, ATOM, "title").unwrap_or_default(),
        ..Feed::default()
    };
    for link in root.children().filter(|n| n.has_tag_name((ATOM, "link"))) {
        let (Some(rel), Some(href)) = (link.attribute("rel"), link.attribute("href")) else {
            continue;
        };
        match rel {
            "next" => feed.next = resolve(href),
            "search" => feed.search = resolve(href),
            "http://opds-spec.org/auth/document" => feed.authentication = resolve(href),
            _ => {}
        }
    }
    feed.entries = root
        .children()
        .filter(|n| n.has_tag_name((ATOM, "entry")))
        .map(|node| entry(node, base))
        .collect();
    Ok(feed)
}

fn entry(node: roxmltree::Node, base: &Url) -> Entry {
    let resolve = |href: &str| base.join(href).map(String::from).ok();
    let mut offers = Vec::new();
    let mut subsection = None;
    for link in node.children().filter(|n| n.has_tag_name((ATOM, "link"))) {
        let (Some(rel), Some(href)) = (link.attribute("rel"), link.attribute("href")) else {
            continue;
        };
        let Some(href) = resolve(href) else { continue };
        if let Some(kind) = Kind::of(rel) {
            offers.push(Offer {
                kind,
                href,
                media_type: link.attribute("type").unwrap_or_default().to_owned(),
                title: link.attribute("title").map(str::to_owned),
                length: link.attribute("length").and_then(|v| v.parse().ok()),
                indirect: indirect(link),
                availability: link
                    .children()
                    .find(|n| n.has_tag_name((OPDS, "availability")))
                    .and_then(|n| n.attribute("status"))
                    .map(str::to_owned),
            });
        } else if rel == "subsection" || rel == "alternate" && subsection.is_none() {
            // Gutenberg links a search result to its per-book feed as
            // subsection; a client that assumed books were one level down
            // would find nothing to download.
            if link
                .attribute("type")
                .is_some_and(|t| t.contains("opds-catalog"))
            {
                subsection = Some(href);
            }
        }
    }
    Entry {
        id: text_of(node, ATOM, "id").unwrap_or_default(),
        title: text_of(node, ATOM, "title").unwrap_or_else(|| "Untitled".to_owned()),
        authors: node
            .children()
            .filter(|n| n.has_tag_name((ATOM, "author")))
            .filter_map(|n| text_of(n, ATOM, "name"))
            .collect(),
        summary: text_of(node, ATOM, "summary"),
        publisher: text_of(node, DCTERMS, "publisher"),
        language: text_of(node, DCTERMS, "language"),
        offers,
        subsection,
    }
}

fn indirect(node: roxmltree::Node) -> Vec<Indirect> {
    node.children()
        .filter(|n| n.has_tag_name((OPDS, "indirectAcquisition")))
        .map(|n| Indirect {
            media_type: n.attribute("type").unwrap_or_default().to_owned(),
            inner: indirect(n),
        })
        .collect()
}

fn text_of(node: roxmltree::Node, ns: &str, tag: &str) -> Option<String> {
    node.children()
        .find(|n| n.has_tag_name((ns, tag)))
        .and_then(|n| n.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Turn a feed's OpenSearch description into a query address.
pub fn search(description: &str, query: &str) -> Result<String> {
    let base = Url::parse(description)?;
    let body = get(
        description,
        "application/opensearchdescription+xml, */*",
        MAX_FEED,
    )?;
    let text = std::str::from_utf8(&body).context("Search description is not text")?;
    let document = roxmltree::Document::parse(text).context("Invalid OpenSearch description")?;
    // Several templates may be offered; the OPDS one is the one that returns a
    // feed rather than a web page.
    let template = document
        .descendants()
        .filter(|n| n.has_tag_name((OPENSEARCH, "Url")))
        .filter_map(|n| Some((n.attribute("type")?, n.attribute("template")?)))
        .find(|(kind, _)| kind.contains("opds-catalog"))
        .map(|(_, template)| template)
        .context("Catalogue offers no OPDS search template")?;
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let filled = template.replace("{searchTerms}", &encoded);
    Ok(base.join(&filled)?.into())
}

/// Download one book. Nothing is parsed here: what the bytes are is decided by
/// the caller, against the format it was promised.
pub fn download(url: &str, limit: usize) -> Result<Vec<u8>> {
    get(url, "*/*", limit)
}

fn get(url: &str, accept: &str, limit: usize) -> Result<Vec<u8>> {
    let mut easy = Easy::new();
    easy.url(url)?;
    // Catalogues redirect: the Palace root goes to its grouped feed, and a
    // fulfillment link goes to wherever the book is actually stored.
    easy.follow_location(true)?;
    easy.max_redirections(8)?;
    easy.connect_timeout(Duration::from_secs(10))?;
    easy.timeout(Duration::from_secs(300))?;
    easy.low_speed_limit(128)?;
    easy.low_speed_time(Duration::from_secs(30))?;
    easy.useragent(concat!("crossload/", env!("CARGO_PKG_VERSION")))?;
    let mut headers = List::new();
    headers.append(&format!("Accept: {accept}"))?;
    easy.http_headers(headers)?;
    let mut response = Vec::new();
    let mut too_large = false;
    let result = {
        let mut transfer = easy.transfer();
        transfer.write_function(|bytes| {
            if bytes.len() > limit.saturating_sub(response.len()) {
                too_large = true;
                return Ok(0);
            }
            response.extend_from_slice(bytes);
            Ok(bytes.len())
        })?;
        transfer.perform()
    };
    ensure!(
        !too_large,
        "Catalogue response is larger than the {} MiB this accepts",
        limit / (1024 * 1024)
    );
    result.with_context(|| format!("Catalogue request failed: {url}"))?;
    let status = easy.response_code()?;
    ensure!(
        status == 200,
        "Catalogue returned HTTP {status} for {url}{}",
        match status {
            401 | 403 => "; this catalogue wants a library card",
            404 => "; the address may be wrong",
            _ => "",
        }
    );
    Ok(response)
}

/// Save a downloaded book into the import folder.
///
/// The catalogue's own title is not trusted for this: what the file says about
/// itself is what the rest of the program will read, so the name comes from
/// the same place. Existing files are never overwritten, as with an ACSM
/// import.
pub fn publish(
    data: &[u8],
    format: Format,
    output: &std::path::Path,
) -> Result<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    crate::format::validate(format, data).with_context(|| {
        format!(
            "The catalogue did not deliver a readable {}",
            format.label()
        )
    })?;
    let described = match format {
        Format::Epub => crate::epub::metadata_from(data)?.and_then(|m| m.title),
        Format::Pdf => crate::pdf::info(data).ok().and_then(|info| info.title),
        Format::Cbz => None,
    };
    std::fs::create_dir_all(output)?;
    let output = output.canonicalize()?;
    let id = format!("{:x}", Sha256::digest(data));
    let name = file_name(
        described.as_deref().unwrap_or("Book"),
        &id,
        format.extension(),
    );
    let target = output.join(&name);
    let mut file = tempfile::NamedTempFile::new_in(&output)?;
    file.write_all(data)?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(&target) {
        Ok(_) => Ok(target),
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
            // The name carries the digest, so an existing file with it is the
            // same bytes. Downloading a book twice is not an error.
            Ok(target)
        }
        Err(e) => Err(e).with_context(|| format!("Cannot save {}", target.display())),
    }
}

fn file_name(title: &str, id: &str, extension: &str) -> String {
    let mut name: String = title
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(80)
        .collect();
    while name.len() > 180 {
        name.pop();
    }
    let name = name.trim_matches([' ', '.']);
    format!(
        "{} [{}].{extension}",
        if name.is_empty() { "Book" } else { name },
        &id[..12]
    )
}

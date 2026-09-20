# OPDS catalogues

Research dates: 2026-09-20 (feeds, NL), 2026-09-20 (US libraries).
Status: investigation, nothing implemented. Findings
below come from fetching real feeds, not from the specification.

## What it is

A syndication format for ebook catalogues. **OPDS 1.2** is Atom XML and is what
every feed reached here actually serves; **2.0** is JSON and was not encountered.
A feed is either *navigation*, whose entries link on to further feeds, or
*acquisition*, whose entries are books carrying links of
`rel="http://opds-spec.org/acquisition"`.

## What was established

**1.2 Atom is what is served.** Project Gutenberg and Cantook Market (formerly
Feedbooks) both return `application/atom+xml` with
`xmlns:opds="http://opds-spec.org/2010/catalog"`. Cantook also declares `odl`
(Open Distribution to Libraries) and `a11y` namespaces, which is where a
library-facing catalogue would describe its loans.

**Entries do not necessarily carry acquisition links.** Gutenberg's search
results are a navigation feed: each entry links to a per-book feed
(`/ebooks/1342.opds`), and only that feed carries the acquisition links. A
client cannot assume books are one level down; it has to follow `subsection`
links until it finds acquisition ones.

**A book offers several acquisition links of the same media type**, distinguished
only by a free-text `title`. *Pride and Prejudice* offers three EPUBs:

| title | bytes |
| --- | --- |
| EPUB (no images, older E-readers) | 558,381 |
| EPUB3 (E-readers incl. Send-to-Kindle) | 24,835,578 |
| EPUB (older E-readers) | 24,846,132 |

Choosing between them is a real decision and cannot be made from the media type.
The wording is per-catalogue, so matching on it is guesswork. For this program
the size is the more useful signal: the 128 MiB ceiling applies to a download as
much as to anything else, and the reader's optimizer shrinks images anyway.

**Size is known before downloading.** Acquisition links carry `length`, which
suits both the 128 MiB ceiling and the room-left check of item 23.

**Paging and search are as expected.** `rel="next"` on Gutenberg's search feed;
`rel="search"` pointing at an OpenSearch description document.

**Authentication is real and not uniform.** Standard Ebooks now answers `401`
with:

```
www-authenticate: Basic realm="Enter your Patrons Circle email address and leave the password empty."
```

HTTP Basic, but with email as the username and an empty password, for donors.
Simple to implement, easy to get wrong by assuming a password is wanted.

## What is still unknown, and it is the important part

**No feed reachable without an account was found that serves an ACSM.** That is
the claim item 32 rests on: that library feeds offer acquisition links of type
`application/vnd.adobe.adept+xml`, which this program already knows how to
fulfil. Gutenberg is public domain and has no DRM to hand out. Cantook's index
returns an empty navigation feed without a library context. Standard Ebooks
wants a patron login.

So the synergy remains plausible and unproven. Settling it needs one feed from a
library that actually lends, with credentials, and an entry inspected for its
acquisition link type. Until then the case for building this rests on an
assumption, and the assumption is the whole case.

**What the library here serves was established, and it is not OPDS.** Dutch
public library lending runs through one national platform rather than per
library, and `onlinebibliotheek.nl/opds` and `/catalog.atom` both answer 404, as
does the local library's own site. The ACSM already in `debug/` names its
operator as `digitaldistribution.cb.nl` — Centraal Boekhuis, which runs the
Adobe Content Server for Dutch retail and library ebooks. That is a web checkout
that hands over a file, not a catalogue a client can walk.

## US libraries: the claim is proven

The unknown above is now settled, in the affirmative, for the United States.

**The Palace Project registry is a real, large deployment.**
`registry.thepalaceproject.org/libraries` answers `200 application/opds+json` —
OPDS **2.0**, the first encountered in the wild — and lists **1457 US
libraries**. Each carries a `http://opds-spec.org/catalog` link to an OPDS 1.2
acquisition feed and a `http://opds-spec.org/auth/document` link. The software
descends from NYPL's Library Simplified, and the registry is run by LYRASIS with
the DPLA.

**Lending libraries declare the DRM chain before you borrow.** Entries there do
not link to files. They carry `rel="http://opds-spec.org/acquisition/borrow"`
pointing at an entry document, and nest `opds:indirectAcquisition` to say what
borrowing will produce. Across Boston Public Library (495 entries) and Los
Angeles Public Library (280):

| indirect acquisition type | count |
| --- | --- |
| `application/epub+zip` | 531 |
| **`application/vnd.adobe.adept+xml`** | **319** |
| `application/vnd.readium.lcp.license.v1.0+json` | 312 |
| `application/vnd.overdrive.circulation.api+json;profile=audiobook` | 242 |
| `application/audiobook+lcp` | 144 |
| `application/pdf` | 13 |

And the ACSM entries nest exactly as hoped:

```xml
<opds:indirectAcquisition type="application/vnd.adobe.adept+xml">
  <opds:indirectAcquisition type="application/epub+zip"/>
</opds:indirectAcquisition>
```

That is the claim item 32 was built on, in real data: borrow → ACSM → EPUB, and
`adobe.rs` already fulfils ACSM with its own ADEPT activation. Readium LCP is
nearly as common and we cannot open it, so a client has to read these types and
say so rather than promise every book.

**Browsing needs no card.** All twelve registry libraries tried — Boston, LA,
Connecticut State Library, several universities — answered `200` unauthenticated,
including availability (`<opds:availability status="available"/>`). A catalogue
view can therefore be built and demonstrated before any credential handling
exists.

**Borrowing does.** Authentication documents declare
`http://opds-spec.org/auth/basic` with labels `Barcode` and `PIN`, alongside
Palace's own `basic-token`, plus a `http://opds-spec.org/shelf` link for loans.
The protocol is easy; storing the credential is the part that needs thought.

**Palace Bookshelf is an open-access fixture.**
`dpla.thepalaceproject.org/bookshelf/` needs no account and is open access
throughout: `rel="http://opds-spec.org/acquisition/open-access"` straight to
`application/epub+zip` (176) and `application/pdf` (46), no DRM elements at all,
100 entries a page. Both a development fixture and a catalogue worth having.

## Where that leaves it

The Dutch finding and the US finding point opposite ways, and both are right.
For the library here, OPDS is nothing: no endpoint exists, and loans arrive as a
CB web checkout ending in a file the import folder already accepts. For a US
library card holder, OPDS is the difference between Adobe Digital Editions plus
Calibre and one tool on Linux.

So this is not a question about the protocol any more. It is a question about
who the program is for. Building it serves other users, not this one.

## If it were built

Client only. Serving our own library would mean becoming a daemon, and the
reader speaks its own File Transfer API rather than OPDS.

The integration stays small: a download into the import folder is discovered,
hashed, grouped and syncable with no new code, and an ACSM dropped there is
already a pending request. `curl` and `roxmltree` cover the fetching and the
parsing; nothing new is needed.

A catalogue is not a fourth place — nothing syncs to it — so it wants its own
view rather than a column in the library.

A catalogue cannot say whether a book is already held. Identity here is
content-based on purpose, and `inventory.rs` states that title and ISBN alone
never prove two books are the same edition. Downloading and letting the content
decide is the honest behaviour, at the cost of occasionally fetching something
already held.

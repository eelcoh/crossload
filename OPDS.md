# OPDS catalogues

Research date: 2026-09-20. Status: investigation, nothing implemented. Findings
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

**What Dutch library systems serve** was not established at all.

## If it is built

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

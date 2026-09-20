# Crossload

Keep your books in sync between a Kobo, your computer, and a reader running
CrossPoint — an XTeink X4, an M5Paper, or another.

Crossload shows one library across all three places and tells you which books
are where, so you can copy what is missing in either direction. It fulfills
Adobe/ByteBooks ACSM files on its own, prepares images for the X4's screen, and
verifies every copy it makes. Nothing is ever overwritten or deleted.

Python and Calibre are not needed to run it.

```
Crossload · Books                            ● Local 7   ● Kobo 4   ● CrossPoint 3
All books  11 of 11  ✓ 2 marked
┌ Library · 11 ────────────────────────────● original  ◐ device copy  · none ┐
│   L K C TITLE                             AUTHOR                           █
│ ✓ · ● · De ontdekking van de hemel        Harry Mulisch                    █
│   ● · · De wraak van Diponegoro           Martin Bossenbroek               █
│ ✓ ● ● · Dune                              Frank Herbert                    █
│   · ● · Het bittere kruid                 Marga Minco                      █
│   ● ● · Neuromancer                       William Gibson                   ║
│   ● · ◐ Piranesi                          Susanna Clarke                   ║
│   ● · ◐ Slow Horses                       Mick Herron                      ║
│   ● · · The Dispossessed                  Ursula K. Le Guin                ║
│   · · ◐ The Left Hand of Darkness         Ursula K. Le Guin                ║
└────────────────────────────────────────────────────────────────────── 3/11 ┘
┌ Dune · Frank Herbert ──────────────────────────────────────────────────────┐
│ Source  Local · original · 1.0 MiB                                         │
│ Path    ~/Books/Dune.epub                                                  │
└────────────────────────────────────────────────────────────────────────────┘
enter copy   space mark   a mark all   f filter   d copies   / search   q quit
· Library ready. Enter chooses a copy destination; r refreshes locations.
```

## Install

### Linux: download it

```sh
VERSION=0.1.1
NAME=crossload-$VERSION-x86_64-unknown-linux-gnu
curl -fLO https://github.com/eelcoh/crossload/releases/download/v$VERSION/$NAME.tar.gz
curl -fLO https://github.com/eelcoh/crossload/releases/download/v$VERSION/$NAME.sha256
sha256sum --ignore-missing -c $NAME.sha256
tar -xzf $NAME.tar.gz
install -Dm755 $NAME/crossload ~/.local/bin/crossload
crossload --version
```

[Releases](https://github.com/eelcoh/crossload/releases) lists the current
version. The binary brings everything with it — no Rust, Python, Calibre or
ebook libraries to install — and needs a distribution from 2024 or later
(glibc 2.39). Anything older builds from source just as happily. Make sure
`~/.local/bin` is on your `PATH`.

### macOS, or building it yourself

```sh
# macOS:         xcode-select --install && brew install cmake
# Debian/Ubuntu: sudo apt install build-essential cmake perl pkg-config
# Fedora:        sudo dnf install gcc gcc-c++ make cmake perl pkgconf
git clone https://github.com/eelcoh/crossload.git
cd crossload
cargo install --path . --locked      # needs Rust: https://rust-lang.org/tools/install/
crossload --version
```

The Adobe fulfillment, database, TLS and HTTP pieces are compiled from source
instead of borrowed from your system, so that the finished binary depends on
nothing but the C runtime. That is the only reason a compiler, CMake and Perl
appear here: they build it, they are not needed to run it. Cargo installs into
`~/.cargo/bin`.

No macOS binary is published, so this is the way in on a Mac — and the only way
on a managed one, where the quarantine that macOS puts on any download cannot be
cleared. Linux is what has been used against real hardware; macOS builds and
passes its tests on every change.

## Start here

Open the library:

```sh
crossload tui
```

Without a saved import folder, Crossload opens setup first. Choose your books
and import folders — the two rows marked with a star — optionally test the
reader address, which starts at `crosspoint.local`, then save with **Ctrl+S** or
the **Save and rescan** row. Use arrows or Tab to select a row and Enter to edit
it; Enter accepts an edit and Esc undoes it. On the Kobo row Enter searches for a
mounted Kobo instead of asking for a path, and **e** types one in by hand. The
line under the fields says what the selected row is for, and each path says
whether it is there. Nothing is written until you save, and a save that cannot
go through takes you to the row that stopped it. You can skip setup with Esc and
browse the current directory. Press **,** in the library to set this up later.
If a reader card mount is configured, copies use the card instead of Wi-Fi;
clear that field to use the reader address.

Your highlights come off the Kobo too:

```sh
crossload kobo notes                     # as Markdown, grouped by book
crossload kobo notes --output ~/Notes    # one file per book
```

You can also save where your things live from the command line:

```sh
crossload config set \
  --browse ~/Books \
  --output ~/Books \
  --device /run/media/$USER/KOBOeReader \
  --reader crosspoint.local
# On macOS the Kobo is usually /Volumes/KOBOeReader.
```

Then open the library:

```sh
crossload tui
```

Every location is scanned independently, so a Kobo you have not plugged in or a
reader that is switched off costs you nothing — the books you do have stay
usable, and you can start with none of them connected. Press **r** after
connecting or disconnecting something.

Scanning only reads. Nothing is copied, changed or removed until you ask for it.

Crossload carries **EPUB**, **PDF** and **CBZ**. EPUB is the format it works on:
it reads the title and author out of the file, optimizes images for the X4's
screen, and files the book under its author. A PDF or CBZ travels byte for byte
— found, identified, copied and verified like any other book, but never
rewritten, so it is titled by its filename and has no author. Books of any
format are capped at 128 MiB, which a long comic can exceed.

The reader is the exception: **the X4's library lists only EPUB.** It will store
a PDF happily — its own file listing marks every entry `isEpub`, and says false
for anything else — but its reading app never shows one. So a PDF reaches the
reader by being **converted to EPUB**, and a CBZ, which cannot be, is refused
with the reason given. Crossload lists only EPUB on the reader, for the same
reason the reader does: a PDF left there is a file, not a book. Kobo and your
computer take every format as it is.

Crossload judges a PDF when it reads it, and the copy dialog acts on that
judgement: a PDF of ordinary text says `convert to EPUB, copy` and goes; one
that will convert badly says so and waits for a **y**; one with nothing to
convert — pages that are scanned images, or a PDF under a password — is refused
in red. Converting writes the EPUB into your import folder and leaves the PDF
exactly as it was, so you keep both and can read the result before trusting it.
Text is rebuilt into paragraphs, columns are read one at a time, and headings
become the EPUB's table of contents; images and tables are not carried over, and
a word broken across a line loses its hyphen.

## Reading the library

The **L K C** column is Local, Kobo and CrossPoint (your reader): `●` an original, `◐` a copy that
went through the X4's image conversion, `·` not there, `✗` unreadable. So *Het
bittere kruid* above is only on the Kobo, *The Left Hand of Darkness* only on the
reader, and *Dune* is in both places but not on the reader yet. ACSM files
waiting to be fulfilled appear at the end of the list, marked `⇩`.

Books that are the same book share one row, matched by contents rather than by
title, so a renamed or repacked copy is still recognized as the same edition.

## Doing things

| Key | |
| --- | --- |
| `enter` | copy the highlighted book, or everything marked |
| `1` `2` `3` | choose Local, Kobo or CrossPoint in the dialog |
| `space` `a` | mark one book, mark or clear everything shown |
| `f` | open the filter menu; arrows and Enter, or `1`–`6`, select |
| `s` | sort by author or title, keeping the highlighted book selected |
| `,` | edit saved folders and devices, find a mounted Kobo, test the reader connection |
| `h` | what was copied lately, and whether it arrived |
| `?` | open keyboard help and the presence legend |
| `d` | list every copy of a book, and delete one of them |
| `/` | search by title or author |
| `r` | rescan |
| `esc` | stop a running copy after the book in progress |
| `q` | quit |

The copy dialog tells you what each destination will do before you choose it —
how many books it would copy, or why it would not: already there, device
unavailable, still scanning. While a set is copying, a bar shows how far it has
come while the line beneath it names the book in progress:

```
Copying ████████████████░░░░░░░░░░░░░░░░ 6 of 12 books
⠹ Sending to CrossPoint and verifying contents…
```

Copying to Kobo produces an ordinary sideloaded file; eject the Kobo normally so
it indexes them. Copying an EPUB to the reader converts its images for the X4
screen and files the book under its author. When a book exists in several
places, the original is always preferred over a device copy, because the
conversion is lossy and cannot be undone. A PDF or CBZ is never converted, so no
copy of one is ever the lesser one — `◐` cannot appear for them — and the reader
is not offered as a destination for one at all.

**d** lists every copy of a book with its size and path — the only place a
duplicate shows itself, since two files of the same book share one row. Pressing
its number asks for confirmation, and only **y** deletes it. Crossload refuses
to remove the last copy of a book, a book the Kobo database lists, anything it
cannot verify is still the copy it found, and anything on a reader reached over
Wi-Fi rather than as a mounted card. A deleted file is gone, not moved aside.

ACSM files are fulfilled by choosing **Local**. The spent request is then moved
into an `archive/` folder beside itself, so it stops asking to be dealt with.
This needs a one-time activation import, which is
[in the manual](MANUAL.md#adobebytebooks-acsm-files).

## Without the terminal interface

The same library, for scripting:

```sh
crossload books                                 # a table, or TSV when piped
crossload books --json | jq '.[].title'
crossload sync --from kobo --to xteink          # what is missing where
crossload sync --from kobo --to xteink --apply  # copy it
```

`sync` is a dry run until you add `--apply`. Single books have their own
commands too: `crossload import book.acsm`, `crossload send book.epub`,
`crossload copy book.epub --to /mnt/card`, `crossload kobo list`.

## Good to know

The first scan of a large library takes a while: every book is read and
fingerprinted so copies can be matched by content. After that it is remembered,
and later scans are close to instant unless a file has actually changed. The
cache lives in `~/.cache/crossload/` and deleting it only costs you one slow
scan.

Crossload never overwrites a file. A copy that would land on an existing,
different file is reported instead, and every transfer is read back and verified
against its source. The one thing that removes anything is `d`, which asks first
and refuses to take the last copy of a book.

## Further reading

- **[MANUAL.md](MANUAL.md)** — every command and option, ACSM activation setup,
  Kobo details, troubleshooting, building and packaging, current limits, and
  what has actually been tested.
- [TUI-ARCHITECTURE.md](TUI-ARCHITECTURE.md) — how the terminal interface is put
  together.
- [NATIVE-OPTIONS.md](NATIVE-OPTIONS.md) and
  [BYTEBOOKS-ACTIVATION.md](BYTEBOOKS-ACTIVATION.md) — why the Adobe side works
  the way it does.
- [THIRD-PARTY.md](THIRD-PARTY.md) — bundled code and licences.

## License

MIT, see [LICENSE](LICENSE). Bundled libgourou is GPL-3.0; see
[THIRD-PARTY.md](THIRD-PARTY.md).

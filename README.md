# Crossload

Keep your books in sync between a Kobo, your computer, and an XTeink X4 or
another CrossPoint reader.

Crossload shows one library across all three places and tells you which books
are where, so you can copy what is missing in either direction. It fulfills
Adobe/ByteBooks ACSM files on its own, prepares images for the X4's screen, and
verifies every copy it makes. Nothing is ever overwritten or deleted.

Python and Calibre are not needed to run it.

```
Crossload · Books                            ● Local 7   ● Kobo 4   ● Xteink 3
All books  11 of 11  ✓ 2 marked
┌ Library · 11 ────────────────────────────● original  ◐ device copy  · none ┐
│   L K X TITLE                             AUTHOR                           █
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

There are no published binaries yet, so build it once from source. You need
[Rust](https://rust-lang.org/tools/install/), a C/C++ compiler, Make, CMake and
Perl:

```sh
# Debian/Ubuntu: sudo apt install build-essential cmake perl pkg-config
# macOS:         xcode-select --install && brew install cmake
git clone https://github.com/eelcoh/crossload.git
cd crossload
cargo install --path . --locked
crossload --version
```

Cargo installs into `~/.cargo/bin`; make sure that is on your `PATH`.

Linux and macOS are supported. Linux is what has been used against real
hardware; macOS builds and passes its tests in CI on every change. Building from
source is the right path on a managed Mac: the packaged binaries are unsigned,
and a corporate policy that forbids clearing macOS quarantine will not run
them.

## Start here

Save where your things live, once:

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
usable. Press **r** after connecting or disconnecting something.

## Reading the library

The **L K X** column is Local, Kobo and Xteink: `●` an original, `◐` a copy that
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
| `1` `2` `3` | choose Local, Kobo or Xteink in the dialog |
| `space` `a` | mark one book, mark or clear everything shown |
| `f` `F` | filter, forwards or back: missing from Local, from Kobo, from Xteink, only on the reader, unreadable |
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

Copying to Kobo produces an ordinary sideloaded EPUB; eject the Kobo normally so
it indexes them. Copying to the reader converts images for the X4 screen and
files the book under its author. When a book exists in several places, the
original is always preferred over a device copy, because the conversion is
lossy and cannot be undone.

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

Crossload never overwrites a file and never deletes a book. A copy that would
land on an existing, different file is reported instead. Every transfer is read
back and verified.

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

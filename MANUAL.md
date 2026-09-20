# Crossload manual

The complete reference: every command, the rules each one follows, what has been
verified and what has not. Start at [README.md](README.md) for installing
Crossload and the everyday workflow; come here for the detail behind it.

## Install

Crossload targets Linux and macOS. Linux has been tested on a real Kobo. macOS
builds, passes the full test suite and runs its terminal smoke test on every
push to `main` through CI, and the current version has been run on a Mac by
hand. Kobo hardware and reader transfers have only been exercised on Linux.
Linux and macOS binaries are published with each tagged release; see
[Releases](https://github.com/eelcoh/crossload/releases). Local distribution
archives are also generated under `dist/` by `mise run package`.

### From a binary archive

`install.sh` in the repository does all of this and is the shortest way in:

```sh
curl -fsSL https://raw.githubusercontent.com/eelcoh/crossload/main/install.sh | sh
```

It picks the archive for the machine it runs on, refuses to install anything
whose checksum does not match, needs no root, and leaves nothing in a temporary
directory. `CROSSLOAD_VERSION` pins a version; `CROSSLOAD_BIN` chooses where the
binary goes. Everything below is what it does, by hand.

Choose the archive matching your OS and CPU, verify its checksum, and extract it:

```sh
# Linux example (run in the directory containing the downloaded packages):
sha256sum -c crossload-0.2.0-x86_64-unknown-linux-gnu.sha256
tar -xzf crossload-0.2.0-x86_64-unknown-linux-gnu.tar.gz
cd crossload-0.2.0-x86_64-unknown-linux-gnu
./crossload --help
mkdir -p "$HOME/.local/bin"
install -m 755 crossload "$HOME/.local/bin/crossload"
```

On macOS use `shasum -a 256 -c FILE.sha256`. The checksum file covers both the
binary and the corresponding source archive, so pass `--ignore-missing` when
only one of them was downloaded.
Apple Silicon packages use `aarch64-apple-darwin`; Intel Macs use
`x86_64-apple-darwin`.

Mac packages are unsigned and not notarized, which is how Homebrew's own
formula bottles are distributed too. macOS does not quarantine every download:
it quarantines what a quarantine-aware application writes, which means a
browser, Mail or AirDrop. `curl`, `wget` and `git` do not set the attribute, and
Gatekeeper only refuses a file that carries it, so fetch the archive with curl
and it runs. A browser download needs `xattr -d com.apple.quarantine crossload`,
and that is what a managed device may forbid: it was tried on a corporate
MacBook and the restrictions did not allow it. That restriction turns out to be
the only one in the way. The same managed MacBook runs the packaged binary when
it arrives by scp, and runs Homebrew, whose formula bottles are unsigned and
unnotarized by the same reasoning. A device whose policy demanded notarization
outright would refuse all of that, and none has been met.

On Apple Silicon an executable needs a code signature to run at all, but an
ad-hoc one satisfies that; the toolchain applies it at link time, and it is not
notarization and costs nothing.

A packaged macOS binary from CI has been run on a Mac and reports its version.
It was moved there with scp, which sets no quarantine attribute, and needed
neither a signature beyond the toolchain's ad-hoc one nor any exception made for
it. Kobo hardware and reader transfers are still Linux-only ground.

`BUILD.json` records the compiler, platform, source revision, whether the working
tree was modified, and dynamic libraries. Local Linux packages use the Debian 12
build container; automated Linux packages use Ubuntu 24.04, so their minimum
system-runtime requirements can differ. No Rust, Python, Calibre, or separately
installed ebook libraries are needed to run the binary.

### From source

Install [Rust through rustup](https://rust-lang.org/tools/install/) and a native
C/C++ compiler, Make, CMake and Perl. On macOS, install the Xcode Command Line
Tools and CMake (for example `brew install cmake`); on Debian/Ubuntu install
`build-essential cmake perl pkg-config`. Git and network access
are needed for the initial dependency download. Rustup selects the version in
[`rust-toolchain.toml`](rust-toolchain.toml).

Run these commands from the project directory:

```sh
cargo install --path . --locked
crossload --version
```

Cargo normally installs the binary into `~/.cargo/bin`. Ensure that directory is
on your `PATH`; reopening the terminal after installing rustup may be enough.
See [Cargo's installation documentation](https://doc.rust-lang.org/cargo/commands/cargo-install.html)
for custom installation locations. Run the same installation command after
updating the checkout to rebuild and update crossload.

### From an existing local build

If `target/release/crossload` has already been built for your OS and architecture,
you can run it directly or install it in your user bin directory:

```sh
./target/release/crossload --help
mkdir -p "$HOME/.local/bin"
install -m 755 target/release/crossload "$HOME/.local/bin/crossload"
```

Add `~/.local/bin` to your shell's `PATH` if needed:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Put that line in your shell startup file to retain it in new terminals.
The installed executable does not need Rust, Python, Calibre, or a system SQLite
installation. Linux builds still use standard system runtime libraries; this is
not yet a fully static executable.

## Use

### Adobe/ByteBooks ACSM files

Calibre's **ACSM Input** plugin supplies the activation once; Crossload can then
import ACSM files without Calibre running. Direct ByteBooks login in Crossload
is not planned.

1. On the Mac where your books already work, open Calibre's **Preferences →
   Plugins**. Select **ACSM Input** (older versions may say **DeACSM**) and choose
   **Customize plugin**. If missing, install its ZIP from the
   [official plugin releases](https://github.com/Leseratte10/acsm-calibre-plugin/releases)
   using **Load plugin from file**, then restart Calibre.
2. If the plugin is already authorized, keep that authorization. Otherwise use
   **Import activation from ADE** to reuse the computer's existing Digital
   Editions activation, or follow the plugin's account-linking instructions.
3. Choose **Export account activation data** and save the ZIP as `activation.zip`.
   This is the activation backup, not **Export account encryption key** (a DER
   key). See the [plugin's setup guide](https://github.com/Leseratte10/acsm-calibre-plugin#setup).
4. Copy that ZIP privately to the computer running Crossload. The same Mac export
   works on Linux; no Kobo USB connection is needed for this setup.
5. Import it once and check it:

```sh
crossload adobe setup --from ~/Downloads/activation.zip
crossload adobe status
```

Then download an ACSM from your store or library and run:

```sh
crossload import ~/Downloads/book.acsm --output ~/Books
```

You can also select the ACSM in the TUI, press Enter, and choose **1 Local**.
Refresh afterward to find and copy the resulting EPUB.

`activation.zip` is created by the plugin's export action; it is not a file that
must already exist in your Calibre book library. It contains `device.xml`,
`activation.xml`, and `devicesalt`. Crossload accepts either that ZIP or a
folder containing those three files. The desktop application's `activation.dat`
alone and a DER encryption key are not supported substitutes.

Setup validates and copies the activation locally without registering another
device. Keep the export private and backed up; it contains account keys.
If an activation is already installed, use `adobe status` rather than trying to
replace it. For a different activation, use a separate state directory as below.

Private state lives in `~/.local/share/xteink` on Linux (or
`$XDG_DATA_HOME/xteink`) and `~/Library/Application Support/xteink` on macOS.
Directories use mode 0700; imported credentials use 0600. To use another location
or keep activations separate, pass the global `--state-dir` option consistently:

```sh
crossload --state-dir ~/private-crossload adobe setup --from ~/activation-export
crossload --state-dir ~/private-crossload import book.acsm --output ~/Books
```

An installed activation is never replaced by setup. `status` checks its keys
locally; it does not verify account status with ByteBooks. ACSM import requires
network access. TLS verification stays enabled; `SSL_CERT_FILE` can point to a
trusted CA bundle when needed.

Fulfillment receipts and the original encrypted EPUB stay in the private
`acsm/<input-hash>/` cache. If downloading fails after receipt creation, repeat the
same command to reuse that receipt. A failed request with an unknown server
outcome is not automatically repeated; the error gives the recovery directory.
Keep that directory when investigating a failure. Loans retain their token data
in the receipt, but returning loans is not implemented yet. Cached files are not
automatically purged. Only validated EPUBs reach the output directory, and
existing output files are never overwritten. PDF ACSM files are not supported.

The user has completed a real ACSM import on Linux and confirmed the resulting
EPUB reads on CrossPoint. Automated tests use generated credentials and a local
test server, and run on macOS in CI; no real ACSM fulfillment has been performed
on a Mac.

### Kobo books

Connect the Kobo and select its USB connection mode. Supply the mount directory
that contains `.kobo`, for example:

```sh
crossload kobo list --device /run/media/you/KOBOeReader
crossload kobo list --device /run/media/you/KOBOeReader --json
crossload kobo import BOOK_ID --device /run/media/you/KOBOeReader --output ~/Books

# On macOS, for example:
crossload kobo list --device /Volumes/KOBOeReader
```

On macOS the device path is typically `/Volumes/KOBOeReader`. Use the exact book
ID printed by `list`. An optional `--serial SERIAL` on `import` supplies the
Kobo's device serial when `.adobe-digital-editions/device.xml` is unavailable.
Unencrypted books do not require a serial.

The `SOURCE` column distinguishes `Kobo store` from `Sideloaded`. Sideloaded books
are discovered recursively in ordinary device folders using their EPUB contents,
including `.kepub.epub` and extensionless files. Hidden folders, the top-level
`fonts` directory, symlinks, and non-EPUB files are skipped. Titles and authors
come from the EPUB package metadata.

Use a sideloaded entry's `file:…` ID with the same import command. These IDs derive
from the device-relative path and remain stable across mount locations. Separate
copies have separate IDs, even when they share a title or also have a store entry.
JSON output includes `source` (`kobo_store` or `sideloaded`) and, for sideloaded
books, a device-relative `path`. Readable sideloaded EPUBs are validated and copied
without requiring Kobo account keys or a serial. For an encrypted sideloaded copy,
the tool can reuse the matching store entry's keys when the embedded identifier
and file hash match the downloaded store file. Such entries show `Kobo` under
`KEYS` and require the device serial, just like store imports. A shared identifier
alone does not cause an already-readable copy to be decrypted again.

Previews are hidden by default in both the CLI and TUI. Use
`crossload kobo list --show-previews` (also works with `--json`) or
`crossload tui --show-previews` to include them.

The `TYPE` column identifies Kobo previews, which can be stored locally even when
the full edition is absent. JSON output includes a `preview` boolean. Importing
a preview reports that the full edition needs to be downloaded on the Kobo.
The table wraps titles and authors to fit the terminal and keeps IDs intact for
copying. The `KEYS` column shows `Kobo` or `—` (none recorded); absence of Kobo keys does not
establish that an entry is a complete, DRM-free book.

The output directory is created as needed. Each filename contains the title and
a short hash of the book ID. Existing files are never overwritten. The tool reads
the device and works with temporary database copies, including any WAL data. It
rejects output directories inside the Kobo. Stop other software syncing the device
before importing; a change detected during the database copy is reported as an error.

## Sync Kobo books

Plan which full books are missing from the reader:

```sh
crossload kobo sync --device /run/media/$USER/KOBOeReader --output ~/Books --to crosspoint.local
```

Repeat with `--apply` to import, optimize and transfer the missing books into
`Author/Title.epub`. Saved device, output, reader and remote folder defaults work
here too. For a mounted card, replace `--to crosspoint.local` with
`--copy-to /path/to/card`. Use `--json` for a machine-readable report.

The default dry run writes only temporary working files. Inventory reads every
EPUB within the selected destination folder, so Wi-Fi planning can take time.
Matching uses full contents, recognizing renamed or repacked EPUBs and the
current optimized copy; titles or ISBNs alone do not count as a match. Previews
and duplicate content are skipped. Nothing is deleted or overwritten.

Rerun after interruption: verified reader copies are skipped, and identical
local imports are reused. Conflicting books are reported individually with a
nonzero exit status. For a matching incomplete Wi-Fi file, add `--repair`:
the apply run preserves the partial file under a backup name before sending a
fresh copy. An unreadable inventory aborts sync before any transfers.

Calibre-converted copies can differ from store versions even when they share a
title. To keep the reader's copy and skip a conflicting source, add
`--exclude BOOK_ID` to both the dry run and apply command. Repeat the option for
multiple books; unknown IDs are rejected. Exclusions apply only to that command.

## Send to CrossPoint over Wi-Fi

The transfer target is **CrossPoint X4 with CrossPoint 1.6.0**. EPUB reading has been
confirmed on that device with an imported book. Wireless transfer uses the
[1.6.0 HTTP and WebSocket APIs](https://github.com/crosspoint-reader/crosspoint-reader/blob/1.6.0/docs/webserver-endpoints.md)
and is tested against local protocol servers. A real transfer of the 3,054,220-byte
Diponegoro EPUB was SHA-256 verified and confirmed readable on the X4 after the
earlier SD-card write failures were resolved.

On the X4, open **File Transfer → Join Network** and connect it to the same
network as your computer. Keep that screen open during transfer. Use the IP
address displayed there (replace the example below), or `crosspoint.local` if
mDNS works on your network:

```sh
crossload send "book.epub" --to 192.168.1.102
crossload send "book.epub" --to http://crosspoint.local --folder /Books
```

Books now go to `Author/Title.epub`, using EPUB metadata. The author directory
is created automatically. Missing authors use `Unknown author`; missing titles
use the local filename. Names are sanitized for the SD card. Different books
with the same author/title are never overwritten. `--folder` selects an
**existing base folder**, so `--folder /Books` gives `/Books/Author/Title.epub`.
Use `--flat` to retain the local filename directly in the base folder.
Alternatively choose **Create Hotspot** on the X4, connect the computer to
`CrossPoint-Reader`, and use the displayed address (typically `192.168.4.1`).
See the [CrossPoint guide](https://github.com/crosspoint-reader/crosspoint-reader/blob/1.6.0/docs/webserver.md).

Import and send in one command:

```sh
crossload import book.acsm --output ~/Books --send-to 192.168.1.102
crossload kobo import BOOK_ID --device /run/media/you/KOBOeReader \
  --output ~/Books --send-to 192.168.1.102 --folder /Books
```

crossload validates the local EPUB and uploads over WebSocket port 81 in 4 KiB
frames, waiting for progress acknowledgements every 64 KiB. Uploads use a random
`.uploading` filename. The reader's copy is downloaded and SHA-256 verified
before being renamed to the final EPUB filename. Failed uploads therefore do
not publish incomplete EPUBs. Verification adds one download to each transfer. An identical existing file is verified and skipped; a different
file with the same name is never overwritten. CrossPoint 1.6.0's actual HTTP
handler also rejects collisions (its endpoint document's overwrite note is stale).

A failed transfer leaves the local EPUB intact. Retry with `crossload send` rather
than importing again. New failures may leave a `.uploading` file; the CLI does
not automatically delete these.

To repair an incomplete EPUB from an older transfer:

```sh
crossload send "book.epub" --to 192.168.1.102 --repair
```

Repair requires a strictly shorter file whose bytes exactly match the beginning
of the local EPUB. It preserves that file under an
`xteink-<hash>.incomplete-<size>` backup name, then transfers a fresh copy. It never
repairs or overwrites an unrelated file. Persistent SD write failures require
checking card free space, direct copying, and card/filesystem health; the firmware
message alone does not establish that the card is full.
Requests have a five-second connection timeout, bounded responses, and up to ten
minutes for each upload/download. Wi-Fi transfer connects directly to the reader,
bypassing HTTP proxy environment variables, and does not follow redirects.

CrossPoint's transfer service uses plain HTTP/WebSocket without authentication. Use your
own network or its hotspot. This feature needs no USB access, Calibre, or external
`curl` command. Credentials and ACSM activation data are never uploaded; `send`
accepts only validated EPUBs.

## Copy to a mounted SD card

Insert the card in a reader, mount it, and choose an existing directory on it:

```sh
crossload copy "book.epub" --to /run/media/you/XTEINK/Books
# macOS example:
crossload copy "book.epub" --to /Volumes/XTEINK/Books

# Import and copy in one command:
crossload import book.acsm --output ~/Books --copy-to /run/media/you/XTEINK/Books
crossload kobo import BOOK_ID --device /run/media/you/KOBOeReader \
  --output ~/Books --copy-to /run/media/you/XTEINK/Books
```

The destination directory must already exist. crossload does not mount the card or
create a missing mount point. `--copy-to` and Wi-Fi `--send-to` are mutually
exclusive. Copying checks EPUB structure, encryption metadata and available
space, writes a temporary file, flushes and verifies the bytes, then publishes
without overwriting. Identical existing files are skipped; different files and
destination symlinks are rejected. Failed copies leave the local EPUB intact.
Safely eject the card before removing it. Direct card access may be unavailable
on your corporate Mac; Wi-Fi transfer remains the alternative.

## Troubleshooting

| Message or situation | What to do |
| --- | --- |
| Cannot reach CrossPoint | Keep the X4 in File Transfer mode, use its displayed IP address, and check both devices are on the same network. |
| Reader copy differs or filename collision | Inspect the existing file on the reader; crossload does not overwrite it. The local EPUB is intact. |
| No `.kobo/KoboReader.sqlite` found | Connect the Kobo in USB mode and pass its mount root, not the `.kobo` folder. |
| An entry is a preview | Download the full edition on the Kobo, then reconnect. A separate sideloaded copy may already appear in the list. |
| Cannot read `device.xml` or no serial found | Supply the device serial with `--serial SERIAL`. Plain sideloaded EPUBs do not need it. |
| Existing output file | The importer never overwrites files. Choose another output directory if you need another copy. |
| EPUB validation fails | The input may be incomplete, still encrypted, or use an unsupported format/encoding. No output EPUB is saved. |
| Several entries share a title | Check `SOURCE` and `TYPE`; use `--json` to inspect sideloaded paths. Every separate file has its own ID. |

Run `crossload --help` or `crossload kobo import --help` for command options. Errors
are printed to stderr and return a nonzero exit status. JSON list output goes
to stdout without table decoration or summary text.

### Crossload is using a whole core

That is a scan, not a loop. Discovery reads and hashes books, decrypts Kobo
books and downloads what is on the reader, and it uses several threads for local
books. While it runs, the location line says `Checking (37 books)` with a
spinner; when every location says `Ready`, no work is left and the interface
sits at zero.

Threads are named after the work they do, so a busy one identifies itself:

```sh
top -H -p "$(pgrep -x crossload)"          # or: ps -L -o tid,pcpu,comm -p "$(pgrep -x crossload)"
```

`scan-local`, `scan-kobo` and `scan-xteink` are the three locations,
`read-local-N` the pool that reads local books, `crossload-ui` the interface.
On macOS, `sample crossload` reports the same names. Sustained work while every
location reads `Ready` is a bug worth reporting.

A first scan after the identity cache is removed costs full price again: every
book is read, every Kobo book decrypted, everything on the reader downloaded.
That is the expected one-off, not a regression.

## Build and check

[mise](https://mise.jdx.dev/getting-started.html) is the task runner. Rust stays
pinned in `rust-toolchain.toml` and dependencies in `Cargo.lock`; mise does not
install a second Rust toolchain. Native builds require rustup, Git, a C/C++
compiler, Make, CMake and Perl. Packaging additionally needs Python 3.11+.

### Linux with Podman (including immutable desktops)

Install mise and Podman using your distribution's supported method, then run
from the checkout:

```sh
mise trust
mise run container:build
CROSSLOAD_BUILD_BACKEND=container mise run check
CROSSLOAD_BUILD_BACKEND=container mise run run -- --help
```

The container backend needs no host Rust or C toolchain. It writes artifacts and
its Cargo cache under `target/`. `mise run run` builds in the container and runs
the resulting binary **on the host**, where it can reach devices, private state,
Wi-Fi and your terminal. The existing `localhost/xteink-build` image name is
retained; `mise run container:build` refreshes it from `Containerfile.build`.

### Native Linux

On Debian/Ubuntu, install the system prerequisites:

```sh
sudo apt-get install build-essential cmake perl pkg-config git
```

Install mise and [rustup](https://rust-lang.org/tools/install/), then:

```sh
mise trust
CROSSLOAD_BUILD_BACKEND=native mise run check
mise run run -- --help
```

### Native macOS (Intel or Apple Silicon)

Install Xcode Command Line Tools, mise, CMake and rustup:

```sh
xcode-select --install
brew install mise cmake
```

Complete the [rustup installation](https://rust-lang.org/tools/install/), ensure
its Cargo commands are on PATH, then from the checkout:

```sh
mise trust
CROSSLOAD_BUILD_BACKEND=native mise run check
mise run run -- tui --output ~/Books --browse ~/Books
```

The binary matches the architecture of the native Rust toolchain. Both native
builds are exercised by CI, and the Linux container workflow is locally tested.

### Everyday tasks

```sh
mise run build                      # release binaries
mise run check                      # formatting, Clippy, tests and release build
mise run fmt                        # apply formatting
mise run test                       # tests only
mise run test -- --test kobo         # forward Cargo test arguments
mise run lint
mise run run -- kobo list
mise run install                    # build, then install into ~/.local/bin
mise run install -- /usr/local/bin  # or another directory
mise run package -- --output dist/new-release
```

`install` builds with whichever backend is in use and then copies the binaries on
the host, so a container build installs a host binary. It writes `crossload` and
the `xteink` compatibility command into `~/.local/bin`, or into
`CROSSLOAD_INSTALL_DIR`, or into a directory given as an argument; it creates the
directory, says so when it is not on your `PATH`, and prints the installed
version. Existing files at those names are replaced, unlike everything Crossload
does with books.

By default, tasks use native Cargo when it is on PATH, otherwise Podman on Linux.
Set `CROSSLOAD_BUILD_BACKEND=native` or `container` to choose explicitly;
`CROSSLOAD_BUILD_IMAGE` selects another compatible container image. Containers
are for Linux builds only. There is no automatic installation of system packages.

The result is `target/release/crossload`, plus the compatibility `xteink` command.
Rustup selects the pinned compiler on first use. The initial build needs network
access for dependencies. These tasks provide repeatable commands and pinned Rust
inputs, not a promise of bit-identical builds across operating systems.

Both CI workflows use mise 2026.9.1 and the same task scripts. If mise is unavailable,
`bash scripts/dev.sh check` uses the identical workflow; direct Cargo commands still
work. Source packages include the mise configuration and task script. Task argument
forwarding follows [mise's task documentation](https://mise.jdx.dev/tasks/task-arguments.html).

## Build distribution packages

With the native build prerequisites and Python 3.11+ installed:

```sh
python3 scripts/package.py
# Use --output DIRECTORY for another destination or a subsequent build.
```

In the existing Linux build container:

```sh
podman run --rm --userns=keep-id --security-opt label=disable \
  -v "$PWD:/work" -w /work -e CARGO_HOME=/work/target/cargo-home \
  localhost/xteink-build python3 scripts/package.py
```

The packager creates a binary `.tar.gz`, a matching `-source.tar.gz`, and a
`.sha256` checksum file under `dist/`. It rejects unexpected dynamic library
requirements and includes native and Cargo dependency notices. The source
archive includes the exact Cargo dependencies and `.cargo/config.toml` needed
for `cargo build --release --locked --offline`; install the pinned Rust toolchain
and native build tools first. `REBUILD.txt` explains how to rebuild with modified
native libraries. Keep the source archive alongside the binary when distributing.
The packager uses an explicit source-file list; local books, activation exports,
private state, Git data, and build caches are excluded.

The [package workflow](.github/workflows/package.yml) can be run manually or by
pushing a `v*` tag. It tests and packages Linux x86-64, macOS Apple Silicon and
macOS Intel on native runners, and retains build artifacts in Actions. It does
not publish a GitHub Release. Native macOS builds require running that workflow;
they cannot be verified from this Linux workspace alone.

## Current limits

- Kobo Desktop data, account login, and downloads from Kobo's servers are not yet covered.
- Encrypted sideloaded Kobo copies currently require an identical store download
  on the same device. Sideloaded formats other than EPUB are not yet supported. Adobe-protected EPUBs
  are listed but cannot yet be imported through the Kobo command.
- ACSM import currently requires an ACSM Input/libgourou activation export and
  supports classic 1024-bit RSA ADEPT EPUBs. Desktop activation.dat, new account
  activation, PDF, and newer DRM schemes are not implemented.
- Validation checks the archive, package, reading order, and UTF-8 XHTML chapters.
  It is not full EPUBCheck. Books with non-UTF-8 XHTML are currently rejected.
- Input files are limited to 128 MiB, expanded archives to 256 MiB and 20,000 entries.
- Automated tests use synthetic books and databases. Linux listing, preview
  detection, and an encrypted import have also been checked on a real Kobo.
  One imported book has also been confirmed readable on an X4 running CrossPoint
  1.6.0. macOS CI runs the same suite on every push; no Kobo has been attached
  to a Mac.

See [NATIVE-OPTIONS.md](NATIVE-OPTIONS.md) for backend research and the roadmap,
and [THIRD-PARTY.md](THIRD-PARTY.md) for the pinned upstream implementation.

## Validation

The Linux build passes 62 integration tests (26 Kobo, 9 ADEPT, 17 CrossPoint,
7 mounted-copy and 3 device-preparation tests), plus 7 TUI and 4 configuration checks,
formatting, and Clippy with warnings denied. CrossPoint tests cover staged WebSocket uploads, acknowledgement windows,
readback verification, safe incomplete-file repair, duplicate handling, remote
failures, folder paths, input validation, and CLI behavior. ADEPT tests cover activation ZIP/directory import,
mismatched keys, permissions, process locking, signed fulfillment against a local
server, failed-download recovery, decryption, and rejection without output.
The release executable was also run directly on the Linux host. Its dynamic
dependencies include the system C/C++ runtime libraries; the ebook, networking,
cryptography, XML, ZIP, and SQLite libraries are bundled.
The tests cover encrypted and plain imports, volume selection, WAL snapshots,
font metadata, incorrect serials, malformed books, previews, and output/source
protection. A preview regression fixture reproduces a nested package pointing to
`kobo-locked.html` while that placeholder lives at the archive root. Preview
classification uses Kobo's `Accessibility = 6`, also recognized by
[Calibre's Kobo driver](https://github.com/kovidgoyal/calibre/blob/master/src/calibre/devices/kobo/driver.py).

## Roadmap

1. Kobo listing and import, including previews and sideloaded EPUBs — implemented.
2. Activation-export import and native ACSM-to-EPUB workflow — implemented and
   tested with a real book on Linux; on macOS only its automated tests have run.
3. CrossPoint Wi-Fi transfer — implemented, verified on an X4 with CrossPoint
   1.6.0, and confirmed readable.
4. Mounted-card copy and distribution packaging — implemented; local Linux
   archives built. The tag workflow packages both macOS architectures but has
   not been run, so no Mac package has been produced or executed.
5. Author/title device folders and X4 image optimization — implemented and
   confirmed working on the X4 with the nine-book anthology.
6. TUI browsing, import and transfer — implemented.

See [NATIVE-OPTIONS.md](NATIVE-OPTIONS.md) for the backend investigation and
[idea](idea) for the original project note.

## License

The Rust application and bridge use the [MIT license](LICENSE). The bundled
libgourou and uPDFParser libraries use LGPL-3.0-or-later; see
[native/README.md](native/README.md) for rebuilding and distribution details.
Kobo processing uses pinned
[Flamberge](https://github.com/kessriga/flamberge) Rust libraries; attribution and
dependency details are in [THIRD-PARTY.md](THIRD-PARTY.md).

## Device preparation

`send`, `copy`, and imports with `--send-to` or `--copy-to` automatically prepare
an X4 device copy. The imported local EPUB remains unchanged. JPEG and PNG images
are resized proportionally to fit 480×800, without upscaling or cropping, and
converted to grayscale. JPEGs are encoded as baseline JPEG at quality 85.
PNG transparency is preserved; animated PNGs are left intact. Other image formats, text, fonts, chapter order,
links and resource names remain unchanged. This chiefly helps image-heavy books;
it does not split anthologies or oversized chapters, and cannot guarantee that
every book will index on the reader. Image conversion is lossy; use the original
for higher-resolution reading or detailed maps.

To inspect a device copy before transferring:

```sh
crossload optimize "book.epub" --output ~/Books/device
```

To transfer without image conversion, add `--no-optimize`. To retain both the
original bytes and the old destination layout:

```sh
crossload send "book.epub" --to crosspoint.local --no-optimize --flat
```

Use those same two flags when repairing an incomplete upload made with the old
version. Existing root-level books are not moved automatically. Repeated sends
from the same original produce the same optimized bytes and skip an identical
destination. A device-profile marker prevents repeated lossy conversion when
you send an already optimized copy. Mounted-card copies use the same author/title
layout; the selected mount/base directory must already exist.

The Slough House nine-book anthology was reduced from 18,690,849 to 3,688,459
bytes (80.3% smaller): 18 images changed, with all original non-image entries
byte-identical. The user confirmed successful reading on the X4.

## Terminal interface

Browse a mounted Kobo and your local books:

```sh
crossload tui --device /run/media/eelco/KOBOeReader \
  --browse ~/Books --output ~/Books --send-to crosspoint.local
```

The TUI is a unified library. It scans `--browse` and `--output` recursively,
plus the configured Kobo and CrossPoint (Wi-Fi or `--copy-to` mounted card).
Saved device settings are used for discovery even without transfer flags.
Each location loads independently; disconnected devices appear as unavailable
while books from other locations remain usable. This is a fresh inventory, not
an offline history of books on disconnected devices. Press **r** after connecting
or disconnecting a device, or after copying. Refresh keeps the current list and
selection visible until the new scan completes, then applies additions/removals.
Copy actions wait during that refresh; an initial scan still shows partial results.
Books appear as they are read rather than only when a location finishes, so the
first of them is on screen almost immediately, and the location line counts them
as they arrive. Local books are read on several threads at once.

Discovery remembers what it learned in an identity cache under
`~/.cache/crossload/index.json` (`XDG_CACHE_HOME` is respected;
`~/Library/Caches/crossload` on macOS), so a repeated scan of an unchanged
library is close to instant. Each location proves a source is unchanged in the
way it can: local and Kobo books by the size and modification time of the file
on disk, so an unchanged Kobo book is never decrypted again; books on the reader
by the path and size its listing reports, so they are not downloaded again. A
book replaced by one of exactly the same size, with its timestamp restored where
there is one, is not detected. Entries are also specific to the device or card
they came from, and to the optimization settings that produced them.

The cache holds identities, never books: a title only appears when the current
scan finds its file, so this remains a fresh inventory rather than an offline
history. Deleting the file only costs the next scan its work; a missing,
unreadable or unwritable cache is never an error.

Matching copies share a row, with an **L K C** column for Local, Kobo and
CrossPoint: `●` an original copy, `◐` a device copy that went through optimization,
`·` absent and `✗` a copy that could not be read. A `⇩` row is a local ACSM
request. The location line above the library reports each place as ready with a
count, checking, not configured, or unavailable with the reason. Matching
uses contents and optimized variants, never title alone. Distinct editions stay
separate. Unreadable EPUBs remain visible with an explanation and cannot be used
as a transfer source until verified. Previews remain hidden unless requested.
Colour is never the only signal, and setting `NO_COLOR` turns it off.

- **Enter** opens copy actions; **1** chooses Local, **2** Kobo, **3** CrossPoint.
  **Escape** cancels. Opening the menu does not transfer anything. The dialog
  marks each destination before you choose it: an allowed copy, or why it is
  not available (already there, device unavailable, or discovery still running).
  It also says how much room each destination has left, and marks one in red
  when the books weigh more than that. A set's weight is taken from its sources,
  which is the most it could need, since optimizing and converting only ever
  make a book smaller. A reader reached over Wi-Fi says `free unknown` rather
  than a number: CrossPoint reports its free memory, which is not its storage,
  and offers no endpoint for the card, so only a mounted card, a Kobo or a local
  folder is measured. The room is read once, when the dialog opens.
- Copies already found at the destination are reported instead of duplicated.
- Original Local/Kobo copies are preferred over optimized/device copies.
  A reader-only copy can be recovered, but any image optimization it went
  through cannot be undone. Device-copy actions wait for discovery to finish so an original
  can be selected when available.
- Local copies go to `--output`. Kobo copies are ordinary sideloaded EPUBs in
  author folders; eject the Kobo normally so it can index them. CrossPoint copies
  retain optimization and author/title organization. Existing files are never
  overwritten, and every copy is verified.
- **Space** marks the highlighted book and **a** marks everything currently
  shown, or clears the marks when all of them are already marked. With books
  marked, **Enter** opens the dialog for the whole set: each destination reports
  how many of them it would copy, and why the rest would not. Books that cannot
  be copied are left behind with their reason rather than refusing the set.
  Marks are cleared when a copy starts and by a refresh.
- A set being copied shows a bar with the count of books finished, kept visible
  while each book's own progress replaces the line beneath it. `crossload sync
  --apply` prints the same count per book to standard error, leaving standard
  output for the result.
- **d** lists every copy of the highlighted book: location, size, whether it is
  an original or a device copy, and its path. Two files of the same book share
  one row in the library, so this is where a redundant copy becomes visible.
  A number asks to delete that copy, and only **y** confirms it. Five rules are
  checked when the dialog is drawn and again before anything is removed: never
  the last copy of a book, never a book the Kobo database lists (remove those on
  the Kobo), never a copy that could not be read, never over Wi-Fi (the reader's
  protocol has no delete; mount its card with `--copy-to`), and never a file
  whose contents no longer match what discovery recorded. The file is deleted
  rather than moved aside, because reclaiming the space is the point. Deleting a
  sideloaded Kobo book leaves the device to notice on its next scan, so eject it
  normally.
- **Escape** during a copy stops it after the book in progress; books already
  copied are complete and verified, and the summary says where it stopped.
- **f** opens a filter menu: all books, missing from Local,
  missing from Kobo, missing from CrossPoint, only on CrossPoint, and unreadable. Every
  location can be the one a book is missing from, so the same key that finds
  what the reader lacks also finds what has never been copied back. Choose with
  arrows and Enter, or 1–6; Escape cancels without changing the filter. The filter
  and search apply together, and the line above the library shows which filter
  is active and how many books it matches.
- **/** searches; arrows and page-navigation keys still move through matches.
  A plain search matches a book's title, its author and the series it belongs
  to. A field named before a colon searches that field alone: `author:herron`,
  `title:dune`, `series:slough`. **Ctrl+U** clears the query.
  Enter/Escape leaves search mode. **j/k**, arrows, Home/End and
  Page Up/Page Down navigate. **q** quits, waiting for active work.
- **s** toggles ascending title/author sorting, keeping the highlighted book
  selected and preserving marks. ACSM requests remain after the books.
- **?** opens keyboard help and the presence legend; arrows scroll and Escape
  closes it.
- **e** corrects what a book says about itself: its title, its author, and the
  series it belongs to with its number in that series. Only a local EPUB can be
  corrected — a book that is only on a device is not ours to rewrite, and a PDF
  or CBZ has no package document to rewrite. Arrows or Tab select a field and
  Enter edits it; Ctrl+U clears one; Ctrl+S saves from any row; Escape closes
  without writing. A field left alone is left alone: only what changed is
  written, so an untouched author cannot rewrite itself into another spelling.
  A series is written both as Calibre writes it and as EPUB 3 does, so whichever
  a reader looks for it finds the same answer, and correcting a series again
  replaces it rather than leaving two.

  The book is rewritten in place, beside itself and then moved over itself, so
  it is never half written. Only the package document changes: every other file
  is copied across byte for byte, and `identity` ignores the package document
  precisely so that a corrected book and the copies of it already on a reader or
  a Kobo stay one book rather than becoming two. The correction reaches those
  copies the ordinary way, by copying the book to them again.

- **,** opens settings when discovery and copying are idle. Edit books/import
  folders, Kobo mount, reader address, mounted reader card, and remote base folder.
  Arrows or Tab select a field/action; Enter edits or runs it. Within a field,
  arrows and Home/End move the cursor, Ctrl+U clears it, Enter accepts, and Escape
  undoes the edit. Escape outside an edit discards the draft. The connection test
  reads CrossPoint status without copying. A configured card takes priority over
  Wi-Fi; clear its field to switch back.

Enter does whatever the selected row is for, and **e** always edits it instead.
The Kobo mount is the one value Crossload can find by itself, so Enter on that
row searches the conventional Linux and macOS mount locations rather than asking
for a path: a single match fills the row in, several offer a choice, and none
says to connect the reader in USB mode and try again. Nothing is detected behind
your back, and **e** types a mount path in by hand at any point.

Each row is its name in bold with its value indented beneath it, so which line
is which stays clear when the selection is elsewhere. The line under the fields
explains whichever row is selected, until an action reports a result there; moving to another row brings its explanation back. The
two folders a save needs carry a star in their label. Each path says what it
currently points at — `✓ folder found`, `✓ Kobo found`, `· created on first
import`, `⚠ no Kobo database here`, `✗ not found` — and that judgement is made
again after every keystroke, so a typo is visible where it is made. Paths inside
the home directory are shown and may be entered as `~/…`; they are saved
expanded. **Ctrl+S** saves from any row, including from inside an edit. A save
that cannot go through moves the selection to the row responsible and says why,
and nothing is written.

When no import folder is supplied or saved, the TUI opens setup before scanning.
Books and import folders initially use the browsing directory (the current
directory by default), and the reader address starts at `crosspoint.local`,
the name CrossPoint announces itself under. While setup is open nothing is being
scanned, and the location pills say `not scanned` rather than showing progress.
Setup can be skipped with Escape; the library then browses the current directory
and, when it holds no books, says to press **,**. **Save and rescan** validates
the draft, saves it to the active configuration file (including `--config`), and
scans the new locations. Settings initially show the effective values, including
CLI overrides; saving makes those displayed values the defaults. No configuration
is written when setup or settings is cancelled. A new import directory is created
only when a book is imported.

Crossload carries EPUB, PDF and CBZ, in every location and in both directions.
EPUB is the only format it rewrites: its title and author come from the file,
its images are optimized for the X4, and it is filed under its author. A PDF or
CBZ is discovered, hashed, copied and verified exactly like an EPUB but never
rebuilt, so it is titled by its filename, has no author, and is never marked as
a device copy. A file whose name claims a format its contents do not match is
listed as unreadable rather than skipped; a format Crossload does not carry,
such as CBR, is not listed at all. `crossload optimize` refuses anything but an
EPUB, since there is nothing in the others for it to rebuild. The 128 MiB limit
on a book applies to every format.

A PDF reaches the reader by being converted. Crossload judges every PDF as it
reads it — structure only, never the words — and stores that judgement with the
book, so the copy dialog never has to open a file to draw itself. A PDF of
ordinary single-column text is offered as `convert to EPUB, copy`. One that will
convert badly is offered in yellow and asks for a **y** first, naming what is
wrong with it. One with nothing to convert, because its pages are scanned images
or because it is password protected, is refused in red. `crossload sync` applies
the same rules without the question.

Converting writes the EPUB into the import folder and sends that; the PDF is not
touched, moved or replaced, so both remain and the result can be read before it
is trusted. Paragraphs are rebuilt from where lines sit on the page, a page's
columns are read one at a time rather than straight across, running heads and
page numbers are dropped, and headings become the table of contents. Images,
tables and footnotes are not carried over, and a word broken across a line
break loses its hyphen, so a compound such as real-time can come back as
realtime.

Discovery lists only EPUB on the reader, mirroring what the reader itself lists.
A PDF or CBZ left on the device by other means is skipped rather than shown as a
copy that has arrived, so the book still reads as missing from CrossPoint and can be
converted and sent properly. Such a file stays where it is; Crossload neither
lists it nor removes it.

The reader takes EPUB only. CrossPoint stores whatever is uploaded, but the X4's
library lists nothing else: every entry its `/api/files` returns carries an
`isEpub` flag that is false for anything but an EPUB, and a PDF left on the
device is never shown by its reading app. CrossPoint is therefore refused as a
destination for a PDF or CBZ — in the copy dialog, in `crossload sync`, and in
`crossload send` — and the reason is given in red rather than the copy being
made and quietly wasted. A copy already on the reader in such a format is
labelled in the details panel as held but not listed. Kobo and local folders
accept every format Crossload carries.

Optimizing means resizing a book's images to the screen that will show them, so
which screen matters. Crossload prepares for the XTeink X4's 480×800 unless
`profile` names another, and the only screen it knows is the X4: a device is
named here once its size is known from the hardware, because guessing would
degrade every image to a size no device has. Saving a name it does not know is
refused at the time it is saved, rather than failing every command afterwards.

The screen a copy was made for is written into the copy, and it is part of the
identity cache's keys, so a copy made for one device is never served as one made
for another. When a book is sent over Wi-Fi the reader is asked what it is, and
says so if that is not the screen the book was prepared for — the one moment its
name is known without an extra request. A mounted card cannot be asked at all.

A book sent to a Kobo is an ordinary EPUB unless `kepub` is saved, in which
case it is sent as a kepub: named `.kepub.epub`, with its prose divided into
`koboSpan` elements. That is what the Kobo's own reading engine counts, and so
what makes progress and reading statistics work; a plain sideloaded EPUB gets
the generic engine instead. Turn it on with `crossload config set --kepub true`.

The rewrite is deliberately shallow. It never parses and re-serializes a
document, only splices spans around text it has already found, so entities
arrive exactly as they left. Every document it touches must still parse
afterwards or the whole conversion is refused rather than a broken book written,
and a book that is already divided — by this or by Calibre — is left alone
rather than divided twice. Only EPUB is converted; a PDF or CBZ goes to the Kobo
as it is.

`crossload kobo notes` prints the highlights and notes made on a Kobo as
Markdown, grouped by book, each with how far into the book it sits and the day
it was made. `--output DIR` writes one file per book instead of printing, and
never overwrites one that is already there; `--json` gives the same as data.
The book's own words are quoted and the reader's are not, so which is which
survives the trip. A dog-ear marks a page and says nothing, so there is nothing
in it to carry off; a mark that was taken back stays taken back. Nothing is
written to the device: its database is read, and only read.

Every copy is written down. `crossload history` shows the most recent, with
`-n` for how many, `--failed` for only the ones that did not arrive, and `--json`
for the record as data. **h** shows the same in the library, newest first.

The record is one JSON object per line in `$XDG_STATE_HOME/crossload/history.jsonl`,
or `~/.local/state/crossload/history.jsonl` when that is unset, and under
`~/Library/Application Support/crossload` on macOS. It is state rather than
cache, so it does not live with the things that are disposable, and it keeps the
last 5000 copies. Writing it is a courtesy and never a condition: a copy that
succeeded is never reported as failed because its line could not be written, and
a line that cannot be parsed is skipped rather than spoiling the rest.

ACSM files appear as local import requests. Choose Local first to fulfill them,
then refresh to copy the resulting EPUB. A fulfilled request is moved into an
`archive/` folder beside itself, so it stops appearing as pending; nothing is
deleted, an archived name is never overwritten, and a request whose outcome is
uncertain stays where it is. `crossload import` archives the same way. Existing
activation setup still applies.
An interactive terminal is required. Repairs remain in `crossload send --repair`.

## The catalog without a terminal

`crossload books` prints the same unified catalog the TUI shows, using the same
discovery and the same identity cache. Location states go to standard error, so
redirected output stays data: a header and one tab-separated line per book.
`--json` prints the catalog as structured JSON instead.

```sh
crossload books
crossload books --json | jq '.[] | select(.copies | length == 1) | .title'
```

`crossload sync --from PLACE --to PLACE` copies every book that one location has
and another lacks, where PLACE is `local`, `kobo` or `xteink`. It is a read-only
dry run by default; `--apply` performs the copies.

```sh
crossload sync --from kobo --to xteink            # show what is missing
crossload sync --from kobo --to xteink --apply    # copy it
```

Both locations must be available, or nothing is planned. Originals are preferred
over device copies exactly as in the TUI, so the FROM column may name a location
other than `--from` when a better source exists. Books already at the
destination are never recopied, existing files are never overwritten, and every
copy is verified; a book that fails is reported on its own line while the rest
continue, and the command then exits non-zero.

`crossload kobo sync` remains separate: it is the Kobo-to-reader path with
Wi-Fi repair of interrupted uploads and per-book exclusions.

## Rename compatibility

Crossload was previously called xteink. Both `crossload` and the compatibility
command `xteink` are built from the same code. Existing activation and recovery
state stays in the documented `xteink` data directory, so no activation import
is needed. Existing optimized EPUB markers and incomplete-transfer names remain
compatible. The repository moved from `eelcoh/xteink` to `eelcoh/crossload` on
2026-09-14; GitHub redirects the old URL, but existing clones should be pointed
at the new one:

```sh
git remote set-url origin git@github.com:eelcoh/crossload.git
```

A local checkout directory still named `xteink` keeps working; only the remote
matters. The local build image is still `localhost/xteink-build` unless
`CROSSLOAD_BUILD_IMAGE` says otherwise.

## Saved defaults

Save the paths and address once:

```sh
crossload config set --device /run/media/eelco/KOBOeReader \
  --output ~/Books --browse ~/Books --reader crosspoint.local
crossload config show
```

Then use shorter commands:

```sh
crossload books
crossload sync --from kobo --to xteink
crossload kobo list
crossload kobo import BOOK_ID                 # local import only
crossload kobo import BOOK_ID --send-to       # explicitly transfer to saved reader
crossload import book.acsm --send-to
crossload send book.epub                     # saved reader address
crossload tui --send-to
```

An explicit value, such as `--device /Volumes/KOBOeReader` or
`--to 192.168.178.114`, overrides the saved default for that invocation.
A saved reader never starts a transfer on its own. In the TUI it enables discovery;
Enter and a destination choice are required to copy.
`--send-to` without a value requests the saved reader; `--copy-to` without a value
requests the saved mounted-card directory. `send` and `copy` use their respective
saved destination when `--to` is omitted. These destinations remain mutually
exclusive for imports and the TUI.

The file is `~/.config/crossload/config.json` on Linux and macOS, or
`$XDG_CONFIG_HOME/crossload/config.json` when XDG_CONFIG_HOME is absolute.
Use global `--config <file>` to select a separate configuration. No file is
created until you save a setting. Activation data stays in its existing location.

Available settings: `device`, `output`, `reader`, `copy-to`, `browse`, `folder`.
Paths must be absolute or start with `~/`; they may point to an unplugged device.
Mounted destinations must exist when actually transferring. Updates preserve
other settings, and invalid settings leave the previous configuration intact.

```sh
crossload config set --copy-to /Volumes/XTEINK/Books --folder /Books
crossload config unset reader
```

## Listing output

Kobo listings use a borderless table in a terminal. When piped or redirected,
output is tab-separated with a header and exactly one line per book: no wrapping,
decorative lines, or summary footer. Tabs and line breaks inside metadata are
replaced with spaces. JSON output remains available through `--json`.

```sh
crossload kobo list | less -S
crossload kobo list | grep -i herron
crossload kobo list > books.tsv
```

The TUI uses Tears 0.8.0 and Ratatui 0.30.2 with Elm-style messages and effects.
The book table shows title, locations and author, with selected-copy details
below when height permits. Background discovery reports location availability;
search and selection remain responsive while locations load.
[TUI-ARCHITECTURE.md](TUI-ARCHITECTURE.md) records the design and migration results.

Direct ByteBooks account activation has been dropped from the roadmap; the
Calibre activation-export workflow above is the supported setup method. [BYTEBOOKS-ACTIVATION.md](BYTEBOOKS-ACTIVATION.md)
records the protocol investigation and implementation requirements.
Run `mise run test:tui` for the synthetic terminal smoke check (host Python 3 is
required). It covers search, verified copying, resizing, failure display and
terminal restoration without a real reader. Native macOS execution is pending.

macOS baseline update: the user compiled and smoke-tested the initial GitHub
Kobo CLI revision (26180b0) on a Mac. Kobo USB could not be tested on the corporate
Mac. This does not yet verify the newer native ADEPT and Tears TUI changes.

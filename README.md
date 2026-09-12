# xteink

A small native CLI for importing books to read on an Xteink. It supports
downloaded Kobo store books and sideloaded EPUBs from a mounted Kobo, with local EPUB output.
It uses Flamberge's Rust libraries and bundled SQLite; Python and Calibre are not
required at runtime.

## Install

xteink targets Linux and macOS. Linux has been tested on a real Kobo; macOS
builds and tests are configured in CI but have not yet been verified here.
There are no published binary releases yet. Install from this checkout or use
its locally built executable.

### From source

Install [Rust through rustup](https://rust-lang.org/tools/install/) and a native
C compiler/linker. On macOS, the Xcode Command Line Tools provide the compiler;
on Linux, use your distribution's C development tools. Git and network access
are needed for the initial dependency download. Rustup selects the version in
[`rust-toolchain.toml`](rust-toolchain.toml).

Run these commands from the project directory:

```sh
cargo install --path . --locked
xteink --version
```

Cargo normally installs the binary into `~/.cargo/bin`. Ensure that directory is
on your `PATH`; reopening the terminal after installing rustup may be enough.
See [Cargo's installation documentation](https://doc.rust-lang.org/cargo/commands/cargo-install.html)
for custom installation locations. Run the same installation command after
updating the checkout to rebuild and update xteink.

### From an existing local build

If `target/release/xteink` has already been built for your OS and architecture,
you can run it directly or install it in your user bin directory:

```sh
./target/release/xteink --help
mkdir -p "$HOME/.local/bin"
install -m 755 target/release/xteink "$HOME/.local/bin/xteink"
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

Connect the Kobo and select its USB connection mode. Supply the mount directory
that contains `.kobo`, for example:

```sh
xteink kobo list --device /run/media/you/KOBOeReader
xteink kobo list --device /run/media/you/KOBOeReader --json
xteink kobo import BOOK_ID --device /run/media/you/KOBOeReader --output ~/Books

# On macOS, for example:
xteink kobo list --device /Volumes/KOBOeReader
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

## Troubleshooting

| Message or situation | What to do |
| --- | --- |
| No `.kobo/KoboReader.sqlite` found | Connect the Kobo in USB mode and pass its mount root, not the `.kobo` folder. |
| An entry is a preview | Download the full edition on the Kobo, then reconnect. A separate sideloaded copy may already appear in the list. |
| Cannot read `device.xml` or no serial found | Supply the device serial with `--serial SERIAL`. Plain sideloaded EPUBs do not need it. |
| Existing output file | The importer never overwrites files. Choose another output directory if you need another copy. |
| EPUB validation fails | The input may be incomplete, still encrypted, or use an unsupported format/encoding. No output EPUB is saved. |
| Several entries share a title | Check `SOURCE` and `TYPE`; use `--json` to inspect sideloaded paths. Every separate file has its own ID. |

Run `xteink --help` or `xteink kobo import --help` for command options. Errors
are printed to stderr and return a nonzero exit status. JSON list output goes
to stdout without table decoration or summary text.

## Build and check

The toolchain is pinned in `rust-toolchain.toml`. With rustup installed:

```sh
cargo build --release --locked
./target/release/xteink --help
cargo test --locked
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
```

The resulting executable is `target/release/xteink`. The normal Linux build
uses system C runtime libraries and embeds SQLite. It does not require a system
SQLite installation. A fully static Linux release is a later packaging step.

For a container build on Linux, using Podman from this project directory
(no host Rust or C toolchain required):

```sh
podman run --rm --userns=keep-id --security-opt label=disable \
  -v "$PWD:/work" -w /work -e CARGO_HOME=/work/target/cargo-home \
  docker.io/library/rust:1-bookworm cargo build --release --locked
```

The container writes build artifacts and its Cargo download cache under
`target/`. The project toolchain remains pinned by `rust-toolchain.toml`.
The same container command can run `cargo test --locked` instead of the release
build. The container needs the source checkout, not access to your Kobo.

The [CI workflow](.github/workflows/check.yml) runs formatting, Clippy, tests,
and release builds on Linux and macOS. `Cargo.lock` is committed so builds use
the reviewed dependency versions. Build artifacts and local caches are ignored
by Git.

## Current limits

- Kobo Desktop data, account login, and downloads from Kobo's servers are not yet covered.
- Encrypted sideloaded Kobo copies currently require an identical store download
  on the same device. Sideloaded formats other than EPUB are not yet supported. Adobe-protected EPUBs
  are listed but cannot be imported until the Adobe backend is implemented.
- Adobe/ByteBooks ACSM imports and device transfer are planned next; their commands
  are not implemented. Adobe-protected files receive an explicit unsupported error.
- Validation checks the archive, package, reading order, and UTF-8 XHTML chapters.
  It is not full EPUBCheck. Books with non-UTF-8 XHTML are currently rejected.
- Input files are limited to 128 MiB, expanded archives to 256 MiB and 20,000 entries.
- Automated tests use synthetic books and databases. Linux listing, preview
  detection, and an encrypted import have also been checked on a real Kobo.
  Xteink rendering still needs validation. macOS CI is configured but has not run here.

See [NATIVE-OPTIONS.md](NATIVE-OPTIONS.md) for backend research and the roadmap,
and [THIRD-PARTY.md](THIRD-PARTY.md) for the pinned upstream implementation.

## Validation

The Linux build passes 26 integration tests and Clippy with warnings denied.
The release executable was also run directly on the Linux host. Its dynamic
dependencies are the system loader, libc, libm, and libgcc_s; SQLite is bundled.
The tests cover encrypted and plain imports, volume selection, WAL snapshots,
font metadata, incorrect serials, malformed books, previews, and output/source
protection. A preview regression fixture reproduces a nested package pointing to
`kobo-locked.html` while that placeholder lives at the archive root. Preview
classification uses Kobo's `Accessibility = 6`, also recognized by
[Calibre's Kobo driver](https://github.com/kovidgoyal/calibre/blob/master/src/calibre/devices/kobo/driver.py).

## Roadmap

1. Kobo listing and import, including previews and sideloaded EPUBs — implemented.
2. Adobe/ByteBooks activation import and ACSM-to-EPUB support through a native backend.
3. Optional transfer to a mounted destination and distributable platform binaries.
4. A TUI using the same import library.

See [NATIVE-OPTIONS.md](NATIVE-OPTIONS.md) for the backend investigation and
[idea](idea) for the original project note.

## License

xteink is licensed under the [MIT license](LICENSE). Kobo processing uses pinned
[Flamberge](https://github.com/kessriga/flamberge) Rust libraries; attribution and
dependency details are in [THIRD-PARTY.md](THIRD-PARTY.md).

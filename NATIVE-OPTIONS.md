# Native implementation investigation

Investigated 2026-09-12. Target: a standalone CLI for Linux and macOS, with
optional device transfer and a TUI later.

## Recommendation

Build the application in Rust. Evaluate Flamberge's Rust crates for Kobo import
and integrate libgourou through a small C++ bridge for Adobe activation,
ACSM fulfillment, and EPUB processing. This can produce one executable per
platform without Python or Calibre. Bundling the native dependencies is a build
milestone, not something already demonstrated here.

Do not begin by porting the entire Adobe protocol to Rust. I did not find a
reusable Rust ACSM fulfillment implementation in this search. Flamberge handles
already-downloaded books, not ACSM authorization and download.

## Candidates

| Component | What exists | Fit |
| --- | --- | --- |
| Flamberge 0.1.1 | Rust libraries for Kobo and Adobe book processing and local key discovery | Best Kobo candidate; needs real-book validation |
| libgourou 0.8.10 | C++ Adobe activation, fulfillment, download, decryption, loan return | Best Adobe candidate; requires Rust bridge and packaging work |
| Native Knock fork | C++ CLI using a bundled libgourou | Useful integration reference; no Kobo support |

### Flamberge

Inspected commit `eff8713bd51e7f8811b9695507abb0d0dc2eda80`.

The project publishes reusable `flamberge-keys` and `flamberge-schemes` crates,
with an MIT license declaration. Its manifest bundles SQLite and uses RustCrypto
for cryptography. Its release workflow targets Linux x86-64 GNU and macOS ARM64;
Intel Mac and Linux ARM64 release builds would be additional work. The GNU target
alone does not establish a fully static Linux binary.

Sources: [manifest](https://github.com/kessriga/flamberge/blob/eff8713bd51e7f8811b9695507abb0d0dc2eda80/Cargo.toml),
[release workflow](https://github.com/kessriga/flamberge/blob/eff8713bd51e7f8811b9695507abb0d0dc2eda80/.github/workflows/release.yml).

Kobo support includes device discovery, user-key derivation, database lookup,
and EPUB reconstruction. The library accepts a database and explicit volume ID.
Our application would supply book enumeration, device-path selection, output
naming, and transfer. Explicit device selection also avoids reliance on desktop
discovery and its external network-interface commands.

Sources: [Kobo keys](https://github.com/kessriga/flamberge/tree/eff8713bd51e7f8811b9695507abb0d0dc2eda80/crates/flamberge-keys/src/kobo),
[Kobo processing](https://github.com/kessriga/flamberge/tree/eff8713bd51e7f8811b9695507abb0d0dc2eda80/crates/flamberge-schemes/src/kobo).

Compatibility limitations found during inspection:

- Integration fixtures are synthetic and generated using the project's own
  primitives. Passing them would not establish compatibility with actual books.
- The Adobe path appears to lack the extra `keyType` unwrapping handled by
  libgourou and Python DeDRM for hardened ADEPT EPUBs.
- EPUB repackaging removes `encryption.xml`; mixed encryption or font
  obfuscation needs closer review before relying on it for arbitrary books.
- Kobo database handling patches a temporary copy's WAL-mode header. We should
  verify snapshot consistency, especially if desktop data has an active WAL.

Sources: [test provenance](https://github.com/kessriga/flamberge/blob/eff8713bd51e7f8811b9695507abb0d0dc2eda80/crates/flamberge-integration-tests/README.md),
[Adobe processing](https://github.com/kessriga/flamberge/blob/eff8713bd51e7f8811b9695507abb0d0dc2eda80/crates/flamberge-schemes/src/adept.rs),
[EPUB handling](https://github.com/kessriga/flamberge/blob/eff8713bd51e7f8811b9695507abb0d0dc2eda80/crates/flamberge-formats/src/ocf.rs),
[database handling](https://github.com/kessriga/flamberge/blob/eff8713bd51e7f8811b9695507abb0d0dc2eda80/crates/flamberge-keys/src/kobo/db.rs).

### libgourou

Inspected upstream commit `906b8a37334ac5d37dcae24c7508a427c19dbb66`, dated
2026-09-02, version 0.8.10. Prefer this upstream over older GitHub mirrors.

It exposes activation, fulfillment, download, DRM removal, and loan-return APIs.
Its reference client uses curl, OpenSSL, libzip, and pugixml; uPDFParser is an
internal dependency. The library declares LGPL-3.0-or-later and utilities BSD.
Record dependency licenses and distribution requirements when packaging.

ByteBooks is the current name following its acquisition of Adobe Digital
Editions, as confirmed by the user. The current upstream README says new device
activation requires a ByteBooks account and describes Adobe ID accounts as
deprecated. Start with importing the working Mac activation; compatibility of
that activation and fresh ByteBooks authorization still need testing.

Source: [upstream README](https://forge.soutade.fr/soutade/libgourou/src/commit/906b8a37334ac5d37dcae24c7508a427c19dbb66/README.md).

The build includes Darwin shared-library targets and Apple-specific device code.
However, `STATIC_UTILS=1` links libgourou statically while other dependencies
remain separately linked. It is not a turnkey self-contained build. The static
archive rule also uses GNU-style `ar --thin`, which warrants macOS toolchain
verification.

Sources: [Makefile](https://forge.soutade.fr/soutade/libgourou/src/commit/906b8a37334ac5d37dcae24c7508a427c19dbb66/Makefile),
[utility Makefile](https://forge.soutade.fr/soutade/libgourou/src/commit/906b8a37334ac5d37dcae24c7508a427c19dbb66/utils/Makefile).

Proposed integration: a narrow bridge that catches C++ exceptions and returns
structured errors to Rust. Link the library into the executable rather than
launching separately installed utilities. On macOS, one distributed executable
may still link to system libraries; it need not be fully static.

### Knock

The [MCTRACO fork](https://github.com/MCTRACO/knock) contains a native C++ CLI
around libgourou. Its README describes Linux support and macOS as pending.
Its Makefile links curl, crypto, zip, and zlib. It is an integration example,
not a complete cross-platform foundation for both requested workflows.
An [older Knock fork](https://github.com/esn/knock/blob/main/knock) instead has a
Python entry point, so the name alone does not identify a native implementation.

## Roadmap

1. **Kobo import:** validate Flamberge against one real device book, then implement
   explicit device selection, listing, import, EPUB validation, and local output.
2. **Adobe:** prove the native bridge builds on Linux and macOS, import existing
   activation, and validate an ACSM-to-readable-EPUB round trip. Add fresh account
   setup after checking current upstream authentication behavior.
3. **Transfer and packaging:** optional copy to a mounted destination, binaries
   for the required architectures, and checks for unexpected runtime dependencies.
4. **TUI:** add browsing and progress UI over the same application library.

This investigation reviewed source and build configuration. No compilation or
real-book tests were run; Cargo, rustc, and a C++ compiler were not on PATH in
this workspace. Source checkouts were placed in `/tmp`; no credentials or books
were accessed.

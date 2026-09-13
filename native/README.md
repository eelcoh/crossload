# Native ADEPT backend

`build.rs` compiles `bridge.cpp`, libgourou, uPDFParser and pugixml with `cc`,
and libzip with CMake. Cargo's `openssl-sys`, `curl-sys` and `libz-sys` supply
vendored static OpenSSL (including its legacy provider), curl and zlib.
Only the platform C/C++ runtime libraries remain external. No command-line
helper, Calibre installation, or downloaded executable is invoked at runtime.

Each vendor directory has an `UPSTREAM` file recording its source and exact
revision. The build does not run upstream setup scripts or fetch floating Git
branches. Cargo downloads are pinned by the root lockfile.

The C ABI accepts paths and a bounded error buffer. It catches all C++ exceptions;
Rust serializes calls because libgourou and its client use global state. A file
lock also serializes processes sharing activation/recovery data. Operations are
activation validation, fulfillment, receipt-based download, and EPUB decryption.

Local changes to libgourou are also recorded in [libgourou.patch](libgourou.patch)
against the revision in `UPSTREAM`:

- Preserve the HMAC name/value before removing its XML node for signing.
- Append fetched license-service information to the actual activation document
  root, supporting both `activationInfo` and `activation_info`.
- Update activation XML using a temporary file and rename.
- Disable connection/receive retries, bound HTTP response/download sizes, add
  timeouts and redirect limits, restrict URLs to HTTP(S), and prohibit an HTTPS
  redirect to HTTP. CA paths come from `openssl-probe` (`SSL_CERT_FILE` supported).
- Bound ZIP reads/inflation, check ZIP reads and close errors, and use the correct
  compressed length for encrypted entries stored in the archive.
- Reject short/misaligned AES ciphertext and propagate inflation failures instead
  of silently skipping encrypted entries.
- Check the interface address pointer before dereferencing it on macOS/BSD.

HTTP is still allowed because some ADEPT operators/book URLs use it. TLS certificate
verification remains enabled. Requests may update the imported activation copy
with operator information. Source activation exports are never changed.

The synthetic integration tests create their own RSA/PKCS12 activation and AES
EPUB; a loopback HTTP server exercises signed fulfillment, a failed download,
receipt reuse, and decryption. The user has additionally confirmed a live Linux ACSM import and reading on
CrossPoint; the synthetic tests remain independent of that account.
macOS is covered by the CI configuration but must be run on a Mac runner.

## Source and binary distribution

The Rust application and bridge are MIT; libgourou and uPDFParser are
LGPL-3.0-or-later. Their license texts and sources are included here, including the incorporated
[GPL version 3 terms](vendor/GPL-3.0.txt). A statically
linked binary must be distributed with the required corresponding source and
material allowing recipients to rebuild/relink with modified LGPL libraries.
This checkout plus the locked Cargo dependencies and build instructions provides
the rebuild route; bundle the corresponding complete source/dependencies and
license notices with any future binary release. Do not label the combined binary
as containing only MIT code. `scripts/package.py` now generates binary notices
and corresponding source archives with vendored Cargo dependencies and offline
build instructions; distribute both archives together.

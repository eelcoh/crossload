# Third-party components

Kobo key derivation and decryption use the `flamberge-keys` and
`flamberge-schemes` crates from
[Flamberge](https://github.com/kessriga/flamberge), pinned to commit
`eff8713bd51e7f8811b9695507abb0d0dc2eda80` (version 0.1.1).
The upstream declares the MIT license; its notice is reproduced below.
`Cargo.lock` records all resolved dependencies. SQLite is compiled into the binary
through `rusqlite`'s bundled feature.

Flamberge provides book processing only. xteink supplies the device workflow,
database snapshots, book selection, output validation, and filesystem handling.

Other dependencies retain their respective licenses. `scripts/package.py`
collects dependency license files and a package inventory into the binary
archive, and bundles all locked sources into its accompanying source archive.


## Flamberge license

```text
MIT License

Copyright (c) 2026 Kessriga Jeükal

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Native ADEPT backend

The source revisions are recorded in each vendor directory's `UPSTREAM` file.

| Component | License | Source and notice |
| --- | --- | --- |
| libgourou 0.8.10 | LGPL-3.0-or-later | [Source](native/vendor/libgourou), [license](native/vendor/libgourou/LICENSE) |
| libgourou reference client | BSD-3-Clause | [Notice in source](native/vendor/libgourou/utils/drmprocessorclientimpl.cpp) |
| uPDFParser | LGPL-3.0-or-later | [Source](native/vendor/updfparser), [license](native/vendor/updfparser/LICENSE) |
| pugixml 1.15 | MIT | [License](native/vendor/pugixml/LICENSE.md) |
| libzip 1.11.4 | BSD-3-Clause | [License](native/vendor/libzip/LICENSE) |

OpenSSL, curl, and zlib are statically compiled from the sources selected by
`Cargo.lock` through their Cargo sys crates. Their licenses and those of all
transitive dependencies also apply. See [native/README.md](native/README.md) for
local upstream changes and the source/relinking requirements for binary releases.

## Terminal interface

Crossload uses Tears 0.8.0 (Apache-2.0), Ratatui (MIT), and Tokio (MIT).
Cargo.lock records all resolved dependencies; packaging collects their license
notices and includes their sources in the offline source archive.

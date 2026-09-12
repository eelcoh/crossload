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

Other dependencies retain their respective licenses. Before publishing binaries,
generate the complete dependency license notices for the resolved Cargo graph.


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

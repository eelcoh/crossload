//! Spike: convert a PDF to an EPUB beside it, and say what it thought.
fn main() {
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).unwrap();
        match crossload::pdf::convert(&data) {
            Ok((epub, verdict)) => {
                let out = format!("{}.epub", path.trim_end_matches(".pdf"));
                std::fs::write(&out, &epub).unwrap();
                println!("{out}  ({} KiB, {})", epub.len() / 1024, verdict.label());
            }
            Err(e) => println!("{path}: {e:#}"),
        }
    }
}

//! Spike: rewrite real EPUBs as kepubs and prove nothing but spans changed.
use std::io::Read;

/// Remove exactly what the rewrite inserts: each koboSpan opening tag and the
/// closing tag that follows it. Our spans wrap pure text, so the matching close
/// is always the next one.
fn undo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("<span class=\"koboSpan\" id=\"kobo.") {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let inner = &after[open_end + 1..];
        let Some(close) = inner.find("</span>") else {
            break;
        };
        out.push_str(&inner[..close]);
        rest = &inner[close + "</span>".len()..];
    }
    out.push_str(rest);
    out
}

fn documents(data: &[u8]) -> Vec<(String, String)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).unwrap();
    let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
    let mut out = vec![];
    for name in names {
        let mut text = String::new();
        if zip
            .by_name(&name)
            .unwrap()
            .read_to_string(&mut text)
            .is_ok()
        {
            out.push((name, text));
        }
    }
    out
}

fn main() {
    let mut checked = 0;
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).unwrap();
        let name = path.rsplit('/').next().unwrap_or(&path);
        match crossload::kepub::kepubify(&data) {
            Ok(out) => {
                let before: std::collections::BTreeMap<_, _> =
                    documents(&data).into_iter().collect();
                let after = documents(&out);
                let mut spans = 0;
                for (file, text) in &after {
                    spans += text.matches("koboSpan").count();
                    if let Some(original) = before.get(file) {
                        // A document left alone has nothing to undo; one that
                        // was divided must come back exactly when undivided.
                        if text != original {
                            assert_eq!(
                                &undo(text),
                                original,
                                "{name}: {file} changed by more than spans"
                            );
                            checked += 1;
                        }
                    }
                }
                println!("ok  {spans:>6} spans, every document reversible  {name}");
            }
            Err(e) => println!("FAILED  {name}: {e:#}"),
        }
    }
    println!("\n{checked} documents verified byte-for-byte after removing the spans");
}

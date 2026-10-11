//! Spike: report what the classifier makes of real PDFs, without converting.
use crossload::pdf::{inspect, Verdict};

fn main() {
    for path in std::env::args().skip(1) {
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(e) => {
                println!("{path}\n  unreadable: {e}\n");
                continue;
            }
        };
        let name = path.rsplit('/').next().unwrap_or(&path);
        match inspect(&data) {
            Ok(report) => {
                println!("{name}  ({} KiB)", data.len() / 1024);
                println!(
                    "  {}  ·  {} pages, {} with text, {} image-only, {} two-column, outline {}",
                    report.verdict.label(),
                    report.pages,
                    report.text_pages,
                    report.image_pages,
                    report.column_pages,
                    if report.outline { "yes" } else { "no" },
                );
                match &report.verdict {
                    Verdict::Good => {}
                    Verdict::Poor(concerns) => {
                        for concern in concerns {
                            println!("    - {}", concern.describe());
                        }
                    }
                    Verdict::Impossible(concern) => println!("    - {}", concern.describe()),
                }
                println!();
            }
            Err(e) => println!("{name}\n  {e:#}\n"),
        }
    }
}

//! Corpus-scale A/B comparison between the hayro and pdfium backends — the
//! migration gate from SPEC.md §5. Renders every page of every PDF given on
//! the command line through both engines and reports pixel and text-layer
//! divergence.
//!
//! ```sh
//! cargo run -p junk-libs-platen --features backend-pdfium \
//!     --example ab_compare -- corpus/*.pdf
//! ```

use junk_libs_platen::{hayro_backend, pdfium_backend};

/// Pixels differing by more than this in any channel count as "different".
const CHANNEL_TOLERANCE: u8 = 32;
/// Flag a page when more than this fraction of pixels differ (engines
/// legitimately antialias edges differently).
const PIXEL_FRACTION_LIMIT: f64 = 0.02;
/// Flag a char box whose left edge differs by more than this many points.
const BOX_X_TOLERANCE: f32 = 2.0;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // --dump <dir>: write side-by-side PNGs for flagged pages.
    let dump_dir = args
        .iter()
        .position(|a| a == "--dump")
        .map(|i| {
            args.remove(i);
            std::path::PathBuf::from(args.remove(i))
        });
    if args.is_empty() {
        eprintln!("usage: ab_compare [--dump <dir>] <file.pdf> [more.pdf ...]");
        std::process::exit(2);
    }
    if let Some(d) = &dump_dir {
        std::fs::create_dir_all(d).expect("create dump dir");
    }

    let mut flagged = 0usize;
    let mut errored = 0usize;

    for path in &args {
        let path = std::path::Path::new(path);
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        let hayro_pages = match hayro_backend::render_pdf(path, 1.0) {
            Ok(p) => p,
            Err(e) => {
                println!("ERROR  {name}: hayro: {e}");
                errored += 1;
                continue;
            }
        };
        let pdfium_pages = match pdfium_backend::render_pdf(path, 1.0) {
            Ok(p) => p,
            Err(e) => {
                println!("ERROR  {name}: pdfium: {e}");
                errored += 1;
                continue;
            }
        };
        if hayro_pages.len() != pdfium_pages.len() {
            println!(
                "FLAG   {name}: page count {} vs {}",
                hayro_pages.len(),
                pdfium_pages.len()
            );
            flagged += 1;
            continue;
        }

        let mut doc_flagged = false;
        for (i, (h, p)) in hayro_pages.iter().zip(pdfium_pages.iter()).enumerate() {
            // --- pixels ---
            let (w, hh) = (
                h.image.width().min(p.image.width()),
                h.image.height().min(p.image.height()),
            );
            let mut differing = 0u64;
            for y in 0..hh {
                for x in 0..w {
                    let a = h.image.get_pixel(x, y).0;
                    let b = p.image.get_pixel(x, y).0;
                    if a.iter()
                        .zip(b.iter())
                        .any(|(c, d)| c.abs_diff(*d) > CHANNEL_TOLERANCE)
                    {
                        differing += 1;
                    }
                }
            }
            let fraction = differing as f64 / f64::from(w * hh).max(1.0);

            // --- text layer ---
            // Compare whitespace-normalized: both engines *synthesize*
            // whitespace PDFs don't contain (pdfium: spaces + \r\n; hayro
            // backend: spaces + \n), and the exact forms legitimately differ.
            let h_text: String = h.char_boxes.iter().map(|b| b.ch).collect();
            let p_text: String = p.char_boxes.iter().map(|b| b.ch).collect();
            // Also strip control chars: pdfium emits markers like \u{2} for
            // end-of-line hyphens, which aren't text.
            let normalize = |s: &str| {
                s.chars()
                    .filter(|c| !c.is_control() || c.is_whitespace())
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let text_match = normalize(&h_text) == normalize(&p_text);
            // Geometry pairs align only across real glyphs (synthetic
            // whitespace boxes are engine-specific).
            let geom_off = h
                .char_boxes
                .iter()
                .filter(|b| !b.ch.is_whitespace())
                .zip(p.char_boxes.iter().filter(|b| !b.ch.is_whitespace()))
                .filter(|(a, b)| (a.x - b.x).abs() > BOX_X_TOLERANCE)
                .count();

            if fraction > PIXEL_FRACTION_LIMIT || !text_match || geom_off > 0 {
                println!(
                    "FLAG   {name} p{}: pixels {:.2}% text {} geom-off {} (h:{} chars, p:{} chars)",
                    i + 1,
                    fraction * 100.0,
                    if text_match { "ok" } else { "DIFF" },
                    geom_off,
                    h.char_boxes.len(),
                    p.char_boxes.len(),
                );
                doc_flagged = true;
                if let Some(d) = &dump_dir {
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                    let _ = h.image.save(d.join(format!("{stem}-p{}-hayro.png", i + 1)));
                    let _ = p.image.save(d.join(format!("{stem}-p{}-pdfium.png", i + 1)));
                    let _ = std::fs::write(
                        d.join(format!("{stem}-p{}-text.txt", i + 1)),
                        format!("hayro : {h_text:?}\npdfium: {p_text:?}\n"),
                    );
                }
            } else {
                println!(
                    "ok     {name} p{}: pixels {:.2}% text ok ({} chars)",
                    i + 1,
                    fraction * 100.0,
                    h.char_boxes.len()
                );
            }
        }
        flagged += usize::from(doc_flagged);
    }

    println!("\n{} file(s); {flagged} flagged, {errored} errored", args.len());
    if flagged + errored > 0 {
        std::process::exit(1);
    }
}

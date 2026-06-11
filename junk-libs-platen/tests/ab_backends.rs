//! A/B comparison between the hayro and pdfium backends — the migration gate
//! from SPEC.md §5, in miniature. Runs only when both backend features are
//! enabled: `cargo test -p junk-libs-platen --features backend-pdfium`.
//!
//! The corpus-scale version of this (imposition inputs, arXiv PDFs) runs as a
//! dev-time xtask; these tests pin the *mechanics* — same text, compatible
//! geometry, similar pixels — on the shared fixture.

#![cfg(all(feature = "backend-hayro", feature = "backend-pdfium"))]

mod common;
use common::minimal_pdf;

use junk_libs_platen::{hayro_backend, pdfium_backend};

fn fixture_path(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, minimal_pdf()).expect("write fixture");
    path
}

#[test]
fn text_layers_agree() {
    let path = fixture_path("junk-libs-platen-ab-text.pdf");
    let hayro_pages = hayro_backend::render_pdf(&path, 1.0).expect("hayro render");
    let pdfium_pages = pdfium_backend::render_pdf(&path, 1.0).expect("pdfium render");
    let _ = std::fs::remove_file(&path);

    // Whitespace-normalized: both engines synthesize whitespace the PDF
    // doesn't contain (pdfium: \r\n + spaces; hayro backend: \n + spaces) and
    // the exact forms differ legitimately.
    let normalize = |pages: &[junk_libs_platen::RenderedPage]| {
        pages[0]
            .char_boxes
            .iter()
            .map(|b| b.ch)
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert_eq!(
        normalize(&hayro_pages),
        normalize(&pdfium_pages),
        "extracted strings differ"
    );

    // Per-character geometry over real glyphs (synthetic whitespace boxes are
    // engine-specific): same left edge within a couple of points, and
    // overlapping vertical extent. Heights are allowed to differ — hayro uses
    // a nominal ascent/descent cell (SPEC.md §4) — but each pair of boxes
    // must cover the same place on the page.
    for (h, p) in hayro_pages[0]
        .char_boxes
        .iter()
        .filter(|b| !b.ch.is_whitespace())
        .zip(
            pdfium_pages[0]
                .char_boxes
                .iter()
                .filter(|b| !b.ch.is_whitespace()),
        )
    {
        assert_eq!(h.ch, p.ch);
        assert!(
            (h.x - p.x).abs() < 2.0,
            "'{}' left edge: hayro {} vs pdfium {}",
            h.ch,
            h.x,
            p.x
        );
        assert!(
            ((h.x + h.w) - (p.x + p.w)).abs() < 4.0,
            "'{}' right edge: hayro {} vs pdfium {}",
            h.ch,
            h.x + h.w,
            p.x + p.w
        );
        let overlap = (h.y + h.h).min(p.y + p.h) - h.y.max(p.y);
        assert!(
            overlap > 0.5 * p.h,
            "'{}' boxes barely overlap vertically: hayro y={}..{} vs pdfium y={}..{}",
            h.ch,
            h.y,
            h.y + h.h,
            p.y,
            p.y + p.h
        );
    }
}

#[test]
fn renders_agree() {
    let path = fixture_path("junk-libs-platen-ab-pixels.pdf");
    let (hayro_img, hayro_size) =
        hayro_backend::render_page_bitmap(&path, 0, 1.0).expect("hayro render");
    let (pdfium_img, pdfium_size) =
        pdfium_backend::render_page_bitmap(&path, 0, 1.0).expect("pdfium render");
    let _ = std::fs::remove_file(&path);

    assert!(
        (hayro_size.0 - pdfium_size.0).abs() < 1.0 && (hayro_size.1 - pdfium_size.1).abs() < 1.0,
        "page sizes differ: hayro {hayro_size:?} vs pdfium {pdfium_size:?}"
    );

    // Compare over the overlapping area (rasters may differ by a pixel of
    // rounding). Count pixels that differ meaningfully in any channel; edges
    // antialias differently between engines, so demand agreement on the bulk,
    // not the boundary.
    let w = hayro_img.width().min(pdfium_img.width());
    let h = hayro_img.height().min(pdfium_img.height());
    assert!(hayro_img.width().abs_diff(pdfium_img.width()) <= 1);
    assert!(hayro_img.height().abs_diff(pdfium_img.height()) <= 1);

    let mut differing = 0u64;
    for y in 0..h {
        for x in 0..w {
            let a = hayro_img.get_pixel(x, y).0;
            let b = pdfium_img.get_pixel(x, y).0;
            if a.iter().zip(b.iter()).any(|(c, d)| c.abs_diff(*d) > 32) {
                differing += 1;
            }
        }
    }
    let fraction = differing as f64 / (u64::from(w) * u64::from(h)) as f64;
    assert!(
        fraction < 0.02,
        "{:.2}% of pixels differ between backends",
        fraction * 100.0
    );
}

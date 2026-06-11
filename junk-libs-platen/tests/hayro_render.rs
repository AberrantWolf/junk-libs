//! End-to-end tests for the hayro backend, using a minimal PDF built in-test
//! (no engine needed to *create* fixtures, unlike junk-libs-pdfium's tests,
//! which used pdfium's creation API).

#![cfg(feature = "backend-hayro")]

mod common;
use common::minimal_pdf;

#[test]
fn renders_a_page_from_bytes() {
    let bytes = minimal_pdf();
    let (image, size_pts) =
        junk_libs_platen::render_page_bitmap_from_bytes(&bytes, 0, 1.0).expect("render");

    // A4 = 595×842pt; at scale 1.0 the raster matches in pixels.
    assert!(
        (size_pts.0 - 595.0).abs() < 5.0 && (size_pts.1 - 842.0).abs() < 5.0,
        "unexpected page size in points: {size_pts:?}"
    );
    assert!(
        (570..=620).contains(&image.width()) && (820..=870).contains(&image.height()),
        "unexpected raster size: {}×{}",
        image.width(),
        image.height()
    );

    // The 200×200pt rectangle must come out black-ish, the margins white, and
    // everything opaque (we render on an opaque white base).
    let in_rect = image.get_pixel(200, 842 - 200); // inside rect (y flipped)
    let in_margin = image.get_pixel(10, 10);
    assert!(
        in_rect.0[0] < 50 && in_rect.0[1] < 50 && in_rect.0[2] < 50,
        "rectangle pixel not black: {in_rect:?}"
    );
    assert_eq!(in_margin.0, [255, 255, 255, 255], "margin not opaque white");
    assert!(image.pixels().all(|p| p.0[3] == 255), "non-opaque pixel");
}

#[test]
fn page_out_of_range_is_typed() {
    let bytes = minimal_pdf();
    match junk_libs_platen::render_page_bitmap_from_bytes(&bytes, 7, 1.0) {
        Err(junk_libs_platen::Error::PageOutOfRange { index: 7, count: 1 }) => {}
        other => panic!("expected PageOutOfRange, got {other:?}"),
    }
}

#[test]
fn garbage_is_invalid_not_panic() {
    match junk_libs_platen::render_page_bitmap_from_bytes(b"not a pdf at all", 0, 1.0) {
        Err(junk_libs_platen::Error::Invalid) => {}
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn extreme_zoom_is_capped_not_oom() {
    let bytes = minimal_pdf();
    let (image, _) =
        junk_libs_platen::render_page_bitmap_from_bytes(&bytes, 0, 100.0).expect("render");
    assert!(image.width() <= junk_libs_platen::MAX_RENDER_DIMENSION);
    assert!(image.height() <= junk_libs_platen::MAX_RENDER_DIMENSION);
}

/// The whole point of leaving pdfium: concurrent renders of the same document
/// from multiple threads, no global lock. This deadlocks/segfaults on the old
/// engine's API shape; here it must just work.
#[test]
fn renders_in_parallel() {
    let bytes = std::sync::Arc::new(minimal_pdf());
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let bytes = bytes.clone();
            std::thread::spawn(move || {
                junk_libs_platen::render_page_bitmap_from_bytes(&bytes, 0, 2.0).expect("render")
            })
        })
        .collect();
    for h in handles {
        let (image, _) = h.join().expect("thread");
        assert!(image.width() > 1000); // scale 2.0 applied
    }
}

#[test]
fn file_roundtrip_and_page_count() {
    let path = std::env::temp_dir().join("junk-libs-platen-test.pdf");
    std::fs::write(&path, minimal_pdf()).expect("write fixture");

    let count = junk_libs_platen::page_count(&path).expect("page count");
    assert_eq!(count, 1);

    let pages = junk_libs_platen::render_pdf(&path, 1.0).expect("render pdf");
    let _ = std::fs::remove_file(&path);
    assert_eq!(pages.len(), 1);
}

/// Text-layer extraction: same expectations junk-libs-pdfium's test had for
/// its pdfium-generated fixture — "Hello" at baseline (72, 720) in 24pt
/// Helvetica, boxes in top-left page points.
#[test]
fn extracts_text_layer() {
    let bytes = minimal_pdf();
    let path = std::env::temp_dir().join("junk-libs-platen-text-test.pdf");
    std::fs::write(&path, &bytes).expect("write fixture");
    let pages = junk_libs_platen::render_pdf(&path, 1.0).expect("render pdf");
    let _ = std::fs::remove_file(&path);

    let boxes = &pages[0].char_boxes;
    let text: String = boxes.iter().map(|b| b.ch).collect();
    assert!(text.contains("Hello"), "extracted text layer was {text:?}");

    // 'H' sits at x≈72pt and, flipped into top-left space, y ≈ 842 − 720 ≈
    // 122pt down minus the ascent — same windows the pdfium test used.
    let h = boxes.iter().find(|b| b.ch == 'H').expect("found 'H' box");
    assert!((55.0..95.0).contains(&h.x), "H.x = {}", h.x);
    assert!((90.0..150.0).contains(&h.y), "H.y = {}", h.y);
    assert!(h.w > 0.0 && h.h > 0.0, "degenerate H box {}×{}", h.w, h.h);

    // Loose bounds must tile: 'H' and 'e' are adjacent, so H's right edge
    // should meet e's left edge (within a point).
    let e = boxes.iter().find(|b| b.ch == 'e').expect("found 'e' box");
    assert!(
        (h.x + h.w - e.x).abs() < 1.0,
        "H and e don't tile: H ends at {}, e starts at {}",
        h.x + h.w,
        e.x
    );
    // And every real glyph box shares a cell height (synthetic whitespace
    // boxes span gaps and may differ).
    assert!(
        boxes
            .iter()
            .filter(|b| !b.ch.is_whitespace())
            .all(|b| (b.h - h.h).abs() < 0.5),
        "uneven cell heights"
    );
}

/// Whitespace synthesis: a newline between baselines, and a space for the
/// TJ-kerned word gap (the fixture contains no space or newline glyphs).
#[test]
fn synthesizes_whitespace() {
    let bytes = minimal_pdf();
    let path = std::env::temp_dir().join("junk-libs-platen-ws-test.pdf");
    std::fs::write(&path, &bytes).expect("write fixture");
    let pages = junk_libs_platen::render_pdf(&path, 1.0).expect("render pdf");
    let _ = std::fs::remove_file(&path);

    let text: String = pages[0].char_boxes.iter().map(|b| b.ch).collect();
    assert!(
        text.contains("Hello\nbig world"),
        "expected synthetic newline + space, got {text:?}"
    );
    // The synthetic space box spans the inter-word gap so region selection
    // catches it.
    let space = pages[0]
        .char_boxes
        .iter()
        .find(|b| b.ch == ' ')
        .expect("synthetic space box");
    assert!(space.w > 1.0, "space box should span the gap: w={}", space.w);
}

//! End-to-end tests for the hayro backend, using a minimal PDF built in-test
//! (no engine needed to *create* fixtures, unlike junk-libs-pdfium's tests,
//! which used pdfium's creation API).

#![cfg(feature = "backend-hayro")]

/// Build a valid single-xref PDF: one A4 page with a filled black rectangle
/// and a line of Helvetica text. Offsets are computed, not hard-coded, so the
/// fixture stays valid if the content changes.
fn minimal_pdf() -> Vec<u8> {
    let content = b"0 0 0 rg 100 100 200 200 re f BT /F1 24 Tf 72 720 Td (Hello) Tj ET";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R \
           /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        {
            let mut s = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            s.extend_from_slice(content);
            s.extend_from_slice(b"\nendstream");
            s
        },
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
            objects.len() + 1,
            xref_offset
        )
        .as_bytes(),
    );
    pdf
}

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
    // M1 will flip this assertion: char boxes are documented-empty on hayro
    // for now. If this starts failing because boxes appeared — delete it and
    // celebrate.
    assert!(pages[0].char_boxes.is_empty());
}

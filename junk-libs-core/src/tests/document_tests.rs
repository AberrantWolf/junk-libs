use super::*;

#[test]
fn classifies_pdf_signatures_and_utf8_text() {
    assert_eq!(classify(b"%PDF-1.7\n...").unwrap(), DocumentFormat::Pdf);
    assert_eq!(
        classify("日本語\n<&text>".as_bytes()).unwrap(),
        DocumentFormat::Utf8Text
    );
    assert!(classify(b"bad\0text").is_err());
    assert!(classify(b"\xffbinary").is_err());
}

#[test]
fn preview_truncates_on_codepoint_boundaries() {
    let preview = text_preview("A€日本語".repeat(1000).as_bytes(), 3).unwrap();
    assert_eq!(preview.text, "A");
    assert!(preview.truncated);

    let preview = text_preview("\u{feff}hello".as_bytes(), 64).unwrap();
    assert_eq!(preview.text, "hello");
    assert!(!preview.truncated);

    // An incomplete codepoint at the real end of input is an error, not a cut.
    assert!(text_preview(&[b'a', 0xe2, 0x82][..], 64).is_err());
    assert!(text_preview(&[0xff, 0, 1, 2, 3, 4, 5][..], 2).is_err());
}

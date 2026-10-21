use junk_libs_core::document::{DocumentFormat, classify, text_preview};
use std::{
    cell::Cell,
    io::{Read, Result},
    rc::Rc,
};
fn pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> >>",
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    bytes
}
#[test]
fn opaque_document_signatures_and_utf8_are_bounded() {
    let pdf = pdf();
    assert_eq!(classify(&pdf, pdf.len()).unwrap(), DocumentFormat::Pdf);
    assert!(classify(&pdf, pdf.len() - 1).is_err());
    assert!(classify(&pdf[..pdf.len() - 10], pdf.len()).is_err());
    assert_eq!(
        classify("日本語\n<&text>".as_bytes(), 64).unwrap(),
        DocumentFormat::Utf8Text
    );
    for bytes in [
        &b""[..],
        &b"bad\0text"[..],
        &b"\xffbinary"[..],
        &b"%PDF-9.9\n%%EOF"[..],
    ] {
        assert!(classify(bytes, 64).is_err());
    }
}
struct Counted {
    bytes: Vec<u8>,
    count: Rc<Cell<usize>>,
}
impl Read for Counted {
    fn read(&mut self, out: &mut [u8]) -> Result<usize> {
        let start = self.count.get();
        let n = out.len().min(self.bytes.len() - start);
        out[..n].copy_from_slice(&self.bytes[start..start + n]);
        self.count.set(start + n);
        Ok(n)
    }
}
#[test]
fn text_preview_bounds_io_preserves_unicode_and_rejects_incomplete_sources() {
    let count = Rc::new(Cell::new(0));
    let preview = text_preview(
        Counted {
            bytes: "A€日本語".repeat(1000).into_bytes(),
            count: count.clone(),
        },
        3,
    )
    .unwrap();
    assert_eq!(preview.text, "A");
    assert!(preview.truncated);
    assert!(count.get() <= 8);
    assert_eq!(
        text_preview("\u{feff}hello".as_bytes(), 64).unwrap().text,
        "hello"
    );
    assert!(!text_preview(b"short".as_slice(), 64).unwrap().truncated);
    assert!(text_preview(&[0xe2, 0x82][..], 64).is_err());
    assert!(text_preview(&[0xff, 0, 1, 2, 3, 4, 5][..], 2).is_err());
    assert!(text_preview(b"text".as_slice(), 0).is_err());
    assert!(text_preview(b"text".as_slice(), 1024 * 1024 + 1).is_err());
}

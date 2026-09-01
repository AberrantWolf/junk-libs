//! Lightweight document metadata used by viewers before any page is rendered.
//! Rendering remains backend-specific; outlines and page labels are ordinary PDF
//! structures and are parsed once here for every backend.

use std::path::Path;

use lopdf::{Dictionary, Document, Object};

use crate::{Error, Result};

/// One local destination in the PDF outline/bookmark tree.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfOutlineItem {
    /// One-based nesting level, matching the PDF outline tree.
    pub level: usize,
    pub title: String,
    /// Zero-based page index.
    pub page_index: usize,
    /// Position from the top of the page when the destination supplies one.
    /// `None` means the destination identifies only the page.
    pub top_fraction: Option<f32>,
}

/// Metadata needed to build viewer navigation without rasterizing a page.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfMetadata {
    pub page_count: usize,
    pub outline: Vec<PdfOutlineItem>,
    /// Display label for every physical page. Falls back to `1`, `2`, ... when
    /// the document has no `/PageLabels` number tree.
    pub page_labels: Vec<String>,
    /// Recoverable metadata problems. Rendering can continue.
    pub warnings: Vec<String>,
}

pub fn read(path: &Path) -> Result<PdfMetadata> {
    let document = Document::load(path).map_err(map_load_error)?;
    from_document(&document)
}

pub fn read_from_bytes(bytes: &[u8]) -> Result<PdfMetadata> {
    let document = Document::load_mem(bytes).map_err(map_load_error)?;
    from_document(&document)
}

fn map_load_error(error: lopdf::Error) -> Error {
    match error {
        lopdf::Error::IO(error) => Error::Io(error),
        _ => Error::Invalid,
    }
}

fn from_document(document: &Document) -> Result<PdfMetadata> {
    let page_count = document.get_pages().len();
    let mut warnings = Vec::new();
    let outline = match document.get_toc() {
        Ok(toc) => {
            warnings.extend(
                toc.errors
                    .into_iter()
                    .map(|error| format!("outline: {error}")),
            );
            toc.toc
                .into_iter()
                .filter_map(|entry| {
                    entry
                        .page
                        .checked_sub(1)
                        .filter(|page| *page < page_count)
                        .map(|page_index| PdfOutlineItem {
                            level: entry.level,
                            title: entry.title,
                            page_index,
                            top_fraction: None,
                        })
                })
                .collect()
        }
        Err(lopdf::Error::NoOutline) => Vec::new(),
        Err(error) => {
            warnings.push(format!("outline could not be read: {error}"));
            Vec::new()
        }
    };
    let page_labels = read_page_labels(document, page_count, &mut warnings);
    Ok(PdfMetadata {
        page_count,
        outline,
        page_labels,
        warnings,
    })
}

#[derive(Debug, Clone)]
struct LabelRange {
    first_page: usize,
    prefix: String,
    style: Option<Vec<u8>>,
    start: usize,
}

fn read_page_labels(
    document: &Document,
    page_count: usize,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut ranges = Vec::new();
    let labels = document
        .catalog()
        .ok()
        .and_then(|catalog| catalog.get(b"PageLabels").ok())
        .and_then(|object| resolve_dictionary(document, object).ok());
    if let Some(labels) = labels
        && collect_label_ranges(document, &labels, &mut ranges).is_err()
    {
        warnings.push("page labels could not be read; using physical page numbers".into());
        ranges.clear();
    }
    ranges.sort_by_key(|range| range.first_page);

    (0..page_count)
        .map(|page_index| {
            let Some(range) = ranges
                .iter()
                .rev()
                .find(|range| range.first_page <= page_index)
            else {
                return (page_index + 1).to_string();
            };
            let number = range.start + page_index - range.first_page;
            format!(
                "{}{}",
                range.prefix,
                format_label(number, range.style.as_deref())
            )
        })
        .collect()
}

fn collect_label_ranges(
    document: &Document,
    node: &Dictionary,
    ranges: &mut Vec<LabelRange>,
) -> lopdf::Result<()> {
    if let Ok(kids) = node.get(b"Kids").and_then(Object::as_array) {
        for kid in kids {
            let dictionary = resolve_dictionary(document, kid)?;
            collect_label_ranges(document, &dictionary, ranges)?;
        }
    }
    if let Ok(numbers) = node.get(b"Nums").and_then(Object::as_array) {
        for pair in numbers.chunks_exact(2) {
            let first_page = usize::try_from(pair[0].as_i64()?).unwrap_or(0);
            let dictionary = resolve_dictionary(document, &pair[1])?;
            let prefix = dictionary
                .get(b"P")
                .ok()
                .and_then(|object| lopdf::decode_text_string(object).ok())
                .unwrap_or_default();
            let style = dictionary
                .get(b"S")
                .ok()
                .and_then(|object| object.as_name().ok())
                .map(ToOwned::to_owned);
            let start = dictionary
                .get(b"St")
                .ok()
                .and_then(|object| object.as_i64().ok())
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(1)
                .max(1);
            ranges.push(LabelRange {
                first_page,
                prefix,
                style,
                start,
            });
        }
    }
    Ok(())
}

fn resolve_dictionary(document: &Document, object: &Object) -> lopdf::Result<Dictionary> {
    match object {
        Object::Dictionary(dictionary) => Ok(dictionary.clone()),
        Object::Reference(id) => Ok(document.get_object(*id)?.as_dict()?.clone()),
        _ => Err(lopdf::Error::ObjectType {
            expected: "Dictionary",
            found: "other",
        }),
    }
}

fn format_label(number: usize, style: Option<&[u8]>) -> String {
    match style {
        Some(b"r") => roman(number).to_lowercase(),
        Some(b"R") => roman(number),
        Some(b"a") => alphabetic(number).to_lowercase(),
        Some(b"A") => alphabetic(number),
        Some(b"D") | None => number.to_string(),
        Some(_) => number.to_string(),
    }
}

fn roman(mut number: usize) -> String {
    const VALUES: &[(usize, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut result = String::new();
    for (value, numeral) in VALUES {
        while number >= *value {
            number -= *value;
            result.push_str(numeral);
        }
    }
    result
}

fn alphabetic(mut number: usize) -> String {
    if number == 0 {
        return String::new();
    }
    let mut result = Vec::new();
    while number > 0 {
        number -= 1;
        result.push(b'A' + u8::try_from(number % 26).unwrap_or(0));
        number /= 26;
    }
    result.reverse();
    String::from_utf8(result).expect("alphabetic page labels are ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Object, Stream, dictionary};

    #[test]
    fn label_numbering_formats() {
        assert_eq!(format_label(14, Some(b"D")), "14");
        assert_eq!(format_label(14, Some(b"r")), "xiv");
        assert_eq!(format_label(27, Some(b"A")), "AA");
    }

    #[test]
    fn reads_real_outline_and_page_label_structures() {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let contents_id = document.add_object(Stream::new(Dictionary::new(), Vec::new()));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 300.into(), 400.into()],
            "Contents" => contents_id,
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let outlines_id = document.new_object_id();
        let item_id = document.add_object(dictionary! {
            "Title" => Object::string_literal("Chapter one"),
            "Parent" => outlines_id,
            "Dest" => vec![page_id.into(), Object::Name(b"Fit".to_vec())],
        });
        document.objects.insert(
            outlines_id,
            Object::Dictionary(dictionary! {
                "Type" => "Outlines",
                "First" => item_id,
                "Last" => item_id,
                "Count" => 1,
            }),
        );
        let labels_id = document.add_object(dictionary! {
            "Nums" => vec![
                0.into(),
                Object::Dictionary(dictionary! {
                    "S" => Object::Name(b"r".to_vec()),
                    "P" => Object::string_literal("front-")
                })
            ]
        });
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "Outlines" => outlines_id,
            "PageLabels" => labels_id,
        });
        document.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();

        let metadata = read_from_bytes(&bytes).unwrap();
        assert_eq!(metadata.page_count, 1);
        assert_eq!(metadata.outline[0].title, "Chapter one");
        assert_eq!(metadata.outline[0].page_index, 0);
        assert_eq!(metadata.page_labels, ["front-i"]);
    }
}

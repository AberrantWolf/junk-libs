//! Geometric reading order for selected text. Synthetic whitespace from content
//! emission order is discarded and rebuilt after selection. This is a heuristic
//! for untagged PDFs; callers can override writing direction for ambiguous layouts.

use crate::CharBox;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Direction {
    #[default]
    Auto,
    LeftToRight,
    RightToLeft,
    VerticalRightToLeft,
}

/// Reconstruct lines/columns from glyph cells already filtered to the selection.
/// Coordinates have a top-left origin. Invalid cells and control characters are
/// ignored; a maximum recursion depth bounds pathological page layouts.
pub fn reconstruct(boxes: &[CharBox], direction: Direction) -> String {
    let mut glyphs: Vec<_> = boxes
        .iter()
        .filter(|b| {
            !b.ch.is_whitespace()
                && !b.ch.is_control()
                && [b.x, b.y, b.w, b.h].iter().all(|v| v.is_finite())
                && b.w >= 0.0
                && b.h > 0.0
        })
        .collect();
    // Overprinted fill/stroke or duplicate OCR layers should not duplicate text.
    let mut seen = std::collections::HashSet::new();
    glyphs.retain(|b| {
        seen.insert((
            b.ch,
            b.x.to_bits(),
            b.y.to_bits(),
            b.w.to_bits(),
            b.h.to_bits(),
        ))
    });
    if glyphs.is_empty() {
        return String::new();
    }
    let vertical = direction == Direction::VerticalRightToLeft
        || (direction == Direction::Auto && is_vertical(&glyphs));
    let direction = if direction == Direction::Auto
        && !vertical
        && glyphs.iter().filter(|b| rtl(b.ch)).count()
            > glyphs
                .iter()
                .filter(|b| b.ch.is_alphabetic() && !rtl(b.ch))
                .count()
    {
        Direction::RightToLeft
    } else {
        direction
    };
    blocks(glyphs, direction, vertical, 0)
}

fn cjk(ch: char) -> bool {
    matches!(ch as u32, 0x3000..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
}
fn rtl(ch: char) -> bool {
    matches!(ch as u32, 0x590..=0x8ff | 0xfb1d..=0xfdff | 0xfe70..=0xfeff)
}
fn median(mut values: Vec<f32>) -> f32 {
    values.sort_by(f32::total_cmp);
    values[values.len() / 2].max(0.01)
}
fn is_vertical(g: &[&CharBox]) -> bool {
    let candidates: Vec<_> = g.iter().filter(|b| cjk(b.ch)).copied().collect();
    if candidates.len() < 3 {
        return false;
    }
    // Count nearest neighbours in narrow x/y bands. Requiring two vertical
    // adjacencies avoids interpreting a single column of short labels as tategaki.
    let mut sorted = candidates;
    sorted.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let down = sorted
        .windows(2)
        .filter(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let dy = b.y - a.y;
            (b.x - a.x).abs() < 0.35 * a.w.max(b.w)
                && dy > 0.5 * a.h.min(b.h)
                && dy < 1.8 * a.h.max(b.h)
        })
        .count();
    sorted.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let across = sorted
        .windows(2)
        .filter(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let dx = b.x - a.x;
            (b.y - a.y).abs() < 0.35 * a.h.max(b.h)
                && dx > 0.5 * a.w.min(b.w)
                && dx < 1.8 * a.w.max(b.w)
        })
        .count();
    down >= 2 && down > across
}

fn gap_cut(g: &[&CharBox], x_axis: bool, threshold: f32) -> Option<f32> {
    let bounds = |b: &CharBox| {
        if x_axis {
            (b.x, b.x + b.w)
        } else {
            (b.y, b.y + b.h)
        }
    };
    let mut intervals: Vec<_> = g.iter().map(|b| bounds(b)).collect();
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut end = intervals[0].1;
    let mut best = None;
    let mut max_gap = threshold;
    for (start, stop) in &intervals[1..] {
        if start - end > max_gap {
            max_gap = start - end;
            best = Some((end + start) * 0.5);
        }
        end = end.max(*stop);
    }
    best
}
fn blocks(g: Vec<&CharBox>, direction: Direction, vertical: bool, depth: usize) -> String {
    let em = median(g.iter().map(|b| if vertical { b.w } else { b.h }).collect());
    if depth < 32 && g.len() > 1 {
        // Prefer an uninterrupted column gutter. A spanning heading blocks that
        // cut and is separated horizontally before columns are considered again.
        let cuts = if vertical {
            [(true, 1.2 * em), (false, 2.0 * em)]
        } else {
            [(true, 1.5 * em), (false, 1.2 * em)]
        };
        for (x_axis, threshold) in cuts {
            if let Some(cut) = gap_cut(&g, x_axis, threshold) {
                let (mut first, mut second): (Vec<_>, Vec<_>) = g
                    .iter()
                    .copied()
                    .partition(|b| if x_axis { b.x < cut } else { b.y < cut });
                if !vertical && x_axis {
                    let multiline = |g: &[&CharBox]| {
                        g.iter().map(|b| b.y).fold(f32::NEG_INFINITY, f32::max)
                            - g.iter().map(|b| b.y).fold(f32::INFINITY, f32::min)
                            > 0.5 * em
                    };
                    if !multiline(&first) || !multiline(&second) {
                        continue;
                    }
                }
                if x_axis && (vertical || direction == Direction::RightToLeft) {
                    std::mem::swap(&mut first, &mut second);
                }
                return format!(
                    "{}\n\n{}",
                    blocks(first, direction, vertical, depth + 1),
                    blocks(second, direction, vertical, depth + 1)
                );
            }
        }
    }
    let mut sorted = g;
    sorted.sort_by(|a, b| {
        if vertical {
            b.x.total_cmp(&a.x).then(a.y.total_cmp(&b.y))
        } else {
            a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x))
        }
    });
    let mut lines: Vec<Vec<&CharBox>> = Vec::new();
    for glyph in sorted {
        // Compare against a stable representative, not an expanding union that
        // could bridge adjacent lines through a large heading or drop capital.
        let center = |b: &CharBox| {
            if vertical {
                b.x + b.w * 0.5
            } else {
                b.y + b.h * 0.5
            }
        };
        let size = |b: &CharBox| if vertical { b.w } else { b.h };
        if let Some(line) = lines.iter_mut().rev().find(|line| {
            (center(line[0]) - center(glyph)).abs() < 0.45 * size(line[0]).max(size(glyph))
        }) {
            line.push(glyph);
        } else {
            lines.push(vec![glyph]);
        }
    }
    lines
        .into_iter()
        .map(|line| assemble(line, direction, vertical))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assemble(mut g: Vec<&CharBox>, direction: Direction, vertical: bool) -> String {
    g.sort_by(|a, b| {
        if vertical {
            a.y.total_cmp(&b.y)
        } else {
            a.x.total_cmp(&b.x)
        }
    });
    let mut visual = String::new();
    for (i, b) in g.iter().enumerate() {
        if let Some(a) = i.checked_sub(1).map(|i| g[i]) {
            let gap = if vertical {
                b.y - a.y - a.h
            } else {
                b.x - a.x - a.w
            };
            let em = a.h.min(b.h);
            if gap > 0.18 * em && !(cjk(a.ch) && cjk(b.ch)) {
                visual.push(' ');
            }
        }
        visual.push(b.ch);
    }
    if vertical || !visual.chars().any(rtl) {
        return visual;
    }
    // Reconstruct logical RTL runs from physical glyph order, preserving Latin
    // and numeric runs. Spaces are grouped with their following directional run.
    let base_rtl = direction == Direction::RightToLeft
        || (direction == Direction::Auto
            && g.iter().filter(|b| rtl(b.ch)).count()
                > g.iter()
                    .filter(|b| b.ch.is_alphabetic() && !rtl(b.ch))
                    .count());
    let mut groups: Vec<(bool, String)> = Vec::new();
    for word in visual.split(' ') {
        let is_rtl = word.chars().any(rtl);
        let mut segments: Vec<(bool, String)> = Vec::new();
        for cluster in word.graphemes(true) {
            let direction = cluster.chars().any(rtl);
            if let Some((previous, text)) = segments.last_mut()
                && *previous == direction
            {
                text.push_str(cluster);
            } else {
                segments.push((direction, cluster.to_owned()));
            }
        }
        if is_rtl && base_rtl {
            segments.reverse();
        }
        let word = segments
            .into_iter()
            .map(|(direction, text)| {
                if direction {
                    text.graphemes(true).rev().collect::<String>()
                } else {
                    text
                }
            })
            .collect();
        groups.push((is_rtl, word));
    }
    if base_rtl {
        groups.reverse();
    }
    groups
        .into_iter()
        .map(|(_, word)| word)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn glyph(c: char, x: f32, y: f32) -> CharBox {
        CharBox {
            ch: c,
            x,
            y,
            w: 8.0,
            h: 10.0,
        }
    }
    #[test]
    fn shuffled_lines_words_and_columns() {
        let mut g = Vec::new();
        for (text, x, y) in [
            ("First", 0., 0.),
            ("line", 50., 0.),
            ("Second", 0., 14.),
            ("Right", 160., 0.),
            ("column", 160., 14.),
        ] {
            for (i, c) in text.chars().enumerate() {
                g.push(glyph(c, x + i as f32 * 8., y));
            }
        }
        g.reverse();
        assert_eq!(
            reconstruct(&g, Direction::Auto),
            "First line\nSecond\n\nRight\ncolumn"
        );
    }
    #[test]
    fn aligned_paragraph_gaps_do_not_interleave_columns() {
        let g = vec![
            glyph('A', 0., 0.),
            glyph('B', 0., 50.),
            glyph('C', 160., 0.),
            glyph('D', 160., 50.),
        ];
        assert_eq!(reconstruct(&g, Direction::Auto), "A\n\nB\n\nC\n\nD");
    }
    #[test]
    fn mixed_words_preserve_latin_and_numeric_runs() {
        let g: Vec<_> = "ABCאבג 123"
            .chars()
            .enumerate()
            .map(|(i, c)| glyph(c, i as f32 * 8., 0.))
            .collect();
        assert_eq!(reconstruct(&g, Direction::LeftToRight), "ABCגבא 123");
    }

    #[test]
    fn vertical_japanese_reads_down_then_right_to_left() {
        let g = vec![
            glyph('四', 0., 12.),
            glyph('二', 16., 12.),
            glyph('三', 0., 0.),
            glyph('一', 16., 0.),
            glyph('五', 0., 24.),
            glyph('六', 16., 24.),
        ];
        assert_eq!(reconstruct(&g, Direction::Auto), "一二六\n三四五");
    }
    #[test]
    fn rtl_keeps_numbers_in_logical_order() {
        let text = "123 םולש";
        let g: Vec<_> = text
            .chars()
            .enumerate()
            .map(|(i, c)| glyph(c, i as f32 * 8., 0.))
            .collect();
        assert_eq!(reconstruct(&g, Direction::Auto), "שלום 123");
    }
    #[test]
    fn duplicated_text_layers_are_not_repeated() {
        let g = vec![
            glyph('A', 0., 0.),
            glyph('B', 8., 0.),
            glyph('A', 0., 0.),
            glyph('B', 8., 0.),
        ];
        assert_eq!(reconstruct(&g, Direction::Auto), "AB");
    }
    #[test]
    fn ignores_content_stream_newlines_after_spatial_selection() {
        let g = vec![glyph('B', 8., 0.), glyph('\n', 0., 0.), glyph('A', 0., 0.)];
        assert_eq!(reconstruct(&g, Direction::Auto), "AB");
    }
}

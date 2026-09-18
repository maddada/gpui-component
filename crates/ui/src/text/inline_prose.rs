use gpui::{Pixels, WrappedLine, px};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Prose wraps at Unicode opportunities using shaped advances. Trailing spaces
/// remain in the source ranges for copying, but do not force a fitting word off
/// the line, matching normal whitespace in rendered Markdown.
pub(super) fn line_ranges(line: &WrappedLine, width: Option<Pixels>) -> Vec<Range<usize>> {
    let text = line.text.as_ref();
    if text.is_empty() {
        return Vec::new();
    }
    let Some(width) = width else {
        return vec![0..text.len()];
    };
    let mut positions = line
        .runs()
        .iter()
        .flat_map(|run| {
            run.glyphs
                .iter()
                .map(|glyph| (glyph.index, glyph.position.x))
        })
        .collect::<Vec<_>>();
    positions.sort_by(|left, right| {
        left.1
            .partial_cmp(&right.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut advances = positions
        .iter()
        .enumerate()
        .map(|(i, (index, x))| {
            (
                *index,
                positions
                    .get(i + 1)
                    .map_or(line.unwrapped_layout.width, |(_, next)| *next)
                    - *x,
            )
        })
        .collect::<Vec<_>>();
    advances.sort_by_key(|(index, _)| *index);
    positions.clear();
    let mut pen = px(0.0);
    for (index, advance) in advances {
        if positions
            .last()
            .is_none_or(|(previous, _)| *previous != index)
        {
            positions.push((index, pen));
        }
        pen += advance;
    }
    positions.push((text.len(), pen));
    let x = |index| positions[positions.partition_point(|(at, _)| *at < index)].1;
    let measured = |start, end| {
        let visible = text[start..end].trim_end_matches([' ', '\t', '\r']);
        px(f32::from(x(start + visible.len()) - x(start)).abs())
    };
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut previous = 0;
    for (end, _) in unicode_linebreak::linebreaks(text) {
        if measured(start, end) > width && previous > start {
            ranges.push(start..previous);
            start = previous;
        }
        if measured(start, end) > width {
            let base = start;
            let mut fitting = start;
            for (index, grapheme) in text[base..end].grapheme_indices(true) {
                let next = base + index + grapheme.len();
                if measured(start, next) > width && fitting > start {
                    ranges.push(start..fitting);
                    start = fitting;
                }
                fitting = next;
            }
        }
        previous = end;
    }
    if start < text.len() {
        ranges.push(start..text.len());
    }
    ranges
}

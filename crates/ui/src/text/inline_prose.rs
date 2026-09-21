use gpui::{HighlightStyle, LineFragment, Pixels, TextStyle, Window, WrappedLine, px};
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

/// True when a highlight paints its text in another face than the paragraph's
/// own, which is the only kind of highlight that changes how wide the text is.
fn restyles_face(base: &TextStyle, highlight: &HighlightStyle) -> bool {
    highlight
        .font_weight
        .is_some_and(|weight| weight != base.font_weight)
        || highlight
            .font_style
            .is_some_and(|style| style != base.font_style)
}

/// The width of one word in the face it will be painted in.
fn measure(word: &str, style: &TextStyle, font_size: Pixels, window: &mut Window) -> Pixels {
    window
        .text_system()
        .shape_line(
            word.to_owned().into(),
            font_size,
            &[style.to_run(word.len())],
            None,
        )
        .width()
}

/// Hand one bold or italic word to the wrapper as an element of its own, split
/// into the chunks that fit when the word alone is wider than the column.
///
/// `lead` is what the whitespace in front of the word costs beyond what the
/// wrapper will charge for it, because that whitespace stays text and is
/// therefore measured in the paragraph's own face.
fn push_word<'a>(
    fragments: &mut Vec<LineFragment<'a>>,
    word: &str,
    lead: Pixels,
    style: &TextStyle,
    font_size: Pixels,
    wrap_width: Pixels,
    window: &mut Window,
) {
    let width = measure(word, style, font_size, window);
    if width + lead <= wrap_width {
        fragments.push(LineFragment::element(width + lead, word.len()));
        return;
    }
    let mut start = 0;
    let mut chunk_width = px(0.0);
    for (index, grapheme) in word.grapheme_indices(true) {
        let end = index + grapheme.len();
        let candidate = measure(&word[start..end], style, font_size, window)
            + if start == 0 { lead } else { px(0.0) };
        if candidate > wrap_width && index > start {
            fragments.push(LineFragment::element(chunk_width, index - start));
            start = index;
            chunk_width = measure(grapheme, style, font_size, window);
        } else {
            chunk_width = candidate;
        }
    }
    fragments.push(LineFragment::element(chunk_width, word.len() - start));
}

/// Break opportunities for prose the line wrapper cannot measure on its own.
///
/// A [`gpui::LineWrapper`] carries one face, so every text fragment handed to it
/// is measured in the paragraph's own. A run that asks for another face (a bold
/// answer, an italic aside) is then measured too narrow, the line it sits on is
/// packed past the column's edge, and the last word of it is painted under the
/// clip. Each word of such a run is handed over as an element measured in the
/// face it will be painted in instead, and the whitespace around it stays text
/// so the wrapper still reads a break opportunity in front of every word.
pub(super) fn wrap_fragments<'a>(
    text: &'a str,
    highlights: &[(Range<usize>, HighlightStyle)],
    base: &TextStyle,
    wrap_width: Pixels,
    window: &mut Window,
) -> Vec<LineFragment<'a>> {
    let font_size = base.font_size.to_pixels(window.rem_size());
    let mut fragments = Vec::new();
    let mut ix = 0;
    for (range, highlight) in highlights {
        let start = range.start.max(ix);
        let end = range.end.min(text.len());
        if start >= end {
            continue;
        }
        if !restyles_face(base, highlight) {
            continue;
        }
        if ix < start {
            fragments.push(LineFragment::text(&text[ix..start]));
        }
        let style = base.clone().highlight(*highlight);
        let run = &text[start..end];
        let mut chunk_start = 0;
        let mut lead = px(0.0);
        while chunk_start < run.len() {
            let rest = &run[chunk_start..];
            let blank = rest.starts_with(char::is_whitespace);
            let chunk_len = rest
                .find(|character: char| character.is_whitespace() != blank)
                .unwrap_or(rest.len());
            let chunk = &rest[..chunk_len];
            if blank {
                // Whitespace stays text, because the wrapper reads a break
                // opportunity from the space in front of a word and never from
                // an element. What that costs in the run's own face is carried
                // over to the word instead, so the line still adds up. A break
                // ends the line it was carried from, and is the one run of
                // whitespace a line cannot be shaped with.
                fragments.push(LineFragment::text(chunk));
                lead = if chunk.contains('\n') {
                    px(0.0)
                } else {
                    lead + measure(chunk, &style, font_size, window)
                        - measure(chunk, base, font_size, window)
                };
            } else {
                push_word(
                    &mut fragments,
                    chunk,
                    lead,
                    &style,
                    font_size,
                    wrap_width,
                    window,
                );
                lead = px(0.0);
            }
            chunk_start += chunk_len;
        }
        ix = end;
    }
    if ix < text.len() {
        fragments.push(LineFragment::text(&text[ix..]));
    }
    fragments
}

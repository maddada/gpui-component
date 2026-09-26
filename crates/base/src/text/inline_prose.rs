//! Wrapping prose set in another face than the paragraph's own.
//!
//! A [`gpui::LineWrapper`] carries one face, so every text fragment handed to
//! it is measured in the paragraph's own. A run that asks for another face (a
//! bold answer, an italic aside) is then measured too narrow, the line it sits
//! on is packed past the column's edge, and the last word of it is painted
//! under the clip. Each word of such a run is handed over as an element
//! measured in the face it will be painted in instead, and the whitespace
//! around it stays text so the wrapper still reads a break opportunity in
//! front of every word.

use std::ops::Range;

use gpui::{HighlightStyle, LineFragment, Pixels, TextStyle, Window, px};
use unicode_segmentation::UnicodeSegmentation as _;

/// True when a highlight paints its text in another face than the paragraph's
/// own, which is the only kind of highlight that changes how wide the text is.
pub(super) fn restyles_face(base: &TextStyle, highlight: &HighlightStyle) -> bool {
    highlight
        .font_weight
        .is_some_and(|weight| weight != base.font_weight)
        || highlight
            .font_style
            .is_some_and(|style| style != base.font_style)
}

/// The width of one word in the face it will be painted in.
fn measure(word: &str, style: &TextStyle, font_size: Pixels, window: &Window) -> Pixels {
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

/// Hand one word to the wrapper as an element of its own, split into the
/// chunks that fit when the word alone is wider than the column.
///
/// `lead` is what the whitespace in front of the word costs beyond what the
/// wrapper will charge for it, because that whitespace stays text and is
/// therefore measured in the paragraph's own face.
fn push_word(
    fragments: &mut Vec<LineFragment<'_>>,
    word: &str,
    lead: Pixels,
    style: &TextStyle,
    font_size: Pixels,
    wrap_width: Pixels,
    window: &Window,
) {
    let width = measure(word, style, font_size, window);
    if width + lead <= wrap_width {
        fragments.push(LineFragment::element(width + lead, word.len()));
        return;
    }
    let mut start = 0;
    let mut chunk_width = px(0.);
    for (index, grapheme) in word.grapheme_indices(true) {
        let end = index + grapheme.len();
        let candidate = measure(&word[start..end], style, font_size, window)
            + if start == 0 { lead } else { px(0.) };
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

/// Appends the wrap fragments for `range` of `text`, a run the paragraph
/// paints with `highlight`, which restyles its face (see [`restyles_face`]).
pub(super) fn push_wrap_fragments<'a>(
    fragments: &mut Vec<LineFragment<'a>>,
    text: &'a str,
    range: Range<usize>,
    highlight: &HighlightStyle,
    base: &TextStyle,
    font_size: Pixels,
    wrap_width: Pixels,
    window: &Window,
) {
    let style = base.clone().highlight(*highlight);
    let run = &text[range.clone()];
    let mut chunk_start = 0;
    let mut lead = px(0.);
    while chunk_start < run.len() {
        let rest = &run[chunk_start..];
        let blank = rest.starts_with(char::is_whitespace);
        let chunk_len = rest
            .find(|character: char| character.is_whitespace() != blank)
            .unwrap_or(rest.len());
        let chunk = &rest[..chunk_len];
        if blank {
            // Whitespace stays text, because the wrapper reads a break
            // opportunity from the space in front of a word and never from an
            // element. What that costs in the run's own face is carried over
            // to the word instead, so the line still adds up. A break ends the
            // line it was carried from, and is the one run of whitespace a
            // line cannot be shaped with.
            let offset = range.start + chunk_start;
            fragments.push(LineFragment::text(&text[offset..offset + chunk_len]));
            lead = if chunk.contains('\n') {
                px(0.)
            } else {
                lead + measure(chunk, &style, font_size, window)
                    - measure(chunk, base, font_size, window)
            };
        } else {
            push_word(
                fragments, chunk, lead, &style, font_size, wrap_width, window,
            );
            lead = px(0.);
        }
        chunk_start += chunk_len;
    }
}

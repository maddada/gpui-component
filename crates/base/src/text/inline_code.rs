//! Inline code drawn as a host's chip ([`InlineCodeStyle`]), and the colour
//! swatches a chip or a sentence can carry.
//!
//! A chip is measured, wrapped and painted by the inline flow: it keeps the
//! span's text in its own typeface and size, adds padding (and a swatch) at
//! the span's two ends only, and paints one rounded, bordered box per wrapped
//! fragment whose outer corners and edges belong to the span's first and last
//! fragments.

use std::{ops::Range, sync::Arc};

use gpui::{
    Bounds, Corners, Edges, Hsla, LineFragment, Pixels, TextStyle, Window, point, px, quad, size,
};
use unicode_segmentation::UnicodeSegmentation as _;

use super::{
    InlineCodeStyle,
    inline::{InlineHighlight, combine_highlights},
    inline_flow::InlineFlowItem,
};

/// The colour an inline-code span names, when naming one is all it does.
///
/// CSS hex only, in the four lengths CSS allows. A span holding anything else
/// (a command, a path, a sentence) is ordinary inline code.
pub(super) fn swatch_color(text: &str) -> Option<Hsla> {
    let hex = text.trim().strip_prefix('#')?;
    if !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |index: usize| -> Option<f32> {
        let value = if hex.len() < 6 {
            let digit = hex.chars().nth(index)?.to_digit(16)?;
            digit * 17
        } else {
            u32::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?
        };
        Some(value as f32 / 255.0)
    };
    match hex.len() {
        3 | 4 | 6 | 8 => Some(
            gpui::Rgba {
                r: channel(0)?,
                g: channel(1)?,
                b: channel(2)?,
                a: if hex.len() == 4 || hex.len() == 8 {
                    channel(3)?
                } else {
                    1.0
                },
            }
            .into(),
        ),
        _ => None,
    }
}

/// Every hex colour written in a run of text, by the rule a chat renderer uses:
/// a `#` that does not follow a word character, `/` or `#`, then exactly three,
/// four, six or eight hex digits, with no word character, `/` or `-` after
/// them.
pub(super) fn hex_colors(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'#' {
            index += 1;
            continue;
        }
        let opens = index == 0 || {
            let previous = bytes[index - 1];
            !(word(previous) || previous == b'/' || previous == b'#')
        };
        let mut end = index + 1;
        while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
            end += 1;
        }
        let closes = end >= bytes.len() || {
            let next = bytes[end];
            !(word(next) || next == b'/' || next == b'-')
        };
        if opens && closes && matches!(end - index - 1, 3 | 4 | 6 | 8) {
            result.push(index..end);
            index = end;
            continue;
        }
        index = end.max(index + 1);
    }
    result
}

/// The text style a chip's text is shaped with: the chip's typeface at its
/// scale, unless it is prose that only names a colour.
pub(super) fn chip_text_style(
    base: &TextStyle,
    chip: &InlineCodeStyle,
    rem_size: Pixels,
) -> TextStyle {
    let mut style = base.clone();
    if !chip.prose {
        style.font_family = chip.font_family.clone();
        style.font_size = (base.font_size.to_pixels(rem_size) * chip.font_scale).into();
    }
    style
}

/// The height of the chip's glyphs, ascent to descent, which the box and the
/// swatch are sized from.
pub(super) fn chip_font_height(
    base: &TextStyle,
    chip: &InlineCodeStyle,
    window: &Window,
) -> Pixels {
    let style = chip_text_style(base, chip, window.rem_size());
    let font_size = style.font_size.to_pixels(window.rem_size());
    let font = window.text_system().resolve_font(&style.font());
    window.text_system().ascent(font, font_size) - window.text_system().descent(font, font_size)
}

fn swatch_side(font_height: Pixels) -> Pixels {
    (font_height * 0.78).round()
}

/// The room the leading edge of a chip's first fragment takes, and the room
/// the trailing edge of its last one takes.
///
/// Every place that decides how wide a chip is has to ask for this: the
/// wrapper that breaks the line, the flow that gives the fragment its advance,
/// and the paint. A measurement that leaves the swatch out reserves less room
/// than the chip paints, and the chip then runs over the next word.
pub(super) fn chip_edges(
    chip: &InlineCodeStyle,
    first: bool,
    last: bool,
    font_height: Pixels,
) -> (Pixels, Pixels) {
    let edge = chip.padding_x + chip.border_width;
    let swatch = if chip.swatch.is_some() {
        swatch_side(font_height) * 1.45
    } else {
        Pixels::ZERO
    };
    (
        if first { edge + swatch } else { Pixels::ZERO },
        if last { edge } else { Pixels::ZERO },
    )
}

/// The chip's box over a fragment whose glyphs are `ascent` above and
/// `descent` below `baseline`, relative to the fragment.
pub(super) fn chip_box(
    chip: &InlineCodeStyle,
    width: Pixels,
    baseline: Pixels,
    ascent: Pixels,
    descent: Pixels,
) -> Bounds<Pixels> {
    let pad = chip.padding_y + chip.border_width;
    Bounds::new(
        point(Pixels::ZERO, baseline - ascent - pad),
        size(width, ascent + descent + pad * 2.),
    )
}

/// Paints one fragment of a chip: the box, with the outer corners and edges of
/// the span's ends, and the swatch in the leading edge of its first fragment.
pub(super) fn paint_chip(
    bounds: Bounds<Pixels>,
    chip: &InlineCodeStyle,
    first: bool,
    last: bool,
    font_height: Pixels,
    window: &mut Window,
) {
    window.paint_quad(quad(
        bounds,
        Corners {
            top_left: if first { chip.radius } else { Pixels::ZERO },
            bottom_left: if first { chip.radius } else { Pixels::ZERO },
            top_right: if last { chip.radius } else { Pixels::ZERO },
            bottom_right: if last { chip.radius } else { Pixels::ZERO },
        },
        chip.background,
        Edges {
            top: chip.border_width,
            bottom: chip.border_width,
            left: if first {
                chip.border_width
            } else {
                Pixels::ZERO
            },
            right: if last {
                chip.border_width
            } else {
                Pixels::ZERO
            },
        },
        chip.border_color,
        Default::default(),
    ));
    // The colour the span names, in the room the leading edge reserved for it.
    if let Some(swatch) = chip.swatch.filter(|_| first) {
        let side = swatch_side(font_height);
        let inset = chip.padding_x + chip.border_width;
        window.paint_quad(quad(
            Bounds::new(
                point(
                    bounds.left() + inset,
                    bounds.top() + (bounds.size.height - side) / 2.,
                ),
                size(side, side),
            ),
            Corners::all(px(3.)),
            swatch,
            Edges::all(chip.swatch_border_width),
            chip.swatch_border_color,
            Default::default(),
        ));
    }
}

/// Appends the wrap fragments for `range` of a chip whose whole span is
/// `span` (both in the item's byte space).
///
/// Each word (with the whitespace after it) enters the wrapper as an element
/// measured in the chip's face, the span's first word carrying its leading
/// edge and the last its trailing one; a word wider than the line alone is
/// split at grapheme boundaries.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_wrap_fragments<'a>(
    fragments: &mut Vec<LineFragment<'a>>,
    text: &'a str,
    range: Range<usize>,
    span: Range<usize>,
    chip: &InlineCodeStyle,
    base: &TextStyle,
    measure: &mut dyn FnMut(&str) -> Pixels,
    wrap_width: Pixels,
    window: &Window,
) {
    let font_height = chip_font_height(base, chip, window);
    let edges = |start: usize, end: usize| {
        let (left, right) = chip_edges(chip, start == span.start, end == span.end, font_height);
        left + right
    };
    let mut offset = range.start;
    for word in text[range.clone()].split_inclusive(char::is_whitespace) {
        let word_end = offset + word.len();
        let width = measure(word) + edges(offset, word_end);
        if width <= wrap_width {
            fragments.push(LineFragment::element(width, word.len()));
        } else {
            let mut start = 0;
            let mut chunk_width = Pixels::ZERO;
            for (index, grapheme) in word.grapheme_indices(true) {
                let end = index + grapheme.len();
                let candidate = measure(&word[start..end]) + edges(offset + start, offset + end);
                if candidate > wrap_width && index > start {
                    fragments.push(LineFragment::element(chunk_width, index - start));
                    start = index;
                    chunk_width = measure(grapheme) + edges(offset + index, offset + end);
                } else {
                    chunk_width = candidate;
                }
            }
            fragments.push(LineFragment::element(chunk_width, word.len() - start));
        }
        offset = word_end;
    }
}

/// Adds the swatch of every colour written in running text to the text items
/// of a flow, as a prose chip over the colour's own words.
///
/// Links and code are left alone: their words are the link's or the code's,
/// and a code span carries its own swatch.
pub(super) fn add_prose_swatches(items: &mut [InlineFlowItem], prose: &InlineCodeStyle) {
    for item in items {
        let InlineFlowItem::Text {
            text,
            links,
            highlights,
            ..
        } = item
        else {
            continue;
        };
        let taken = |range: &Range<usize>| {
            links
                .iter()
                .map(|(link, _)| link)
                .chain(
                    highlights
                        .iter()
                        .filter(|(_, highlight)| highlight.font_family.is_some())
                        .map(|(range, _)| range),
                )
                .any(|other| other.start < range.end && range.start < other.end)
        };
        let swatches = hex_colors(text)
            .into_iter()
            .filter(|range| !taken(range))
            .filter_map(|range| {
                let color = swatch_color(&text[range.clone()])?;
                let mut chip = prose.clone();
                chip.swatch = Some(color);
                Some((
                    range,
                    InlineHighlight {
                        chip: Some(Arc::new(chip)),
                        ..Default::default()
                    },
                ))
            })
            .collect::<Vec<_>>();
        if !swatches.is_empty() {
            *highlights = combine_highlights(std::mem::take(highlights), swatches);
        }
    }
}

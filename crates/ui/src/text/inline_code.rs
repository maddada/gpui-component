use std::{
    ops::Range,
    sync::{Arc, Mutex},
};
use unicode_segmentation::UnicodeSegmentation as _;

use gpui::{
    AnyElement, Bounds, Corners, Edges, IntoElement, LineFragment, ParentElement, Pixels,
    SharedString, Size, Styled, TextStyle, Window, canvas, div, point, px, quad, size,
};

use super::{
    InlineCodeStyle,
    inline::{Inline, InlineState},
    inline_flow::InlineFlowItem,
};

pub(super) fn text_style(
    base: &TextStyle,
    code: Option<&InlineCodeStyle>,
    rem_size: Pixels,
) -> TextStyle {
    let mut style = base.clone();
    // A prose span carries a swatch and nothing else, so it keeps the paragraph's typeface.
    if let Some(code) = code.filter(|code| !code.prose) {
        style.font_family = code.font_family.clone();
        style.font_size = (base.font_size.to_pixels(rem_size) * code.font_scale).into();
    }
    style
}

/// The colour an inline-code span names, when naming one is all it does.
///
/// CSS hex only, in the four lengths CSS allows. A span holding anything else
/// (a command, a path, a sentence) is ordinary inline code.
fn swatch_color(text: &str) -> Option<gpui::Hsla> {
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

/// Every hex colour written in a run of text, by the rule a chat renderer uses: a `#` that does not
/// follow a word character, `/` or `#`, then exactly three, four, six or eight hex digits, with no
/// word character, `/` or `-` after them.
fn hex_colors(text: &str) -> Vec<Range<usize>> {
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

/// True when a run of text names at least one colour a swatch would be drawn for.
pub(super) fn has_hex_color(text: &str) -> bool {
    !hex_colors(text).is_empty()
}

/// Splits the spans that are not already code at the colours they name, so each one carries its
/// swatch. Links are left alone: their words are the link's, not a colour's.
pub(super) fn split_prose_swatches(
    items: Vec<InlineFlowItem>,
    style: &InlineCodeStyle,
) -> Vec<InlineFlowItem> {
    let mut result = Vec::new();
    for item in items {
        let InlineFlowItem::Text {
            text,
            links,
            highlights,
            code: None,
            reference: None,
            ..
        } = &item
        else {
            result.push(item);
            continue;
        };
        if !links.is_empty() {
            result.push(item);
            continue;
        }
        let colors = hex_colors(text)
            .into_iter()
            .filter_map(|range| swatch_color(&text[range.clone()]).map(|color| (range, color)))
            .collect::<Vec<_>>();
        if colors.is_empty() {
            result.push(item);
            continue;
        }
        let fragment = |range: Range<usize>, swatch: Option<gpui::Hsla>| {
            let mut code = style.clone();
            code.swatch = swatch;
            InlineFlowItem::Text {
                state: Arc::new(Mutex::new(InlineState::default())),
                text: SharedString::from(text[range.clone()].to_owned()),
                links: Vec::new(),
                highlights: clip_ranges(highlights, range),
                code: swatch.map(|_| code),
                reference: None,
            }
        };
        let mut start = 0;
        for (range, swatch) in colors {
            if start < range.start {
                result.push(fragment(start..range.start, None));
            }
            start = range.end;
            result.push(fragment(range, Some(swatch)));
        }
        if start < text.len() {
            result.push(fragment(start..text.len(), None));
        }
    }
    result
}

/// Width the swatch and its gap take from the span's leading edge.
fn swatch_space(code: &InlineCodeStyle, font_height: Pixels) -> Pixels {
    if code.swatch.is_none() {
        return px(0.0);
    }
    swatch_size(font_height) * 1.45
}

/// The leading edge a span's swatch is painted in, for the base style the span is measured with.
///
/// Every place that decides how wide a swatched span is has to ask for it: the wrapper that breaks
/// the line, the flow that gives the fragment its advance, and the element that paints it. A
/// measurement that leaves it out reserves less room than the span paints and the chip then runs
/// over the word after it.
pub(super) fn leading_space(
    code: &InlineCodeStyle,
    base: &TextStyle,
    window: &mut Window,
) -> Pixels {
    if code.swatch.is_none() {
        return px(0.0);
    }
    let style = text_style(base, Some(code), window.rem_size());
    let font_size = style.font_size.to_pixels(window.rem_size());
    let font = window.text_system().resolve_font(&style.font());
    let font_height = window.text_system().ascent(font, font_size)
        - window.text_system().descent(font, font_size);
    swatch_space(code, font_height)
}

fn swatch_size(font_height: Pixels) -> Pixels {
    (font_height * 0.78).round()
}

pub(super) fn split_items(
    items: Vec<InlineFlowItem>,
    ranges: &[Range<usize>],
    style: &InlineCodeStyle,
) -> Vec<InlineFlowItem> {
    let mut result = Vec::new();
    let mut offset = 0;
    for item in items {
        match item {
            InlineFlowItem::Text {
                text,
                links,
                highlights,
                ..
            } => {
                let mut boundaries = vec![0, text.len()];
                for (index, _) in text.match_indices('\n') {
                    boundaries.extend([index, index + 1]);
                }
                for range in ranges {
                    if range.start < offset + text.len() && range.end > offset {
                        boundaries.push(range.start.saturating_sub(offset));
                        boundaries.push((range.end - offset).min(text.len()));
                    }
                }
                boundaries.sort_unstable();
                boundaries.dedup();
                for pair in boundaries.windows(2) {
                    let (start, end) = (pair[0], pair[1]);
                    let fragment = &text[start..end];
                    let code = fragment != "\n"
                        && ranges.iter().any(|range| range.contains(&(offset + start)));
                    result.push(InlineFlowItem::Text {
                        state: Arc::new(Mutex::new(InlineState::default())),
                        text: SharedString::from(fragment.to_owned()),
                        links: clip_ranges(&links, start..end),
                        highlights: clip_ranges(&highlights, start..end),
                        reference: None,
                        code: code.then(|| {
                            let mut style = style.clone();
                            // A colour is short and never wraps, so a span that
                            // names one is always one whole fragment.
                            style.swatch = style
                                .swatches
                                .then(|| {
                                    ranges
                                        .iter()
                                        .any(|range| {
                                            range.start == offset + start
                                                && range.end == offset + end
                                        })
                                        .then(|| swatch_color(fragment))
                                        .flatten()
                                })
                                .flatten();
                            style
                        }),
                    });
                }
                offset += text.len();
            }
            image @ InlineFlowItem::Image { .. } => {
                offset += 1;
                result.push(image);
            }
        }
    }
    result
}

pub(super) fn clip_ranges<T: Clone>(
    ranges: &[(Range<usize>, T)],
    clip: Range<usize>,
) -> Vec<(Range<usize>, T)> {
    ranges
        .iter()
        .filter_map(|(range, value)| {
            let start = range.start.max(clip.start);
            let end = range.end.min(clip.end);
            (start < end).then(|| (start - clip.start..end - clip.start, value.clone()))
        })
        .collect()
}

pub(super) fn wrap_fragments<'a>(
    text: &'a str,
    code: &InlineCodeStyle,
    base: &TextStyle,
    wrap_width: Pixels,
    window: &mut Window,
) -> Vec<LineFragment<'a>> {
    let style = text_style(base, Some(code), window.rem_size());
    let font_size = style.font_size.to_pixels(window.rem_size());
    // Measured before `measure` takes the window: both need it, and only one at a time can.
    let leading = leading_space(code, base, window);
    let measure = |text: &str| {
        window
            .text_system()
            .shape_line(
                text.to_owned().into(),
                font_size,
                &[style.to_run(text.len())],
                None,
            )
            .width()
    };
    let padding = code.padding_x + code.border_width;
    let mut result = Vec::new();
    let mut offset = 0;
    for word in text.split_inclusive(char::is_whitespace) {
        let edge = padding
            * (usize::from(offset == 0) + usize::from(offset + word.len() == text.len())) as f32
            + if offset == 0 { leading } else { px(0.0) };
        let width = measure(word) + edge;
        if width <= wrap_width {
            result.push(LineFragment::element(width, word.len()));
        } else {
            let mut start = 0;
            let mut chunk_width = px(0.0);
            for (index, grapheme) in word.grapheme_indices(true) {
                let end = index + grapheme.len();
                let edge = padding
                    * (usize::from(offset + start == 0) + usize::from(offset + end == text.len()))
                        as f32
                    + if offset + start == 0 {
                        leading
                    } else {
                        px(0.0)
                    };
                let width = measure(&word[start..end]) + edge;
                if width > wrap_width && index > start {
                    result.push(LineFragment::element(chunk_width, index - start));
                    start = index;
                    let edge = if offset + end == text.len() {
                        padding
                    } else {
                        px(0.0)
                    };
                    chunk_width = measure(grapheme) + edge;
                } else {
                    chunk_width = width;
                }
            }
            result.push(LineFragment::element(chunk_width, word.len() - start));
        }
        offset += word.len();
    }
    result
}

pub(super) fn element(
    inline: Inline,
    code: &InlineCodeStyle,
    first: bool,
    last: bool,
    fragment_size: Size<Pixels>,
    window: &mut Window,
) -> AnyElement {
    let style = text_style(&window.text_style(), Some(code), window.rem_size());
    let font_size = style.font_size.to_pixels(window.rem_size());
    let font = window.text_system().resolve_font(&style.font());
    let font_height = window.text_system().ascent(font, font_size)
        - window.text_system().descent(font, font_size);
    let leading = swatch_space(code, font_height);
    let left = if first {
        code.padding_x + code.border_width + leading
    } else {
        px(0.0)
    };
    let right = if last {
        code.padding_x + code.border_width
    } else {
        px(0.0)
    };
    let code = code.clone();
    div()
        .relative()
        .w(fragment_size.width)
        .h(fragment_size.height)
        .font_family(style.font_family)
        .text_size(font_size)
        .line_height(fragment_size.height)
        .whitespace_nowrap()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let height = font_height + (code.padding_y + code.border_width) * 2.0;
                    let bounds = Bounds::new(
                        point(
                            bounds.left(),
                            bounds.top() + (bounds.size.height - height) / 2.0,
                        ),
                        size(bounds.size.width, height),
                    );
                    window.paint_quad(quad(
                        bounds,
                        Corners {
                            top_left: if first { code.radius } else { px(0.0) },
                            bottom_left: if first { code.radius } else { px(0.0) },
                            top_right: if last { code.radius } else { px(0.0) },
                            bottom_right: if last { code.radius } else { px(0.0) },
                        },
                        code.background,
                        Edges {
                            top: code.border_width,
                            bottom: code.border_width,
                            left: if first { code.border_width } else { px(0.0) },
                            right: if last { code.border_width } else { px(0.0) },
                        },
                        code.border_color,
                        Default::default(),
                    ));
                    // The colour the span names, in the room the leading edge
                    // reserved for it.
                    if let Some(swatch) = code.swatch.filter(|_| first) {
                        let side = swatch_size(font_height);
                        let inset = code.padding_x + code.border_width;
                        window.paint_quad(quad(
                            Bounds::new(
                                point(
                                    bounds.left() + inset,
                                    bounds.top() + (bounds.size.height - side) / 2.0,
                                ),
                                size(side, side),
                            ),
                            Corners::all(px(3.0)),
                            swatch,
                            Edges::all(code.swatch_border_width),
                            code.swatch_border_color,
                            Default::default(),
                        ));
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(div().pl(left).pr(right).child(inline))
        .into_any_element()
}

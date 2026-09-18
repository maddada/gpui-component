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
    if let Some(code) = code {
        style.font_family = code.font_family.clone();
        style.font_size = (base.font_size.to_pixels(rem_size) * code.font_scale).into();
    }
    style
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
                    let code = &text[start..end] != "\n"
                        && ranges.iter().any(|range| range.contains(&(offset + start)));
                    result.push(InlineFlowItem::Text {
                        state: Arc::new(Mutex::new(InlineState::default())),
                        text: SharedString::from(text[start..end].to_owned()),
                        links: clip_ranges(&links, start..end),
                        highlights: clip_ranges(&highlights, start..end),
                        reference: None,
                        code: code.then(|| style.clone()),
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
            * (usize::from(offset == 0) + usize::from(offset + word.len() == text.len())) as f32;
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
                        as f32;
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
    let left = if first {
        code.padding_x + code.border_width
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
                },
            )
            .absolute()
            .size_full(),
        )
        .child(div().pl(left).pr(right).child(inline))
        .into_any_element()
}

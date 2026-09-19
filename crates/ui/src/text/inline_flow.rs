use std::{
    ops::Range,
    sync::{Arc, Mutex},
};

use gpui::{
    AbsoluteLength, AnyElement, App, AvailableSpace, Bounds, DefiniteLength, Element, ElementId,
    GlobalElementId, HighlightStyle, InspectorElementId, InteractiveElement as _, IntoElement,
    LayoutId, LineFragment as WrapLineFragment, ObjectFit, ParentElement as _, Pixels, ShapedLine,
    SharedString, SharedUri, Size, StatefulInteractiveElement as _, Styled, StyledImage as _,
    TextRun, TextStyle, WhiteSpace, Window, img, point, prelude::FluentBuilder as _, px, relative,
    size,
};

use crate::{WindowExt as _, tooltip::Tooltip};

use super::{
    InlineCodeStyle,
    inline::{Inline, InlineState},
    inline_code,
    node::LinkMark,
};

const IMAGE_LEN: usize = 1;

pub(super) struct InlineFlow {
    id: ElementId,
    items: Vec<InlineFlowItem>,
    link_click: Option<Arc<super::LinkClickFn>>,
    link_secondary_click: Option<Arc<super::LinkClickFn>>,
    /// [`TextViewStyle::default_cursor`]: keep the plain arrow over decorated
    /// references and linked images instead of the pointing hand.
    default_cursor: bool,
    selection_states: Arc<Mutex<Vec<Arc<Mutex<InlineState>>>>>,
}

pub(super) enum InlineFlowItem {
    Text {
        code: Option<InlineCodeStyle>,
        reference: Option<super::InlineLink>,
        state: Arc<Mutex<InlineState>>,
        text: SharedString,
        links: Vec<(Range<usize>, LinkMark)>,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    },
    Image {
        url: SharedUri,
        link: Option<LinkMark>,
        title: String,
        width: Option<DefiniteLength>,
        height: Option<DefiniteLength>,
    },
}

#[derive(Default)]
pub(crate) struct InlineFlowLayoutState {
    layout: Arc<Mutex<Option<InlineFlowLayout>>>,
}

pub(super) struct InlineFlowPrepaintState {
    elements: Vec<AnyElement>,
    selection_states: Vec<Arc<Mutex<InlineState>>>,
}

#[derive(Default)]
struct InlineFlowLayout {
    fragments: Vec<PositionedFragment>,
    size: Size<Pixels>,
}

#[derive(Clone)]
enum PositionedFragment {
    Text {
        item_ix: usize,
        origin: gpui::Point<Pixels>,
        size: Size<Pixels>,
        source_range: Range<usize>,
        text: SharedString,
        links: Vec<(Range<usize>, LinkMark)>,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    },
    Image {
        item_ix: usize,
        origin: gpui::Point<Pixels>,
        size: Size<Pixels>,
    },
}

enum MeasureItem {
    Text {
        code: Option<InlineCodeStyle>,
        reference: Option<super::InlineLink>,
        text: SharedString,
        links: Vec<(Range<usize>, LinkMark)>,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    },
    Image {
        url: SharedUri,
        width: Option<DefiniteLength>,
        height: Option<DefiniteLength>,
    },
}

struct LineFragmentLayout {
    item_ix: usize,
    kind: LineFragmentKind,
    size: Size<Pixels>,
    source_range: Range<usize>,
}

enum LineFragmentKind {
    Text {
        text: SharedString,
        links: Vec<(Range<usize>, LinkMark)>,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    },
    Image,
}

impl InlineFlow {
    pub(super) fn new(
        id: impl Into<ElementId>,
        items: Vec<InlineFlowItem>,
        selection_states: Arc<Mutex<Vec<Arc<Mutex<InlineState>>>>>,
    ) -> Self {
        Self {
            id: id.into(),
            items,
            link_click: None,
            link_secondary_click: None,
            default_cursor: false,
            selection_states,
        }
    }

    pub(super) fn with_link_click(mut self, handler: Option<Arc<super::LinkClickFn>>) -> Self {
        self.link_click = handler;
        self
    }

    /// See [`TextViewStyle::default_cursor`].
    pub(super) fn with_default_cursor(mut self, default_cursor: bool) -> Self {
        self.default_cursor = default_cursor;
        self
    }

    /// A secondary (right) press on a decorated reference. The plain-link path
    /// answers one from [`Inline`], but a reference the host draws itself keeps
    /// its own element, so it needs the handler here as well.
    pub(super) fn with_link_secondary_click(
        mut self,
        handler: Option<Arc<super::LinkClickFn>>,
    ) -> Self {
        self.link_secondary_click = handler;
        self
    }

    pub(super) fn with_link_presentation(
        mut self,
        resolve: Option<&super::inline_link::LinkPresentationFn>,
    ) -> Self {
        if let Some(resolve) = resolve {
            self.items = super::inline_link::split_items(self.items, resolve);
        }
        self
    }

    pub(super) fn with_inline_code(
        mut self,
        style: Option<InlineCodeStyle>,
        ranges: Vec<Range<usize>>,
    ) -> Self {
        if let Some(style) = style {
            self.items = inline_code::split_items(self.items, &ranges, &style);
        }
        self
    }

    /// Runs after `with_inline_code`, so a colour already inside a code span keeps that span's
    /// swatch and is not split a second time.
    pub(super) fn with_prose_swatches(mut self, style: Option<InlineCodeStyle>) -> Self {
        if let Some(style) = style {
            self.items = inline_code::split_prose_swatches(self.items, &style);
        }
        self
    }

    fn image_element(
        ix: usize,
        url: &SharedUri,
        link: &Option<LinkMark>,
        title: &str,
        size: Size<Pixels>,
        handler: Option<Arc<super::LinkClickFn>>,
        default_cursor: bool,
    ) -> AnyElement {
        img(url.clone())
            .id(ix)
            .object_fit(ObjectFit::Contain)
            .max_w(relative(1.))
            .w(size.width)
            .h(size.height)
            .when_some(link.clone(), |this, link| {
                let title = title.to_string();
                this.when(!default_cursor, |this| this.cursor_pointer())
                    .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
                    .on_click(move |event, window, cx| {
                        window.end_text_selection(cx);
                        cx.stop_propagation();
                        if let Some(handler) = &handler {
                            handler(&link.url, event.modifiers(), window, cx);
                        } else {
                            cx.open_url(&link.url);
                        }
                    })
            })
            .into_any_element()
    }
}

impl IntoElement for InlineFlow {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for InlineFlow {
    type RequestLayoutState = InlineFlowLayoutState;
    type PrepaintState = InlineFlowPrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let measure_items = self.items.iter().map(MeasureItem::from).collect::<Vec<_>>();
        let line_height = window.line_height();
        let rem_size = window.rem_size();
        // Measurement runs after this element's inherited text-style scope has ended.
        // Keep the font and line height that its paint phase will use.
        let text_style = window.text_style();
        let image_sizes = measure_items
            .iter()
            .enumerate()
            .map(|(ix, item)| match item {
                MeasureItem::Image { url, width, height } => Some(measure_image_size(
                    ix,
                    url,
                    *width,
                    *height,
                    line_height,
                    rem_size,
                    window,
                    cx,
                )),
                MeasureItem::Text { .. } => None,
            })
            .collect::<Vec<_>>();
        let layout_state = InlineFlowLayoutState::default();
        let layout_ref = layout_state.layout.clone();

        let layout_id = window.request_measured_layout(Default::default(), {
            move |known_dimensions, available_space, window, _cx| {
                let wrap_width = if text_style.white_space == WhiteSpace::Normal {
                    known_dimensions.width.or(match available_space.width {
                        AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    })
                } else {
                    None
                };
                let layout = layout_flow(
                    &measure_items,
                    &image_sizes,
                    &text_style,
                    line_height,
                    wrap_width,
                    window,
                );
                let size = layout.size;
                if let Ok(mut state) = layout_ref.lock() {
                    *state = Some(layout);
                }
                size
            }
        });

        (layout_id, layout_state)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let fragments = request_layout
            .layout
            .lock()
            .ok()
            .and_then(|layout| layout.as_ref().map(|layout| layout.fragments.clone()))
            .unwrap_or_default();
        let mut elements = Vec::with_capacity(fragments.len());
        let mut selection_states = Vec::new();
        let previous_states = self
            .selection_states
            .lock()
            .map(|states| states.clone())
            .unwrap_or_default();

        for fragment in fragments {
            match fragment {
                PositionedFragment::Text {
                    item_ix,
                    origin,
                    size: fragment_size,
                    source_range,
                    text,
                    links,
                    highlights,
                    ..
                } => {
                    let previous = previous_states
                        .get(selection_states.len())
                        .filter(|state| state.lock().is_ok_and(|state| state.text == text));
                    let state = previous
                        .cloned()
                        .unwrap_or_else(|| match &self.items[item_ix] {
                            InlineFlowItem::Text {
                                state,
                                text: source,
                                ..
                            } if source_range == (0..source.len()) => state.clone(),
                            _ => Arc::new(Mutex::new(InlineState::default())),
                        });
                    if let Ok(mut state) = state.lock() {
                        state.set_text(text);
                    }
                    selection_states.push(state.clone());

                    let link = links.first().map(|(_, link)| link.clone());
                    let decorated = matches!(
                        &self.items[item_ix],
                        InlineFlowItem::Text {
                            reference: Some(_),
                            ..
                        }
                    );
                    let inline = Inline::new(
                        elements.len(),
                        state,
                        if decorated { Vec::new() } else { links },
                        highlights,
                    );
                    let mut element = if let (
                        InlineFlowItem::Text {
                            reference: Some(reference),
                            ..
                        },
                        Some(link),
                    ) = (&self.items[item_ix], link)
                    {
                        super::inline_link::element(
                            inline,
                            reference,
                            &link,
                            fragment_size,
                            elements.len(),
                            self.link_click.clone(),
                            self.link_secondary_click.clone(),
                            self.default_cursor,
                        )
                    } else if let InlineFlowItem::Text {
                        code: Some(code),
                        text: source,
                        ..
                    } = &self.items[item_ix]
                    {
                        inline_code::element(
                            inline,
                            code,
                            source_range.start == 0,
                            source_range.end == source.len(),
                            fragment_size,
                            window,
                        )
                    } else {
                        gpui::div()
                            .w(fragment_size.width)
                            .h(fragment_size.height)
                            .whitespace_nowrap()
                            .child(inline)
                            .into_any_element()
                    };
                    element.prepaint_as_root(
                        bounds.origin + origin,
                        size(
                            AvailableSpace::Definite(fragment_size.width),
                            AvailableSpace::Definite(fragment_size.height),
                        ),
                        window,
                        cx,
                    );
                    elements.push(element);
                }
                PositionedFragment::Image {
                    item_ix,
                    origin,
                    size: fragment_size,
                } => {
                    let InlineFlowItem::Image {
                        url, link, title, ..
                    } = &self.items[item_ix]
                    else {
                        continue;
                    };
                    let mut element = Self::image_element(
                        elements.len(),
                        url,
                        link,
                        title.as_str(),
                        fragment_size,
                        self.link_click.clone(),
                        self.default_cursor,
                    );
                    element.prepaint_as_root(
                        bounds.origin + origin,
                        size(
                            AvailableSpace::Definite(fragment_size.width),
                            AvailableSpace::Definite(fragment_size.height),
                        ),
                        window,
                        cx,
                    );
                    elements.push(element);
                }
            }
        }

        InlineFlowPrepaintState {
            elements,
            selection_states,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Layout can prepaint an element that is later discarded. Only publish
        // the states belonging to painted text, and keep them alive between frames.
        if let Ok(mut states) = self.selection_states.lock() {
            *states = prepaint.selection_states.clone();
        }
        for element in &mut prepaint.elements {
            element.paint(window, cx);
        }
    }
}

impl From<&InlineFlowItem> for MeasureItem {
    fn from(item: &InlineFlowItem) -> Self {
        match item {
            InlineFlowItem::Text {
                state: _,
                text,
                links,
                highlights,
                code,
                reference,
                ..
            } => MeasureItem::Text {
                text: text.clone(),
                links: links.clone(),
                highlights: highlights.clone(),
                code: code.clone(),
                reference: reference.clone(),
            },
            InlineFlowItem::Image {
                url, width, height, ..
            } => MeasureItem::Image {
                url: url.clone(),
                width: *width,
                height: *height,
            },
        }
    }
}

impl MeasureItem {
    fn len(&self) -> usize {
        match self {
            MeasureItem::Text { text, .. } => text.len(),
            MeasureItem::Image { .. } => IMAGE_LEN,
        }
    }
}

fn layout_flow(
    items: &[MeasureItem],
    image_sizes: &[Option<Size<Pixels>>],
    text_style: &TextStyle,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    window: &mut Window,
) -> InlineFlowLayout {
    let rem_size = window.rem_size();
    let total_len = items.iter().map(MeasureItem::len).sum::<usize>();
    if total_len == 0 {
        return InlineFlowLayout::default();
    }

    let line_ranges = line_ranges(items, image_sizes, text_style, wrap_width, window);
    let mut fragments = Vec::new();
    let mut max_width = Pixels::ZERO;
    let mut y = Pixels::ZERO;

    for line_range in line_ranges {
        let mut line_fragments = Vec::new();
        let mut line_width = Pixels::ZERO;
        let mut actual_line_height = line_height;
        let mut item_start = 0;

        for (item_ix, item) in items.iter().enumerate() {
            let item_end = item_start + item.len();
            if item_end <= line_range.start {
                item_start = item_end;
                continue;
            }
            if item_start >= line_range.end {
                break;
            }

            match item {
                MeasureItem::Text {
                    text,
                    links,
                    highlights,
                    code,
                    reference,
                } => {
                    let local_start = line_range.start.max(item_start) - item_start;
                    let local_end = line_range.end.min(item_end) - item_start;
                    if local_start < local_end {
                        let subtext = SharedString::from(text[local_start..local_end].to_string());
                        let highlights =
                            slice_ranges(highlights, local_start, local_end, |range, style| {
                                (range, *style)
                            });
                        let links = slice_ranges(links, local_start, local_end, |range, link| {
                            (range, link.clone())
                        });
                        let style = inline_code::text_style(text_style, code.as_ref(), rem_size);
                        let runs = runs_for_highlights(&subtext, &style, highlights.clone());
                        let shaped_line = shape_line(
                            subtext.clone(),
                            style.font_size.to_pixels(rem_size),
                            &runs,
                            window,
                        );
                        let padding = code.as_ref().map_or(px(0.0), |code| {
                            (code.padding_x + code.border_width)
                                * (usize::from(local_start == 0)
                                    + usize::from(local_end == text.len()))
                                    as f32
                        });
                        // A swatch is painted in the same leading edge `inline_code::element` pads
                        // for, so the fragment's advance has to carry it: a chip measured without
                        // it is laid out narrower than it paints and swallows the next word's
                        // first character.
                        let swatch = code
                            .as_ref()
                            .filter(|_| local_start == 0)
                            .map_or(px(0.0), |code| {
                                inline_code::leading_space(code, text_style, window)
                            });
                        let width = shaped_line.width()
                            + padding
                            + swatch
                            + reference
                                .as_ref()
                                .map_or(px(0.0), |reference| reference.icon_size + reference.gap);
                        line_width += width;
                        line_fragments.push(LineFragmentLayout {
                            item_ix,
                            kind: LineFragmentKind::Text {
                                text: subtext,
                                links,
                                highlights,
                            },
                            size: size(width, line_height),
                            source_range: local_start..local_end,
                        });
                    }
                }
                MeasureItem::Image { .. } => {
                    if line_range.start <= item_start && item_end <= line_range.end {
                        let size = image_sizes[item_ix]
                            .expect("image size should be measured before layout");
                        line_width += size.width;
                        actual_line_height = actual_line_height.max(size.height);
                        line_fragments.push(LineFragmentLayout {
                            item_ix,
                            kind: LineFragmentKind::Image,
                            size,
                            source_range: 0..IMAGE_LEN,
                        });
                    }
                }
            }

            item_start = item_end;
        }

        let mut x = Pixels::ZERO;
        for fragment in line_fragments {
            let origin = point(x, y + (actual_line_height - fragment.size.height) / 2.);
            let positioned = match fragment.kind {
                LineFragmentKind::Text {
                    text,
                    links,
                    highlights,
                } => PositionedFragment::Text {
                    item_ix: fragment.item_ix,
                    origin,
                    size: fragment.size,
                    source_range: fragment.source_range,
                    text,
                    links,
                    highlights,
                },
                LineFragmentKind::Image => PositionedFragment::Image {
                    item_ix: fragment.item_ix,
                    origin,
                    size: fragment.size,
                },
            };
            x += fragment.size.width;
            fragments.push(positioned);
        }

        max_width = max_width.max(line_width);
        y += actual_line_height;
    }

    InlineFlowLayout {
        fragments,
        size: size(max_width, y),
    }
}

fn shaped_prose_ranges(
    items: &[MeasureItem],
    text_style: &TextStyle,
    wrap_width: Option<Pixels>,
    window: &mut Window,
) -> Option<Vec<Range<usize>>> {
    let mut text = String::new();
    let mut runs = Vec::new();
    for item in items {
        let MeasureItem::Text {
            text: fragment,
            highlights,
            code: None,
            reference: None,
            ..
        } = item
        else {
            return None;
        };
        text.push_str(fragment);
        runs.extend(runs_for_highlights(
            fragment,
            text_style,
            highlights.clone(),
        ));
    }
    let len = text.len();
    let shaped = window
        .text_system()
        .shape_text(
            text.into(),
            text_style.font_size.to_pixels(window.rem_size()),
            &runs,
            None,
            None,
        )
        .ok()?;
    let mut result = Vec::new();
    let mut offset = 0;
    for line in shaped {
        let line_end = (offset + line.len() + 1).min(len);
        let mut ranges = super::inline_prose::line_ranges(&line, wrap_width);
        if let Some(last) = ranges.last_mut() {
            last.end = line_end - offset;
        } else if offset < line_end {
            ranges.push(0..line_end - offset);
        }
        result.extend(
            ranges
                .into_iter()
                .map(|range| offset + range.start..offset + range.end),
        );
        offset = line_end;
    }
    Some(result)
}

fn line_ranges(
    items: &[MeasureItem],
    image_sizes: &[Option<Size<Pixels>>],
    text_style: &TextStyle,
    wrap_width: Option<Pixels>,
    window: &mut Window,
) -> Vec<Range<usize>> {
    if let Some(ranges) = shaped_prose_ranges(items, text_style, wrap_width, window) {
        return ranges;
    }
    let rem_size = window.rem_size();
    let mut lines = Vec::new();
    let mut fragments = Vec::new();
    let mut line_len = 0;
    for (ix, item) in items.iter().enumerate() {
        line_len += item.len();
        match item {
            MeasureItem::Text {
                text,
                reference: Some(reference),
                highlights,
                ..
            } => {
                let runs = runs_for_highlights(text, text_style, highlights.clone());
                let width = shape_line(
                    text.clone(),
                    text_style.font_size.to_pixels(rem_size),
                    &runs,
                    window,
                )
                .width();
                fragments.push(WrapLineFragment::element(
                    width + reference.icon_size + reference.gap,
                    text.len(),
                ));
            }
            MeasureItem::Text {
                text,
                code: Some(code),
                ..
            } => {
                fragments.extend(inline_code::wrap_fragments(
                    text,
                    code,
                    text_style,
                    wrap_width.unwrap_or(px(f32::MAX)),
                    window,
                ));
            }
            MeasureItem::Text { text, .. } => fragments.push(WrapLineFragment::text(text)),
            MeasureItem::Image { .. } => fragments.push(WrapLineFragment::element(
                image_sizes[ix]
                    .expect("image measured before wrapping")
                    .width,
                IMAGE_LEN,
            )),
        }
        if matches!(item, MeasureItem::Text { text, .. } if text.as_ref() == "\n") {
            lines.push((std::mem::take(&mut fragments), line_len));
            line_len = 0;
        }
    }
    if line_len > 0 {
        lines.push((fragments, line_len));
    }
    let font_size = text_style.font_size.to_pixels(rem_size);
    let mut wrapper = window
        .text_system()
        .line_wrapper(text_style.font(), font_size);
    let mut ranges = Vec::new();
    let mut offset = 0;
    for (fragments, len) in lines {
        let mut start = 0;
        if let Some(width) = wrap_width {
            for boundary in wrapper.wrap_line(&fragments, width) {
                let end = boundary.ix.min(len);
                if start < end {
                    ranges.push(offset + start..offset + end);
                }
                start = end;
            }
        }
        if start < len {
            ranges.push(offset + start..offset + len);
        }
        offset += len;
    }
    ranges
}

#[allow(clippy::too_many_arguments)]
fn measure_image_size(
    ix: usize,
    url: &SharedUri,
    width: Option<DefiniteLength>,
    height: Option<DefiniteLength>,
    line_height: Pixels,
    rem_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Size<Pixels> {
    let intrinsic_size = if width.is_some() && height.is_some() {
        None
    } else {
        intrinsic_image_size(ix, url, width, height, window, cx)
    };
    image_size(width, height, intrinsic_size, line_height, rem_size)
}

fn intrinsic_image_size(
    ix: usize,
    url: &SharedUri,
    width: Option<DefiniteLength>,
    height: Option<DefiniteLength>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Size<Pixels>> {
    let mut element = img(url.clone())
        .id(ix)
        .object_fit(ObjectFit::Contain)
        .max_w(relative(1.))
        .when_some(width, |this, width| this.w(width))
        .when_some(height, |this, height| this.h(height))
        .into_any_element();
    let measured_size = element.layout_as_root(AvailableSpace::min_size(), window, cx);

    if measured_size.width <= Pixels::ZERO || measured_size.height <= Pixels::ZERO {
        None
    } else {
        Some(measured_size)
    }
}

fn image_size(
    width: Option<DefiniteLength>,
    height: Option<DefiniteLength>,
    intrinsic_size: Option<Size<Pixels>>,
    line_height: Pixels,
    rem_size: Pixels,
) -> Size<Pixels> {
    let base_size = AbsoluteLength::Pixels(line_height);
    match (width, height) {
        (Some(width), Some(height)) => size(
            width.to_pixels(base_size, rem_size),
            height.to_pixels(base_size, rem_size),
        ),
        (Some(width), None) => {
            let width = width.to_pixels(base_size, rem_size);
            let height = intrinsic_size
                .and_then(|intrinsic_size| {
                    (intrinsic_size.width > Pixels::ZERO && intrinsic_size.height > Pixels::ZERO)
                        .then(|| width * (intrinsic_size.height / intrinsic_size.width))
                })
                .unwrap_or(line_height);
            size(width, height)
        }
        (None, Some(height)) => {
            let height = height.to_pixels(base_size, rem_size);
            let width = intrinsic_size
                .and_then(|intrinsic_size| {
                    (intrinsic_size.width > Pixels::ZERO && intrinsic_size.height > Pixels::ZERO)
                        .then(|| height * (intrinsic_size.width / intrinsic_size.height))
                })
                .unwrap_or(height);
            size(width, height)
        }
        (None, None) => inline_image_size_for_line(intrinsic_size, line_height),
    }
}

fn inline_image_size_for_line(
    intrinsic_size: Option<Size<Pixels>>,
    line_height: Pixels,
) -> Size<Pixels> {
    let height = line_height * 0.75;
    let aspect_ratio = intrinsic_size
        .and_then(|intrinsic_size| {
            (intrinsic_size.width > Pixels::ZERO && intrinsic_size.height > Pixels::ZERO)
                .then(|| intrinsic_size.width / intrinsic_size.height)
        })
        .unwrap_or(1.);

    size((height * aspect_ratio).max(px(1.)), height.max(px(1.)))
}

fn runs_for_highlights(
    text: &str,
    default_style: &TextStyle,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut ix = 0;

    for (range, highlight) in highlights {
        if ix < range.start {
            runs.push(default_style.clone().to_run(range.start - ix));
        }
        runs.push(
            default_style
                .clone()
                .highlight(highlight)
                .to_run(range.len()),
        );
        ix = range.end;
    }

    if ix < text.len() {
        runs.push(default_style.to_run(text.len() - ix));
    }

    runs
}

fn shape_line(
    text: SharedString,
    font_size: Pixels,
    runs: &[TextRun],
    window: &mut Window,
) -> ShapedLine {
    window.text_system().shape_line(text, font_size, runs, None)
}

fn slice_ranges<T, U>(
    ranges: &[(Range<usize>, T)],
    start: usize,
    end: usize,
    map: impl Fn(Range<usize>, &T) -> U,
) -> Vec<U> {
    ranges
        .iter()
        .filter_map(|(range, value)| {
            let clipped_start = range.start.max(start);
            let clipped_end = range.end.min(end);
            (clipped_start < clipped_end)
                .then(|| map((clipped_start - start)..(clipped_end - start), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_image_without_explicit_size_scales_intrinsic_ratio_to_line_height() {
        let line_height = px(20.);
        let intrinsic_size = size(px(160.), px(40.));

        let measured = inline_image_size_for_line(Some(intrinsic_size), line_height);

        assert_eq!(measured, size(px(60.), px(15.)));
    }

    #[test]
    fn inline_image_without_intrinsic_size_uses_compact_square_fallback() {
        let measured = inline_image_size_for_line(None, px(20.));

        assert_eq!(measured, size(px(15.), px(15.)));
    }
}

//! Links a host presents as reference chips: an icon and a coloured label in
//! place of the link's own text, one indivisible piece of the line.

use std::sync::Arc;

use gpui::{
    AnyElement, AnyView, App, ClickEvent, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Pixels, SharedString, Size, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px, svg,
};

use super::{
    inline::{Inline, InlineHighlight},
    inline_flow::{InlineFlowItem, slice_ranges},
    node::LinkMark,
    text_view::{
        LinkClickHandlerFn, LinkSecondaryClickFn, handle_link_click, is_claimed_secondary_click,
    },
};

/// Host-provided presentation for an indivisible inline reference.
///
/// Returned by [`TextView::link_presentation`](super::TextView::link_presentation)
/// for a link it should draw as a chip: `icon` then `label` in `color`,
/// `gap` apart, with `title` as its tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineLink {
    pub label: SharedString,
    pub title: SharedString,
    pub icon: SharedString,
    pub icon_size: Pixels,
    pub gap: Pixels,
    pub color: Hsla,
    /// Ellipsize from the start when the chip is wider than the line, so a
    /// path keeps its file name visible.
    pub truncate_start: bool,
}

pub(crate) type LinkPresentationFn = dyn Fn(&str, &str) -> Option<InlineLink> + Send + Sync;

/// Builds the tooltip a reference chip shows its title in.
pub(crate) type LinkTooltipFn =
    dyn Fn(SharedString, &mut Window, &mut App) -> AnyView + Send + Sync;

/// A reference drawn as a chip in place of its link's text.
#[derive(Clone)]
pub(super) struct ReferenceChip {
    pub(super) link: InlineLink,
    /// How long the link's own text is in the item's source text, which is
    /// the text a selection of the chip stands for.
    pub(super) source_len: usize,
}

/// Splits the text items of a flow at every link `resolve` presents as a
/// chip, so each chip is an item of its own.
///
/// A chip shows its presentation's label, but it stands for the link's own
/// text: it keeps the item's source state and the link's range in it, so a
/// selection over the chip selects the whole link, and copy reads the link's
/// text, as source copy reads its Markdown.
pub(super) fn split_references(
    items: Vec<InlineFlowItem>,
    resolve: &LinkPresentationFn,
) -> Vec<InlineFlowItem> {
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let InlineFlowItem::Text {
            state,
            text,
            links,
            highlights,
            backgrounds,
            reveal,
            source_offset,
            reference: None,
        } = item
        else {
            result.push(item);
            continue;
        };
        // A link whose text is cut by a mark arrives as several ranges.
        let mut merged: Vec<(std::ops::Range<usize>, LinkMark)> = Vec::new();
        for (range, link) in &links {
            match merged.last_mut() {
                Some((previous, mark)) if previous.end == range.start && mark.url == link.url => {
                    previous.end = range.end;
                }
                _ => merged.push((range.clone(), link.clone())),
            }
        }
        let chips = merged
            .into_iter()
            .filter_map(|(range, link)| {
                let presentation = resolve(&link.url, text.get(range.clone())?)?;
                Some((range, link, presentation))
            })
            .collect::<Vec<_>>();
        if chips.is_empty() {
            result.push(InlineFlowItem::Text {
                state,
                text,
                links,
                highlights,
                backgrounds,
                reveal,
                source_offset,
                reference: None,
            });
            continue;
        }

        let piece = |start: usize, end: usize| InlineFlowItem::Text {
            state: state.clone(),
            text: text[start..end].to_string().into(),
            links: slice_ranges(&links, start, end, |range, link| (range, link.clone())),
            highlights: slice_ranges(&highlights, start, end, |range, highlight| {
                (range, highlight.clone())
            }),
            backgrounds: slice_ranges(&backgrounds, start, end, |range, color| (range, *color)),
            reveal: reveal.as_ref().and_then(|reveal| reveal.rebase(start, end)),
            source_offset: source_offset + start,
            reference: None,
        };
        let mut start = 0;
        for (range, link, presentation) in chips {
            if range.start < start {
                continue;
            }
            if start < range.start {
                result.push(piece(start, range.start));
            }
            let len = presentation.label.len();
            result.push(InlineFlowItem::Text {
                state: state.clone(),
                text: presentation.label.clone(),
                links: vec![(0..len, link)],
                highlights: vec![(
                    0..len,
                    InlineHighlight::from(gpui::HighlightStyle {
                        color: Some(presentation.color),
                        ..Default::default()
                    }),
                )],
                backgrounds: Vec::new(),
                reveal: reveal
                    .as_ref()
                    .and_then(|reveal| reveal.rebase(range.start, range.end))
                    .map(|reveal| reveal.clamp(0, 0)),
                source_offset: source_offset + range.start,
                reference: Some(ReferenceChip {
                    link: presentation,
                    source_len: range.len(),
                }),
            });
            start = range.end;
        }
        if start < text.len() {
            result.push(piece(start, text.len()));
        }
    }
    result
}

/// The chip element for one reference fragment: the icon, then the label
/// laid out by `inline` (clipped with an ellipsis when the chip was narrowed
/// to the line), with the link's click, secondary press and tooltip.
#[allow(clippy::too_many_arguments)]
pub(super) fn element(
    inline: Inline,
    reference: &InlineLink,
    link: &LinkMark,
    size: Size<Pixels>,
    id: usize,
    handler: Option<Arc<LinkClickHandlerFn>>,
    secondary: Option<Arc<LinkSecondaryClickFn>>,
    tooltip: Option<Arc<LinkTooltipFn>>,
    default_cursor: bool,
) -> AnyElement {
    let url = link.url.clone();
    let aux_url = link.url.clone();
    let secondary_url = link.url.clone();
    let aux_handler = handler.clone();
    let has_secondary = secondary.is_some();
    let title = reference.title.clone();
    div()
        .id(("inline-link", id))
        .role(gpui::Role::Link)
        .aria_label(SharedString::from(format!("Open {}", reference.label)))
        .w(size.width)
        .h(size.height)
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(reference.gap)
        .text_color(reference.color)
        .when(!default_cursor, |this| this.cursor_pointer())
        .whitespace_nowrap()
        .when_some(tooltip, |this, tooltip| {
            this.tooltip(move |window, cx| tooltip(title.clone(), window, cx))
        })
        .child(
            svg()
                .path(reference.icon.clone())
                .size(reference.icon_size)
                .text_color(reference.color)
                .flex_shrink_0(),
        )
        .child(
            div()
                .w((size.width - reference.icon_size - reference.gap).max(px(0.)))
                .min_w_0()
                .overflow_hidden()
                .child(inline),
        )
        .on_click(move |event, window, cx| {
            if matches!(event, ClickEvent::Mouse(_))
                && crate::TextSelection::has_selection(window, cx)
            {
                return;
            }
            crate::TextSelection::end(window, cx);
            cx.stop_propagation();
            handle_link_click(&handler, url.clone(), event.clone(), window, cx);
        })
        .on_aux_click(move |event, window, cx| {
            if is_claimed_secondary_click(event, has_secondary) {
                return;
            }
            crate::TextSelection::end(window, cx);
            cx.stop_propagation();
            handle_link_click(&aux_handler, aux_url.clone(), event.clone(), window, cx);
        })
        // The host's menu opens on the press, the way every other context
        // menu in a desktop app does, and before the release can start a
        // selection.
        .when_some(secondary, |this, secondary| {
            this.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                crate::TextSelection::end(window, cx);
                cx.stop_propagation();
                secondary(&secondary_url, event.modifiers, window, cx);
            })
        })
        .into_any_element()
}

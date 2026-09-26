use std::sync::{Arc, Mutex};

use super::{
    inline::{Inline, InlineState},
    inline_code::clip_ranges,
    inline_flow::InlineFlowItem,
    node::LinkMark,
};
use crate::{WindowExt as _, tooltip::Tooltip};
use gpui::{
    AnyElement, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, Size,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px, svg,
};

/// Host-provided presentation for an indivisible inline reference.
#[derive(Clone, PartialEq)]
pub struct InlineLink {
    pub label: SharedString,
    pub title: SharedString,
    pub icon: SharedString,
    pub icon_size: Pixels,
    pub gap: Pixels,
    pub color: Hsla,
    /// Ellipsize from the start when the chip is wider than the line, so a path keeps its file
    /// name visible.
    pub truncate_start: bool,
}

pub(super) type LinkPresentationFn = dyn Fn(&str, &str) -> Option<InlineLink> + Send + Sync;

pub(super) fn split_items(
    items: Vec<InlineFlowItem>,
    resolve: &LinkPresentationFn,
) -> Vec<InlineFlowItem> {
    let mut result = Vec::new();
    for item in items {
        let InlineFlowItem::Text {
            text,
            links,
            highlights,
            code,
            ..
        } = &item
        else {
            result.push(item);
            continue;
        };
        let mut merged: Vec<(std::ops::Range<usize>, LinkMark)> = Vec::new();
        for (range, link) in links {
            if let Some((previous, mark)) = merged.last_mut()
                && previous.end == range.start
                && mark.url == link.url
            {
                previous.end = range.end;
            } else {
                merged.push((range.clone(), link.clone()));
            }
        }
        let mut start = 0;
        for (range, link) in merged {
            let Some(reference) = resolve(&link.url, &text[range.clone()]) else {
                continue;
            };
            if range.start < start {
                continue;
            }
            if start < range.start {
                result.push(InlineFlowItem::Text {
                    state: Arc::new(Mutex::new(InlineState::default())),
                    text: text[start..range.start].to_owned().into(),
                    links: clip_ranges(links, start..range.start),
                    highlights: clip_ranges(highlights, start..range.start),
                    code: code.clone(),
                    reference: None,
                });
            }
            let len = reference.label.len();
            result.push(InlineFlowItem::Text {
                state: Arc::new(Mutex::new(InlineState::default())),
                text: reference.label.clone(),
                links: vec![(0..len, link)],
                highlights: vec![(
                    0..len,
                    gpui::HighlightStyle {
                        color: Some(reference.color),
                        ..Default::default()
                    },
                )],
                code: None,
                reference: Some(reference),
            });
            start = range.end;
        }
        if start == 0 {
            result.push(item);
        } else if start < text.len() {
            result.push(InlineFlowItem::Text {
                state: Arc::new(Mutex::new(InlineState::default())),
                text: text[start..].to_owned().into(),
                links: clip_ranges(links, start..text.len()),
                highlights: clip_ranges(highlights, start..text.len()),
                code: code.clone(),
                reference: None,
            });
        }
    }
    result
}

pub(super) fn element(
    inline: Inline,
    reference: &InlineLink,
    link: &LinkMark,
    size: Size<Pixels>,
    clamped: bool,
    id: usize,
    handler: Option<Arc<super::LinkClickFn>>,
    secondary: Option<Arc<super::LinkClickFn>>,
    default_cursor: bool,
) -> AnyElement {
    let title = reference.title.clone();
    let url = link.url.clone();
    let secondary_url = link.url.clone();
    div()
        .id(("inline-link", id))
        .tab_index(0)
        .role(gpui::Role::Button)
        .aria_label(format!("Open {}", reference.label))
        .w(size.width)
        .h(size.height)
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(reference.gap)
        .text_color(reference.color)
        .when(!default_cursor, |this| this.cursor_pointer())
        .whitespace_nowrap()
        .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
        .child(
            svg()
                .path(reference.icon.clone())
                .size(reference.icon_size)
                .text_color(reference.color)
                .flex_shrink_0(),
        )
        // Only a chip narrowed to the line may ellipsize. Every other chip is exactly as wide as
        // its shaped label, and GPUI's truncation sums per-character advances without kerning,
        // which comes out a pixel or two wider than the shaped line and cut labels that fit.
        .child(
            div()
                .w((size.width - reference.icon_size - reference.gap).max(px(0.0)))
                .min_w_0()
                .when(clamped, |this| {
                    let this = this.overflow_hidden();
                    if reference.truncate_start {
                        this.text_ellipsis_start()
                    } else {
                        this.text_ellipsis()
                    }
                })
                .child(inline),
        )
        .on_click(move |event, window, cx| {
            if matches!(event, gpui::ClickEvent::Mouse(_)) && window.has_text_selection(cx) {
                return;
            }
            window.end_text_selection(cx);
            cx.stop_propagation();
            if let Some(handler) = &handler {
                handler(&url, event.modifiers(), window, cx);
            } else {
                cx.open_url(&url);
            }
        })
        // The host's menu opens on the press, the way every other context menu
        // in a desktop app does, and before the release can start a selection.
        .when_some(secondary, |this, secondary| {
            this.on_mouse_down(gpui::MouseButton::Right, move |event, window, cx| {
                window.end_text_selection(cx);
                cx.stop_propagation();
                secondary(&secondary_url, event.modifiers, window, cx);
            })
        })
        .into_any_element()
}

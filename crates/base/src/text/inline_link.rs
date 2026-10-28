//! Links a host presents as reference chips: an icon and a coloured label in
//! place of the link's own text, one indivisible piece of the line.

use std::sync::Arc;

use gpui::{
    Anchor, AnyElement, AnyView, App, ClickEvent, Entity, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, SharedString,
    Size, StatefulInteractiveElement as _, Styled as _, Window, anchored, deferred, div, point,
    prelude::FluentBuilder as _, px, svg,
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineLink {
    pub label: SharedString,
    pub title: SharedString,
    /// Empty for a chip without an icon (give it a zero `icon_size` and `gap`).
    pub icon: SharedString,
    pub icon_size: Pixels,
    pub gap: Pixels,
    pub color: Hsla,
    /// Ghostex fork: the icon's own tint, when it differs from the label's
    /// (an agent's logo in its brand colour beside a neutral label).
    pub icon_color: Option<Hsla>,
    /// Ellipsize from the start when the chip is wider than the line, so a
    /// path keeps its file name visible.
    pub truncate_start: bool,
    /// Ghostex fork: an outlined box around the chip, or none.
    pub frame: Option<InlineLinkFrame>,
    /// Ghostex fork: the label's weight, when it differs from the line's.
    pub font_weight: Option<FontWeight>,
    /// Ghostex fork: the label is struck through.
    pub strikethrough: bool,
    /// Ghostex fork: the label's typeface, when it differs from the line's
    /// (a chip drawn as inline code).
    pub font_family: Option<SharedString>,
    /// Ghostex fork: the label's size relative to the line's.
    pub font_scale: Option<f32>,
    /// Ghostex fork: buttons shown in a small bar above the chip while the
    /// pointer is on the chip or the bar; the bar never covers the label.
    pub hover_actions: Vec<InlineLinkAction>,
    /// Ghostex fork: how that bar looks.
    pub hover_bar: Option<InlineLinkBar>,
}

/// Ghostex fork: one button of a chip's hover bar. Its press goes to the link
/// click handler with `url`, as a press on a link with that address would.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineLinkAction {
    pub url: SharedString,
    /// What the button does, read by assistive technology and shown as its
    /// tooltip.
    pub label: SharedString,
    pub icon: SharedString,
}

/// Ghostex fork: the bar a chip's hover buttons sit in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineLinkBar {
    pub background: Hsla,
    pub border_color: Hsla,
    pub border_width: Pixels,
    pub radius: Pixels,
    pub padding: Pixels,
    pub gap: Pixels,
    /// The room between the bar and the chip below it.
    pub spacing: Pixels,
    pub button_size: Pixels,
    pub button_radius: Pixels,
    pub button_hover_background: Hsla,
    pub icon_size: Pixels,
    pub icon_color: Hsla,
}

/// Ghostex fork: whether the pointer is on a chip or on its hover bar, kept
/// across frames so the bar stays while the pointer crosses into it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct InlineLinkHover {
    chip: bool,
    bar: bool,
}

/// Ghostex fork: the outlined box a chip can sit in (a chat's question tags):
/// `padding_x` inside a `border_width` edge on both ends, `radius` corners,
/// and a fill only under the pointer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineLinkFrame {
    pub padding_x: Pixels,
    pub border_width: Pixels,
    pub border_color: Hsla,
    pub radius: Pixels,
    pub hover_background: Option<Hsla>,
    /// Ghostex fork: the fill under the chip when the pointer is elsewhere.
    pub background: Option<Hsla>,
}

impl InlineLink {
    /// The room the chip adds around its label: the icon and its gap, and the
    /// frame's padding and edges on both ends.
    pub(super) fn extra_width(&self) -> Pixels {
        let frame = self.frame.as_ref().map_or(Pixels::ZERO, |frame| {
            (frame.padding_x + frame.border_width) * 2.
        });
        self.icon_size + self.gap + frame
    }
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
                highlights: vec![(0..len, {
                    let mut highlight = InlineHighlight::from(gpui::HighlightStyle {
                        color: Some(presentation.color),
                        font_weight: presentation.font_weight,
                        strikethrough: presentation.strikethrough.then(|| {
                            gpui::StrikethroughStyle {
                                thickness: px(1.),
                                color: Some(presentation.color),
                            }
                        }),
                        ..Default::default()
                    });
                    highlight.font_family = presentation.font_family.clone();
                    highlight.font_size_scale = presentation.font_scale;
                    highlight
                })],
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
    hover: Option<Entity<InlineLinkHover>>,
    bar_below: bool,
    cx: &App,
) -> AnyElement {
    let url = link.url.clone();
    let aux_url = link.url.clone();
    let secondary_url = link.url.clone();
    let aux_handler = handler.clone();
    let has_secondary = secondary.is_some();
    let title = reference.title.clone();
    let label_width = (size.width - reference.extra_width()).max(px(0.));
    let bar = hover.as_ref().and_then(|hover| {
        let state = *hover.read(cx);
        hover_bar(
            reference,
            id,
            hover,
            state,
            bar_below,
            handler.clone(),
            tooltip.clone(),
        )
    });
    let chip_hover = hover.clone();
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
        .when_some(reference.frame.as_ref(), |this, frame| {
            this.px(frame.padding_x)
                .border(frame.border_width)
                .border_color(frame.border_color)
                .rounded(frame.radius)
                .when_some(frame.background, |this, fill| this.bg(fill))
                .when_some(frame.hover_background, |this, fill| {
                    this.hover(move |style| style.bg(fill))
                })
        })
        .when(!default_cursor, |this| this.cursor_pointer())
        .whitespace_nowrap()
        .when_some(tooltip.filter(|_| !title.is_empty()), |this, tooltip| {
            this.tooltip(move |window, cx| tooltip(title.clone(), window, cx))
        })
        .when(!reference.icon.is_empty(), |this| {
            this.child(
                svg()
                    .path(reference.icon.clone())
                    .size(reference.icon_size)
                    .text_color(reference.icon_color.unwrap_or(reference.color))
                    .flex_shrink_0(),
            )
        })
        .child(
            div()
                .w(label_width)
                .min_w_0()
                .overflow_hidden()
                .child(inline),
        )
        .when_some(chip_hover, |this, hover| {
            this.on_hover(move |hovered, window, cx| {
                let hovered = *hovered;
                hover.update(cx, |state, _| state.chip = hovered);
                window.refresh();
            })
        })
        // Above the chip (below it when `bar_below`), out of its clip: an
        // absolute anchor at its top or bottom left, the bar drawn after
        // everything else from there.
        .when_some(bar, |this, bar| {
            this.relative().child(
                div()
                    .absolute()
                    .left_0()
                    .map(|anchor| {
                        if bar_below {
                            anchor.bottom_0()
                        } else {
                            anchor.top_0()
                        }
                    })
                    .child(deferred(bar)),
            )
        })
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

/// Ghostex fork: the bar of a chip's hover buttons, anchored above the chip
/// (below it when `below`, which the flow decides from the room its clip
/// leaves), shown while the pointer is on the chip or the bar. Its edge
/// facing the chip reaches into the chip's own, so the pointer crossing from
/// the chip into the bar never leaves both.
#[allow(clippy::too_many_arguments)]
fn hover_bar(
    reference: &InlineLink,
    id: usize,
    hover: &Entity<InlineLinkHover>,
    state: InlineLinkHover,
    below: bool,
    handler: Option<Arc<LinkClickHandlerFn>>,
    tooltip: Option<Arc<LinkTooltipFn>>,
) -> Option<AnyElement> {
    let look = reference.hover_bar.clone()?;
    if reference.hover_actions.is_empty() || !(state.chip || state.bar) {
        return None;
    }
    let bar_hover = hover.clone();
    let overlap = px(2.);
    Some(
        anchored()
            .anchor(if below {
                Anchor::TopLeft
            } else {
                Anchor::BottomLeft
            })
            .offset(point(Pixels::ZERO, if below { -overlap } else { overlap }))
            .child(
                div()
                    .id(("inline-link-bar", id))
                    .map(|bridge| {
                        if below {
                            bridge.pt(look.spacing + overlap)
                        } else {
                            bridge.pb(look.spacing + overlap)
                        }
                    })
                    .on_hover(move |hovered, window, cx| {
                        let hovered = *hovered;
                        bar_hover.update(cx, |state, _| state.bar = hovered);
                        window.refresh();
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.gap)
                            .p(look.padding)
                            .rounded(look.radius)
                            .border(look.border_width)
                            .border_color(look.border_color)
                            .bg(look.background)
                            .shadow_md()
                            .children(reference.hover_actions.iter().enumerate().map(
                                |(index, action)| {
                                    let url = action.url.clone();
                                    let label = action.label.clone();
                                    let handler = handler.clone();
                                    div()
                                        .id(("inline-link-bar-action", index))
                                        .role(gpui::Role::Button)
                                        .aria_label(action.label.clone())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(look.button_size)
                                        .rounded(look.button_radius)
                                        .cursor_pointer()
                                        .hover(move |style| style.bg(look.button_hover_background))
                                        .when_some(tooltip.clone(), |this, tooltip| {
                                            this.tooltip(move |window, cx| {
                                                tooltip(label.clone(), window, cx)
                                            })
                                        })
                                        .child(
                                            svg()
                                                .path(action.icon.clone())
                                                .size(look.icon_size)
                                                .text_color(look.icon_color)
                                                .flex_shrink_0(),
                                        )
                                        .on_click(move |event, window, cx| {
                                            crate::TextSelection::end(window, cx);
                                            cx.stop_propagation();
                                            handle_link_click(
                                                &handler,
                                                url.clone(),
                                                event.clone(),
                                                window,
                                                cx,
                                            );
                                        })
                                },
                            )),
                    ),
            )
            .into_any_element(),
    )
}

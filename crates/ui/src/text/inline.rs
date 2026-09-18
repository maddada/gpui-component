use gpui::Corners;
use std::{
    ops::Range,
    rc::Rc,
    sync::{Arc, Mutex},
};

use gpui::{
    App, BorderStyle, Bounds, CursorStyle, Edges, Element, ElementId, GlobalElementId,
    HighlightStyle, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, StyledText,
    TextLayout, Window, px, quad,
};

use crate::{
    ActiveTheme, WindowExt as _,
    global_state::GlobalState,
    input::Selection,
    text::node::LinkMark,
    text::selection_registry::{clamp_boundary, range_rects},
};

/// A inline element used to render a inline text and support selectable.
///
/// All text in TextView (including the CodeBlock) used this for text rendering.
pub(super) struct Inline {
    id: ElementId,
    text: SharedString,
    links: Rc<Vec<(Range<usize>, LinkMark)>>,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    styled_text: StyledText,

    state: Arc<Mutex<InlineState>>,
}

/// The inline text state, used RefCell to keep the selection state.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct InlineState {
    /// Index of the link under the pointer as of the last paint or mouse move.
    hovered_index: Option<usize>,
    /// The text that actually rendering, matched with selection.
    pub(super) text: SharedString,
    pub(super) selection: Option<Selection>,
}

impl InlineState {
    /// Save actually rendered text for selected text to use.
    pub(crate) fn set_text(&mut self, text: SharedString) {
        self.text = text;
    }
}

impl Inline {
    pub(super) fn new(
        id: impl Into<ElementId>,
        state: Arc<Mutex<InlineState>>,
        links: Vec<(Range<usize>, LinkMark)>,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> Self {
        let text = state
            .lock()
            .map(|state| state.text.clone())
            .unwrap_or_default();

        Self {
            id: id.into(),
            links: Rc::new(links),
            highlights,
            text: text.clone(),
            styled_text: StyledText::new(text),
            state,
        }
    }

    /// Index into `links` of the link at given mouse position.
    fn link_index_for_position(
        layout: &TextLayout,
        links: &[(Range<usize>, LinkMark)],
        position: Point<Pixels>,
    ) -> Option<usize> {
        let offset = layout.index_for_position(position).ok()?;
        links.iter().position(|(range, _)| range.contains(&offset))
    }

    /// Get link at given mouse position.
    fn link_for_position(
        layout: &TextLayout,
        links: &Vec<(Range<usize>, LinkMark)>,
        position: Point<Pixels>,
    ) -> Option<LinkMark> {
        let offset = layout.index_for_position(position).ok()?;
        for (range, link) in links.iter() {
            if range.contains(&offset) {
                return Some(link.clone());
            }
        }

        None
    }

    /// Paint selected bounds for debug.
    #[allow(unused)]
    fn paint_selected_bounds(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        window.paint_quad(gpui::PaintQuad {
            bounds,
            background: cx.theme().blue.alpha(0.01).into(),
            corner_radii: Corners::default(),
            border_color: gpui::transparent_black(),
            border_style: BorderStyle::default(),
            border_widths: gpui::Edges::all(px(0.)),
        });
    }

    /// Paint the selection wash for `range`, one box per visual row.
    fn paint_selection(
        range: &Range<usize>,
        text_layout: &TextLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        for bounds in range_rects(text_layout, range) {
            window.paint_quad(quad(
                bounds,
                px(0.),
                cx.theme().selection,
                Edges::default(),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));
        }
    }
}

impl IntoElement for Inline {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Inline {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_element_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let text_style = window.text_style();

        let mut runs = Vec::new();
        let mut ix = 0;
        for (range, highlight) in self.highlights.iter() {
            if ix < range.start {
                runs.push(text_style.clone().to_run(range.start - ix));
            }
            runs.push(text_style.clone().highlight(*highlight).to_run(range.len()));
            ix = range.end;
        }
        if ix < self.text.len() {
            runs.push(text_style.to_run(self.text.len() - ix));
        }

        self.styled_text = StyledText::new(self.text.clone()).with_runs(runs);
        let (layout_id, _) =
            self.styled_text
                .request_layout(global_element_id, inspector_id, window, cx);

        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled_text
            .prepaint(id, inspector_id, bounds, &mut (), window, cx);

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let current_view = window.current_view();
        let hitbox = prepaint;
        let Ok(mut state) = self.state.lock() else {
            return;
        };

        let text_layout = self.styled_text.layout().clone();

        // Register in document order and take this frame's selected range from
        // the window selection (see `selection_registry`).
        let text_view_state = GlobalState::global(cx).text_view_state().cloned();
        let is_selectable = text_view_state
            .as_ref()
            .is_some_and(|view| view.read(cx).is_selectable());
        let selection = match (&text_view_state, global_id) {
            (Some(view), Some(id)) if is_selectable => {
                let all_selected = view.read(cx).is_all_selected();
                let range = crate::Root::register_selectable_inline(
                    view,
                    id,
                    self.text.clone(),
                    text_layout.clone(),
                    hitbox.clone(),
                    self.state.clone(),
                    window,
                    cx,
                );
                if all_selected {
                    Some(0..self.text.len())
                } else {
                    range.map(|range| {
                        clamp_boundary(&self.text, range.start)
                            ..clamp_boundary(&self.text, range.end)
                    })
                }
            }
            _ => None,
        }
        .filter(|range| !range.is_empty());
        state.selection = selection.clone().map(Selection::from);

        // The wash goes under the glyphs so selected text stays crisp.
        if let Some(range) = &selection {
            Self::paint_selection(range, &text_layout, window, cx);
        }
        self.styled_text
            .paint(global_id, None, bounds, &mut (), &mut (), window, cx);

        if is_selectable {
            window.set_cursor_style(CursorStyle::IBeam, &hitbox);
        }

        // A run without links needs no link cursor and no mouse listeners at
        // all: selection input is handled once per window by the controller.
        if self.links.is_empty() {
            return;
        }

        // link cursor pointer
        let mouse_position = window.mouse_position();
        let hovered_link = Self::link_index_for_position(&text_layout, &self.links, mouse_position);
        state.hovered_index = hovered_link;
        if hovered_link.is_some() {
            window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
        }

        // secondary press on a link, for the host's own link menu
        window.on_mouse_event({
            let hitbox = hitbox.clone();
            let text_layout = text_layout.clone();
            let links = self.links.clone();
            let text_view_state = GlobalState::global(cx).text_view_state().cloned();

            move |event: &MouseDownEvent, phase, window, cx| {
                if event.button != MouseButton::Right
                    || !phase.bubble()
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                let Some(link) = Self::link_for_position(&text_layout, &links, event.position)
                else {
                    return;
                };
                let handler = text_view_state
                    .as_ref()
                    .and_then(|state| state.read(cx).link_secondary_click.clone());
                if let Some(handler) = handler {
                    cx.stop_propagation();
                    handler(&link.url, event.modifiers, window, cx);
                }
            }
        });

        // Mouse move: repaint only when the pointer enters or leaves a link, so
        // the cursor style set above follows it. Notifying on every glyph the
        // pointer crosses would redraw the whole window on each mouse move.
        window.on_mouse_event({
            let hitbox = hitbox.clone();
            let text_layout = text_layout.clone();
            let links = self.links.clone();
            let inline_state = self.state.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if !phase.bubble() {
                    return;
                }
                let updated = hitbox
                    .is_hovered(window)
                    .then(|| Self::link_index_for_position(&text_layout, &links, event.position))
                    .flatten();
                let Ok(mut inline_state) = inline_state.lock() else {
                    return;
                };
                if inline_state.hovered_index != updated {
                    inline_state.hovered_index = updated;
                    cx.notify(current_view);
                }
            }
        });

        // click to open link
        window.on_mouse_event({
            let links = self.links.clone();
            let text_layout = text_layout.clone();
            let hitbox = hitbox.clone();
            let text_view_state = GlobalState::global(cx).text_view_state().cloned();

            move |event: &MouseUpEvent, phase, window, cx| {
                if event.button != gpui::MouseButton::Left
                    || !phase.bubble()
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                if text_view_state
                    .as_ref()
                    .is_some_and(|state| state.read(cx).has_selection(window, cx))
                {
                    return;
                }

                if let Some(link) = Self::link_for_position(&text_layout, &links, event.position) {
                    window.end_text_selection(cx);
                    cx.stop_propagation();
                    let handler = text_view_state
                        .as_ref()
                        .and_then(|state| state.read(cx).link_click.clone());
                    if let Some(handler) = handler {
                        handler(&link.url, event.modifiers, window, cx);
                    } else {
                        cx.open_url(&link.url);
                    }
                }
            }
        });
    }
}

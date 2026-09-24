use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{Arc, Mutex},
};

use gpui::{
    App, Bounds, Context, Element, ElementId, Entity, EntityId, GlobalElementId, Hitbox,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, ScrollWheelEvent, SharedString, Style, TextLayout, WeakEntity,
    Window,
};

use crate::{
    Root,
    global_state::GlobalState,
    scroll::AutoScroll,
    text::{
        TextViewState,
        inline::InlineState,
        selection::word_range_at,
        selection_registry::{InlineKey, RegisteredInline, SelectedSpan},
    },
};

/// The modal layer a selectable [`TextView`](crate::text::TextView) belongs to.
///
/// Window text selection is global, but when a modal (Dialog/Sheet) is open the
/// selection must be confined to that modal so a drag that leaves the modal
/// (e.g. over the overlay) cannot select TextViews behind it. Each selectable
/// view is tagged with the scope it painted under (see [`SelectionScopeMarker`]),
/// and selection only considers views whose scope matches the active layer (see
/// [`Root::active_selection_scope`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SelectionScope {
    /// The base window content, outside any Dialog/Sheet.
    Base,
    /// A Dialog at the given layer index (matches `Dialog::layer_ix`, i.e. the
    /// position in `Root::active_dialogs`).
    Dialog(usize),
    /// The active Sheet.
    Sheet,
}

/// Extension trait that confines window text selection started inside an
/// element's subtree to a modal [`SelectionScope`]. Chains like `Styled` /
/// `focus_trap`, so a Dialog/Sheet wraps its content with a single call:
///
/// ```ignore
/// v_flex().child(content).selection_scope(SelectionScope::Dialog(layer_ix))
/// ```
pub(crate) trait SelectionScopeElement: IntoElement + Sized {
    fn selection_scope(self, scope: SelectionScope) -> SelectionScopeMarker<Self::Element> {
        SelectionScopeMarker {
            scope,
            element: self.into_element(),
        }
    }
}

impl<E: IntoElement> SelectionScopeElement for E {}

/// A layout-transparent wrapper element (created by
/// [`SelectionScopeElement::selection_scope`]) that marks its subtree with a
/// [`SelectionScope`] during paint, so selectable
/// [`TextView`](crate::text::TextView)s painted inside it register under that
/// scope. It delegates every [`Element`] method to the wrapped element and only
/// brackets `paint` with a scope push/pop — mirroring the `text_view_state_stack`
/// idiom in `TextView::paint`.
pub(crate) struct SelectionScopeMarker<E> {
    scope: SelectionScope,
    element: E,
}

impl<E: Element> IntoElement for SelectionScopeMarker<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: Element> Element for SelectionScopeMarker<E> {
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.element.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.element.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.element.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.element
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Mark the subtree so selectable TextViews register under this scope.
        // Registration happens during the child's paint (see `TextView::paint`),
        // so bracketing the child paint is sufficient. Paint is depth-first and
        // single-threaded, so the bracket is exact even if the dialog layer is
        // later wrapped in a deferred draw.
        GlobalState::global_mut(cx).push_selection_scope(self.scope);
        self.element.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
        GlobalState::global_mut(cx).pop_selection_scope();
    }
}

/// How far a drag extends from its anchor: by character, or by the word or
/// line a double or triple click picked, as browsers and Zed do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SelectionGranularity {
    #[default]
    Character,
    Word,
    Line,
}

/// Where the selecting press landed, content-anchored to registered text runs
/// so the selection follows the content when an outer container scrolls.
#[derive(Clone)]
pub(crate) struct SelectionAnchor {
    /// Start and end of the anchored unit as `(run, byte offset)`. Both are the
    /// press point for a single press and bound the word for a double click; a
    /// triple click's line can span the wrapped fragments of one paragraph.
    start: (InlineKey, usize),
    end: (InlineKey, usize),
    view: WeakEntity<TextViewState>,
    /// True when the press hit the owning TextView, false when it landed in
    /// blank space and was proxied to the nearest run. Only a true hit focuses
    /// the view and auto-scrolls it.
    inside: bool,
}

/// Window-level text selection state, owned by [`Root`].
///
/// All text selection (including within a single TextView) is driven by this
/// state. It is an anchor `(run, byte range)` plus the per-run byte ranges the
/// last drag resolved to, see [`super::selection_registry`].
#[derive(Default)]
pub struct WindowTextSelection {
    pub(crate) anchor: Option<SelectionAnchor>,
    pub(crate) granularity: SelectionGranularity,
    pub(crate) is_selecting: bool,
    /// Whether the press or the drag touched a glyph. A drag that stays in
    /// blank space or a gutter selects nothing.
    pub(crate) did_hit_text: bool,
    spans: HashMap<InlineKey, SelectedSpan>,
    /// Views owning at least one span, for repaint and copy.
    views: HashSet<EntityId>,
    /// Inline states this selection wrote a range into, so a later resolve or
    /// clear also resets runs that are no longer painted.
    written: Vec<Arc<Mutex<InlineState>>>,
}

impl WindowTextSelection {
    /// The selected byte range of one painted run now showing `text`, if any.
    /// A span whose text changed underneath it is dropped.
    fn range_for(&mut self, key: &InlineKey, text: &str) -> Option<Range<usize>> {
        let range = self.spans.get(key)?.range_in(text);
        if range.is_none() {
            self.spans.remove(key);
        }
        range
    }

    /// Whether the window selection covers text in `view_id`.
    pub(crate) fn contains_view(&self, view_id: EntityId) -> bool {
        self.views.contains(&view_id)
    }

    fn reset_written(&mut self) {
        for state in self.written.drain(..) {
            if let Ok(mut state) = state.lock() {
                state.selection = None;
            }
        }
    }
}

impl Root {
    /// Register a selectable TextView for window-level selection.
    /// Called from TextView's paint on every frame.
    pub(crate) fn register_selectable_text_view(
        state: &Entity<TextViewState>,
        hitbox: &Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(root) = window.root::<Root>().flatten() else {
            return;
        };
        let id = state.entity_id();
        let weak = state.downgrade();
        let hitbox = hitbox.clone();
        // Capture the modal scope this view is painting under (set by the
        // `SelectionScopeMarker` wrapping a Dialog/Sheet content subtree).
        let scope = GlobalState::global(cx).current_selection_scope();
        root.update(cx, |root, _| {
            root.selectable_text_views.insert(id, (weak, hitbox, scope));
        });
    }

    /// Start a frame: called once, before any window content paints.
    fn begin_text_selection_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // While a drag is live, content can move under a stationary pointer
        // (auto-scroll, a streaming reply pushing text down). Re-resolve against
        // the frame that is on screen, before its geometry is dropped.
        if self.text_selection.is_selecting && self.text_selection.did_hit_text {
            self.update_text_selection(window.mouse_position(), window, cx);
        }
        // The registry then holds exactly this frame's painted runs, in paint
        // (document) order.
        self.text_registry.clear();
        // Dead views are pruned here, once per frame, not on every registration.
        self.selectable_text_views
            .retain(|_, (view, _, _)| view.upgrade().is_some());
    }

    /// Register one painted selectable `Inline` in document order and return
    /// the byte range of it the window selection covers this frame.
    /// Called from Inline's paint on every frame.
    pub(crate) fn register_selectable_inline(
        view: &Entity<TextViewState>,
        id: &GlobalElementId,
        text: SharedString,
        layout: TextLayout,
        hitbox: Hitbox,
        state: Arc<Mutex<InlineState>>,
        flow: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        let root = window.root::<Root>().flatten()?;
        let scope = GlobalState::global(cx).current_selection_scope();
        let visible = hitbox.bounds.intersect(&window.content_mask().bounds);
        root.update(cx, |root, _| {
            let key = root.text_registry.key_for(id);
            let range = root.text_selection.range_for(&key, &text);
            root.text_registry.push(RegisteredInline {
                key,
                view: view.downgrade(),
                view_id: view.entity_id(),
                scope,
                text,
                layout,
                hitbox,
                visible,
                state,
                flow,
            });
            range
        })
    }

    /// Whether there is an active text selection (window-level or view-local).
    pub(crate) fn has_text_selection(&self, cx: &App) -> bool {
        if !self.text_selection.spans.is_empty() {
            return true;
        }
        self.selectable_text_views.values().any(|(view, _, _)| {
            view.upgrade()
                .is_some_and(|view| view.read(cx).has_view_selection())
        })
    }

    /// Internal: collect selected text using `&self` directly, so it is safe
    /// to call while the Root entity is leased (e.g. inside Root's own action
    /// handler).
    ///
    /// Per-view text is collected from `InlineState`, which the selection
    /// writes into as soon as a drag resolves, so a copy never waits on paint.
    pub(crate) fn window_selected_text(&self, cx: &App) -> String {
        // A window selection lives in exactly one scope (its runs are confined
        // to the active modal, and the selection is cleared when a modal
        // opens/closes). Only views in that scope contribute, so copying never
        // mixes text across layers.
        let anchor_scope = self.active_selection_scope();

        let mut items: Vec<(Point<Pixels>, String)> = Vec::new();
        for (id, (view, _, scope)) in self.selectable_text_views.iter() {
            let Some(view) = view.upgrade() else { continue };
            let state = view.read(cx);
            let in_window_selection = self.text_selection.contains_view(*id)
                && state.is_selectable()
                && *scope == anchor_scope;
            if !state.has_view_selection() && !in_window_selection {
                continue;
            }
            let text = state.selected_text();
            if text.trim().is_empty() {
                continue;
            }
            items.push((state.bounds().origin, text));
        }

        items.sort_by(|a, b| {
            a.0.y
                .partial_cmp(&b.0.y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.0.x
                        .partial_cmp(&b.0.x)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });

        items
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Clear the window selection and all view-local selections.
    pub fn clear_text_selection(&mut self, cx: &mut Context<Self>) {
        let selected_views = std::mem::take(&mut self.text_selection.views);
        self.text_selection.anchor = None;
        self.text_selection.granularity = SelectionGranularity::Character;
        self.text_selection.is_selecting = false;
        self.text_selection.did_hit_text = false;
        self.text_selection.spans.clear();
        self.text_selection.reset_written();
        self.selectable_text_views.retain(|id, (view, _, _)| {
            let Some(view) = view.upgrade() else {
                return false;
            };
            // Only views that painted a highlight need to clear and re-render;
            // notifying every selectable view would re-render all of them on
            // every click.
            if selected_views.contains(id) || view.read(cx).has_view_selection() {
                view.update(cx, |state, cx| {
                    state.is_selecting = false;
                    state.clear_selection(cx);
                });
            }
            true
        });
    }

    /// Start the selection for this press unless one of the press's earlier
    /// listeners already did. Every left press clears the selection in the
    /// capture phase, so a live one here belongs to the same press.
    pub(crate) fn start_text_selection_once(
        &mut self,
        position: Point<Pixels>,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.text_selection.is_selecting {
            self.start_text_selection(position, click_count, window, cx);
        }
    }

    /// Start a selection for a left press. `click_count` 2 selects the word and
    /// 3 the line under the press, and a following drag extends by that unit.
    fn start_text_selection(
        &mut self,
        position: Point<Pixels>,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Components that own their own mouse-down interaction (Input, Button,
        // etc.) set `GlobalState::suppress_text_selection` in their bubble-phase
        // handler; the controller checks that flag before calling this, so a
        // press starts a selection from any point that is not consumed by such a
        // component — including blank space, which is proxied to the nearest
        // run in document flow.
        let scope = self.active_selection_scope();
        let hit = self.text_registry.hit(scope, position, window);
        let Some((index, offset, on_glyph)) =
            hit.or_else(|| self.text_registry.nearest(scope, position))
        else {
            return;
        };
        let entry = &self.text_registry.entries()[index];
        let inside = self
            .selectable_text_views
            .get(&entry.view_id)
            .is_some_and(|(_, hitbox, _)| hitbox.is_hovered(window));

        let (granularity, (start, end)) = match (hit.is_some(), click_count) {
            (true, 2) => {
                let word = word_range_at(&entry.text, offset).unwrap_or(offset..offset);
                (
                    SelectionGranularity::Word,
                    ((index, word.start), (index, word.end)),
                )
            }
            (true, count) if count >= 3 => (
                SelectionGranularity::Line,
                self.text_registry.line_bounds(index, offset),
            ),
            _ => (
                SelectionGranularity::Character,
                ((index, offset), (index, offset)),
            ),
        };
        let multi_click = granularity != SelectionGranularity::Character;

        // Only focus the view when the press actually hit it. A press proxied
        // from blank space must not steal focus from wherever it was.
        if inside {
            if let Some(view) = entry.view.upgrade() {
                view.update(cx, |state, cx| {
                    state.is_selecting = true;
                    state.focus_handle.focus(window, cx);
                });
            }
        }

        let entries = self.text_registry.entries();
        self.text_selection.anchor = Some(SelectionAnchor {
            start: (entries[start.0].key.clone(), start.1),
            end: (entries[end.0].key.clone(), end.1),
            view: entry.view.clone(),
            inside,
        });
        self.text_selection.granularity = granularity;
        self.text_selection.did_hit_text = on_glyph || multi_click;
        self.text_selection.is_selecting = true;

        if start < end {
            let spans = self.text_registry.resolve(scope, start, end);
            self.set_selection_spans(spans, cx);
        }
    }

    pub(crate) fn update_text_selection(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.text_selection.is_selecting {
            return;
        }
        // Do not update the selection while a GPUI drag-and-drop is active
        // (e.g. dragging a dock tab or a resize handle across TextViews).
        if cx.has_active_drag() {
            return;
        }
        let Some(anchor) = self.text_selection.anchor.clone() else {
            return;
        };
        // An anchor that is no longer painted (scrolled out of a virtualized
        // list) keeps the spans it already resolved instead of collapsing them.
        let (Some(anchor_start), Some(anchor_end)) = (
            self.text_registry.position(&anchor.start.0),
            self.text_registry.position(&anchor.end.0),
        ) else {
            return;
        };
        let scope = self.active_selection_scope();
        let Some((index, offset, on_glyph)) = self.text_registry.nearest(scope, position) else {
            return;
        };
        let entry = &self.text_registry.entries()[index];

        // CDXC:FocusRouting 2026-09-17 WHY:
        // A drag from blank space can highlight text while the composer retains keyboard focus and consumes Copy.
        // Focus the first actual text hit without stealing focus for an ordinary blank click.
        if !self.text_selection.did_hit_text && on_glyph {
            if let Some(view) = entry.view.upgrade() {
                let focus_handle = view.read(cx).focus_handle.clone();
                focus_handle.focus(window, cx);
            }
        }
        self.text_selection.did_hit_text |= on_glyph;

        let (head_start, head_end) = match self.text_selection.granularity {
            SelectionGranularity::Character => ((index, offset), (index, offset)),
            SelectionGranularity::Word => {
                let word = word_range_at(&entry.text, offset).unwrap_or(offset..offset);
                ((index, word.start), (index, word.end))
            }
            SelectionGranularity::Line => self.text_registry.line_bounds(index, offset),
        };
        // The selection is the union of the anchored unit and the unit under
        // the pointer, compared in document order.
        let start = (anchor_start, anchor.start.1).min(head_start);
        let end = (anchor_end, anchor.end.1).max(head_end);
        let spans = if self.text_selection.did_hit_text {
            self.text_registry.resolve(scope, start, end)
        } else {
            HashMap::new()
        };

        // Auto-scroll the anchor view when dragging near its viewport edges.
        // Only a true hit anchor auto-scrolls; a proxied view was never pressed.
        if anchor.inside {
            if let Some(view) = anchor.view.upgrade() {
                view.update(cx, |state, cx| {
                    if state.scrollable {
                        let delta = AutoScroll::compute_delta(position.y, state.bounds());
                        state.set_auto_scroll(delta, cx);
                    }
                });
            }
        }

        self.set_selection_spans(spans, cx);
    }

    pub(crate) fn end_text_selection(&mut self, cx: &mut Context<Self>) {
        if !self.text_selection.is_selecting {
            return;
        }
        self.text_selection.is_selecting = false;
        let anchor = if self.text_selection.spans.is_empty() {
            self.text_selection.did_hit_text = false;
            self.text_selection.anchor.take()
        } else {
            self.text_selection.anchor.clone()
        };
        // Only a true hit anchor had `is_selecting` and auto-scroll set in
        // `start_text_selection`; a proxied view has nothing to tear down.
        if let Some(view) = anchor
            .filter(|anchor| anchor.inside)
            .and_then(|anchor| anchor.view.upgrade())
        {
            view.update(cx, |state, cx| {
                state.is_selecting = false;
                state.stop_auto_scroll();
                cx.notify();
            });
        }
    }

    /// Replace the resolved spans: write them into the painted runs' states
    /// (so copy sees them before the next paint) and re-render only the views
    /// whose highlight changed. A long drag leaves the fully covered views
    /// between the anchor and the pointer alone.
    fn set_selection_spans(
        &mut self,
        spans: HashMap<InlineKey, SelectedSpan>,
        cx: &mut Context<Self>,
    ) {
        if spans == self.text_selection.spans {
            return;
        }
        self.text_selection.reset_written();
        let mut views = HashSet::new();
        let mut changed = HashSet::new();
        for entry in self.text_registry.entries() {
            let span = spans.get(&entry.key);
            if span != self.text_selection.spans.get(&entry.key) {
                changed.insert(entry.view_id);
            }
            let Some(span) = span else {
                continue;
            };
            views.insert(entry.view_id);
            if let Ok(mut state) = entry.state.lock() {
                state.selection = Some(span.range.clone().into());
            }
            self.text_selection.written.push(entry.state.clone());
        }
        // Views that left the selection without being painted this frame.
        changed.extend(self.text_selection.views.difference(&views));
        self.text_selection.spans = spans;
        self.text_selection.views = views;
        for id in changed {
            if let Some(view) = self
                .selectable_text_views
                .get(&id)
                .and_then(|(view, _, _)| view.upgrade())
            {
                view.update(cx, |_, cx| cx.notify());
            }
        }
    }

    /// The scope window text selection is confined to right now. When any
    /// Dialog is open, selection is limited to the topmost dialog (highest
    /// `layer_ix`); otherwise to the active Sheet if one is open; otherwise the
    /// base window. Runs registered under a different scope never join a
    /// selection.
    fn active_selection_scope(&self) -> SelectionScope {
        if !self.active_dialogs.is_empty() {
            SelectionScope::Dialog(self.active_dialogs.len() - 1)
        } else if self.active_sheet.is_some() {
            SelectionScope::Sheet
        } else {
            SelectionScope::Base
        }
    }
}

/// A zero-size element that drives window-level text selection.
///
/// Must be the FIRST child of Root's container div: bubble-phase mouse
/// listeners fire in reverse registration order, so registering earliest makes
/// the controller run AFTER interactive components (which may stop
/// propagation or prevent default).
///
/// Note: `window.on_mouse_event` handlers are window-global (not scoped to
/// any hitbox); the phase check and the `GlobalState::suppress_text_selection`
/// flag are the only guards. The flag is reset in the capture phase of every
/// left mouse down and set in the bubble phase by components that own their own
/// press/drag interaction (Button, Input, etc.). Because bubble-phase listeners
/// fire in reverse registration order and this controller registers earliest,
/// it observes the flag after those components have set it, so presses consumed
/// by them are excluded while presses on blank space (even inside a focusable
/// container) still start a selection.
pub(crate) struct TextSelectionController;

impl IntoElement for TextSelectionController {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextSelectionController {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // This element paints before any window content.
        if let Some(root) = window.root::<Root>().flatten() {
            root.update(cx, |root, cx| root.begin_text_selection_frame(window, cx));
        }

        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if event.button != MouseButton::Left {
                return;
            }
            if phase.capture() {
                // Reset the suppression flag at the start of every press, then
                // clear the previous selection (browser behavior), even when an
                // interactive component consumes the event in the bubble phase.
                GlobalState::global_mut(cx).suppress_text_selection = false;
                Root::update(window, cx, |root, _, cx| root.clear_text_selection(cx));
            } else {
                // Reaching bubble phase means no component stopped propagation.
                // Components that own their own press (Button, Input, etc.) set
                // `suppress_text_selection` in their bubble handler; if set, the
                // press is theirs and must not start a window selection.
                //
                // A press on a TextView has already started the selection from
                // the view's own listener, which runs before its ancestors'; a
                // component around it that claims the press takes it back here.
                if GlobalState::global(cx).suppress_text_selection {
                    Root::update(window, cx, |root, _, cx| root.clear_text_selection(cx));
                    return;
                }
                Root::update(window, cx, |root, window, cx| {
                    root.start_text_selection_once(event.position, event.click_count, window, cx);
                });
            }
        });

        // The drag and the release run in the capture phase: a host element that
        // stops their propagation must not freeze or strand a live selection.
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if !phase.capture() {
                return;
            }
            Root::update(window, cx, |root, window, cx| {
                root.update_text_selection(event.position, window, cx);
            });
        });

        window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
            if !phase.capture() {
                return;
            }
            Root::update(window, cx, |root, _, cx| root.end_text_selection(cx));
        });

        window.on_mouse_event(move |_: &ScrollWheelEvent, phase, window, cx| {
            if !phase.bubble() {
                return;
            }
            // While drag-selecting, a wheel scroll moves content under the
            // stationary cursor; re-resolve the cursor endpoint at the current
            // mouse position so the selection keeps extending to the pointer
            // (browser behavior). `update_text_selection` is a no-op unless a
            // selection drag is active, so the idle cost is negligible.
            //
            // Bounds are refreshed in the next frame's prepaint, so a single
            // wheel event may resolve one frame stale; continuous scrolling
            // converges, so this is left unhandled.
            let position = window.mouse_position();
            Root::update(window, cx, |root, window, cx| {
                root.update_text_selection(position, window, cx);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{SelectionScope, SelectionScopeElement};
    use crate::global_state::GlobalState;
    use crate::{
        Placement, Root,
        text::{TextView, TextViewState},
    };
    use gpui::{
        AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
        Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    struct ChatTestView {
        focus_handle: FocusHandle,
        first: Entity<TextViewState>,
        second: Entity<TextViewState>,
        second_selectable: bool,
        /// Top padding above the views. Bumping it shifts the whole content
        /// down, which is the layout-level equivalent of an outer container
        /// scrolling (see `selection_follows_content_when_layout_shifts`).
        top_offset: gpui::Pixels,
        /// Blank gap between the two views, used to anchor a selection in blank
        /// space (the proxy-anchored endpoint path).
        mid_gap: gpui::Pixels,
    }

    impl ChatTestView {
        fn new(second_selectable: bool, cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                first: cx.new(|cx| TextViewState::markdown("Hello world", cx)),
                second: cx.new(|cx| TextViewState::markdown("Second message", cx)),
                second_selectable,
                top_offset: px(10.),
                mid_gap: px(0.),
            }
        }
    }

    impl Render for ChatTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // `track_focus` makes the root a focusable container, so GPUI's
            // focus-on-mouse-down marks every press inside it default-prevented.
            // Selection must still start from blank space here (regression
            // guard for `drag_from_blank_space_selects_views_below`), which the
            // `suppress_text_selection` mechanism guarantees because blank-space
            // presses never set that flag.
            div()
                .track_focus(&self.focus_handle)
                .size_full()
                .pt(self.top_offset)
                .child(
                    div()
                        .h(px(40.))
                        .child(TextView::new(&self.first).selectable(true)),
                )
                // A blank gap between the two views. It is not over any
                // TextView hitbox, so a press here exercises the blank-space
                // (proxy-anchored) endpoint path.
                .child(div().h(self.mid_gap))
                .child(
                    div()
                        .h(px(40.))
                        .child(TextView::new(&self.second).selectable(self.second_selectable)),
                )
                // A 20px region below the views that owns its press the way
                // Input/Button do: its bubble-phase handler sets the suppress
                // flag, so a press starting here must not start a selection.
                .child(
                    div()
                        .h(px(20.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            GlobalState::suppress_text_selection(cx);
                        }),
                )
        }
    }

    fn setup(
        second_selectable: bool,
        cx: &mut TestAppContext,
    ) -> (Entity<ChatTestView>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let chat = cx.new(|cx| ChatTestView::new(second_selectable, cx));
            Root::new(chat, window, cx)
        });
        let chat = root.read_with(cx, |root, _| {
            root.view().clone().downcast::<ChatTestView>().unwrap()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (chat, cx)
    }

    fn drag(
        cx: &mut VisualTestContext,
        from: gpui::Point<gpui::Pixels>,
        to: gpui::Point<gpui::Pixels>,
    ) {
        drag_through(cx, &[from, to]);
    }

    fn drag_through(cx: &mut VisualTestContext, points: &[gpui::Point<gpui::Pixels>]) {
        assert!(points.len() >= 2);
        let from = points[0];
        let to = *points.last().unwrap();

        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        for point in &points[1..] {
            cx.simulate_mouse_move(*point, Some(MouseButton::Left), Modifiers::default());
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }

        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn window_selected_text(cx: &mut VisualTestContext) -> String {
        use crate::WindowExt as _;
        cx.update(|window, cx| window.selected_text(cx))
    }

    #[gpui::test]
    fn cross_view_drag_merges_text_top_to_bottom(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // From the very start of the first view down into the second view.
        drag(cx, point(px(0.), px(15.)), point(px(300.), px(70.)));

        let text = window_selected_text(cx);
        let first = text.find("Hello world").expect("first view text missing");
        let second = text
            .find("Second message")
            .expect("second view text missing");
        assert!(first < second, "wrong order: {text:?}");
        assert!(text.contains('\n'), "expected newline separator: {text:?}");
    }

    #[gpui::test]
    fn drag_from_blank_space_selects_views_below(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // Start in the blank padding above the first view, enter the second
        // view's rendered text, then drag past its end.
        drag_through(
            cx,
            &[
                point(px(5.), px(2.)),
                point(px(20.), px(70.)),
                point(px(300.), px(70.)),
            ],
        );

        let text = window_selected_text(cx);
        assert!(text.contains("Hello world"), "got: {text:?}");
        assert!(text.contains("Second message"), "got: {text:?}");
    }

    #[gpui::test]
    fn drag_entirely_in_blank_gap_selects_nothing(cx: &mut TestAppContext) {
        let (chat, cx) = setup(true, cx);

        // Layout: first [10,50], gap [50,110], second [110,150].
        chat.update(cx, |chat, cx| {
            chat.mid_gap = px(60.);
            cx.notify();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        // Drag only inside the gap. The selection never enters either TextView.
        drag(cx, point(px(5.), px(70.)), point(px(300.), px(90.)));

        let text = window_selected_text(cx);
        assert_eq!(text, "", "blank-only drag selected text: {text:?}");
    }

    #[gpui::test]
    fn drag_entirely_in_right_gutter_selects_nothing(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // x=300 is far to the right of the rendered text. Dragging vertically
        // through only that blank gutter must not select nearby TextViews.
        drag(cx, point(px(300.), px(2.)), point(px(300.), px(70.)));

        let text = window_selected_text(cx);
        assert_eq!(text, "", "right-gutter drag selected text: {text:?}");
    }

    #[gpui::test]
    fn selection_follows_content_when_layout_shifts(cx: &mut TestAppContext) {
        let (chat, cx) = setup(true, cx);

        // Open a blank gap between the two views so we can anchor a selection
        // in blank space that sits *below* the first view's text and *above*
        // the second. Layout: first [10,50], gap [50,110], second [110,150].
        chat.update(cx, |chat, cx| {
            chat.mid_gap = px(60.);
            cx.notify();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        // Anchor in the gap (blank space) and drag down-right into the second
        // view, ending past the end of its text so the whole line is selected.
        // The anchor sits below "Hello world", so only the second view is
        // selected.
        drag_through(
            cx,
            &[
                point(px(0.), px(80.)),
                point(px(20.), px(120.)),
                point(px(300.), px(120.)),
            ],
        );
        let before = window_selected_text(cx);
        assert!(
            before.contains("Second message") && !before.contains("Hello world"),
            "expected only the second view selected, got: {before:?}"
        );

        // Shift the whole content down by 80px — the equivalent of an outer
        // container scrolling. A window-anchored blank endpoint stays at window
        // y=80, which the first view now covers (first moves to ~[90,130]), so
        // the selection drifts to also grab "Hello world". A proxy-anchored
        // endpoint moves with the content and the selection stays stable.
        chat.update(cx, |chat, cx| {
            chat.top_offset = px(90.);
            cx.notify();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let after = window_selected_text(cx);
        assert_eq!(before, after, "selection drifted after layout shift");
    }

    #[gpui::test]
    fn suppressed_mouse_down_does_not_start_selection(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // The suppress region sits below the two views (root pt=10, two 40px
        // view rows -> y in [90, 110)). Pressing inside it makes its bubble
        // handler set the suppress flag, so dragging up across both views must
        // not produce any window selection.
        drag(cx, point(px(20.), px(100.)), point(px(20.), px(15.)));

        let text = window_selected_text(cx);
        assert!(text.is_empty(), "expected no selection, got: {text:?}");
    }

    #[gpui::test]
    fn non_selectable_view_is_excluded(cx: &mut TestAppContext) {
        let (_, cx) = setup(false, cx);

        drag_through(
            cx,
            &[
                point(px(5.), px(2.)),
                point(px(20.), px(15.)),
                point(px(300.), px(15.)),
            ],
        );

        let text = window_selected_text(cx);
        assert!(text.contains("Hello world"), "got: {text:?}");
        assert!(!text.contains("Second message"), "got: {text:?}");
    }

    #[gpui::test]
    fn drag_within_single_view_excludes_others(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // Entirely inside the first view.
        drag(cx, point(px(5.), px(15.)), point(px(60.), px(15.)));

        let text = window_selected_text(cx);
        assert!(!text.contains("Second message"), "got: {text:?}");
        assert!(!text.trim().is_empty(), "expected some selection");
    }

    #[gpui::test]
    fn mouse_down_clears_previous_selection(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        drag(cx, point(px(5.), px(15.)), point(px(300.), px(70.)));
        assert!(!window_selected_text(cx).is_empty());

        // A plain click clears the selection.
        cx.simulate_click(point(px(300.), px(100.)), Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        assert_eq!(window_selected_text(cx), "");
    }

    #[gpui::test]
    fn double_click_selects_word_under_root(cx: &mut TestAppContext) {
        let (_, cx) = setup(true, cx);

        // Double-click inside the first view: must trigger the per-view word
        // selection (Inline), not a window-level drag selection.
        let position = point(px(10.), px(15.));
        cx.simulate_event(MouseDownEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let text = window_selected_text(cx);
        assert_eq!(text.trim(), "Hello", "expected word selection: {text:?}");
        assert!(!text.contains("Second message"), "got: {text:?}");
    }

    #[gpui::test]
    fn drag_back_into_anchor_view_clears_other_views(cx: &mut TestAppContext) {
        let (chat, cx) = setup(true, cx);
        let second = chat.read_with(cx, |chat, _| chat.second.clone());

        // Drag from view A down into view B: this is a cross-view selection, so
        // B paints a highlight and `selected_text` reports it.
        cx.simulate_mouse_down(
            point(px(0.), px(15.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_mouse_move(
            point(px(300.), px(70.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let text = second.read_with(cx, |state, _| state.selected_text());
        assert!(
            text.contains("Second message"),
            "precondition: B should be selected, got {text:?}"
        );

        // Observe B's re-render requests. A view only drops a stale highlight
        // when it is notified and repaints; this asserts the controller does
        // notify B, independently of whether the test harness happens to
        // repaint B for unrelated reasons.
        let b_notified = Rc::new(Cell::new(false));
        let _subscription = cx.update({
            let b_notified = b_notified.clone();
            let second = second.clone();
            move |_, cx| cx.observe(&second, move |_, _| b_notified.set(true))
        });
        b_notified.set(false);

        // Drag back up inside view A. The drag now lives entirely in A, so
        // `single_view` is Some(A) and the fast path runs. It must still notify
        // B (whose old band crossed B) so B can clear its now-stale highlight.
        //
        // We check this on the in-drag frame, not after mouse-up:
        // `end_text_selection` notifies every selectable view, which would
        // notify B for an unrelated reason and mask the bug.
        cx.simulate_mouse_move(
            point(px(60.), px(15.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.run_until_parked();

        assert!(
            b_notified.get(),
            "view B was not notified when the drag returned to the anchor view, \
             so its stale highlight would never be repainted away",
        );
    }

    /// A view with a selectable TextView in the base window that also mounts the
    /// Dialog/Sheet layers (which `Root::render` does not mount itself), so a
    /// real modal can be opened on top of the base content.
    struct ModalScopeTestView {
        focus_handle: FocusHandle,
        base: Entity<TextViewState>,
    }

    impl ModalScopeTestView {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                base: cx.new(|cx| TextViewState::markdown("Hello world", cx)),
            }
        }
    }

    impl Render for ModalScopeTestView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let sheet_layer = Root::render_sheet_layer(window, cx);
            let dialog_layer = Root::render_dialog_layer(window, cx);
            div()
                .track_focus(&self.focus_handle)
                .size_full()
                .child(
                    div()
                        .h(px(40.))
                        .child(TextView::new(&self.base).selectable(true)),
                )
                .children(sheet_layer)
                .children(dialog_layer)
        }
    }

    fn setup_modal(
        cx: &mut TestAppContext,
    ) -> (Entity<ModalScopeTestView>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(ModalScopeTestView::new);
            Root::new(view, window, cx)
        });
        let view = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<ModalScopeTestView>()
                .unwrap()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (view, cx)
    }

    /// Advance past the modal open animation so it reaches its resting position,
    /// then redraw so its TextViews register and their bounds are stable for the
    /// subsequent drag.
    fn settle(cx: &mut VisualTestContext) {
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn open_dialog_with_text(
        cx: &mut VisualTestContext,
        text: &'static str,
    ) -> Entity<TextViewState> {
        let state = cx.update(|_, cx| cx.new(|cx| TextViewState::markdown(text, cx)));
        let state_for_builder = state.clone();
        cx.update(|window, cx| {
            Root::update(window, cx, |root, window, cx| {
                root.open_dialog(
                    move |dialog, _, _| {
                        dialog.child(TextView::new(&state_for_builder).selectable(true))
                    },
                    window,
                    cx,
                );
            });
        });
        settle(cx);
        state
    }

    #[gpui::test]
    fn drag_inside_dialog_still_selects_its_text(cx: &mut TestAppContext) {
        let (_, cx) = setup_modal(cx);
        let dialog_state = open_dialog_with_text(cx, "Dialog text");

        // A drag entirely within the dialog's TextView must still select (the
        // scope filter must not break in-dialog selection — see #2501).
        let b = dialog_state.read_with(cx, |s, _| s.bounds());
        drag(
            cx,
            point(b.origin.x + px(1.), b.center().y),
            point(b.origin.x + b.size.width + px(80.), b.center().y),
        );

        let text = window_selected_text(cx);
        assert!(
            text.contains("Dialog text"),
            "dialog text was not selectable: {text:?}"
        );
    }

    #[gpui::test]
    fn opening_dialog_clears_base_selection(cx: &mut TestAppContext) {
        let (view, cx) = setup_modal(cx);

        let b = view.read_with(cx, |v, cx| v.base.read(cx).bounds());
        drag(
            cx,
            point(b.origin.x + px(1.), b.center().y),
            point(b.origin.x + b.size.width + px(80.), b.center().y),
        );
        assert!(window_selected_text(cx).contains("Hello world"));

        let _dialog = open_dialog_with_text(cx, "Dialog text");

        let text = window_selected_text(cx);
        assert!(
            !text.contains("Hello world"),
            "base selection was not cleared when the dialog opened: {text:?}"
        );
    }

    /// A behind-the-modal selectable TextView covered by a full-window
    /// occluding overlay (mirroring a Dialog/Sheet overlay), plus a `front`
    /// TextView marked with a modal [`SelectionScope`] and painted on top of the
    /// overlay. This reproduces the modal stacking at fixed coordinates without a
    /// real modal's open animation (which cannot be settled under the test
    /// clock).
    struct SyntheticModalView {
        focus_handle: FocusHandle,
        behind: Entity<TextViewState>,
        front: Entity<TextViewState>,
        front_scope: SelectionScope,
    }

    impl SyntheticModalView {
        fn new(front_scope: SelectionScope, cx: &mut Context<Self>) -> Self {
            Self {
                focus_handle: cx.focus_handle(),
                behind: cx.new(|cx| TextViewState::markdown("Behind text", cx)),
                front: cx.new(|cx| TextViewState::markdown("Front text", cx)),
                front_scope,
            }
        }
    }

    impl Render for SyntheticModalView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.focus_handle)
                .size_full()
                // Behind the modal, at the top. Occluded by the overlay below.
                .child(
                    div()
                        .h(px(40.))
                        .child(TextView::new(&self.behind).selectable(true)),
                )
                // A full-window occluding overlay (mirrors the modal overlay)
                // with modal-scoped content painted on top of it.
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .child(
                            div()
                                .absolute()
                                .top(px(100.))
                                .left_0()
                                .h(px(40.))
                                .child(TextView::new(&self.front).selectable(true))
                                .selection_scope(self.front_scope),
                        ),
                )
        }
    }

    fn setup_synthetic(
        front_scope: SelectionScope,
        cx: &mut TestAppContext,
    ) -> (Entity<SyntheticModalView>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| SyntheticModalView::new(front_scope, cx));
            Root::new(view, window, cx)
        });
        let view = root.read_with(cx, |root, _| {
            root.view()
                .clone()
                .downcast::<SyntheticModalView>()
                .unwrap()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        (view, cx)
    }

    /// Open an empty dialog (its layer is not mounted, so nothing renders) purely
    /// to make `active_selection_scope()` return `Dialog(0)`.
    fn activate_dialog_scope(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            Root::update(window, cx, |root, window, cx| {
                root.open_dialog(|dialog, _, _| dialog, window, cx);
            });
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// Open an empty sheet purely to make `active_selection_scope()` return
    /// `Sheet`.
    fn activate_sheet_scope(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            Root::update(window, cx, |root, window, cx| {
                root.open_sheet_at(Placement::Right, |sheet, _, _| sheet, window, cx);
            });
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// Regression guard: with a dialog active, a drag that starts in
    /// the dialog-scoped content and leaves it over the overlay must not select
    /// the TextView behind the overlay.
    #[gpui::test]
    fn selection_behind_active_dialog_is_excluded(cx: &mut TestAppContext) {
        let (view, cx) = setup_synthetic(SelectionScope::Dialog(0), cx);
        activate_dialog_scope(cx);

        // Anchor inside the modal-scoped content, then drag up onto the behind
        // view's glyphs (left side; the behind view spans the full window width,
        // so its center is far from its text).
        let from = view.read_with(cx, |v, cx| v.front.read(cx).bounds().center());
        let to = view.read_with(cx, |v, cx| {
            let b = v.behind.read(cx).bounds();
            point(b.origin.x + px(4.), b.center().y)
        });
        drag(cx, from, to);

        let behind = view.read_with(cx, |v, cx| v.behind.read(cx).selected_text());
        assert!(
            behind.trim().is_empty(),
            "view behind the dialog overlay was selected: {behind:?}"
        );
    }

    /// The same guard for a Sheet (#2501 de-guarded both Dialog and Sheet).
    #[gpui::test]
    fn selection_behind_active_sheet_is_excluded(cx: &mut TestAppContext) {
        let (view, cx) = setup_synthetic(SelectionScope::Sheet, cx);
        activate_sheet_scope(cx);

        let from = view.read_with(cx, |v, cx| v.front.read(cx).bounds().center());
        let to = view.read_with(cx, |v, cx| {
            let b = v.behind.read(cx).bounds();
            point(b.origin.x + px(4.), b.center().y)
        });
        drag(cx, from, to);

        let behind = view.read_with(cx, |v, cx| v.behind.read(cx).selected_text());
        assert!(
            behind.trim().is_empty(),
            "view behind the sheet overlay was selected: {behind:?}"
        );
    }

    /// The scope filter must not over-exclude: content in the active modal scope
    /// stays selectable.
    #[gpui::test]
    fn front_view_in_active_scope_is_selectable(cx: &mut TestAppContext) {
        let (view, cx) = setup_synthetic(SelectionScope::Dialog(0), cx);
        activate_dialog_scope(cx);

        let b = view.read_with(cx, |v, cx| v.front.read(cx).bounds());
        drag(
            cx,
            point(b.origin.x + px(1.), b.center().y),
            point(b.origin.x + b.size.width + px(80.), b.center().y),
        );

        let front = view.read_with(cx, |v, cx| v.front.read(cx).selected_text());
        assert!(
            front.contains("Front"),
            "active-scope content was not selectable: {front:?}"
        );
    }
}

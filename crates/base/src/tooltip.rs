use std::{rc::Rc, time::Duration};

use gpui::{
    AnyElement, AnyView, App, Bounds, Context, DispatchPhase, Display, Div, Edges, Element,
    ElementId, Entity, GlobalElementId, Half as _, InspectorElementId, InteractiveElement,
    IntoElement, LayoutId, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Position,
    Render, RenderOnce, Role, ScrollWheelEvent, Size, Stateful, StatefulInteractiveElement, Style,
    Styled, Task, Window, canvas, deferred, div, point, px,
};

use crate::{
    Placement, Positioner, Root, RootPlugin,
    positioner::{clamp, frame_insets, resolve_side},
};

const TOOLTIP_PRIORITY: usize = 200;
const WINDOW_MARGIN: Pixels = px(4.);
const GRACE_PERIOD: Duration = Duration::from_millis(300);
const SHOW_DELAY: Duration = Duration::from_millis(500);

type TooltipBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;
type TooltipRenderer = Rc<dyn Fn(AnyView, TooltipTransition, &mut Window, &mut App) -> AnyElement>;

/// An unstyled tooltip popup.
///
/// This corresponds to Base UI's `Tooltip.Popup`: it owns the accessible
/// tooltip role and accepts application-owned content and presentation.
#[derive(IntoElement)]
pub struct Tooltip {
    base: Stateful<Div>,
}

impl Tooltip {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(id).role(Role::Tooltip),
        }
    }
}

impl Styled for Tooltip {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.base.style()
    }
}

impl ParentElement for Tooltip {
    fn extend(&mut self, children: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(children);
    }
}

impl RenderOnce for Tooltip {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.base
    }
}

/// Positions a managed tooltip relative to the bounds of its trigger element.
///
/// Every placement but [`ManagedTooltipPlacement::Auto`] and
/// [`ManagedTooltipPlacement::Preferred`] keeps its side and only shifts to
/// stay inside the window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ManagedTooltipPlacement {
    /// Preserve the overlay's automatic above/below placement.
    #[default]
    Auto,
    /// Place the tooltip on this side of the trigger, centered on it, flipped to
    /// the opposite side when it does not fit (GPUI Kit's own tooltip placement).
    Preferred(Placement),
    /// Place the tooltip to the left, vertically centered on the trigger.
    Left,
    /// Place the tooltip to the right, vertically centered on the trigger.
    Right,
    /// Place the tooltip beside the trigger, vertically centered on it, on whichever side has more
    /// room in the window. For a trigger in a row whose neighbours above and below are covered by
    /// something the tooltip cannot draw over, so it must stay in that row.
    WiderSide,
    /// Place the tooltip below the trigger, horizontally centered on it.
    Below,
    /// Place the tooltip above the trigger with their right edges aligned.
    AboveLeft,
    /// Place the tooltip below the trigger with their right edges aligned.
    BelowLeft,
    /// Place the tooltip below the trigger with their left edges aligned.
    BelowRight,
    /// Place the tooltip below the trigger with their left edges aligned, then
    /// shift it horizontally so it stays between `left` and `right` (window
    /// coordinates). Keeps a tooltip inside the panel that owns its trigger.
    BelowWithin { left: Pixels, right: Pixels },
}

impl From<Placement> for ManagedTooltipPlacement {
    fn from(placement: Placement) -> Self {
        Self::Preferred(placement)
    }
}

impl From<Option<Placement>> for ManagedTooltipPlacement {
    fn from(placement: Option<Placement>) -> Self {
        placement.map_or(Self::Auto, Self::Preferred)
    }
}

/// Where a tooltip of `tooltip_size` goes for `placement`, kept `margin` inside the viewport.
fn tooltip_bounds(
    trigger_bounds: Bounds<Pixels>,
    tooltip_size: Size<Pixels>,
    viewport_size: Size<Pixels>,
    margin: Edges<Pixels>,
    placement: ManagedTooltipPlacement,
) -> Bounds<Pixels> {
    let preferred = match placement {
        ManagedTooltipPlacement::Auto => Some(None),
        ManagedTooltipPlacement::Preferred(placement) => Some(Some(placement)),
        _ => None,
    };
    if let Some(preferred) = preferred {
        return resolve_side(
            trigger_bounds,
            tooltip_size,
            viewport_size,
            margin,
            preferred,
        )
        .bounds;
    }

    let centered_y = trigger_bounds.center().y - tooltip_size.height.half();
    let left_of = point(trigger_bounds.left() - tooltip_size.width, centered_y);
    let right_of = point(trigger_bounds.right(), centered_y);
    let origin = match placement {
        ManagedTooltipPlacement::Auto | ManagedTooltipPlacement::Preferred(_) => unreachable!(),
        ManagedTooltipPlacement::Left => left_of,
        ManagedTooltipPlacement::Right => right_of,
        ManagedTooltipPlacement::WiderSide => {
            let room_left = trigger_bounds.left();
            let room_right = viewport_size.width - trigger_bounds.right();
            if room_right >= room_left {
                right_of
            } else {
                left_of
            }
        }
        ManagedTooltipPlacement::Below => point(
            trigger_bounds.center().x - tooltip_size.width.half(),
            trigger_bounds.bottom(),
        ),
        ManagedTooltipPlacement::AboveLeft => point(
            trigger_bounds.right() - tooltip_size.width,
            trigger_bounds.top() - tooltip_size.height,
        ),
        ManagedTooltipPlacement::BelowLeft => point(
            trigger_bounds.right() - tooltip_size.width,
            trigger_bounds.bottom(),
        ),
        ManagedTooltipPlacement::BelowRight => {
            point(trigger_bounds.left(), trigger_bounds.bottom())
        }
        ManagedTooltipPlacement::BelowWithin { left, right } => {
            let max_x = (right - tooltip_size.width).max(left);
            let x = trigger_bounds.left().min(max_x).max(left);
            point(x, trigger_bounds.bottom())
        }
    };

    clamp(Bounds::new(origin, tooltip_size), viewport_size, margin)
}

/// Content requested by a tooltip trigger.
#[derive(Clone)]
pub struct TooltipRequest {
    build: TooltipBuilder,
    trigger_bounds: Bounds<Pixels>,
    placement: ManagedTooltipPlacement,
    discrete_show_delay: Option<Duration>,
}

impl TooltipRequest {
    pub fn new(
        trigger_bounds: Bounds<Pixels>,
        build: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        Self {
            build: Rc::new(build),
            trigger_bounds,
            placement: ManagedTooltipPlacement::Auto,
            discrete_show_delay: None,
        }
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = ManagedTooltipPlacement::Preferred(placement);
        self
    }

    /// Position the tooltip with a [`ManagedTooltipPlacement`].
    pub fn managed_placement(mut self, placement: ManagedTooltipPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Show the tooltip after `show_delay` every time, including when the pointer
    /// comes from another trigger, and hide it as soon as its trigger is left.
    pub fn discrete(mut self, show_delay: Duration) -> Self {
        self.discrete_show_delay = Some(show_delay);
        self
    }

    /// The bounds of the trigger, in window coordinates.
    pub fn trigger_bounds(&self) -> Bounds<Pixels> {
        self.trigger_bounds
    }
}

/// Presentation transition requested by the Base tooltip lifecycle.
#[derive(Clone, Copy, Debug)]
pub enum TooltipTransition {
    Enter {
        epoch: usize,
    },
    Switch {
        epoch: usize,
        previous: Bounds<Pixels>,
        current: Bounds<Pixels>,
    },
}

/// Per-window tooltip provider and overlay.
///
/// Show requests are ignored on iOS and Android, where touch input must not
/// open hover tooltips. This does not control GPUI's native `.tooltip()` API.
///
/// Registered as a [`RootPlugin`], it is the window's managed-tooltip overlay
/// that [`Root::tooltip_overlay`] and the other `Root` tooltip helpers reach.
pub struct TooltipOverlay {
    enabled: bool,
    content: Option<TooltipRequest>,
    /// The trigger the pointer entered last, whose tooltip is shown or pending.
    active_trigger_bounds: Option<Bounds<Pixels>>,
    previous_bounds: Option<Bounds<Pixels>>,
    epoch: usize,
    had_recent_tooltip: bool,
    animation_epoch: usize,
    is_switching: bool,
    /// The trigger the last press landed on, for [`TooltipOverlay::flash_pressed`].
    pressed: Option<TooltipRequest>,
    show_task: Option<Task<()>>,
    hide_task: Option<Task<()>>,
    renderer: TooltipRenderer,
}

impl TooltipOverlay {
    pub fn new() -> Self {
        Self {
            enabled: !crate::is_mobile(),
            content: None,
            active_trigger_bounds: None,
            previous_bounds: None,
            epoch: 0,
            had_recent_tooltip: false,
            animation_epoch: 0,
            is_switching: false,
            pressed: None,
            show_task: None,
            hide_task: None,
            renderer: Rc::new(|view, _, _, _| div().child(view).into_any_element()),
        }
    }

    pub fn render_with(
        mut self,
        renderer: impl Fn(AnyView, TooltipTransition, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.renderer = Rc::new(renderer);
        self
    }

    fn next_epoch(&mut self) -> usize {
        self.epoch += 1;
        self.epoch
    }

    /// Request showing a tooltip. If another tooltip is active or was recently
    /// hidden, shows immediately with a slide animation. Otherwise starts a delay.
    /// A [`TooltipRequest::discrete`] request always waits for its own delay.
    pub fn request_show(
        &mut self,
        content: TooltipRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Gate both delayed display and the immediate grace-period switch.
        // Keep this in Base so every managed component shares the policy.
        if !self.enabled {
            return;
        }
        self.hide_task = None;
        self.active_trigger_bounds = Some(content.trigger_bounds);

        if let Some(show_delay) = content.discrete_show_delay {
            self.content = None;
            self.previous_bounds = None;
            self.had_recent_tooltip = false;
            self.is_switching = false;
            cx.notify();
            self.show_after(show_delay, content, window, cx);
            return;
        }

        let was_visible = self.content.is_some();
        if was_visible || self.had_recent_tooltip {
            self.previous_bounds = self.content.as_ref().map(|content| content.trigger_bounds);
            self.content = Some(content);
            self.show_task = None;
            self.is_switching = was_visible;
            self.animation_epoch += 1;
            cx.notify();
            return;
        }

        self.show_after(SHOW_DELAY, content, window, cx);
    }

    fn show_after(
        &mut self,
        delay: Duration,
        content: TooltipRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let epoch = self.next_epoch();
        self.show_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.epoch != epoch {
                    return;
                }
                // A trigger removed while hovered (a closed row) never reports
                // its leave, so the pending show must check the pointer itself.
                if !content.trigger_bounds.contains(&window.mouse_position()) {
                    this.clear_state();
                    return;
                }
                this.content = Some(content);
                this.previous_bounds = None;
                this.is_switching = false;
                this.animation_epoch += 1;
                cx.notify();
            });
        }));
    }

    /// Show a tooltip anchored to `trigger_bounds`, for an element that draws its own hoverable
    /// regions instead of giving each one a child element that could carry a managed tooltip.
    pub fn show_for_bounds(
        &mut self,
        trigger_bounds: Bounds<Pixels>,
        build: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_show(TooltipRequest::new(trigger_bounds, build), window, cx);
    }

    /// Show a tooltip at once, anchored to `trigger_bounds` in this window, for a trigger drawn in
    /// a child window over it: that trigger's hover never reaches this window, and a small child
    /// window cannot hold the tooltip itself. The caller owns the show delay and the hide.
    pub fn show_now(
        &mut self,
        trigger_bounds: Bounds<Pixels>,
        placement: ManagedTooltipPlacement,
        build: Rc<dyn Fn(&mut Window, &mut App) -> AnyView>,
        cx: &mut Context<Self>,
    ) {
        self.next_epoch();
        self.show_task = None;
        self.hide_task = None;
        self.active_trigger_bounds = Some(trigger_bounds);
        self.previous_bounds = None;
        self.had_recent_tooltip = false;
        self.is_switching = false;
        self.content = Some(TooltipRequest {
            build,
            trigger_bounds,
            placement,
            discrete_show_delay: None,
        });
        self.animation_epoch += 1;
        cx.notify();
    }

    /// Request hiding the current tooltip. Starts a brief grace period so that
    /// moving to another tooltip-bearing element feels instant.
    pub fn request_hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_task = None;
        self.active_trigger_bounds = None;
        self.hide_after_grace(window, cx);
    }

    /// Request hiding the tooltip of the trigger at `trigger_bounds`, which the pointer left.
    ///
    /// A `discrete` tooltip hides at once; any other keeps the grace period of
    /// [`TooltipOverlay::request_hide`].
    pub fn request_hide_for(
        &mut self,
        trigger_bounds: Bounds<Pixels>,
        discrete: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Hover transitions can deliver the previous trigger's leave after the
        // next trigger's enter. Never let that stale leave hide the new tooltip.
        // A trigger that moved while hovered (a scroll, a list reorder) reports
        // its leave with new bounds; it is only stale while the pointer really is
        // over the trigger the tooltip was shown for.
        if self.active_trigger_bounds != Some(trigger_bounds)
            && self
                .active_trigger_bounds
                .is_some_and(|active| active.contains(&window.mouse_position()))
        {
            return;
        }

        self.show_task = None;
        self.active_trigger_bounds = None;

        if discrete {
            self.next_epoch();
            let was_visible = self.content.take().is_some();
            self.previous_bounds = None;
            self.had_recent_tooltip = false;
            self.is_switching = false;
            self.hide_task = None;
            if was_visible {
                cx.notify();
            }
            return;
        }

        self.hide_after_grace(window, cx);
    }

    fn hide_after_grace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.content.is_none() {
            return;
        }
        let epoch = self.next_epoch();
        self.had_recent_tooltip = true;
        self.hide_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(GRACE_PERIOD).await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.epoch == epoch {
                    this.content = None;
                    this.previous_bounds = None;
                    this.had_recent_tooltip = false;
                    cx.notify();
                }
            });
        }));
    }

    /// Hide the tooltip because its trigger was pressed, and remember the trigger for
    /// [`TooltipOverlay::flash_pressed`].
    pub fn press(&mut self, trigger: TooltipRequest, cx: &mut Context<Self>) {
        self.hide(cx);
        self.pressed = Some(trigger);
    }

    /// Shows `build`'s view for `duration` in place of the tooltip of the trigger just pressed,
    /// while the pointer is still on it, then shows that trigger's own tooltip again or hides.
    /// Returns false when the pointer is not on the last pressed trigger.
    pub fn flash_pressed(
        &mut self,
        duration: Duration,
        build: Rc<dyn Fn(&mut Window, &mut App) -> AnyView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pressed) = self.pressed.take() else {
            return false;
        };
        if window.last_input_was_keyboard()
            || !pressed.trigger_bounds.contains(&window.mouse_position())
        {
            return false;
        }
        self.show_now(pressed.trigger_bounds, pressed.placement, build, cx);
        let epoch = self.epoch;
        self.hide_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.epoch != epoch {
                    return;
                }
                if pressed.trigger_bounds.contains(&window.mouse_position()) {
                    this.show_now(pressed.trigger_bounds, pressed.placement, pressed.build, cx);
                } else {
                    this.hide(cx);
                }
            });
        }));
        true
    }

    /// Dismiss the tooltip and cancel any pending show.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if self.clear_state() {
            cx.notify();
        }
    }

    fn clear_state(&mut self) -> bool {
        let changed = self.content.is_some()
            || self.active_trigger_bounds.is_some()
            || self.previous_bounds.is_some()
            || self.had_recent_tooltip
            || self.is_switching
            || self.show_task.is_some()
            || self.hide_task.is_some();
        self.content = None;
        self.active_trigger_bounds = None;
        self.previous_bounds = None;
        self.had_recent_tooltip = false;
        self.is_switching = false;
        self.show_task = None;
        self.hide_task = None;
        changed
    }
}

impl Default for TooltipOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl RootPlugin for TooltipOverlay {}

impl Render for TooltipOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(content) = self.content.as_ref() else {
            return div().into_any_element();
        };
        let view = (content.build)(window, cx);
        let trigger_bounds = content.trigger_bounds;
        let placement = content.placement;
        let transition = match (self.is_switching, self.previous_bounds) {
            (true, Some(previous)) => TooltipTransition::Switch {
                epoch: self.animation_epoch,
                previous,
                current: trigger_bounds,
            },
            _ => TooltipTransition::Enter {
                epoch: self.animation_epoch,
            },
        };
        let rendered = (self.renderer)(view, transition, window, cx);

        // A managed tooltip otherwise only hides on its trigger's hover-leave, which
        // never arrives when the trigger is removed or replaced while hovered (a list
        // row closed from under the pointer). While one is showing, the overlay itself
        // dismisses it on any press, on any scroll, and once the pointer is off the trigger.
        let overlay = cx.entity().downgrade();
        let dismiss_guard = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let on_press = overlay.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = on_press.update(cx, |overlay, cx| overlay.hide(cx));
                    }
                });
                // A scroll moves the content out from under the tooltip it belongs to.
                let on_scroll = overlay.clone();
                window.on_mouse_event(move |_: &ScrollWheelEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = on_scroll.update(cx, |overlay, cx| overlay.hide(cx));
                    }
                });
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && !trigger_bounds.contains(&event.position)
                    {
                        let _ = overlay.update(cx, |overlay, cx| overlay.hide(cx));
                    }
                });
            },
        )
        .absolute()
        .size_0();

        let tooltip = deferred(
            TooltipOverlayPositioner {
                trigger_bounds,
                placement,
                children: vec![rendered],
            }
            .into_any_element(),
        )
        .with_priority(TOOLTIP_PRIORITY);

        div().child(dismiss_guard).child(tooltip).into_any_element()
    }
}

impl Root {
    /// The window's managed-tooltip overlay, when its root mounts one.
    pub fn tooltip_overlay(window: &Window, cx: &App) -> Option<Entity<TooltipOverlay>> {
        window.root::<Root>()??.read(cx).plugin::<TooltipOverlay>()
    }

    /// Dismiss the window's managed tooltip and cancel any pending show.
    ///
    /// A managed tooltip only hides on its trigger's hover-leave, which never
    /// arrives when the trigger is removed from the tree while hovered; callers
    /// that remove a trigger (a view switch, a closing menu) dismiss it here.
    pub fn hide_tooltip(window: &Window, cx: &mut App) {
        if let Some(overlay) = Self::tooltip_overlay(window, cx) {
            overlay.update(cx, |overlay, cx| overlay.hide(cx));
        }
    }

    /// Show a managed tooltip in this window at once, anchored to `trigger_bounds` (in this
    /// window's coordinates), for a trigger that lives in a child window over it and so never
    /// reports its hover here. The caller owns the delay; dismiss it with [`Root::hide_tooltip`].
    pub fn show_tooltip_for_bounds(
        window: &Window,
        cx: &mut App,
        trigger_bounds: Bounds<Pixels>,
        placement: ManagedTooltipPlacement,
        build: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) {
        if let Some(overlay) = Self::tooltip_overlay(window, cx) {
            overlay.update(cx, |overlay, cx| {
                overlay.show_now(trigger_bounds, placement, Rc::new(build), cx)
            });
        }
    }

    /// Shows `build`'s view for `duration` as the tooltip of the managed-tooltip trigger the last
    /// press in this window landed on, when the pointer is still on it, then gives the trigger its
    /// own tooltip back. Returns false, showing nothing, otherwise.
    pub fn flash_pressed_tooltip(
        window: &mut Window,
        cx: &mut App,
        duration: Duration,
        build: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> bool {
        let Some(overlay) = Self::tooltip_overlay(window, cx) else {
            return false;
        };
        overlay.update(cx, |overlay, cx| {
            overlay.flash_pressed(duration, Rc::new(build), window, cx)
        })
    }
}

/// Lays out a managed tooltip against its trigger for its [`ManagedTooltipPlacement`].
struct TooltipOverlayPositioner {
    trigger_bounds: Bounds<Pixels>,
    placement: ManagedTooltipPlacement,
    children: Vec<AnyElement>,
}

impl IntoElement for TooltipOverlayPositioner {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TooltipOverlayPositioner {
    type RequestLayoutState = Vec<LayoutId>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child_layout_ids = self
            .children
            .iter_mut()
            .map(|child| child.request_layout(window, cx))
            .collect::<Vec<_>>();
        let layout_id = window.request_layout(
            Style {
                position: Position::Absolute,
                display: Display::Flex,
                ..Style::default()
            },
            child_layout_ids.iter().copied(),
            cx,
        );
        (layout_id, child_layout_ids)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child_layout_ids: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if child_layout_ids.is_empty() {
            return;
        }

        let mut child_min = point(Pixels::MAX, Pixels::MAX);
        let mut child_max = Point::default();
        for child_layout_id in child_layout_ids.iter() {
            let child_bounds = window.layout_bounds(*child_layout_id);
            child_min = child_min.min(&child_bounds.origin);
            child_max = child_max.max(&child_bounds.bottom_right());
        }

        let tooltip_size = (child_max - child_min).into();
        let frame = frame_insets(
            window.window_decorations(),
            window.client_inset().unwrap_or(px(0.)),
        );
        let tooltip_bounds = tooltip_bounds(
            self.trigger_bounds,
            tooltip_size,
            window.viewport_size(),
            frame.map(|inset| *inset + WINDOW_MARGIN),
            self.placement,
        );

        let offset = tooltip_bounds.origin - bounds.origin;
        let offset = point(offset.x.round(), offset.y.round());
        window.with_element_offset(offset, |window| {
            for child in &mut self.children {
                child.prepaint(window, cx);
            }
        });
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
        for child in &mut self.children {
            child.paint(window, cx);
        }
    }
}

/// An unstyled tooltip positioner with viewport-aware flipping and clamping.
///
/// This is a tooltip-named view of [`crate::Positioner`]'s side placement. It
/// adds no element of its own; the shared positioner is what gets rendered.
pub struct TooltipPositioner(Positioner);

impl TooltipPositioner {
    pub fn new(trigger_bounds: Bounds<Pixels>) -> Self {
        Self(Positioner::side(trigger_bounds).margin(WINDOW_MARGIN))
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        self.0 = self.0.placement(placement);
        self
    }
}

impl ParentElement for TooltipPositioner {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.0.extend(elements);
    }
}

impl IntoElement for TooltipPositioner {
    type Element = Positioner;

    fn into_element(self) -> Self::Element {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, point, size};

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[gpui::test]
    fn provider_owns_grace_switch_and_dismiss(cx: &mut gpui::TestAppContext) {
        let state = cx.update(|cx| cx.new(|_| TooltipOverlay::new()));
        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            state.update(cx, |tooltip, cx| {
                tooltip.had_recent_tooltip = true;
                tooltip.request_show(
                    TooltipRequest::new(bounds(0., 0., 20., 20.), |_, _| {
                        panic!("content is not rendered by this lifecycle test")
                    }),
                    window,
                    cx,
                );
            });
        });
        cx.update(|_, cx| assert!(state.read(cx).content.is_some()));

        cx.update(|_, cx| {
            state.update(cx, |tooltip, cx| tooltip.hide(cx));
        });
        cx.update(|_, cx| assert!(state.read(cx).content.is_none()));
    }

    #[test]
    fn tooltip_priority_exceeds_popup_layer() {
        assert!(TOOLTIP_PRIORITY > crate::POPUP_PRIORITY);
    }

    #[gpui::test]
    fn disabled_provider_ignores_delayed_and_immediate_requests(cx: &mut gpui::TestAppContext) {
        let state = cx.update(|cx| {
            cx.new(|_| TooltipOverlay {
                enabled: false,
                ..TooltipOverlay::new()
            })
        });
        let cx = cx.add_empty_window();
        for had_recent_tooltip in [false, true] {
            cx.update(|window, cx| {
                state.update(cx, |tooltip, cx| {
                    tooltip.had_recent_tooltip = had_recent_tooltip;
                    tooltip.request_show(
                        TooltipRequest::new(bounds(0., 0., 20., 20.), |_, _| {
                            panic!("disabled tooltips must not build content")
                        }),
                        window,
                        cx,
                    );
                    assert!(tooltip.content.is_none());
                    assert!(tooltip.show_task.is_none());
                    assert!(tooltip.hide_task.is_none());
                    assert_eq!(tooltip.animation_epoch, 0);
                });
            });
        }
    }
}

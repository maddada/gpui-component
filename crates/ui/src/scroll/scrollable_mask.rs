use std::cell::Cell;

use gpui::{
    App, Axis, BorderStyle, Bounds, ContentMask, Edges, Element, ElementId, GlobalElementId,
    Hitbox, HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, IsZero as _, LayoutId,
    PaintQuad, ParentElement as _, Point, Position, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement as _, Style, StyleRefinement, Styled as _, TouchPhase, Window,
    canvas, div, point, prelude::FluentBuilder as _, px, relative,
};
use gpui::{Corners, DispatchPhase, Pixels};
use instant::{Duration, Instant};

use crate::{
    AxisExt, StyledExt as _,
    scroll::{Scrollbar, ScrollbarShow},
};

/// A horizontal scroll viewport that only consumes horizontal wheel gestures.
///
/// GPUI's native `overflow_x_scroll` maps vertical wheel input onto horizontal
/// scrolling when there is no vertical overflow, so the viewport clips with
/// `overflow_hidden` and scrolls itself from [`horizontal_wheel`] instead.
///
/// With `scrollbar` set, a horizontal bar of that thickness sits in a strip of
/// its own under the viewport and shows while the pointer is over the area; it
/// draws nothing while the content fits.
pub(crate) fn horizontal_scroll_area(
    id: impl Into<ElementId>,
    scroll_handle: &ScrollHandle,
    style: &StyleRefinement,
    scrollbar: Option<Pixels>,
    child: impl IntoElement,
) -> impl IntoElement {
    div()
        .id(id)
        .group(HORIZONTAL_SCROLL_GROUP)
        .relative()
        .w_full()
        .child(
            div()
                .id("viewport")
                .w_full()
                .refine_style(style)
                .overflow_hidden()
                .track_scroll(scroll_handle)
                .child(child),
        )
        // Outside the scrolled viewport: a child of it is laid out at the
        // scroll offset, so a wheel target in there slides left with the
        // content and stops covering the part of the viewport scrolled into.
        .child(horizontal_wheel(scroll_handle))
        .when_some(scrollbar, |this, thickness| {
            this.child(
                div()
                    .relative()
                    .w_full()
                    .h(thickness + SCROLLBAR_GAP)
                    .opacity(0.)
                    .group_hover(HORIZONTAL_SCROLL_GROUP, |style| style.opacity(1.))
                    .child(
                        Scrollbar::horizontal(scroll_handle)
                            .id("horizontal-scrollbar")
                            .thickness(thickness)
                            .scrollbar_show(ScrollbarShow::Always),
                    ),
            )
        })
}

/// The group a horizontal scroll area's bar is revealed by.
const HORIZONTAL_SCROLL_GROUP: &str = "gpui-horizontal-scroll-area";
/// The air between the bottom of the content and the bar under it.
const SCROLLBAR_GAP: Pixels = px(3.);

/// How long a pause ends one wheel gesture, for wheels that report no phases.
const GESTURE_GAP: Duration = Duration::from_millis(200);
/// How far a gesture travels before its axis is decided.
const GESTURE_SLOP: f32 = 3.;

/// The axis the current wheel gesture was decided on, shared by every
/// horizontal scroll area in the thread because there is one pointer.
#[derive(Clone, Copy, Default)]
struct WheelGesture {
    axis: Option<Axis>,
    travel: Point<Pixels>,
    last_event: Option<Instant>,
}

thread_local! {
    static WHEEL_GESTURE: Cell<WheelGesture> = Cell::new(WheelGesture::default());
}

/// The axis `event` belongs to, deciding it once per gesture.
///
/// A trackpad swipe is never exactly straight, so reading each event on its
/// own lets a sideways swipe scroll the page a little on every event whose
/// vertical part happens to win, and a vertical scroll grab a table it passes
/// over. The gesture is decided from its first few pixels and keeps that axis
/// through its momentum; it restarts when the fingers touch down again or the
/// wheel pauses. Every area's handler sees every event, so this is recorded
/// idempotently: a second read of the same event decides the same way.
fn wheel_gesture_axis(event: &ScrollWheelEvent, line_height: Pixels) -> Option<Axis> {
    WHEEL_GESTURE.with(|cell| {
        let mut gesture = cell.get();
        let now = Instant::now();
        let paused = gesture
            .last_event
            .is_none_or(|last| now.duration_since(last) > GESTURE_GAP);
        if event.touch_phase == TouchPhase::Started || paused {
            gesture = WheelGesture::default();
        }
        gesture.last_event = Some(now);
        if gesture.axis.is_none() {
            let delta = event.delta.pixel_delta(line_height);
            gesture.travel = gesture.travel + point(delta.x.abs(), delta.y.abs());
            let travel = gesture.travel;
            if f32::from(travel.x.max(travel.y)) >= GESTURE_SLOP {
                gesture.axis = Some(if travel.x > travel.y {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                });
            }
        }
        cell.set(gesture);
        gesture.axis
    })
}

/// The wheel target over a horizontal scroll area.
///
/// It scrolls on the capture pass and stops the event there. A GPUI `list`
/// (a chat transcript) registers its own wheel handler after painting its rows,
/// so on the bubble pass the list scrolls first and a table inside a row would
/// only see the wheel after the page had already moved under it. A sideways
/// gesture over an area that can scroll is consumed whole, at its edges too, so
/// the page never drifts while the reader pans a table; a vertical gesture, or
/// one over content that fits, passes through untouched.
fn horizontal_wheel(scroll_handle: &ScrollHandle) -> impl IntoElement {
    let scroll_handle = scroll_handle.clone();
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            let scroll_handle = scroll_handle.clone();
            let view_id = window.current_view();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let line_height = window.line_height();
                let axis = wheel_gesture_axis(event, line_height);
                if axis != Some(Axis::Horizontal) || !hitbox.should_handle_scroll(window) {
                    return;
                }
                let max = scroll_handle.max_offset().x;
                if max <= px(0.) {
                    return;
                }
                let offset = scroll_handle.offset();
                let delta = event.delta.pixel_delta(line_height).x;
                let next = (offset.x + delta).clamp(-max, px(0.));
                if next != offset.x {
                    scroll_handle.set_offset(point(next, offset.y));
                    cx.notify(view_id);
                }
                cx.stop_propagation();
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .w_full()
    .h_full()
}

/// Make a scrollable mask element to cover the parent view with the mouse wheel event listening.
///
/// When the mouse wheel is scrolled, will move the `scroll_handle` scrolling with the `axis` direction.
/// You can use this `scroll_handle` to control what you want to scroll.
/// This is only can handle once axis scrolling.
pub struct ScrollableMask {
    axis: Axis,
    scroll_handle: ScrollHandle,
    debug: Option<Hsla>,
}

impl ScrollableMask {
    /// Create a new scrollable mask element.
    pub fn new(axis: Axis, scroll_handle: &ScrollHandle) -> Self {
        Self {
            scroll_handle: scroll_handle.clone(),
            axis,
            debug: None,
        }
    }

    /// Enable the debug border, to show the mask bounds.
    #[allow(dead_code)]
    pub fn debug(mut self) -> Self {
        self.debug = Some(gpui::yellow());
        self
    }
}

impl IntoElement for ScrollableMask {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ScrollableMask {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        // Set the layout style relative to the table view to get same size.
        style.position = Position::Absolute;
        style.flex_grow = 1.0;
        style.flex_shrink = 1.0;
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();

        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        // Move y to bounds height to cover the parent view.
        let cover_bounds = Bounds {
            origin: Point {
                x: bounds.origin.x,
                y: bounds.origin.y - bounds.size.height,
            },
            size: bounds.size,
        };

        window.insert_hitbox(cover_bounds, gpui::HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        let is_horizontal = self.axis.is_horizontal();
        let line_height = window.line_height();
        let bounds = hitbox.bounds;

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(color) = self.debug {
                window.paint_quad(PaintQuad {
                    bounds,
                    border_widths: Edges::all(px(1.0)),
                    border_color: color,
                    background: gpui::transparent_white().into(),
                    corner_radii: Corners::all(px(0.)),
                    border_style: BorderStyle::default(),
                });
            }

            window.on_mouse_event({
                let view_id = window.current_view();
                let scroll_handle = self.scroll_handle.clone();

                move |event: &ScrollWheelEvent, phase, _, cx| {
                    if !(bounds.contains(&event.position) && phase.bubble()) {
                        return;
                    }

                    let mut offset = scroll_handle.offset();
                    let mut delta = event.delta.pixel_delta(line_height);

                    // Limit for only one way scrolling at same time.
                    // When use MacBook touchpad we may get both x and y delta,
                    // only allows the one that more to scroll.
                    if !delta.x.is_zero() && !delta.y.is_zero() {
                        if delta.x.abs() > delta.y.abs() {
                            delta.y = px(0.);
                        } else {
                            delta.x = px(0.);
                        }
                    }

                    if is_horizontal {
                        offset.x += delta.x;
                    } else {
                        offset.y += delta.y;
                    }

                    if offset != scroll_handle.offset() {
                        scroll_handle.set_offset(offset);
                        cx.notify(view_id);
                        cx.stop_propagation();
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Context, IntoElement, Render, ScrollDelta, ScrollWheelEvent, TestAppContext,
        VisualTestContext, Window, div, point, px,
    };

    struct HorizontalScrollAreaTest {
        scroll_handle: ScrollHandle,
    }

    impl Render for HorizontalScrollAreaTest {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(100.)).h(px(40.)).child(horizontal_scroll_area(
                "horizontal-scroll-area",
                &self.scroll_handle,
                &Default::default(),
                None,
                div().w(px(300.)).h(px(40.)),
            ))
        }
    }

    #[gpui::test]
    fn horizontal_scroll_area_ignores_vertical_wheel(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let (_, cx) = cx.add_window_view({
            let scroll_handle = scroll_handle.clone();
            move |_, _| HorizontalScrollAreaTest {
                scroll_handle: scroll_handle.clone(),
            }
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });

        assert_eq!(scroll_handle.offset().x, px(0.));
    }

    #[gpui::test]
    fn horizontal_scroll_area_uses_horizontal_wheel(cx: &mut TestAppContext) {
        let scroll_handle = ScrollHandle::new();
        let (_, cx) = cx.add_window_view({
            let scroll_handle = scroll_handle.clone();
            move |_, _| HorizontalScrollAreaTest {
                scroll_handle: scroll_handle.clone(),
            }
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(10.)),
            delta: ScrollDelta::Pixels(point(px(-40.), px(0.))),
            ..Default::default()
        });

        assert_eq!(scroll_handle.offset().x, px(-40.));
    }
}

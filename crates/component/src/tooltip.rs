use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{
    Action, AnyElement, AnyView, App, AppContext, Bounds, Context, ElementId, IntoElement,
    MouseButton, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_base::{
    Root, Tooltip as BaseTooltip, TooltipOverlay as BaseTooltipOverlay,
    TooltipRequest as BaseTooltipRequest, TooltipTransition as BaseTooltipTransition,
};

pub use gpui_base::ManagedTooltipPlacement;

use crate::{
    ActiveTheme, Placement, StyledExt,
    animation::{EffectTransition, ease_in_out_cubic, ease_out_cubic},
    kbd::Kbd,
    text::Text,
};

/// Ghostex: the corner radius of a tooltip bubble drawn in a frosted window.
pub const FROSTED_TOOLTIP_RADIUS: f32 = 7.;

static FROSTED_TOOLTIP_ALPHA: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0x3f1e_b852); // 0.62

/// Ghostex: how much of the popover colour a bubble in a frosted window paints over its blur.
pub fn set_frosted_tooltip_alpha(alpha: f32) {
    FROSTED_TOOLTIP_ALPHA.store(alpha.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

fn frosted_tooltip_alpha() -> f32 {
    f32::from_bits(FROSTED_TOOLTIP_ALPHA.load(std::sync::atomic::Ordering::Relaxed))
}

pub(crate) fn init(_cx: &mut App) {
    // No app-level init needed — TooltipOverlay is per-window via Root.
}

// ── Tooltip view (unchanged API) ────────────────────────────────────────────

enum TooltipContext {
    Text(Text),
    Element(Box<dyn Fn(&mut Window, &mut App) -> AnyElement>),
}

/// A Tooltip element that can display text or custom content,
/// with optional key binding information.
pub struct Tooltip {
    style: StyleRefinement,
    content: TooltipContext,
    key_binding: Option<Kbd>,
    action: Option<(Box<dyn Action>, Option<SharedString>)>,
}

impl Tooltip {
    /// Create a Tooltip with a text content.
    pub fn new(text: impl Into<Text>) -> Self {
        Self {
            style: StyleRefinement::default(),
            content: TooltipContext::Text(text.into()),
            key_binding: None,
            action: None,
        }
    }

    /// Create a Tooltip with a custom element.
    pub fn element<E, F>(builder: F) -> Self
    where
        E: IntoElement,
        F: Fn(&mut Window, &mut App) -> E + 'static,
    {
        Self {
            style: StyleRefinement::default(),
            key_binding: None,
            action: None,
            content: TooltipContext::Element(Box::new(move |window, cx| {
                builder(window, cx).into_any_element()
            })),
        }
    }

    /// Set Action to display key binding information for the tooltip if it exists.
    pub fn action(mut self, action: &dyn Action, context: Option<&str>) -> Self {
        self.action = Some((action.boxed_clone(), context.map(SharedString::new)));
        self
    }

    /// Set KeyBinding information for the tooltip.
    pub fn key_binding(mut self, key_binding: Option<Kbd>) -> Self {
        self.key_binding = key_binding;
        self
    }

    /// Build the tooltip and return it as an `AnyView`.
    pub fn build(self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|_| self).into()
    }
}

impl FluentBuilder for Tooltip {}
impl Styled for Tooltip {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let key_binding = if let Some(key_binding) = &self.key_binding {
            Some(key_binding.clone())
        } else {
            if let Some((action, context)) = &self.action {
                Kbd::binding_for_action(
                    action.as_ref(),
                    context.as_ref().map(|s| s.as_ref()),
                    window,
                )
            } else {
                None
            }
        };

        // The wrapper must be a flex container: block layout collapses the
        // bubble's vertical margins, so the overlay positioner would measure a
        // box without them and place the bubble flush against its trigger.
        //
        // CDXC:Tooltips 2026-09-24 WHY:
        // Taffy measures a flex item's base and automatic minimum size with its text unwrapped, so a bubble can only shrink to a narrower box (the room next to a native view, see the base `TooltipOverlay`) when every flex item down to the text's container has min-width 0. A bubble that fits keeps its one-line width.
        // Ghostex: in a frosted window (the desktop's tooltip host under window glass) the bubble
        // is a thinned fill over the window's blur, which is limited to the bubble's own frame.
        //
        // CDXC:Tooltips 2026-09-26 WHY:
        // The wrapper reports the bubble's laid-out frame (its only child, borders included). A canvas inside the bubble, the obvious way, sits at the bubble's content origin because an absolute child without insets keeps its static position, so the blur ran one padding plus border to the right of the bubble and the bubble no longer matched its text.
        let frosted = window.frosted_surface();
        div()
            .flex()
            .min_w_0()
            .when(frosted, |this| {
                this.on_children_prepainted(|bounds, window, _| {
                    if let Some(bubble) = bounds.first() {
                        window.report_frosted_region(*bubble, px(FROSTED_TOOLTIP_RADIUS));
                    }
                })
            })
            .child(
                BaseTooltip::new("tooltip-popup")
                    .h_flex()
                    .min_w_0()
                    .font_family(cx.theme().font_family.clone())
                    .mx_3()
                    .my_2()
                    .text_color(cx.theme().popover_foreground)
                    .map(|this| {
                        if frosted {
                            this.bg(cx.theme().tokens.popover.opacity(frosted_tooltip_alpha()))
                        } else {
                            this.bg(cx.theme().tokens.popover).shadow_md()
                        }
                    })
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(if frosted {
                        px(FROSTED_TOOLTIP_RADIUS)
                    } else {
                        cx.theme().radius
                    })
                    .justify_between()
                    .py_0p5()
                    .px_2()
                    .text_sm()
                    .gap_3()
                    .refine_style(&self.style)
                    .map(|this| {
                        this.child(div().min_w_0().map(|this| match self.content {
                            TooltipContext::Text(ref text) => this.child(text.clone()),
                            TooltipContext::Element(ref builder) => this.child(builder(window, cx)),
                        }))
                    })
                    .when_some(key_binding, |this, kbd| {
                        this.child(
                            div()
                                .text_xs()
                                .flex_shrink_0()
                                .text_color(cx.theme().muted_foreground)
                                .child(kbd.appearance(false)),
                        )
                    }),
            )
    }
}

// ── Managed tooltip system ──────────────────────────────────────────────────

/// Duration of the slide-down enter animation.
const ENTER_DURATION: Duration = Duration::from_millis(150);
/// Duration of the position-slide animation when switching tooltips.
const SLIDE_DURATION: Duration = Duration::from_millis(200);
pub(crate) fn render_tooltip(
    content_view: AnyView,
    transition: BaseTooltipTransition,
    _: &mut Window,
    _: &mut App,
) -> AnyElement {
    // Flex with min-width 0, a link in the chain `Tooltip::render` describes.
    div()
        .flex()
        .min_w_0()
        .child(content_view)
        .map(|element| match transition {
            BaseTooltipTransition::Switch {
                epoch,
                previous,
                current,
            } => {
                let same_row = (current.origin.y - previous.origin.y).abs() < px(10.);
                if !same_row {
                    return element.into_any_element();
                }
                let dx = current.center().x - previous.center().x;
                EffectTransition::new(SLIDE_DURATION)
                    .ease(ease_in_out_cubic)
                    .slide_x(-dx, px(0.))
                    .apply(
                        element,
                        ElementId::NamedInteger("tooltip-slide".into(), epoch as u64),
                    )
                    .into_any_element()
            }
            BaseTooltipTransition::Enter { epoch } => EffectTransition::new(ENTER_DURATION)
                .ease(ease_out_cubic)
                .slide_y(px(4.), px(0.))
                .fade(0.0, 1.0)
                .apply(
                    element,
                    ElementId::NamedInteger("tooltip-enter".into(), epoch as u64),
                )
                .into_any_element(),
        })
}

// ── Extension trait for managed tooltips ─────────────────────────────────────

// ── Shared tooltip state for components ─────────────────────────────────────

/// Shared tooltip state that components (Button, Switch, Checkbox, Radio, etc.)
/// can embed to get `.tooltip()` support with minimal boilerplate.
#[derive(Default)]
pub(crate) struct ComponentTooltip {
    pub text: Option<(
        SharedString,
        Option<(Rc<Box<dyn Action>>, Option<SharedString>)>,
    )>,
    pub builder: Option<Rc<dyn Fn(&mut Window, &mut App) -> AnyView>>,
}

impl ComponentTooltip {
    /// Apply this tooltip to a `Stateful<Div>` (or any `ManagedTooltipExt` element).
    pub fn apply<E: ManagedTooltipExt>(self, el: E) -> E {
        if let Some(builder) = self.builder {
            el.managed_tooltip(move |window, cx| builder(window, cx))
        } else if let Some((text, action)) = self.text {
            el.managed_tooltip(move |window, cx| {
                Tooltip::new(text.clone())
                    .when_some(action.clone(), |this, (action, context)| {
                        this.action(
                            action.boxed_clone().as_ref(),
                            context.as_ref().map(|c| c.as_ref()),
                        )
                    })
                    .build(window, cx)
            })
        } else {
            el
        }
    }
}

// ── Managed tooltip trait ───────────────────────────────────────────────────

/// Tooltips drawn by the window's managed overlay: a shared show delay,
/// placements relative to the trigger, discrete tooltips, and dismissal when
/// the trigger is pressed, scrolled or left.
pub trait ManagedTooltipExt: StatefulInteractiveElement + crate::ElementExt + Sized {
    fn managed_tooltip(
        self,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.managed_tooltip_with_placement(ManagedTooltipPlacement::Auto, build_tooltip)
    }

    fn managed_tooltip_at(
        self,
        placement: Placement,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.managed_tooltip_with_placement(placement, build_tooltip)
    }

    /// Attach a managed tooltip at an explicit position relative to this element.
    ///
    /// Takes a [`ManagedTooltipPlacement`], or a preferred [`Placement`] (also as an
    /// `Option`, `None` being [`ManagedTooltipPlacement::Auto`]).
    fn managed_tooltip_with_placement(
        self,
        placement: impl Into<ManagedTooltipPlacement>,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.managed_tooltip_with_configuration(placement.into(), None, build_tooltip)
    }

    /// Attach a managed tooltip that hides immediately when its trigger is
    /// left and waits for `show_delay` again when switching between triggers.
    fn managed_discrete_tooltip_with_placement(
        self,
        placement: ManagedTooltipPlacement,
        show_delay: Duration,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.managed_tooltip_with_configuration(placement, Some(show_delay), build_tooltip)
    }

    fn managed_tooltip_with_configuration(
        self,
        placement: ManagedTooltipPlacement,
        discrete_show_delay: Option<Duration>,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        let build_tooltip = Rc::new(build_tooltip);
        let trigger_bounds_cell: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
        let bounds_writer = trigger_bounds_cell.clone();
        let request = move |bounds: Bounds<Pixels>| {
            let build = build_tooltip.clone();
            let request = BaseTooltipRequest::new(bounds, move |window, cx| build(window, cx))
                .managed_placement(placement);
            match discrete_show_delay {
                Some(show_delay) => request.discrete(show_delay),
                None => request,
            }
        };
        let request = Rc::new(request);

        self.on_prepaint(move |bounds, _, _| {
            bounds_writer.set(bounds);
        })
        .on_hover({
            let trigger_bounds_cell = trigger_bounds_cell.clone();
            let request = request.clone();
            move |hovered, window, cx| {
                if let Some(overlay) = Root::tooltip_overlay(window, cx) {
                    let bounds = trigger_bounds_cell.get();
                    if *hovered {
                        overlay.update(cx, |o: &mut BaseTooltipOverlay, cx| {
                            o.request_show(request(bounds), window, cx);
                        });
                    } else {
                        overlay.update(cx, |o: &mut BaseTooltipOverlay, cx| {
                            o.request_hide_for(bounds, discrete_show_delay.is_some(), window, cx);
                        });
                    }
                }
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if let Some(overlay) = Root::tooltip_overlay(window, cx) {
                let trigger = request(trigger_bounds_cell.get());
                overlay.update(cx, |overlay, cx| overlay.press(trigger, cx));
            }
        })
    }
}

impl<E: StatefulInteractiveElement + crate::ElementExt> ManagedTooltipExt for E {}

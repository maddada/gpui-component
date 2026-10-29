use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{
    Action, AnyElement, AnyView, App, AppContext, Bounds, Context, ElementId, FontWeight,
    IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, div, prelude::FluentBuilder, px,
    relative, rems,
};
pub use gpui_base::TooltipDefaults;
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

/// Ghostex: the corner radius of every tooltip bubble, in a frosted window or not.
///
/// CDXC:Tooltips 2026-10-05 DECISION:
/// User: "Please make the tooltips in this app all have rounded corners, not square ones." One radius for every tooltip, the 8px the app's menus and popovers already use, replacing the theme's 2px radius (which read as square) and the 5px and 7px values individual tooltips carried. Supersedes the 2026-06-30 5px radius of the React tooltips. Callers must not set their own `.rounded(..)` on a tooltip.
pub const TOOLTIP_RADIUS: f32 = 8.;

/// Ghostex: the corner radius of a tooltip bubble drawn in a frosted window (the same as every tooltip's).
pub const FROSTED_TOOLTIP_RADIUS: f32 = TOOLTIP_RADIUS;

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

/// Ghostex: one gesture row under a tooltip's title ("Interrupt and send now" with its keys).
#[derive(Clone, Debug)]
pub struct TooltipRow {
    label: SharedString,
    keys: Option<SharedString>,
    hint: Option<SharedString>,
}

impl TooltipRow {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            keys: None,
            hint: None,
        }
    }

    /// The row's shortcut as the app writes it ("⌃Enter", "Ctrl+Alt+M"), drawn as one key cap
    /// per key on the right. An empty chord draws nothing, so an unbound hotkey needs no branch.
    pub fn keys(mut self, chord: impl Into<SharedString>) -> Self {
        self.keys = Some(chord.into()).filter(|chord| !chord.trim().is_empty());
        self
    }

    /// A gesture that is not a key ("Right-click"), drawn as muted text on the right.
    pub fn hint(mut self, text: impl Into<SharedString>) -> Self {
        self.hint = Some(text.into());
        self
    }
}

/// A Tooltip element that can display text or custom content,
/// with optional key binding information.
///
/// CDXC:Tooltips 2026-10-15 DECISION:
/// User picked "mini menu + frosted glass (when transparency enabled)" for every tooltip in the app: a compact title row, then one row per gesture or shortcut ([`Tooltip::row`]) with its keys right-aligned as small key caps ([`Tooltip::keys`], [`TooltipRow::keys`]), like a little menu. A plain-text tooltip is a single-row mini menu. Under window glass the bubble is the frosted menus' material (the desktop's `frosted_tooltip_fill`); otherwise it is the solid dark tooltip colour. Shortcuts move out of the sentence into key caps, so call sites pass rows instead of "Send (^Enter interrupts…, right-click for more)".
pub struct Tooltip {
    style: StyleRefinement,
    content: TooltipContext,
    key_binding: Option<Kbd>,
    action: Option<(Box<dyn Action>, Option<SharedString>)>,
    keys: Option<SharedString>,
    rows: Vec<TooltipRow>,
}

impl Tooltip {
    /// Create a Tooltip with a text content.
    ///
    /// Ghostex: a shortcut written after the label in brackets ("Talk to the voice agent (⌃⌥V)",
    /// "Back (Ctrl+[)") moves out of the text into the title row's key caps, as if the caller had
    /// passed it to [`Tooltip::keys`] (see [`split_trailing_shortcut`]).
    pub fn new(text: impl Into<Text>) -> Self {
        let mut keys = None;
        let content = match text.into() {
            Text::String(text) => match split_trailing_shortcut(&text) {
                Some((label, chord)) => {
                    keys = Some(SharedString::from(chord.to_owned()));
                    Text::String(label.to_owned().into())
                }
                None => Text::String(text),
            },
            other => other,
        };
        Self {
            style: StyleRefinement::default(),
            content: TooltipContext::Text(content),
            key_binding: None,
            action: None,
            keys,
            rows: Vec::new(),
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
            keys: None,
            rows: Vec::new(),
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

    /// Ghostex: the title's shortcut as the app writes it ("⌥⌃M"), drawn as key caps on the
    /// title row's right. Takes precedence over [`Tooltip::key_binding`] and [`Tooltip::action`];
    /// an empty chord draws nothing.
    pub fn keys(mut self, chord: impl Into<SharedString>) -> Self {
        self.keys = Some(chord.into()).filter(|chord| !chord.trim().is_empty());
        self
    }

    /// Ghostex: adds a gesture row under the title.
    pub fn row(mut self, row: TooltipRow) -> Self {
        self.rows.push(row);
        self
    }

    /// Ghostex: adds gesture rows under the title.
    pub fn rows(mut self, rows: impl IntoIterator<Item = TooltipRow>) -> Self {
        self.rows.extend(rows);
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
        let title_keys: Option<SharedString> = self.keys.clone().or_else(|| {
            self.key_binding
                .clone()
                .or_else(|| {
                    self.action.as_ref().and_then(|(action, context)| {
                        Kbd::binding_for_action(
                            action.as_ref(),
                            context.as_ref().map(|s| s.as_ref()),
                            window,
                        )
                    })
                })
                .map(|kbd| Kbd::format(kbd.keystroke()).into())
        });

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
        let measure = text_measure(&self.style, window);
        let foreground = cx.theme().popover_foreground;
        let text_content = matches!(self.content, TooltipContext::Text(_));
        let title = div()
            .min_w_0()
            .when(text_content, |this| this.font_weight(FontWeight::MEDIUM))
            .map(|this| match self.content {
                TooltipContext::Text(ref text) => this.max_w(measure).child(text.clone()),
                TooltipContext::Element(ref builder) => this.child(builder(window, cx)),
            });
        // A text tooltip is a mini menu: every row, the title too, carries the menu row's inset
        // inside the bubble's thin frame. Custom content keeps one inset of the same total size,
        // so callers that set their own bubble padding (the sidebar's cards) keep it.
        let row_shell = |first: bool| {
            div()
                .flex()
                .min_w_0()
                .items_center()
                .justify_between()
                .gap_4()
                .px(px(TOOLTIP_ROW_PADDING_X))
                .py(px(TOOLTIP_ROW_PADDING_Y))
                .when(!first, |this| {
                    this.border_t_1().border_color(foreground.opacity(0.06))
                })
        };
        let title_in_row = text_content || title_keys.is_some() || !self.rows.is_empty();
        // CDXC:Tooltips 2026-10-17 WHY:
        // A one-row mini menu centres its row in the bubble instead of stacking it from the top. The row's inset made the content taller than a bubble whose caller fixes its height (the 20px titlebar tooltip), and a block bubble lets the extra spill out of the bottom only, so the text sat low and ran past the bubble's bottom edge. Centred, the row's spare inset overflows evenly and the text line sits in the middle at every zoom; a bubble of natural height is unchanged. It is a row flex box, not a column (the 2026-10-15 note below), so the row keeps its one-line width.
        let center_single_row = title_in_row && self.rows.is_empty();
        let title_row = if title_in_row {
            row_shell(true)
                .when(center_single_row, |this| this.flex_grow(1.))
                .child(title)
                .when_some(title_keys, |this, keys| this.child(key_caps(&keys, cx)))
                .into_any_element()
        } else {
            title.into_any_element()
        };
        let mini_menu = text_content || !self.rows.is_empty();
        let rows = self.rows.iter().map(|row| {
            row_shell(false)
                .child(
                    div()
                        .min_w_0()
                        .max_w(measure)
                        .text_color(foreground.opacity(0.68))
                        .child(row.label.clone()),
                )
                .when_some(row.keys.clone(), |this, keys| {
                    this.child(key_caps(&keys, cx))
                })
                .when_some(row.hint.clone(), |this, hint| {
                    this.child(
                        div()
                            .flex_shrink_0()
                            .text_size(rems(0.71875))
                            .text_color(foreground.opacity(0.5))
                            .child(hint),
                    )
                })
        });
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
                // CDXC:Tooltips 2026-10-15 WHY:
                // The bubble is a block, not a column flex box: a column flex box measures its
                // rows at their min-content width, which with min-width 0 down to the text (the
                // 2026-09-24 note above) collapsed every title to one character per line.
                BaseTooltip::new("tooltip-popup")
                    .min_w_0()
                    .when(center_single_row, |this| this.flex().items_center())
                    .font_family(cx.theme().font_family.clone())
                    .mx_3()
                    .my_2()
                    .text_color(foreground)
                    .map(|this| {
                        if frosted {
                            this.bg(cx.theme().tokens.popover.opacity(frosted_tooltip_alpha()))
                        } else {
                            this.bg(cx.theme().tokens.popover).shadow_md()
                        }
                    })
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(TOOLTIP_RADIUS))
                    .map(|this| {
                        if mini_menu {
                            this.p(px(TOOLTIP_PADDING))
                        } else {
                            this.px(px(TOOLTIP_PADDING + TOOLTIP_ROW_PADDING_X))
                                .py(px(TOOLTIP_PADDING + TOOLTIP_ROW_PADDING_Y))
                        }
                    })
                    .text_size(rems(TOOLTIP_TEXT_REMS))
                    .line_height(relative(1.4))
                    .refine_style(&self.style)
                    .child(title_row)
                    .children(rows),
            )
    }
}

/// Ghostex: the mini menu's frame around its rows, and each row's own inset.
const TOOLTIP_PADDING: f32 = 4.;
const TOOLTIP_ROW_PADDING_X: f32 = 7.;
const TOOLTIP_ROW_PADDING_Y: f32 = 4.;

/// Ghostex: the mini menu's text size, 12.5px at the default 16px rem.
const TOOLTIP_TEXT_REMS: f32 = 0.78125;

/// Ghostex: a chord as the app writes it, one cap per key: "⌥⌃M" is ⌥ ⌃ M, "⌃Enter" is ⌃ Enter,
/// "Ctrl+Alt+M" is Ctrl Alt M. Separate chords (a space between them) stay in order.
fn chord_keys(chord: &str) -> Vec<SharedString> {
    const MODIFIERS: [char; 5] = ['⌘', '⌃', '⌥', '⇧', '⇪'];
    let mut keys = Vec::new();
    for stroke in chord.split_whitespace() {
        if stroke.chars().count() > 1 && stroke.contains('+') {
            let mut parts = stroke.split('+').peekable();
            while let Some(part) = parts.next() {
                if !part.is_empty() {
                    keys.push(SharedString::from(part.to_owned()));
                } else if parts.peek().is_some_and(|next| next.is_empty()) {
                    // "Ctrl++": the key itself is the plus sign.
                    parts.next();
                    keys.push("+".into());
                }
            }
            continue;
        }
        let mut rest = String::new();
        for ch in stroke.chars() {
            if MODIFIERS.contains(&ch) && rest.is_empty() {
                keys.push(ch.to_string().into());
            } else {
                rest.push(ch);
            }
        }
        if !rest.is_empty() {
            keys.push(rest.into());
        }
    }
    keys
}

/// Ghostex: splits "Label (⌃⌥V)" into the label and the shortcut in its trailing brackets, when
/// every stroke in the brackets is a chord: one that holds a modifier symbol (⌘ ⌃ ⌥ ⇧) or starts
/// with a modifier name and a plus ("Ctrl+Alt+V", "Shift+Enter"). "Runs in agentbox (main)" or
/// "Send (⌃Enter interrupts)" keep their brackets.
///
/// CDXC:Tooltips 2026-10-29 DECISION:
/// User, of the header's voice button tooltip "Talk to the voice agent (^⌥V)": "this tooltip is very bad man", and of the context ring's "Hide session details": "this one is much better ... please apply 2nd one everywhere that makes sense instead of the one we have in Image #1". Every plain-text tooltip is this mini menu bubble at its natural size, and a shortcut the app glued to the label in brackets reads as muted key caps after the label instead, so the dozens of call sites that format "{label} ({shortcut})" all take the new look from here.
pub fn split_trailing_shortcut(text: &str) -> Option<(&str, &str)> {
    let trimmed = text.trim_end();
    let inner_end = trimmed.strip_suffix(')')?;
    let open = inner_end.rfind(" (")?;
    let label = inner_end[..open].trim_end();
    let chord = inner_end[open + 2..].trim();
    if label.is_empty() || chord.is_empty() || label.contains('\n') {
        return None;
    }
    chord
        .split_whitespace()
        .all(is_chord_stroke)
        .then_some((label, chord))
}

fn is_chord_stroke(stroke: &str) -> bool {
    const SYMBOLS: [char; 4] = ['⌘', '⌃', '⌥', '⇧'];
    const NAMES: [&str; 9] = [
        "ctrl", "control", "alt", "option", "opt", "shift", "cmd", "super", "win",
    ];
    if stroke.contains(SYMBOLS) {
        return true;
    }
    match stroke.split_once('+') {
        Some((modifier, key)) => {
            !key.is_empty() && NAMES.contains(&modifier.to_ascii_lowercase().as_str())
        }
        None => false,
    }
}

/// Ghostex: a chord drawn as small key caps, right-aligned in a mini menu row.
fn key_caps(chord: &str, cx: &App) -> impl IntoElement {
    let foreground = cx.theme().popover_foreground;
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(3.))
        .children(chord_keys(chord).into_iter().map(move |key| {
            div()
                .min_w(px(17.))
                .px(px(4.))
                .rounded(px(4.))
                .bg(foreground.opacity(0.1))
                .border_1()
                .border_color(foreground.opacity(0.08))
                .text_center()
                .text_size(rems(0.6875))
                .line_height(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground.opacity(0.82))
                .child(key)
        }))
}

/// Ghostex: the widest a text tooltip's line may run before it wraps.
///
/// CDXC:Tooltips 2026-09-26 DECISION:
/// User: "the tooltip should wrap after 60 chars ALWAYS in this app I don't want massive width tooltips". Sixty characters of UI text average 30em (a path measured about half an em per character), in the tooltip's own text size, so a tooltip that sets a smaller size wraps at 60 of its own characters. CSS's `60ch` measures the wide "0" and kept a 71-character path on one line. The React tooltip of the desktop's pages (`packages/components/ui/tooltip.tsx` in Ghostex) uses the same measure.
fn text_measure(style: &StyleRefinement, window: &Window) -> Pixels {
    let rem_size = window.rem_size();
    let font_size = style.text.font_size.map_or_else(
        || rems(TOOLTIP_TEXT_REMS).to_pixels(rem_size),
        |size| size.to_pixels(rem_size),
    );
    font_size * 30.
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
    pub show_delay: Option<Duration>,
}

impl ComponentTooltip {
    /// Apply this tooltip to a `Stateful<Div>` (or any `ManagedTooltipExt` element).
    pub fn apply<E: ManagedTooltipExt>(self, el: E) -> E {
        let show_delay = self.show_delay;
        if let Some(builder) = self.builder {
            el.managed_tooltip_with(None, show_delay, move |window, cx| builder(window, cx))
        } else if let Some((text, action)) = self.text {
            el.managed_tooltip_with(None, show_delay, move |window, cx| {
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
        self.managed_tooltip_with_configuration(placement.into(), None, None, build_tooltip)
    }

    /// Attach a managed tooltip with a preferred side and its own show delay,
    /// overriding [`gpui_base::TooltipDefaults::show_delay`] for this trigger.
    fn managed_tooltip_with(
        self,
        preferred_placement: Option<Placement>,
        show_delay: Option<Duration>,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.managed_tooltip_with_configuration(
            preferred_placement.into(),
            None,
            show_delay,
            build_tooltip,
        )
    }

    /// Attach a managed tooltip that hides immediately when its trigger is
    /// left and waits for `show_delay` again when switching between triggers.
    fn managed_discrete_tooltip_with_placement(
        self,
        placement: ManagedTooltipPlacement,
        show_delay: Duration,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.managed_tooltip_with_configuration(placement, Some(show_delay), None, build_tooltip)
    }

    fn managed_tooltip_with_configuration(
        self,
        placement: ManagedTooltipPlacement,
        discrete_show_delay: Option<Duration>,
        show_delay: Option<Duration>,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        let build_tooltip = Rc::new(build_tooltip);
        let trigger_bounds_cell: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
        let bounds_writer = trigger_bounds_cell.clone();
        let request = move |bounds: Bounds<Pixels>| {
            let build = build_tooltip.clone();
            let request = BaseTooltipRequest::new(bounds, move |window, cx| build(window, cx))
                .managed_placement(placement);
            let request = match show_delay {
                Some(delay) => request.with_show_delay(delay),
                None => request,
            };
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

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc, time::Duration};

    use gpui::{
        AppContext as _, Context, Entity, InputEvent as _, InteractiveElement as _, IntoElement,
        MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement as _, Render,
        StatefulInteractiveElement as _, Styled as _, TestAppContext, VisualTestContext, Window,
        div, point, prelude::FluentBuilder as _, px,
    };

    use super::ManagedTooltipExt as _;
    use crate::Root;

    // Distinct scene markers let assertions observe retained tooltip output
    // without requiring its renderer to run on every frame.
    const TOOLTIP_COLORS: [u32; 2] = [0x7a135e, 0x136b7a];

    fn assert_tooltip_painted(cx: &mut VisualTestContext, index: usize, shown: bool) {
        draw(cx);
        let painted = cx.update(|window, _| {
            let marker: gpui::Background = gpui::rgb(TOOLTIP_COLORS[index]).into();
            window
                .painted_quads()
                .iter()
                .any(|quad| quad.background == marker)
        });
        assert_eq!(
            painted, shown,
            "tooltip {index} visibility in the rendered scene"
        );
    }

    /// A trigger that removes itself when clicked, like a button that
    /// replaces the view it sits in.
    struct Probe {
        shown: bool,
        removes_on_click: bool,
        builds: Rc<Cell<usize>>,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let builds = self.builds.clone();
            div().size_full().when(self.shown, |this| {
                this.child(
                    div()
                        .id("trigger")
                        .absolute()
                        .left(px(10.))
                        .top(px(10.))
                        .w(px(100.))
                        .h(px(30.))
                        .managed_tooltip(move |_, cx| {
                            builds.set(builds.get() + 1);
                            cx.new(|_| super::Tooltip::new("tip").bg(gpui::rgb(TOOLTIP_COLORS[0])))
                                .into()
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.removes_on_click {
                                this.shown = false;
                            }
                            cx.notify();
                        })),
                )
            })
        }
    }

    fn open(
        cx: &mut TestAppContext,
        removes_on_click: bool,
        builds: Rc<Cell<usize>>,
    ) -> (VisualTestContext, Entity<Probe>) {
        cx.update(|cx| {
            crate::init(cx);
            // These tests cover tooltip lifetime, not intermediate fade frames.
            cx.set_reduce_motion(true);
        });
        let probe = cx.new(|_| Probe {
            shown: true,
            removes_on_click,
            builds,
        });
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| Root::new(probe.clone(), window, cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        draw(&mut cx);
        (cx, probe)
    }

    /// Presses at `at` and releases a pixel away, as most real clicks do.
    fn click_with_drift(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
        let drift = point(at.x + px(1.), at.y);
        cx.simulate_mouse_down(at, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(drift, Some(MouseButton::Left), Default::default());
        cx.simulate_mouse_up(drift, MouseButton::Left, Default::default());
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        })
    }

    /// Lets any show delay or grace period run out, then draws.
    fn wait(cx: &mut VisualTestContext) {
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        draw(cx);
    }

    struct AdjacentTriggers;

    impl Render for AdjacentTriggers {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children((0..2).map(|index| {
                div()
                    .id(("trigger", index))
                    .absolute()
                    .left(px(10. + index as f32 * 100.))
                    .top(px(10.))
                    .w(px(100.))
                    .h(px(30.))
                    .managed_tooltip(move |_, cx| {
                        cx.new(|_| super::Tooltip::new("tip").bg(gpui::rgb(TOOLTIP_COLORS[index])))
                            .into()
                    })
            }))
        }
    }

    fn open_adjacent_triggers(
        cx: &mut TestAppContext,
    ) -> (VisualTestContext, Entity<AdjacentTriggers>) {
        cx.update(|cx| {
            crate::init(cx);
            // These tests cover tooltip lifetime, not intermediate fade frames.
            cx.set_reduce_motion(true);
        });
        let probe = cx.new(|_| AdjacentTriggers);
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.new(|cx| Root::new(probe.clone(), window, cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        draw(&mut cx);
        (cx, probe)
    }

    fn assert_tooltip_shown(cx: &mut VisualTestContext, index: usize) {
        assert_tooltip_painted(cx, index, true);
        assert_tooltip_painted(cx, 1 - index, false);
    }

    #[ignore = "Ghostex fork: managed tooltips use the fork's Root overlay (placements, press flash, host-driven dismissal), not upstream's WindowState trigger tracking (#3492, #3501, #3528)"]
    #[gpui::test]
    fn tooltips_switch_in_both_directions(cx: &mut TestAppContext) {
        let (mut cx, probe) = open_adjacent_triggers(cx);
        for (step, index) in [0, 1, 0].into_iter().enumerate() {
            let at = point(px(50. + index as f32 * 100.), px(25.));
            cx.simulate_mouse_move(at, None, Default::default());
            if step > 0 {
                draw(&mut cx);
                assert_tooltip_shown(&mut cx, index);
            }
            wait(&mut cx);
            assert_tooltip_shown(&mut cx, index);
            // Rebuilding the triggers must preserve ownership for hover-leave.
            cx.update(|_, cx| probe.update(cx, |_, cx| cx.notify()));
            draw(&mut cx);
        }
        cx.simulate_mouse_move(point(px(300.), px(300.)), None, Default::default());
        wait(&mut cx);
        assert_tooltip_painted(&mut cx, 0, false);
        assert_tooltip_painted(&mut cx, 1, false);
    }

    #[gpui::test]
    fn leaving_a_trigger_does_not_cancel_the_next_tooltip_delay(cx: &mut TestAppContext) {
        for (source, destination) in [(0, 1), (1, 0)] {
            let (mut cx, _) = open_adjacent_triggers(cx);
            cx.simulate_mouse_move(
                point(px(50. + source as f32 * 100.), px(25.)),
                None,
                Default::default(),
            );
            cx.executor().advance_clock(Duration::from_millis(100));
            cx.run_until_parked();
            cx.simulate_mouse_move(
                point(px(50. + destination as f32 * 100.), px(25.)),
                None,
                Default::default(),
            );
            wait(&mut cx);
            assert_tooltip_shown(&mut cx, destination);
        }
    }

    /// A press dismisses the tooltip, and it stays dismissed while the
    /// pointer stays on the trigger; leaving and coming back shows it again.
    #[ignore = "Ghostex fork: managed tooltips use the fork's Root overlay (placements, press flash, host-driven dismissal), not upstream's WindowState trigger tracking (#3492, #3501, #3528)"]
    #[gpui::test]
    fn a_press_dismisses_the_tooltip_until_the_pointer_leaves(cx: &mut TestAppContext) {
        let builds = Rc::new(Cell::new(0));
        let (mut cx, _) = open(cx, false, builds.clone());
        let at = point(px(50.), px(25.));
        cx.simulate_mouse_move(at, None, Default::default());
        wait(&mut cx);
        assert!(builds.get() > 0, "the hovered trigger shows its tooltip");

        // Hover reads false while the button is held, so the move after the
        // release is a hover-enter that must not show the tooltip.
        click_with_drift(&mut cx, at);
        cx.simulate_mouse_move(point(px(52.), px(25.)), None, Default::default());
        wait(&mut cx);
        assert_tooltip_painted(&mut cx, 0, false);

        cx.simulate_mouse_move(point(px(300.), px(300.)), None, Default::default());
        wait(&mut cx);
        cx.simulate_mouse_move(at, None, Default::default());
        wait(&mut cx);
        assert_tooltip_painted(&mut cx, 0, true);
    }

    /// A pointer move that lands between the release and the next frame is
    /// dispatched to the old frame, where the trigger is still hovered, so it
    /// used to request the tooltip again. The trigger is gone from the next
    /// frame, nothing ever sent the hover-leave, and the tooltip appeared
    /// after the show delay and stayed.
    #[ignore = "Ghostex fork: managed tooltips use the fork's Root overlay (placements, press flash, host-driven dismissal), not upstream's WindowState trigger tracking (#3492, #3501, #3528)"]
    #[gpui::test]
    fn clicking_a_trigger_away_does_not_leave_its_tooltip(cx: &mut TestAppContext) {
        let builds = Rc::new(Cell::new(0));
        let (mut cx, _) = open(cx, true, builds.clone());

        let at = point(px(50.), px(25.));
        cx.simulate_mouse_move(at, None, Default::default());
        draw(&mut cx);
        wait(&mut cx);
        draw(&mut cx);
        assert!(builds.get() > 0, "the hovered trigger shows its tooltip");

        // Press, drift a pixel while held, release, and drift again before
        // the next frame is drawn.
        cx.simulate_mouse_down(at, MouseButton::Left, Default::default());
        let drift = point(px(51.), px(25.));
        cx.simulate_mouse_move(drift, Some(MouseButton::Left), Default::default());
        // One update, so no frame is drawn between the two: the test
        // platform otherwise draws a dirty window after every event.
        cx.update(|window, cx| {
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position: drift,
                    modifiers: Default::default(),
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseMoveEvent {
                    position: point(px(52.), px(25.)),
                    pressed_button: None,
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
        });
        draw(&mut cx);
        wait(&mut cx);

        let before = builds.get();
        draw(&mut cx);
        assert_eq!(
            builds.get(),
            before,
            "no tooltip may be shown for a trigger the click removed"
        );
    }

    /// A trigger that a click removed never reports a hover-leave, so the
    /// press must end when the pointer moves away, or a trigger later shown
    /// at the same bounds would lose its tooltip.
    #[gpui::test]
    fn a_press_on_a_removed_trigger_ends_when_the_pointer_moves_away(cx: &mut TestAppContext) {
        let builds = Rc::new(Cell::new(0));
        let (mut cx, probe) = open(cx, true, builds.clone());
        let at = point(px(50.), px(25.));
        cx.simulate_mouse_move(at, None, Default::default());
        wait(&mut cx);

        cx.simulate_click(at, Default::default());
        wait(&mut cx);
        cx.simulate_mouse_move(point(px(300.), px(300.)), None, Default::default());
        wait(&mut cx);

        cx.update(|_, cx| {
            probe.update(cx, |probe, cx| {
                probe.shown = true;
                cx.notify();
            })
        });
        wait(&mut cx);
        let before = builds.get();
        cx.simulate_mouse_move(at, None, Default::default());
        wait(&mut cx);
        draw(&mut cx);
        assert!(
            builds.get() > before,
            "the trigger shown again has its tooltip"
        );
    }

    struct Buttons {
        shown: [bool; 2],
        delay: Duration,
    }

    impl Render for Buttons {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children((0..2).filter_map(|index| {
                self.shown[index].then(|| {
                    crate::button::Button::new(("button", index))
                        .absolute()
                        .left(px(10. + index as f32 * 100.))
                        .top(px(10.))
                        .w(px(80.))
                        .h(px(30.))
                        .label(if index == 0 { "A" } else { "B" })
                        .tooltip(if index == 0 { "First" } else { "Second" })
                        .tooltip_show_delay(self.delay)
                })
            }))
        }
    }

    struct CachedButtons(Entity<Buttons>);

    impl Render for CachedButtons {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.0
                    .clone()
                    .cached(gpui::StyleRefinement::default().size_full()),
            )
        }
    }

    fn open_buttons(
        cx: &mut TestAppContext,
        shown: [bool; 2],
        delay: Duration,
    ) -> (VisualTestContext, Entity<Buttons>, Rc<Cell<usize>>) {
        cx.update(|cx| {
            crate::init(cx);
            // These tests cover tooltip lifetime, not intermediate fade frames.
            cx.set_reduce_motion(true);
        });
        let buttons = cx.new(|_| Buttons { shown, delay });
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let content = cx.new(|_| CachedButtons(buttons.clone()));
                cx.new(|cx| Root::new(content, window, cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        draw(&mut cx);
        let builds = Rc::new(Cell::new(0));
        cx.update(|window, cx| {
            let builds = builds.clone();
            // Observe the real Button's overlay without changing Base's public
            // API or adding test-only markup to the production Tooltip.
            super::Root::tooltip_overlay(window, cx)
                .unwrap()
                .update(cx, |overlay, _| {
                    *overlay =
                        std::mem::take(overlay).render_with(move |view, transition, window, cx| {
                            builds.set(builds.get() + 1);
                            div()
                                .bg(gpui::rgb(TOOLTIP_COLORS[0]))
                                .child(super::render_tooltip(view, transition, window, cx))
                                .into_any_element()
                        });
                });
        });
        (cx, buttons, builds)
    }

    #[ignore = "Ghostex fork: managed tooltips use the fork's Root overlay (placements, press flash, host-driven dismissal), not upstream's WindowState trigger tracking (#3492, #3501, #3528)"]
    #[gpui::test]
    fn removing_button_hides_visible_tooltip(cx: &mut TestAppContext) {
        let (mut cx, buttons, _) = open_buttons(cx, [true, false], Duration::ZERO);
        cx.simulate_mouse_move(point(px(50.), px(25.)), None, Default::default());
        wait(&mut cx);
        assert_tooltip_painted(&mut cx, 0, true);
        // A redraw reusing the cached button subtree preserves the owner.
        draw(&mut cx);
        assert_tooltip_painted(&mut cx, 0, true);
        cx.update(|_, cx| {
            buttons.update(cx, |buttons, cx| {
                buttons.shown[0] = false;
                cx.notify();
            })
        });
        draw(&mut cx);
        cx.run_until_parked();
        assert_tooltip_painted(&mut cx, 0, false);
    }

    #[ignore = "Ghostex fork: managed tooltips use the fork's Root overlay (placements, press flash, host-driven dismissal), not upstream's WindowState trigger tracking (#3492, #3501, #3528)"]
    #[gpui::test]
    fn removing_button_cancels_pending_tooltip(cx: &mut TestAppContext) {
        let (mut cx, buttons, builds) = open_buttons(cx, [true, false], Duration::from_millis(500));
        cx.simulate_mouse_move(point(px(50.), px(25.)), None, Default::default());
        draw(&mut cx);
        // Finish the hover frame and drain its deferred cleanup before removal,
        // without advancing the pending tooltip's show delay.
        cx.run_until_parked();
        assert_eq!(builds.get(), 0, "tooltip is still pending");
        cx.update(|_, cx| {
            buttons.update(cx, |buttons, cx| {
                buttons.shown[0] = false;
                cx.notify();
            })
        });
        draw(&mut cx);
        cx.run_until_parked();
        wait(&mut cx);
        assert_eq!(
            builds.get(),
            0,
            "a removed trigger must never build its tooltip"
        );
        assert_tooltip_painted(&mut cx, 0, false);
    }

    #[gpui::test]
    fn pending_button_shows_after_deferred_work_drains(cx: &mut TestAppContext) {
        let (mut cx, _, builds) = open_buttons(cx, [true, false], Duration::from_millis(500));
        cx.simulate_mouse_move(point(px(50.), px(25.)), None, Default::default());
        draw(&mut cx);
        cx.run_until_parked();
        assert_eq!(builds.get(), 0, "tooltip is still pending");
        wait(&mut cx);
        assert!(
            builds.get() > 0,
            "a surviving trigger builds its delayed tooltip"
        );
        assert_tooltip_painted(&mut cx, 0, true);
    }

    #[gpui::test]
    fn removing_old_button_preserves_new_tooltip(cx: &mut TestAppContext) {
        let (mut cx, buttons, _) = open_buttons(cx, [true, true], Duration::ZERO);
        cx.simulate_mouse_move(point(px(50.), px(25.)), None, Default::default());
        wait(&mut cx);
        cx.simulate_mouse_move(point(px(150.), px(25.)), None, Default::default());
        wait(&mut cx);
        assert_tooltip_painted(&mut cx, 0, true);
        cx.update(|_, cx| {
            buttons.update(cx, |buttons, cx| {
                buttons.shown[0] = false;
                cx.notify();
            })
        });
        draw(&mut cx);
        cx.run_until_parked();
        assert_tooltip_painted(&mut cx, 0, true);
    }
}

//! Host-owned spans of the text drawn as compact pills instead of their source.
//!
//! Unlike an [`InlineToken`](super::InlineToken), a replacement is not part of
//! the document: the host derives it from the text on every change and sets
//! the whole list again, the buffer keeps the source (so the value, the
//! clipboard and undo are unchanged), and nothing about it enters history.
//! Layout reuses the token pipeline: a replacement is an atomic inline object
//! for wrapping, caret movement and hit testing, painted as the replacement
//! text in its own color instead of as an element.
use std::ops::Range;

use gpui::{Bounds, Context, Hsla, Pixels, Point, SharedString, px};
use ropey::Rope;

use super::{InputBaseState, InputModeKind, RopeExt as _};

/// A span of buffer text drawn as a compact glyph run instead of its own source.
///
/// CDXC:SessionChat 2026-09-18 WHY: The chat composer shows a markdown
/// reference such as `[Image #1](/tmp/shot.png)` as a short pill, so the input
/// needs a display projection: the buffer keeps the markdown source, which is
/// what gets sent, copied, and undone, while wrapping, hit testing, and
/// painting use the shortened text. Styling the source in place was tried
/// first and rejected because the path still occupied the composer.
///
/// CDXC:SessionChat 2026-09-26 WHY: Upstream's `InlineToken` was evaluated as
/// the carrier and rejected: a token is document content the input owns (an
/// edit to create it, an undo step, a click that selects it, an arrow cursor,
/// an element painted over the text), while a composer reference is derived by
/// the host from plain markdown on every change and must behave like text the
/// caret steps over. The replacement therefore keeps its host-driven API and
/// only reuses the token layout pipeline underneath.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineReplacement {
    /// Byte range of the source text this replacement stands for.
    pub range: Range<usize>,
    /// Text drawn in place of the source range.
    pub text: SharedString,
    /// Color of the replacement's glyphs and of its icon.
    pub color: Option<Hsla>,
    /// Asset path of a monochrome icon drawn over the replacement's leading space.
    pub icon: Option<SharedString>,
    /// Edge length of the icon.
    pub icon_size: Pixels,
    /// Distance from the replacement's left edge to the icon's left edge.
    pub icon_inset: Pixels,
    /// Whether hovering the replacement shows a hand cursor because a click does something.
    pub pointer: bool,
    /// Text the window's tooltip shows while the pointer rests on the replacement.
    pub tooltip: Option<SharedString>,
}

impl InlineReplacement {
    pub fn new(range: Range<usize>, text: impl Into<SharedString>) -> Self {
        Self {
            range,
            text: text.into(),
            color: None,
            icon: None,
            icon_size: px(0.),
            icon_inset: px(0.),
            pointer: false,
            tooltip: None,
        }
    }

    pub fn color(mut self, color: impl Into<Hsla>) -> Self {
        self.color = Some(color.into());
        self
    }

    pub fn icon(mut self, icon: impl Into<SharedString>, size: Pixels, inset: Pixels) -> Self {
        self.icon = Some(icon.into());
        self.icon_size = size;
        self.icon_inset = inset;
        self
    }

    pub fn pointer(mut self, pointer: bool) -> Self {
        self.pointer = pointer;
        self
    }

    /// Show `tooltip` while the pointer rests on the replacement.
    ///
    /// The source a pill stands for is not on screen, so the destination it
    /// points at is only readable through this. A replacement is painted text,
    /// not an element that could carry a tooltip, so the input reports the
    /// hovered pill to the styled control, which drives the window's managed
    /// tooltip from the pill's bounds.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// Reports the replacement tooltip to show, `(text, pill bounds)`, and the one
/// shown before it. Installed by a styled control; not part of the supported
/// API.
#[doc(hidden)]
pub type InlineReplacementTooltipHandler = std::rc::Rc<
    dyn Fn(
        Option<(SharedString, Bounds<Pixels>)>,
        Option<(SharedString, Bounds<Pixels>)>,
        &mut gpui::Window,
        &mut gpui::App,
    ),
>;

/// A replacement as measured for layout.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct InlineReplacementLayout {
    /// Source range in the buffer.
    pub(super) range: Range<usize>,
    /// The text drawn: the replacement text, ellipsized when it is wider than
    /// a row.
    pub(super) text: SharedString,
    pub(super) width: Pixels,
    pub(super) color: Option<Hsla>,
}

/// The replacements of one input.
#[derive(Default)]
pub(super) struct InlineReplacementStore {
    /// The host's list as last set, carried across edits. Kept whole, invalid
    /// entries included, so an unchanged list from the host is recognized.
    requested: Vec<InlineReplacement>,
    /// The valid replacements: ordered, disjoint, inside one line and clear of
    /// every token.
    spans: Vec<InlineReplacement>,
    /// Advances whenever `spans` changes, so layout knows to measure again.
    revision: u64,
}

impl InlineReplacementStore {
    pub(super) fn spans(&self) -> &[InlineReplacement] {
        &self.spans
    }

    pub(super) fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }

    /// Adopt the host's list. Returns whether the valid spans changed.
    fn set(
        &mut self,
        text: &Rope,
        tokens: &[Range<usize>],
        replacements: Vec<InlineReplacement>,
    ) -> bool {
        if self.requested == replacements {
            return false;
        }
        self.requested = replacements;
        self.validate(text, tokens)
    }

    /// Carry the replacements across an edit that replaced `edited` with
    /// `new_len` bytes of `text`, the text after the edit.
    ///
    /// CDXC:SessionChat 2026-09-19 WHY: Dropping every replacement on each
    /// edit and waiting for the host to set them again showed the raw markdown
    /// source for a frame whenever the host could not answer within that
    /// paint, which read as the pills flickering open while typing fast. A
    /// replacement the edit did not touch is still the same reference, so it
    /// moves with the text the way an atomic pill node does in the React
    /// composer; only one the edit cut into is dropped, and the host's next
    /// `set_inline_replacements` remains the authority.
    pub(super) fn edit(
        &mut self,
        text: &Rope,
        tokens: &[Range<usize>],
        edited: &Range<usize>,
        new_len: usize,
    ) {
        if self.requested.is_empty() {
            return;
        }
        let shift = new_len as isize - edited.len() as isize;
        self.requested.retain_mut(|replacement| {
            let range = &mut replacement.range;
            if range.end <= edited.start {
                return true;
            }
            if range.start < edited.end {
                return false;
            }
            range.start = range.start.saturating_add_signed(shift);
            range.end = range.end.saturating_add_signed(shift);
            true
        });
        self.validate(text, tokens);
    }

    /// Keep the requested replacements that describe `text`. Returns whether
    /// the valid spans changed.
    ///
    /// Replacements that overlap each other or a token, leave the text, split
    /// a character, or cross a line break are skipped: layout assumes atomic
    /// objects are ordered, disjoint, and confined to one line.
    fn validate(&mut self, text: &Rope, tokens: &[Range<usize>]) -> bool {
        let mut spans = Vec::with_capacity(self.requested.len());
        let mut cursor = 0;
        for replacement in &self.requested {
            let range = replacement.range.clone();
            let valid = range.start >= cursor
                && range.start < range.end
                && range.end <= text.len()
                && !replacement.text.is_empty()
                && text.clip_offset(range.start, sum_tree::Bias::Left) == range.start
                && text.clip_offset(range.end, sum_tree::Bias::Left) == range.end
                && !text
                    .slice(range.clone())
                    .chars()
                    .any(|c| matches!(c, '\n' | '\r'))
                && !tokens
                    .iter()
                    .any(|token| token.start < range.end && range.start < token.end);
            if !valid {
                continue;
            }
            cursor = range.end;
            spans.push(replacement.clone());
        }
        if spans == self.spans {
            return false;
        }
        self.spans = spans;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    /// `offset` moved out of the replacement it falls strictly inside, to the
    /// replacement's start (`Bias::Left`) or end.
    pub(super) fn boundary(&self, offset: usize, bias: sum_tree::Bias) -> usize {
        let ix = self.spans.partition_point(|s| s.range.end <= offset);
        match self.spans.get(ix).filter(|s| s.range.start < offset) {
            Some(span) if bias == sum_tree::Bias::Left => span.range.start,
            Some(span) => span.range.end,
            None => offset,
        }
    }
}

impl<M: InputModeKind> InputBaseState<M> {
    /// Whether the replacements are drawn: a masked or secret value never
    /// shows its references.
    pub(super) fn inline_replacements_visible(&self) -> bool {
        !self.masked
            && !self.token_is_secret()
            && self.mask_pattern.is_none()
            && !self.inline_replacements.is_empty()
    }

    fn token_ranges(&self) -> Vec<Range<usize>> {
        self.token_spans().iter().map(|span| span.range()).collect()
    }

    /// Carry the replacements across an edit of `range` to `new_len` bytes.
    pub(super) fn edit_inline_replacements(&mut self, range: &Range<usize>, new_len: usize) {
        if self.inline_replacements.requested.is_empty() {
            return;
        }
        let tokens = self.token_ranges();
        self.inline_replacements
            .edit(&self.text, &tokens, range, new_len);
    }

    fn apply_inline_replacements(
        &mut self,
        replacements: Vec<InlineReplacement>,
        cx: &mut Context<Self>,
    ) {
        let tokens = self.token_ranges();
        if self
            .inline_replacements
            .set(&self.text, &tokens, replacements)
        {
            cx.notify();
        }
    }

    fn inline_replacement_under(&self, position: Point<Pixels>) -> Option<Range<usize>> {
        if !self.inline_replacements_visible() {
            return None;
        }
        self.inline_replacement_hits
            .iter()
            .find(|(_, bounds)| bounds.contains(&position))
            .map(|(range, _)| range.clone())
    }

    /// Install the styled control's tooltip presenter for replacement
    /// tooltips. Not part of the supported API.
    #[doc(hidden)]
    pub fn install_inline_replacement_tooltip(
        &mut self,
        handler: Option<InlineReplacementTooltipHandler>,
    ) {
        self.inline_replacement_tooltip_handler = handler;
    }
}

macro_rules! inline_replacement_api {
    ($mode:ty) => {
        impl InputBaseState<$mode> {
            /// Draw the given source ranges as compact glyph runs instead of
            /// their own text.
            ///
            /// The buffer keeps the original text, so the value, the clipboard,
            /// and undo are unchanged; only wrapping, hit testing, and painting
            /// use the replacement text, and the caret steps over a replacement
            /// as one unit. Replacements an edit leaves intact move with the
            /// text and one the edit cuts into is dropped, so the caller sets
            /// them again from its own render pass to pick up references the
            /// edit created or changed. Setting an unchanged list is free.
            ///
            /// CDXC:SessionChat 2026-09-18 WHY: Ghostex composers show markdown
            /// references as pills, and doing that with an overlay or by
            /// storing the pill text in the buffer both lost the real source on
            /// copy and on send.
            pub fn set_inline_replacements(
                &mut self,
                replacements: Vec<InlineReplacement>,
                cx: &mut Context<Self>,
            ) {
                self.apply_inline_replacements(replacements, cx);
            }

            /// The replacements currently drawn, ordered by position.
            pub fn inline_replacements(&self) -> &[InlineReplacement] {
                self.inline_replacements.spans()
            }

            /// The source range of the replacement under `position`, in window
            /// coordinates, as laid out by the last paint.
            pub fn inline_replacement_at(&self, position: Point<Pixels>) -> Option<Range<usize>> {
                self.inline_replacement_under(position)
            }
        }
    };
}
inline_replacement_api!(super::InputMode);
inline_replacement_api!(super::TextareaMode);

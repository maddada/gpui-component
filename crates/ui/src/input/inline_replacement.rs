use std::ops::Range;

use gpui::{Hsla, Pixels, SharedString, px};
use ropey::Rope;

/// A span of buffer text drawn as a compact glyph run instead of its own source.
///
/// CDXC:SessionChat 2026-09-18 WHY:
/// The chat composer shows a markdown reference such as `[Image #1](/tmp/shot.png)` as a short
/// pill, so the input needs a display projection: the buffer keeps the markdown source, which is
/// what gets sent, copied, and undone, while wrapping, hit testing, and painting run on the
/// shortened string. Styling the source in place was tried first and rejected because the path
/// still occupied the composer.
/// SEE-ALSO: apps/desktop/src/app/native_chat/composer_references.rs.
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
    /// The source a pill stands for is not on screen, so the destination it points at is only
    /// readable through this; the input drives the window's managed tooltip from the pill's own
    /// hitbox because a replacement is painted text, not an element that could carry a tooltip.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// A replacement resolved against the buffer, carrying both coordinate spaces.
#[derive(Clone, Debug)]
pub(super) struct ProjectedSpan {
    /// Byte range in the buffer text.
    pub(super) source: Range<usize>,
    /// Byte range in the projected text.
    pub(super) display: Range<usize>,
    pub(super) replacement: InlineReplacement,
}

/// Buffer text projected through its inline replacements.
///
/// Empty means "no projection": every offset maps to itself and the buffer text is displayed as
/// it is stored, which keeps inputs that never set a replacement on their original code path.
pub(super) struct InlineProjection {
    replacements: Vec<InlineReplacement>,
    spans: Vec<ProjectedSpan>,
    text: Rope,
}

impl Default for InlineProjection {
    fn default() -> Self {
        Self {
            replacements: Vec::new(),
            spans: Vec::new(),
            text: Rope::new(),
        }
    }
}

impl InlineProjection {
    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    #[inline]
    pub(super) fn spans(&self) -> &[ProjectedSpan] {
        &self.spans
    }

    #[inline]
    pub(super) fn text(&self) -> &Rope {
        &self.text
    }

    #[inline]
    pub(super) fn matches(&self, replacements: &[InlineReplacement]) -> bool {
        self.replacements == replacements
    }

    /// Drop the projection. Returns whether anything was dropped.
    pub(super) fn clear(&mut self) -> bool {
        if self.replacements.is_empty() && self.spans.is_empty() {
            return false;
        }
        self.replacements.clear();
        self.spans.clear();
        self.text = Rope::new();
        true
    }

    /// Carry the projection across an edit that replaced `edited` with `inserted_len` bytes, then
    /// rebuild it from `text`. Returns whether there was a projection to carry.
    ///
    /// CDXC:SessionChat 2026-09-19 WHY:
    /// Dropping the projection on every edit and waiting for the host to set it again showed the
    /// raw markdown source for a frame whenever the host could not answer within that paint, which
    /// read as the pills flickering open while typing fast. A replacement the edit did not touch
    /// is still the same reference, so it moves with the text the way an atomic pill node does in
    /// the React composer; only a replacement the edit cut into is dropped, and the host's next
    /// `set_inline_replacements` remains the authority.
    pub(super) fn apply_edit(
        &mut self,
        text: &Rope,
        edited: &Range<usize>,
        inserted_len: usize,
    ) -> bool {
        if self.replacements.is_empty() && self.spans.is_empty() {
            return false;
        }
        let mut replacements = std::mem::take(&mut self.replacements);
        replacements.retain_mut(|replacement| {
            let range = &mut replacement.range;
            if range.end <= edited.start {
                return true;
            }
            if range.start < edited.end {
                return false;
            }
            range.start = range.start - edited.len() + inserted_len;
            range.end = range.end - edited.len() + inserted_len;
            true
        });
        self.rebuild(text, replacements);
        true
    }

    /// Rebuild the projected text from `text`.
    ///
    /// Replacements that overlap each other, leave the buffer, split a character, or cross a line
    /// break are skipped: every consumer below assumes spans are ordered, disjoint, and confined
    /// to one line so row bookkeeping stays shared between both coordinate spaces.
    pub(super) fn rebuild(&mut self, text: &Rope, replacements: Vec<InlineReplacement>) {
        self.replacements = replacements;
        self.spans.clear();
        let source = text.to_string();
        let mut projected = String::with_capacity(source.len());
        let mut cursor = 0;
        for replacement in &self.replacements {
            let range = replacement.range.clone();
            if range.start < cursor
                || range.start >= range.end
                || range.end > source.len()
                || !source.is_char_boundary(range.start)
                || !source.is_char_boundary(range.end)
                || source[range.clone()].contains('\n')
            {
                continue;
            }
            projected.push_str(&source[cursor..range.start]);
            let display_start = projected.len();
            projected.push_str(&replacement.text);
            self.spans.push(ProjectedSpan {
                source: range.clone(),
                display: display_start..projected.len(),
                replacement: replacement.clone(),
            });
            cursor = range.end;
        }
        if self.spans.is_empty() {
            self.text = Rope::new();
            return;
        }
        projected.push_str(&source[cursor..]);
        self.text = Rope::from(projected.as_str());
    }

    /// Map a buffer offset into the projected text.
    ///
    /// An offset inside a replacement resolves to the replacement's leading edge, so a caret the
    /// host placed in the middle of a reference source still paints somewhere sensible.
    pub(super) fn to_display(&self, offset: usize) -> usize {
        let mut delta: isize = 0;
        for span in &self.spans {
            if offset < span.source.start {
                break;
            }
            if offset >= span.source.end {
                delta += span.display.len() as isize - span.source.len() as isize;
                continue;
            }
            return span.display.start;
        }
        offset.saturating_add_signed(delta)
    }

    /// Map a projected offset back into the buffer.
    ///
    /// An offset inside a replacement snaps to its nearer edge, which is what makes a click land
    /// either before or after the whole reference instead of inside its hidden source.
    pub(super) fn to_buffer(&self, offset: usize) -> usize {
        let mut delta: isize = 0;
        for span in &self.spans {
            if offset < span.display.start {
                break;
            }
            if offset >= span.display.end {
                delta += span.source.len() as isize - span.display.len() as isize;
                continue;
            }
            return if offset - span.display.start <= span.display.len() / 2 {
                span.source.start
            } else {
                span.source.end
            };
        }
        offset.saturating_add_signed(delta)
    }

    /// Display ranges the soft wrap must keep whole, in order.
    pub(super) fn display_ranges(&self) -> Vec<Range<usize>> {
        self.spans.iter().map(|span| span.display.clone()).collect()
    }

    /// The source range that strictly contains `offset`, if any.
    pub(super) fn enclosing(&self, offset: usize) -> Option<Range<usize>> {
        self.spans
            .iter()
            .map(|span| span.source.clone())
            .find(|range| offset > range.start && offset < range.end)
    }
}

//! CDXC:SessionChat 2026-09-18 DECISION:
//! Double-click and drag selection in GPUI markdown must work like egoist/waku's transcript: a per-frame, document-ordered registry of painted text runs with byte-offset spans. Do not go back to hit-testing glyph positions against the rectangle between two window points.
//!
//! Document-ordered registry of the painted text runs a selection can span.
//!
//! GPUI has no text selection of its own, and a markdown `TextView` paints a
//! tree of separate [`Inline`](super::inline::Inline) elements. Every frame each
//! selectable `Inline` registers here in paint order, which is document order.
//! A selection is then an anchor `(element, byte offset)` plus a head resolved
//! against this registry, and turns into one byte range per element: partial in
//! the first and last, whole for everything between. This replaces testing each
//! glyph's position against the rectangle between two window points, which
//! selected the wrong glyphs on wrapped lines, across inline pills, and in
//! side-by-side layouts. Modelled on egoist/waku's transcript selection.

use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    sync::{Arc, Mutex},
};

use gpui::{
    Bounds, EntityId, GlobalElementId, Hitbox, Pixels, Point, SharedString, TextLayout, WeakEntity,
    Window, point, size,
};

use super::{
    TextViewState, inline::InlineState, selection::line_range_at, window_selection::SelectionScope,
};

/// Stable identity of one painted `Inline` across frames.
///
/// The element path is structural (`"p", ix` / `"code"` / …) under the owning
/// view, so it survives repaints and re-parses of the same content. `nth`
/// disambiguates the rare paths that repeat within one frame.
///
/// The path is hashed once per paint. Map lookups reuse that hash and equality
/// checks it first, so a mouse move never re-hashes or deep-compares the
/// element paths of every painted run.
#[derive(Clone, Debug)]
pub(crate) struct InlineKey {
    hash: u64,
    id: GlobalElementId,
    nth: u32,
}

impl PartialEq for InlineKey {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.nth == other.nth && self.id == other.id
    }
}

impl Eq for InlineKey {}

impl Hash for InlineKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
        state.write_u32(self.nth);
    }
}

/// The selected part of one run.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SelectedSpan {
    pub(crate) range: Range<usize>,
    /// The run's text when the span was resolved. Keys are structural, so after
    /// a re-parse the same key can name different text; see [`Self::range_in`].
    text: SharedString,
}

impl SelectedSpan {
    /// The range to highlight in a run now showing `text`, or `None` when the
    /// selected slice changed and the span is stale. Append-only growth (a
    /// streaming message) leaves the slice intact and keeps the selection.
    pub(crate) fn range_in(&self, text: &str) -> Option<Range<usize>> {
        (text.get(self.range.clone()) == self.text.get(self.range.clone()))
            .then(|| self.range.clone())
    }
}

/// One painted `Inline` as seen by the current frame.
pub(crate) struct RegisteredInline {
    pub(crate) key: InlineKey,
    pub(crate) view: WeakEntity<TextViewState>,
    pub(crate) view_id: EntityId,
    pub(crate) scope: SelectionScope,
    pub(crate) text: SharedString,
    pub(crate) layout: TextLayout,
    pub(crate) hitbox: Hitbox,
    /// The part of the element left visible by its content mask.
    pub(crate) visible: Bounds<Pixels>,
    pub(crate) state: Arc<Mutex<InlineState>>,
    /// Set on the wrapped fragments of one `InlineFlow` paragraph, which
    /// together hold the paragraph's text in document order.
    pub(crate) flow: Option<usize>,
}

impl RegisteredInline {
    /// Byte offset nearest to `position`; `Ok` only when it lands on a glyph.
    pub(crate) fn offset_for(&self, position: Point<Pixels>) -> Result<usize, usize> {
        self.layout.index_for_position(position)
    }
}

/// The frame's document-ordered text elements. Cleared when Root starts
/// painting, before any `Inline` paints.
#[derive(Default)]
pub(crate) struct SelectionRegistry {
    entries: Vec<RegisteredInline>,
    /// Paint count per element-path hash this frame, the source of `nth`.
    seen: HashMap<u64, u32>,
}

impl SelectionRegistry {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.seen.clear();
    }

    pub(crate) fn key_for(&mut self, id: &GlobalElementId) -> InlineKey {
        let mut hasher = DefaultHasher::new();
        id.hash(&mut hasher);
        let hash = hasher.finish();
        let nth = self.seen.entry(hash).or_default();
        let key = InlineKey {
            hash,
            id: id.clone(),
            nth: *nth,
        };
        *nth += 1;
        key
    }

    pub(crate) fn push(&mut self, entry: RegisteredInline) {
        self.entries.push(entry);
    }

    pub(crate) fn entries(&self) -> &[RegisteredInline] {
        &self.entries
    }

    pub(crate) fn position(&self, key: &InlineKey) -> Option<usize> {
        self.entries.iter().position(|entry| entry.key == *key)
    }

    /// The entry whose hitbox is under the mouse in `scope`, with the offset
    /// under `position` and whether it lands on a glyph.
    pub(crate) fn hit(
        &self,
        scope: SelectionScope,
        position: Point<Pixels>,
        window: &Window,
    ) -> Option<(usize, usize, bool)> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, entry)| {
                entry.scope == scope
                    && entry.visible.contains(&position)
                    && entry.hitbox.is_hovered(window)
            })
            .map(|(index, entry)| match entry.offset_for(position) {
                Ok(offset) => (index, offset, true),
                Err(offset) => (index, offset, false),
            })
    }

    /// The visible entry in `scope` nearest to `position`: vertical distance
    /// first, then horizontal, so a drag through a gutter, between blocks, or
    /// beside a table cell clamps to the text a reader would expect. Returns the
    /// offset and whether `position` is on a glyph.
    pub(crate) fn nearest(
        &self,
        scope: SelectionScope,
        position: Point<Pixels>,
    ) -> Option<(usize, usize, bool)> {
        let mut best: Option<(usize, (f32, f32))> = None;
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.scope != scope || entry.visible.size.width <= Pixels::ZERO {
                continue;
            }
            let bounds = entry.visible;
            let dy = axis_distance(position.y, bounds.top(), bounds.bottom());
            let dx = axis_distance(position.x, bounds.left(), bounds.right());
            if best.is_none_or(|(_, best)| (dy, dx) < best) {
                best = Some((index, (dy, dx)));
            }
            if dy == 0.0 && dx == 0.0 {
                break;
            }
        }
        let (index, _) = best?;
        Some(match self.entries[index].offset_for(position) {
            Ok(offset) => (index, offset, true),
            Err(offset) => (index, offset, false),
        })
    }

    /// The hard line around `offset` in entry `index`, for a triple click, as
    /// document-ordered `(entry index, byte offset)` points. A paragraph laid
    /// out by `InlineFlow` paints one entry per wrapped fragment, so the line
    /// continues through the neighbouring entries of the same flow until a
    /// newline; any other run is a line on its own.
    pub(crate) fn line_bounds(
        &self,
        index: usize,
        offset: usize,
    ) -> ((usize, usize), (usize, usize)) {
        let entry = &self.entries[index];
        let local = line_range_at(&entry.text, offset);
        let same_flow = |other: &RegisteredInline| {
            entry.flow.is_some() && other.flow == entry.flow && other.scope == entry.scope
        };

        let mut start = (index, local.start);
        if local.start == 0 {
            for prev in (0..index).rev() {
                let prev_entry = &self.entries[prev];
                if !same_flow(prev_entry) {
                    break;
                }
                if let Some(newline) = prev_entry.text.rfind('\n') {
                    start = (prev, newline + 1);
                    break;
                }
                start = (prev, 0);
            }
        }

        let mut end = (index, local.end);
        if local.end == entry.text.len() {
            for next in index + 1..self.entries.len() {
                let next_entry = &self.entries[next];
                if !same_flow(next_entry) {
                    break;
                }
                if let Some(newline) = next_entry.text.find('\n') {
                    end = (next, newline);
                    break;
                }
                end = (next, next_entry.text.len());
            }
        }

        (start, end)
    }

    /// Per-element ranges for the document-ordered span `start..end`, where
    /// each point is `(entry index, byte offset)`. Elements of other scopes are
    /// skipped so text behind a modal never joins the selection.
    pub(crate) fn resolve(
        &self,
        scope: SelectionScope,
        start: (usize, usize),
        end: (usize, usize),
    ) -> HashMap<InlineKey, SelectedSpan> {
        let mut spans = HashMap::new();
        let Some(last) = self.entries.len().checked_sub(1) else {
            return spans;
        };
        for index in start.0..=end.0.min(last) {
            let entry = &self.entries[index];
            if entry.scope != scope {
                continue;
            }
            let from = if index == start.0 { start.1 } else { 0 };
            let to = if index == end.0 {
                end.1
            } else {
                entry.text.len()
            };
            let from = clamp_boundary(&entry.text, from);
            let to = clamp_boundary(&entry.text, to);
            if from < to {
                spans.insert(
                    entry.key.clone(),
                    SelectedSpan {
                        range: from..to,
                        text: entry.text.clone(),
                    },
                );
            }
        }
        spans
    }
}

fn axis_distance(value: Pixels, low: Pixels, high: Pixels) -> f32 {
    if value < low {
        f32::from(low - value)
    } else if value > high {
        f32::from(value - high)
    } else {
        0.0
    }
}

/// Clamp a byte offset into `text` and snap it down to a char boundary.
pub(crate) fn clamp_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Highlight boxes for a byte range, one per visual row, in window coordinates.
///
/// A soft-wrap boundary belongs to both adjacent rows, but `position_for_index`
/// gives it the preceding row, so walking with it drops or misplaces the first
/// glyph of each continuation row. The shaped wrap boundaries are used directly
/// instead (as Zed's and waku's markdown renderers do), so adjacent rows share
/// the exact byte boundary and the boxes tile without gaps.
pub(crate) fn range_rects(layout: &TextLayout, range: &Range<usize>) -> Vec<Bounds<Pixels>> {
    let mut rects = Vec::new();
    let lines = layout.line_layouts();
    if range.is_empty() || lines.is_empty() {
        return rects;
    }

    let bounds = layout.bounds();
    let line_height = layout.line_height();
    let mut row_top = bounds.top();
    let mut line_start = 0;

    for line in lines.iter() {
        let line_end = line_start + line.len();
        let unwrapped = &line.unwrapped_layout;
        let row_ends = line
            .wrap_boundaries()
            .iter()
            .map(|boundary| {
                let glyph = &unwrapped.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
                (line_start + glyph.index, glyph.position.x)
            })
            .chain([(line_end, unwrapped.width)]);
        let mut row_start = line_start;
        let mut row_start_x = Pixels::ZERO;

        for (row_end, row_end_x) in row_ends {
            let selected_start = range.start.max(row_start);
            let selected_end = range.end.min(row_end);
            if selected_start < selected_end {
                let x_for_index =
                    |index| bounds.left() + unwrapped.x_for_index(index - line_start) - row_start_x;
                let start_x = x_for_index(selected_start);
                let end_x = x_for_index(selected_end);
                if end_x > start_x {
                    rects.push(Bounds::new(
                        point(start_x, row_top),
                        size(end_x - start_x, line_height),
                    ));
                }
            }
            row_start = row_end;
            row_start_x = row_end_x;
            row_top += line_height;
        }

        // Hard lines are separated by one newline byte with no glyph box.
        line_start = line_end + 1;
        if line_start > range.end {
            break;
        }
    }
    rects
}

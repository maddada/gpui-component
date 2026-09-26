//! CDXC:SessionChat 2026-09-18 DECISION:
//! Double-click and drag selection in GPUI markdown must work like egoist/waku's transcript: a per-frame, document-ordered registry of painted text runs with byte-offset spans. Do not go back to hit-testing glyph positions against the rectangle between two window points.
//!
//! Document-ordered registry of the painted text runs a TextView selection can
//! span.
//!
//! A markdown `TextView` paints a tree of separate `Inline` elements. Every
//! frame each selectable `Inline` registers here, through its view's selection
//! participant, in paint order, which is document order. A selection is then an
//! anchor `(run, byte offset)` plus a head resolved against this registry, and
//! turns into one byte range per source text: partial in the first and last,
//! whole for everything between. Testing each glyph's position against the
//! rectangle between two window points instead selected the wrong glyphs on
//! wrapped lines, across inline chips, and in side-by-side layouts such as
//! table cells. Modelled on egoist/waku's transcript selection.
//!
//! Offsets live in the byte space of a run's *source* text (a paragraph's run
//! of text, a table cell, a code block), not of the painted fragment: a
//! paragraph wraps into one run per line, and a new width, a streamed chunk or
//! a re-parse re-wraps it, while the source text and its byte offsets stay put.
//! A source is known by the block it starts in, so a selection survives a
//! re-parse of the same content, and a span is dropped when the text it covered
//! changed underneath it.

use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Mutex},
};

use gpui::{
    Bounds, EntityId, Hitbox, Pixels, Point, SharedString, TextLayout, WeakEntity, Window,
    WrappedLineLayout,
};
use smallvec::SmallVec;

use crate::{
    TextSelectionScopeId,
    text::{InlineState, TextLeafKey, TextViewState},
    text_boundary::{line_range_at, word_range_at},
};

/// Which source text a painted run is a fragment of, within its view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RunSource {
    /// A text leaf of a parsed Markdown document (a paragraph, a heading, a
    /// table cell, a code block), and the run of it between two images or
    /// inline objects. Known by where it starts in the source, so it is the
    /// same leaf after a re-parse of the same content.
    Leaf(TextLeafKey, usize),
    /// Text with no source position (HTML), known by its state's address.
    State(usize),
}

/// The stable identity of a run's source text across frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RunKey {
    pub(crate) view: EntityId,
    pub(crate) source: RunSource,
}

/// One painted text run as the last frame saw it.
#[derive(Clone)]
pub(crate) struct TextRun {
    pub(crate) key: RunKey,
    pub(crate) view: WeakEntity<TextViewState>,
    /// Where this fragment's text sits in its source text.
    pub(crate) source_range: Range<usize>,
    /// The run stands for its whole `source_range` rather than byte for byte:
    /// a reference chip, an inline object.
    pub(crate) atomic: bool,
    /// The painted text.
    pub(crate) text: SharedString,
    /// Where its glyphs were painted; `None` for an inline object, which has
    /// no glyphs of its own.
    pub(crate) geometry: Option<RunGeometry>,
    pub(crate) hitbox: Hitbox,
    /// The part of the run left visible by its content mask.
    pub(crate) visible: Bounds<Pixels>,
    /// Where copy reads the selection from.
    pub(crate) target: RunTarget,
    pub(crate) source_text: SharedString,
    /// The inline flow (one paragraph) this run is a wrapped fragment of.
    pub(crate) flow: Option<usize>,
}

/// A painted run's laid-out lines, as they were painted.
///
/// Kept as a copy rather than as the element's `TextLayout`: a layout is
/// measured again at the start of the next frame, and until that frame
/// prepaints it has no position to answer from.
#[derive(Clone)]
pub(crate) struct RunGeometry {
    origin: Point<Pixels>,
    line_height: Pixels,
    lines: SmallVec<[Arc<WrappedLineLayout>; 1]>,
}

impl RunGeometry {
    /// Reads a painted `layout`. Call from paint, after the layout's prepaint.
    pub(crate) fn new(layout: &TextLayout) -> Self {
        Self {
            origin: layout.bounds().origin,
            line_height: layout.line_height(),
            lines: layout.line_layouts(),
        }
    }

    /// The byte index at `position`, as [`TextLayout::index_for_position`]
    /// answers it: `Ok` on a glyph (the glyph's own index, or the nearest
    /// boundary with `closest`), `Err` with the nearest index otherwise.
    fn index(&self, position: Point<Pixels>, closest: bool) -> Result<usize, usize> {
        if position.y < self.origin.y {
            return Err(0);
        }
        let mut line_origin = self.origin;
        let mut line_start = 0;
        for line in &self.lines {
            let line_bottom = line_origin.y + line.size(self.line_height).height;
            if position.y > line_bottom {
                line_origin.y = line_bottom;
                line_start += line.len() + 1;
                continue;
            }
            let within = position - line_origin;
            let index = if closest {
                line.closest_index_for_position(within, self.line_height)
            } else {
                line.index_for_position(within, self.line_height)
            };
            return match index {
                Ok(index) => Ok(line_start + index),
                Err(index) => Err(line_start + index),
            };
        }
        Err(line_start.saturating_sub(1))
    }
}

/// Where a position falls in a run: the caret boundary nearest to it, the
/// glyph under it (what a double or triple click picks its unit from), and
/// whether it is on a glyph at all.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RunHit {
    pub(crate) caret: DocPoint,
    pub(crate) glyph: DocPoint,
    pub(crate) on_glyph: bool,
}

impl TextRun {
    fn hit(&self, index: usize, position: Point<Pixels>) -> RunHit {
        let (caret, glyph, on_glyph) = match &self.geometry {
            Some(geometry) => {
                let glyph = geometry.index(position, false);
                let caret = geometry.index(position, true);
                let unwrap = |index: Result<usize, usize>| match index {
                    Ok(index) | Err(index) => index.min(self.text.len()),
                };
                (unwrap(caret), unwrap(glyph), glyph.is_ok())
            }
            None => {
                let offset = if position.x < self.hitbox.bounds.center().x {
                    0
                } else {
                    self.text.len()
                };
                (offset, 0, self.hitbox.bounds.contains(&position))
            }
        };
        RunHit {
            caret: DocPoint {
                index,
                local: caret,
            },
            glyph: DocPoint {
                index,
                local: glyph,
            },
            on_glyph,
        }
    }

    /// The source offset of `local`, an offset into the painted text.
    fn source_offset(&self, local: usize) -> usize {
        if self.atomic {
            if local == 0 {
                self.source_range.start
            } else {
                self.source_range.end
            }
        } else {
            (self.source_range.start + local).min(self.source_range.end)
        }
    }

    /// The offset into the painted text of `source`, a source offset inside
    /// this run.
    fn local_offset(&self, source: usize) -> usize {
        if self.atomic {
            if source <= self.source_range.start {
                0
            } else {
                self.text.len()
            }
        } else {
            source
                .saturating_sub(self.source_range.start)
                .min(self.text.len())
        }
    }
}

/// Where a selection of a run is written for copy to read before the next
/// paint: the state holding a run's source text, or an inline object's
/// selected flag.
#[derive(Clone, Debug)]
pub(crate) enum RunTarget {
    Text(Arc<Mutex<InlineState>>),
    Object(Arc<Mutex<bool>>),
}

impl RunTarget {
    fn write(&self, range: Option<Range<usize>>) {
        match self {
            RunTarget::Text(state) => {
                if let Ok(mut state) = state.lock() {
                    state.selection = range
                        .filter(|range| state.text.get(range.clone()).is_some())
                        .map(Into::into);
                }
            }
            RunTarget::Object(selected) => {
                if let Ok(mut selected) = selected.lock() {
                    *selected = range.is_some();
                }
            }
        }
    }
}

/// The runs one participant (one selectable `TextView`) painted last frame.
pub(crate) struct RunGroup {
    pub(crate) order: u64,
    pub(crate) scope: TextSelectionScopeId,
    pub(crate) runs: Vec<TextRun>,
}

/// A point of a selection held across frames: a source text and a byte
/// offset in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RunPoint {
    pub(crate) key: RunKey,
    pub(crate) offset: usize,
}

/// A point of this frame's document: the index of a run in [`Document::runs`]
/// and an offset into its painted text. Compared in document order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DocPoint {
    pub(crate) index: usize,
    pub(crate) local: usize,
}

/// How far a drag extends from its anchor: by character, or by the word or
/// line a double or triple click picked, as browsers and Zed do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Granularity {
    #[default]
    Character,
    Word,
    Line,
}

/// The selected part of one source text.
#[derive(Clone, Debug)]
pub(crate) struct Span {
    pub(crate) range: Range<usize>,
    /// The source text when the span was resolved: a later frame keeps the
    /// span only while the text it covers is unchanged. Append-only growth (a
    /// streaming reply) leaves it intact.
    pub(crate) text: SharedString,
    pub(crate) participant: EntityId,
    pub(crate) view: WeakEntity<TextViewState>,
    pub(crate) target: RunTarget,
}

impl PartialEq for Span {
    fn eq(&self, other: &Self) -> bool {
        self.range == other.range && self.participant == other.participant
    }
}

impl Span {
    /// The range to paint in a source now holding `text`, or `None` when the
    /// selected slice changed and the span is stale.
    pub(crate) fn range_in(&self, text: &str) -> Option<Range<usize>> {
        (text.get(self.range.clone()) == self.text.get(self.range.clone()))
            .then(|| self.range.clone())
    }
}

/// The document of one active scope, in order: every run with the
/// participant that painted it.
pub(crate) struct Document<'a> {
    pub(crate) runs: Vec<(EntityId, &'a TextRun)>,
}

impl<'a> Document<'a> {
    pub(crate) fn new(
        groups: &'a HashMap<EntityId, RunGroup>,
        scope: TextSelectionScopeId,
    ) -> Self {
        let mut ordered = groups
            .iter()
            .filter(|(_, group)| group.scope == scope)
            .collect::<Vec<_>>();
        ordered.sort_by_key(|(participant, group)| (group.order, participant.as_u64()));
        Self {
            runs: ordered
                .into_iter()
                .flat_map(|(participant, group)| {
                    group.runs.iter().map(move |run| (*participant, run))
                })
                .collect(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// The run under the mouse, and where `position` falls in it.
    pub(crate) fn hit(&self, position: Point<Pixels>, window: &Window) -> Option<RunHit> {
        self.runs
            .iter()
            .enumerate()
            .find(|(_, (_, run))| run.visible.contains(&position) && run.hitbox.is_hovered(window))
            .map(|(index, (_, run))| run.hit(index, position))
    }

    /// The run nearest to `position`: vertical distance first, then
    /// horizontal, so a drag through a gutter, between blocks, or beside a
    /// table cell clamps to the text a reader would expect.
    ///
    /// A run clipped by a scrolled container still counts: every painted run
    /// is laid out, so a drag past the edge of the viewport selects through
    /// the text beyond it rather than stopping at what is on screen. A press
    /// only lands on visible text ([`Self::hit`]).
    pub(crate) fn nearest(&self, position: Point<Pixels>) -> Option<RunHit> {
        let mut best: Option<(usize, (f32, f32))> = None;
        for (index, (_, run)) in self.runs.iter().enumerate() {
            let bounds = run.hitbox.bounds;
            if bounds.size.width <= Pixels::ZERO || bounds.size.height <= Pixels::ZERO {
                continue;
            }
            let dy = axis_distance(position.y, bounds.top(), bounds.bottom());
            let dx = axis_distance(position.x, bounds.left(), bounds.right());
            if best.is_none_or(|(_, best)| (dy, dx) < best) {
                best = Some((index, (dy, dx)));
            }
            if dy == 0. && dx == 0. {
                break;
            }
        }
        let (index, _) = best?;
        Some(self.runs[index].1.hit(index, position))
    }

    /// The document point of a point held across frames, when its run was
    /// painted.
    pub(crate) fn locate(&self, point: RunPoint) -> Option<DocPoint> {
        let mut found = None;
        for (index, (_, run)) in self.runs.iter().enumerate() {
            if run.key != point.key
                || point.offset < run.source_range.start
                || point.offset > run.source_range.end
            {
                continue;
            }
            let local = run.local_offset(point.offset);
            // A boundary between two fragments belongs to the later one, where
            // the text it starts continues.
            if point.offset < run.source_range.end || found.is_none() {
                found = Some(DocPoint { index, local });
            }
            if point.offset < run.source_range.end {
                break;
            }
        }
        found
    }

    /// The point held across frames for a document point.
    pub(crate) fn run_point(&self, point: DocPoint) -> RunPoint {
        let run = self.runs[point.index].1;
        RunPoint {
            key: run.key,
            offset: run.source_offset(point.local),
        }
    }

    pub(crate) fn participant(&self, point: DocPoint) -> EntityId {
        self.runs[point.index].0
    }

    pub(crate) fn run(&self, point: DocPoint) -> &'a TextRun {
        self.runs[point.index].1
    }

    /// The unit around a hit for a multi-click: the word for a double
    /// click, the line for a triple click, the caret itself otherwise. A
    /// paragraph laid out by an inline flow paints one run per wrapped line,
    /// so a line continues through the neighbouring runs of the same flow
    /// until a newline.
    pub(crate) fn unit(&self, hit: RunHit, granularity: Granularity) -> (DocPoint, DocPoint) {
        let point = hit.glyph;
        let run = self.run(point);
        match granularity {
            Granularity::Character => (hit.caret, hit.caret),
            Granularity::Word if run.atomic => (
                DocPoint {
                    index: point.index,
                    local: 0,
                },
                DocPoint {
                    index: point.index,
                    local: run.text.len(),
                },
            ),
            Granularity::Word => {
                let word =
                    word_range_at(&run.text, point.local).unwrap_or(point.local..point.local);
                (
                    DocPoint {
                        index: point.index,
                        local: word.start,
                    },
                    DocPoint {
                        index: point.index,
                        local: word.end,
                    },
                )
            }
            Granularity::Line => self.line_bounds(point),
        }
    }

    fn line_bounds(&self, point: DocPoint) -> (DocPoint, DocPoint) {
        let index = point.index;
        let run = self.run(point);
        let local = line_range_at(&run.text, point.local);
        let same_flow = |other: &TextRun| run.flow.is_some() && other.flow == run.flow;

        let mut start = DocPoint {
            index,
            local: local.start,
        };
        if local.start == 0 {
            for previous in (0..index).rev() {
                let other = self.runs[previous].1;
                if !same_flow(other) {
                    break;
                }
                if let Some(newline) = other.text.rfind('\n') {
                    start = DocPoint {
                        index: previous,
                        local: newline + 1,
                    };
                    break;
                }
                start = DocPoint {
                    index: previous,
                    local: 0,
                };
            }
        }

        let mut end = DocPoint {
            index,
            local: local.end,
        };
        if local.end == run.text.len() {
            for next in index + 1..self.runs.len() {
                let other = self.runs[next].1;
                if !same_flow(other) {
                    break;
                }
                if let Some(newline) = other.text.find('\n') {
                    end = DocPoint {
                        index: next,
                        local: newline,
                    };
                    break;
                }
                end = DocPoint {
                    index: next,
                    local: other.text.len(),
                };
            }
        }
        (start, end)
    }

    /// The spans of the document-ordered selection `start..end`: one byte
    /// range per source text, partial where the selection starts and ends,
    /// whole for every run between.
    pub(crate) fn resolve(&self, start: DocPoint, end: DocPoint) -> HashMap<RunKey, Span> {
        let mut spans: HashMap<RunKey, Span> = HashMap::new();
        if self.runs.is_empty() || start >= end {
            return spans;
        }
        let last = end.index.min(self.runs.len() - 1);
        for index in start.index..=last {
            let (participant, run) = self.runs[index];
            let from = if index == start.index { start.local } else { 0 };
            let to = if index == end.index {
                end.local
            } else {
                run.text.len()
            };
            if from >= to {
                continue;
            }
            let range = if run.atomic {
                run.source_range.clone()
            } else {
                run.source_offset(from)..run.source_offset(to)
            };
            if range.is_empty() {
                continue;
            }
            spans
                .entry(run.key)
                .and_modify(|span| {
                    span.range.start = span.range.start.min(range.start);
                    span.range.end = span.range.end.max(range.end);
                })
                .or_insert_with(|| Span {
                    range,
                    text: run.source_text.clone(),
                    participant,
                    view: run.view.clone(),
                    target: run.target.clone(),
                });
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
        0.
    }
}

/// Where a run selection was anchored: the unit the press picked, held across
/// frames so the selection follows its text when a container scrolls, the
/// window resizes or a reply streams in.
#[derive(Clone, Copy)]
pub(crate) struct RunAnchor {
    pub(crate) start: RunPoint,
    pub(crate) end: RunPoint,
    pub(crate) participant: EntityId,
    /// True when the press hit the anchor's view, false when it landed in
    /// blank space and was proxied to the nearest run. Only a true hit
    /// focuses the view and auto-scrolls it.
    pub(crate) inside: bool,
}

/// A selection of `TextView` text, made against the run registry.
#[derive(Default)]
pub(crate) struct RunSelection {
    pub(crate) anchor: Option<RunAnchor>,
    pub(crate) granularity: Granularity,
    /// Whether the press or the drag touched a glyph. A drag that stays in
    /// blank space or a gutter selects nothing.
    pub(crate) did_hit_text: bool,
    pub(crate) spans: HashMap<RunKey, Span>,
    /// Where this selection wrote a range, so copy sees it before the next
    /// paint, and a later resolve or clear resets them.
    pub(crate) written: Vec<RunTarget>,
    /// The anchor a shift-press extends from, kept across the press's
    /// capture-phase clear.
    pub(crate) pending_extension: Option<RunAnchor>,
    /// The participants the current gesture focused.
    pub(crate) focused: Vec<EntityId>,
}

impl RunSelection {
    pub(crate) fn has_spans(&self) -> bool {
        !self.spans.is_empty()
    }

    pub(crate) fn covers(&self, participant: EntityId) -> bool {
        self.spans
            .values()
            .any(|span| span.participant == participant)
    }

    /// Forgets the selection, returning the views whose painted selection
    /// changed.
    pub(crate) fn clear(&mut self) -> Vec<WeakEntity<TextViewState>> {
        self.anchor = None;
        self.granularity = Granularity::Character;
        self.did_hit_text = false;
        self.reset_written();
        std::mem::take(&mut self.spans)
            .into_values()
            .map(|span| span.view)
            .collect()
    }

    fn reset_written(&mut self) {
        for target in self.written.drain(..) {
            target.write(None);
        }
    }

    /// Replaces the spans: writes them into their source states (so copy sees
    /// them before the next paint) and returns the views whose highlight
    /// changed. A long drag leaves the fully covered views between the anchor
    /// and the pointer alone.
    pub(crate) fn set_spans(
        &mut self,
        spans: HashMap<RunKey, Span>,
    ) -> Vec<WeakEntity<TextViewState>> {
        let mut changed: HashMap<EntityId, WeakEntity<TextViewState>> = HashMap::new();
        for (key, span) in &spans {
            if self.spans.get(key) != Some(span) {
                changed.insert(key.view, span.view.clone());
            }
        }
        for (key, span) in &self.spans {
            if !spans.contains_key(key) {
                changed.insert(key.view, span.view.clone());
            }
        }
        if changed.is_empty() {
            return Vec::new();
        }
        self.reset_written();
        for span in spans.values() {
            span.target.write(Some(span.range.clone()));
            self.written.push(span.target.clone());
        }
        self.spans = spans;
        changed.into_values().collect()
    }

    /// The selected part of a painted run: `source_range` of the source text
    /// `key` names, now holding `source_text`, as an offset range into the
    /// run's own text of `len` bytes.
    pub(crate) fn range_for(
        &self,
        key: RunKey,
        source_range: &Range<usize>,
        source_text: &str,
        atomic: bool,
        len: usize,
    ) -> Option<Range<usize>> {
        let span = self.spans.get(&key)?.range_in(source_text)?;
        let start = span.start.max(source_range.start);
        let end = span.end.min(source_range.end);
        if start >= end {
            return None;
        }
        Some(if atomic {
            0..len
        } else {
            let start = start - source_range.start;
            let end = (end - source_range.start).min(len);
            start..end
        })
    }
}

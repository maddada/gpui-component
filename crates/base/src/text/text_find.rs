//! Find-bar highlighting: every occurrence of a query in the rendered text,
//! one of them drawn as the current match.
//!
//! A [`TextView`](super::TextView) given a [`TextFind`] searches its own
//! [`RenderedText`](super::RenderedText), case-insensitively, and paints the
//! occurrences as [range highlights](super::RangeHighlight). It searches again
//! only when the query or its text changes, so a find can stay on a view that
//! is laid out every frame.
//!
//! Views laid out one after another (the paragraphs, tables and quotes of one
//! message, say) can share a counter, so the current match is numbered across
//! all of them: each view adds its count as it is laid out and numbers its own
//! occurrences after the ones before it. Create a fresh counter each time the
//! views are built.

use std::{cell::Cell, ops::Range, rc::Rc};

use gpui::{Hsla, SharedString};

/// The find a [`TextView`](super::TextView) draws.
#[derive(Clone)]
pub struct TextFind {
    pub(super) query: SharedString,
    pub(super) background: Hsla,
    pub(super) active_background: Hsla,
    pub(super) counter: Rc<Cell<usize>>,
    pub(super) active: Option<usize>,
    pub(super) reveal: Option<u64>,
}

impl TextFind {
    /// Highlights every case-insensitive occurrence of `query` (trimmed)
    /// with `background`; the current match takes `active_background`.
    pub fn new(
        query: impl Into<SharedString>,
        background: impl Into<Hsla>,
        active_background: impl Into<Hsla>,
    ) -> Self {
        Self {
            query: query.into(),
            background: background.into(),
            active_background: active_background.into(),
            counter: Rc::default(),
            active: None,
            reveal: None,
        }
    }

    /// Numbers this view's occurrences after those of the views laid out
    /// before it with the same counter, and adds its own.
    pub fn counter(mut self, counter: Rc<Cell<usize>>) -> Self {
        self.counter = counter;
        self
    }

    /// The current match, counted across the views sharing the counter.
    pub fn active(mut self, index: Option<usize>) -> Self {
        self.active = index;
        self
    }

    /// Scrolls the current match into view, once for each new `token`, as
    /// [`TextViewState::reveal_range`](super::TextViewState::reveal_range)
    /// does.
    pub fn reveal(mut self, token: u64) -> Self {
        self.reveal = Some(token);
        self
    }
}

/// The byte ranges of every occurrence of `needle`, which is already
/// lowercase, in `text` compared lowercase, without overlaps.
pub(super) fn find_occurrences(text: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    if text.is_ascii() {
        let lower = text.to_ascii_lowercase();
        return lower
            .match_indices(needle)
            .map(|(start, found)| start..start + found.len())
            .collect();
    }
    // Lowercasing can change a character's length, so remember where each
    // byte of the lowercase text came from.
    let mut lower = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len() + 1);
    for (at, ch) in text.char_indices() {
        let start = lower.len();
        lower.extend(ch.to_lowercase());
        origin.resize(origin.len() + lower.len() - start, at);
    }
    origin.push(text.len());
    lower
        .match_indices(needle)
        .filter_map(|(start, found)| {
            let range = origin[start]..origin[start + found.len()];
            (!range.is_empty()).then_some(range)
        })
        .collect()
}

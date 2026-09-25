use instant::{Duration, Instant};

use super::{Selection, change::Change};

/// A pause longer than this ends the current undo step, even mid-word.
const PAUSE_BREAKS_STEP_AFTER: Duration = Duration::from_secs(1);
/// Oldest steps are dropped whole once the stack is this deep.
const MAX_UNDO_STEPS: usize = 500;

/// How one edit joins the undo stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepPolicy {
    /// Join the open step when it is the same kind of edit, adjacent to the last one, recent,
    /// and (for typing) not the first character of a new word.
    Coalesce(EditKind),
    /// Join the open step unconditionally (the updates of one IME composition).
    Continue,
    /// A step of its own (paste, cut, replacing a selection, programmatic edits).
    Separate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditKind {
    Typing,
    Deleting,
}

/// One user-visible undo step: every change in it is undone or redone together, and the
/// selection is put back where it was before (undo) or after (redo) the step.
#[derive(Debug, Clone)]
pub(super) struct UndoStep {
    /// In the order they were applied.
    pub(super) changes: Vec<Change>,
    pub(super) selection_before: Selection,
    pub(super) selection_after: Selection,
    kind: Option<EditKind>,
    last_edit_at: Instant,
}

/// The undo history of one text input.
///
/// CDXC:UndoRedo 2026-09-25 WHY: The generic time-grouped `History` made one undo erase everything typed without a one-second pause (a whole prompt), merged a paste into the typing before it, never cleared redo on a new edit (redo then replayed stale edits into new text), and could pop edits from other steps. Steps here are split at word starts, edit-kind changes, non-adjacent edits, pastes and programmatic edits, and restore the selection, the way text editors do.
#[derive(Debug)]
pub(super) struct EditHistory {
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    /// The last undo step may still absorb edits.
    open: bool,
    /// Set while undo/redo replays changes, so the replay is not recorded.
    pub(super) ignore: bool,
}

impl EditHistory {
    pub(super) fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            open: false,
            ignore: false,
        }
    }

    pub(super) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open = false;
    }

    /// The next edit starts a new step.
    pub(super) fn close_step(&mut self) {
        self.open = false;
    }

    #[cfg(test)]
    pub(super) fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub(super) fn record(
        &mut self,
        change: Change,
        selection_before: Selection,
        selection_after: Selection,
        policy: StepPolicy,
    ) {
        if self.ignore {
            return;
        }
        self.redo.clear();
        let now = Instant::now();

        if let Some(step) = self.undo.last_mut()
            && self.open
            && step_accepts(step, &change, policy, now)
        {
            step.changes.push(change);
            step.selection_after = selection_after;
            step.last_edit_at = now;
            self.open = policy != StepPolicy::Separate;
            return;
        }

        if self.undo.len() >= MAX_UNDO_STEPS {
            self.undo.remove(0);
        }
        self.undo.push(UndoStep {
            changes: vec![change],
            selection_before,
            selection_after,
            kind: match policy {
                StepPolicy::Coalesce(kind) => Some(kind),
                StepPolicy::Continue | StepPolicy::Separate => None,
            },
            last_edit_at: now,
        });
        self.open = policy != StepPolicy::Separate;
    }

    pub(super) fn undo(&mut self) -> Option<UndoStep> {
        let step = self.undo.pop()?;
        self.redo.push(step.clone());
        self.open = false;
        Some(step)
    }

    pub(super) fn redo(&mut self) -> Option<UndoStep> {
        let step = self.redo.pop()?;
        self.undo.push(step.clone());
        self.open = false;
        Some(step)
    }
}

fn step_accepts(step: &UndoStep, change: &Change, policy: StepPolicy, now: Instant) -> bool {
    let kind = match policy {
        StepPolicy::Continue => return true,
        StepPolicy::Separate => return false,
        StepPolicy::Coalesce(kind) => kind,
    };
    if step.kind != Some(kind) || now.duration_since(step.last_edit_at) > PAUSE_BREAKS_STEP_AFTER {
        return false;
    }
    let Some(last) = step.changes.last() else {
        return false;
    };
    match kind {
        EditKind::Typing => {
            let adjacent = change.old_range.start == last.new_range.end;
            let starts_new_word = change.new_text.chars().next().is_some_and(|c| !c.is_whitespace())
                && last.new_text.chars().last().is_some_and(char::is_whitespace);
            adjacent && !starts_new_word
        }
        // Backspace runs move left from the last deletion; forward-delete runs stay put.
        EditKind::Deleting => {
            change.old_range.end == last.old_range.start
                || change.old_range.start == last.old_range.start
        }
    }
}

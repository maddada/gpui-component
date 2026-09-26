use gpui::{App, Bounds, IntoElement, ParentElement, Pixels, Styled as _, Window, canvas};

use crate::TextSelectionScopeId;

/// Extends a GPUI parent element with post-layout prepaint observation.
pub trait ElementExt: ParentElement + Sized {
    /// Marks this element subtree as belonging to a text-selection scope.
    fn text_selection_scope(self, scope: TextSelectionScopeId) -> impl IntoElement
    where
        Self: IntoElement,
    {
        crate::text_selection::text_selection_scope(scope, self)
    }

    /// Invokes `callback` during prepaint with this element's resolved bounds.
    fn on_prepaint<F>(self, callback: F) -> Self
    where
        F: FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static,
    {
        self.child(
            canvas(
                move |bounds, window, cx| callback(bounds, window, cx),
                |_, _, _, _| {},
            )
            .absolute()
            // Pinned to the corner: without insets an absolute child keeps its static
            // position, which in a block parent is below the parent's text, so the
            // reported bounds sat one line lower than the element.
            .top_0()
            .left_0()
            .size_full(),
        )
    }
}

impl<T: ParentElement> ElementExt for T {}

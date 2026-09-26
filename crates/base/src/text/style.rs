use std::sync::Arc;

use gpui::{HighlightStyle, Hsla, Rems, StyleRefinement, rems};

use crate::ColorTokens;

/// TextViewStyle used to customize the style for [`super::TextView`].
///
/// The fields are private because this type crosses the `gpui-base` seam:
/// build one with the `with_*` methods and read it back through the accessors
/// of the same name, so a later field is an additive change rather than a
/// breaking one.
#[derive(Clone)]
pub struct TextViewStyle {
    foreground: Hsla,
    muted_foreground: Hsla,
    link: Hsla,
    selection: Hsla,
    code_background: Hsla,
    border: Hsla,
    paragraph_gap: Rems,
    heading: Arc<dyn Fn(u8) -> StyleRefinement + Send + Sync + 'static>,
    code_block: StyleRefinement,
    table: StyleRefinement,
    table_head: StyleRefinement,
    table_cell: StyleRefinement,
    table_track: StyleRefinement,
    table_row: StyleRefinement,
    table_head_cell: StyleRefinement,
    list: StyleRefinement,
    list_marker: StyleRefinement,
    inline_code: HighlightStyle,
    default_cursor: bool,
    is_dark: bool,
}

impl PartialEq for TextViewStyle {
    fn eq(&self, other: &Self) -> bool {
        self.paragraph_gap == other.paragraph_gap
            && self.foreground == other.foreground
            && self.muted_foreground == other.muted_foreground
            && self.link == other.link
            && self.selection == other.selection
            && self.code_background == other.code_background
            && self.border == other.border
            && (1..=6).all(|level| (self.heading)(level) == (other.heading)(level))
            && self.code_block == other.code_block
            && self.table == other.table
            && self.table_head == other.table_head
            && self.table_cell == other.table_cell
            && self.table_track == other.table_track
            && self.table_row == other.table_row
            && self.table_head_cell == other.table_head_cell
            && self.list == other.list
            && self.list_marker == other.list_marker
            && self.inline_code == other.inline_code
            && self.default_cursor == other.default_cursor
            && self.is_dark == other.is_dark
    }
}

impl Default for TextViewStyle {
    fn default() -> Self {
        Self::from_colors(&ColorTokens::light(), false)
    }
}

impl TextViewStyle {
    /// Derives rich-text colors from Base semantic theme tokens.
    pub fn from_theme(theme: &crate::Theme) -> Self {
        Self::from_colors(
            &theme.tokens.colors,
            theme.appearance == crate::ThemeAppearance::Dark,
        )
    }

    /// Derives rich-text colors from one palette.
    ///
    /// Rich text needs a handful of roles the palette does not name directly —
    /// a code background, a link color — so they are mapped here once instead
    /// of at every call site.
    fn from_colors(colors: &ColorTokens, is_dark: bool) -> Self {
        Self {
            foreground: colors.foreground,
            muted_foreground: colors.muted_foreground,
            link: colors.primary,
            selection: colors.selection,
            code_background: colors.accent,
            border: colors.border,
            paragraph_gap: rems(1.),
            heading: Arc::new(|_| StyleRefinement::default()),
            code_block: StyleRefinement::default(),
            table: StyleRefinement::default(),
            table_head: StyleRefinement::default(),
            table_cell: StyleRefinement::default(),
            table_track: StyleRefinement::default(),
            table_row: StyleRefinement::default(),
            table_head_cell: StyleRefinement::default(),
            list: StyleRefinement::default(),
            list_marker: StyleRefinement::default(),
            inline_code: HighlightStyle {
                background_color: Some(colors.accent),
                ..Default::default()
            },
            default_cursor: false,
            is_dark,
        }
    }

    /// Sets the default body-text color.
    pub fn with_foreground(mut self, color: Hsla) -> Self {
        self.foreground = color;
        self
    }

    /// Sets the secondary text color.
    pub fn with_muted_foreground(mut self, color: Hsla) -> Self {
        self.muted_foreground = color;
        self
    }

    /// Sets the link text color.
    pub fn with_link(mut self, color: Hsla) -> Self {
        self.link = color;
        self
    }

    /// Sets the background painted behind selected text.
    ///
    /// Selection quads are painted under the glyphs, so this is normally a
    /// translucent wash rather than a solid fill.
    pub fn with_selection(mut self, color: Hsla) -> Self {
        self.selection = color;
        self
    }

    /// Sets the background of fenced code blocks and table header rows.
    pub fn with_code_background(mut self, color: Hsla) -> Self {
        self.code_background = color;
        self
    }

    /// Sets the color of borders and horizontal rules.
    pub fn with_border(mut self, color: Hsla) -> Self {
        self.border = color;
        self
    }

    /// Sets the gap between paragraphs. Defaults to 1 rem.
    pub fn with_paragraph_gap(mut self, gap: Rems) -> Self {
        self.paragraph_gap = gap;
        self
    }

    /// Sets the style refinement for headings, selected by heading level (1-6).
    pub fn with_heading<F>(mut self, heading: F) -> Self
    where
        F: Fn(u8) -> StyleRefinement + Send + Sync + 'static,
    {
        self.heading = Arc::new(heading);
        self
    }

    /// Sets the style refinement for code blocks.
    pub fn with_code_block(mut self, style: StyleRefinement) -> Self {
        self.code_block = style;
        self
    }

    /// Sets the highlight style for inline code spans.
    ///
    /// When `background_color` is `None`, the neutral code background is used,
    /// which keeps [`TextViewStyle::default`] usable without a theme.
    pub fn with_inline_code(mut self, style: HighlightStyle) -> Self {
        self.inline_code = style;
        self
    }

    /// Sets the style refinement for the table container (the bordered wrapper
    /// in wrap mode, the scroll viewport in horizontal-scroll mode).
    ///
    /// Set `overflow_x: scroll` on the refinement for adaptive table layout:
    /// columns fit their content when space allows, shrink (wrapping cell
    /// text) down to a per-column floor when the frame is narrower, and below
    /// that the table scrolls horizontally instead of squeezing further.
    pub fn with_table(mut self, style: StyleRefinement) -> Self {
        self.table = style;
        self
    }

    /// Sets the style refinement for the header row (the first row) of a
    /// table, applied on top of the header background and foreground.
    pub fn with_table_head(mut self, style: StyleRefinement) -> Self {
        self.table_head = style;
        self
    }

    /// Sets the style refinement for each table cell.
    ///
    /// With the scroll table layout, `white_space: nowrap` here keeps cells on
    /// a single line — columns then never shrink and the table scrolls as soon
    /// as the content is wider than the frame.
    pub fn with_table_cell(mut self, style: StyleRefinement) -> Self {
        self.table_cell = style;
        self
    }

    /// Leaves the mouse cursor alone over this document: no I-beam over
    /// selectable text, no pointing hand over a link, a decorated reference or
    /// a linked image. Selection, link clicks and the link context menu are
    /// unaffected; only the cursor shape is. Default `false`, which keeps the
    /// usual text and link cursors.
    pub fn with_default_cursor(mut self, default_cursor: bool) -> Self {
        self.default_cursor = default_cursor;
        self
    }

    /// Sets the style refinement for the box that draws a table's frame and
    /// holds its rows, applied after [`Self::with_table`]. In the scroll
    /// layout this is the scroll viewport.
    pub fn with_table_track(mut self, style: StyleRefinement) -> Self {
        self.table_track = style;
        self
    }

    /// Sets the style refinement applied to every table row, before the
    /// header refinement of [`Self::with_table_head`].
    pub fn with_table_row(mut self, style: StyleRefinement) -> Self {
        self.table_row = style;
        self
    }

    /// Sets the style refinement applied to a header cell, after
    /// [`Self::with_table_cell`].
    pub fn with_table_head_cell(mut self, style: StyleRefinement) -> Self {
        self.table_head_cell = style;
        self
    }

    /// Sets the style refinement applied to a list, where its indent and item
    /// spacing (`gap`) live.
    pub fn with_list(mut self, style: StyleRefinement) -> Self {
        self.list = style;
        self
    }

    /// Sets the style refinement applied to the box a list item's marker sits
    /// in.
    ///
    /// Give it a minimum width to get CSS's `list-style-position: outside`
    /// gutter: the marker is pushed to the gutter's right edge and every
    /// item's text, and every block the item holds, starts on one column.
    pub fn with_list_marker(mut self, style: StyleRefinement) -> Self {
        self.list_marker = style;
        self
    }

    /// Sets whether content-specific assets should use their dark variant.
    pub fn with_dark(mut self, is_dark: bool) -> Self {
        self.is_dark = is_dark;
        self
    }

    /// The default body-text color.
    pub fn foreground(&self) -> Hsla {
        self.foreground
    }

    /// The secondary text color.
    pub fn muted_foreground(&self) -> Hsla {
        self.muted_foreground
    }

    /// The link text color.
    pub fn link(&self) -> Hsla {
        self.link
    }

    /// The background painted behind selected text.
    pub fn selection(&self) -> Hsla {
        self.selection
    }

    /// The background of fenced code blocks and table header rows.
    pub fn code_background(&self) -> Hsla {
        self.code_background
    }

    /// The color of borders and horizontal rules.
    pub fn border(&self) -> Hsla {
        self.border
    }

    /// The gap between paragraphs.
    pub fn paragraph_gap(&self) -> Rems {
        self.paragraph_gap
    }

    /// The style refinement for a heading at `level` (1-6).
    pub fn heading(&self, level: u8) -> StyleRefinement {
        (self.heading)(level)
    }

    /// The style refinement for code blocks.
    pub fn code_block(&self) -> &StyleRefinement {
        &self.code_block
    }

    /// The style refinement for the table container.
    pub fn table(&self) -> &StyleRefinement {
        &self.table
    }

    /// The style refinement for table header rows.
    pub fn table_head(&self) -> &StyleRefinement {
        &self.table_head
    }

    /// The style refinement for table cells.
    pub fn table_cell(&self) -> &StyleRefinement {
        &self.table_cell
    }

    /// The style refinement for the box that draws a table's frame.
    pub fn table_track(&self) -> &StyleRefinement {
        &self.table_track
    }

    /// The style refinement for every table row.
    pub fn table_row(&self) -> &StyleRefinement {
        &self.table_row
    }

    /// The style refinement for a table header cell.
    pub fn table_head_cell(&self) -> &StyleRefinement {
        &self.table_head_cell
    }

    /// The style refinement for a list.
    pub fn list(&self) -> &StyleRefinement {
        &self.list
    }

    /// The style refinement for a list item's marker box.
    pub fn list_marker(&self) -> &StyleRefinement {
        &self.list_marker
    }

    /// The highlight style for inline code, before the code-background
    /// fallback in [`Self::inline_code_highlight`] applies.
    pub fn inline_code(&self) -> HighlightStyle {
        self.inline_code
    }

    /// Whether the document keeps the plain arrow cursor; see
    /// [`Self::with_default_cursor`].
    pub fn default_cursor(&self) -> bool {
        self.default_cursor
    }

    /// Whether content-specific assets should use their dark variant.
    pub fn is_dark(&self) -> bool {
        self.is_dark
    }

    /// Returns the [`HighlightStyle`] to use for inline code, falling back to
    /// the code background when no custom background was supplied.
    pub(crate) fn inline_code_highlight(&self) -> HighlightStyle {
        let mut style = self.inline_code;
        if style.background_color.is_none() {
            style.background_color = Some(self.code_background);
        }
        style
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Styled as _, px};

    use super::*;

    #[test]
    fn selection_layout_fingerprint_covers_callback_table_and_theme_fields() {
        let base = TextViewStyle::default();
        let heading = base
            .clone()
            .with_heading(|_| StyleRefinement::default().text_size(px(14.)));
        assert!(
            heading
                == base
                    .clone()
                    .with_heading(|_| StyleRefinement::default().text_size(px(14.)))
        );
        assert!(
            heading
                != base
                    .clone()
                    .with_heading(|_| StyleRefinement::default().text_size(px(28.)))
        );

        let mut table = StyleRefinement::default();
        table.text.white_space = Some(gpui::WhiteSpace::Nowrap);
        assert!(base != base.clone().with_table_cell(table));

        assert!(base != base.clone().with_dark(true));
    }

    #[test]
    fn cloning_preserves_the_same_heading_callback_fingerprint() {
        let style = TextViewStyle::default()
            .with_heading(|_| StyleRefinement::default().text_size(px(14.)));
        assert!(style == style.clone());
    }

    #[test]
    fn default_style_is_readable_without_an_application_theme() {
        let style = TextViewStyle::default();

        assert_eq!(style.foreground().a, 1.0);
        assert_eq!(style.link().a, 1.0);
        assert!(style.selection().a > 0.0);
        assert!(style.inline_code().background_color.is_some());
        assert!(style.code_background().a > 0.0);
        assert!(style.border().a > 0.0);
        assert_eq!(style.code_block().corner_radii.top_left, None);
        assert_eq!(style.code_block().corner_radii.top_right, None);
        assert_eq!(style.code_block().corner_radii.bottom_left, None);
        assert_eq!(style.code_block().corner_radii.bottom_right, None);
    }

    #[test]
    fn heading_refinement_defaults_empty_and_resolves_by_level() {
        let base = TextViewStyle::default();
        assert_eq!(base.heading(1), StyleRefinement::default());

        let style = base.clone().with_heading(|level| match level {
            1 => StyleRefinement::default().pt(rems(1.)).pb(rems(0.5)),
            _ => StyleRefinement::default().pb(rems(0.25)),
        });

        assert_eq!(
            style.heading(1),
            StyleRefinement::default().pt(rems(1.)).pb(rems(0.5))
        );
        assert_eq!(style.heading(2), StyleRefinement::default().pb(rems(0.25)));
        assert!(style != base);
    }

    #[test]
    fn inline_code_falls_back_to_the_code_background() {
        let style = TextViewStyle::default()
            .with_code_background(gpui::rgb(0x123456).into())
            .with_inline_code(HighlightStyle::default());

        assert_eq!(
            style.inline_code_highlight().background_color,
            Some(gpui::rgb(0x123456).into())
        );
    }

    #[test]
    fn from_theme_maps_base_semantic_tokens() {
        let mut theme = crate::Theme::default();
        theme.tokens.colors.foreground = gpui::rgb(0x112233).into();
        theme.tokens.colors.muted_foreground = gpui::rgb(0x445566).into();
        theme.tokens.colors.primary = gpui::rgb(0x3366ff).into();
        theme.tokens.colors.accent = gpui::rgb(0xddeeff).into();
        theme.tokens.colors.border = gpui::rgb(0x778899).into();
        theme.tokens.colors.selection = gpui::rgb(0x55a0fc).into();

        let style = TextViewStyle::from_theme(&theme);
        assert_eq!(style.foreground(), theme.tokens.colors.foreground);
        assert_eq!(style.link(), theme.tokens.colors.primary);
        assert_eq!(style.selection(), theme.tokens.colors.selection);
        assert_eq!(style.code_background(), theme.tokens.colors.accent);
        assert_eq!(style.border(), theme.tokens.colors.border);
    }
}

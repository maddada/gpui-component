use std::sync::Arc;

use gpui::{Hsla, Pixels, Rems, SharedString, StyleRefinement, px, rems};

use crate::highlighter::HighlightTheme;

/// Typography and decoration for inline code, independent of fenced blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineCodeStyle {
    pub font_family: SharedString,
    pub font_scale: f32,
    pub padding_x: Pixels,
    pub padding_y: Pixels,
    pub border_width: Pixels,
    pub radius: Pixels,
    pub background: Hsla,
    pub border_color: Hsla,
    /// Draw a small square of the colour a span names (`#1d4ed8`, `#fff`)
    /// before its text.
    pub swatches: bool,
    /// The colour this particular span named, filled in while laying it out.
    pub swatch: Option<Hsla>,
    /// Ring around the swatch, kept apart from the span's own border so a
    /// colour named in running text can carry the ring without a code chip.
    pub swatch_border_width: Pixels,
    pub swatch_border_color: Hsla,
    /// True when the span is ordinary prose that merely names a colour: it
    /// keeps the paragraph's own typeface instead of the code one.
    pub prose: bool,
}

/// TextViewStyle used to customize the style for [`TextView`].
#[derive(Clone)]
pub struct TextViewStyle {
    /// Gap of each paragraphs, default is 1 rem.
    pub paragraph_gap: Rems,
    /// Base font size for headings, default is 14px.
    pub heading_base_font_size: Pixels,
    /// Function to calculate heading font size based on heading level (1-6).
    ///
    /// The first parameter is the heading level (1-6), the second parameter is the base font size.
    /// The second parameter is the base font size.
    pub heading_font_size: Option<Arc<dyn Fn(u8, Pixels) -> Pixels + Send + Sync + 'static>>,
    /// Additional typography and spacing applied to headings.
    pub heading: StyleRefinement,
    /// Highlight theme for code blocks, applied when the document is rendered.
    ///
    /// `None` keeps the theme the document was parsed with, which is the
    /// application theme's `highlight_theme`. Set it to paint fenced blocks with
    /// a palette of your own: unlike the parse-time theme it follows a later
    /// dark/light switch, because the styles are resolved during render.
    pub highlight_theme: Option<Arc<HighlightTheme>>,
    /// The style refinement for code blocks.
    pub code_block: StyleRefinement,
    pub inline_code: Option<InlineCodeStyle>,
    /// Draw a swatch before every hex colour written in running text, the way
    /// `inline_code` draws one inside a code span. Set `prose` on it.
    pub prose_swatch: Option<InlineCodeStyle>,
    /// Style refinement applied to the table container (the bordered wrapper).
    ///
    /// Set `overflow_x: scroll` here to keep table cells on a single line and
    /// scroll the table horizontally instead of wrapping cell content, e.g.
    /// `TextViewStyle::default().table({ let mut s = StyleRefinement::default(); s.overflow.x = Some(Overflow::Scroll); s })`.
    pub table: StyleRefinement,
    /// In scroll mode, the widest a column may grow to fit its longest cell
    /// (480px when unset).
    pub table_cell_max_width: Option<Pixels>,
    /// In scroll mode, wrap a cell's text inside its column instead of keeping
    /// it on one line and clipping it at `table_cell_max_width`.
    pub table_wrap_cells: bool,
    /// In scroll mode, draw a horizontal scrollbar of this thickness under a
    /// table wider than its frame, shown while the pointer is over the table.
    pub table_scrollbar: Option<Pixels>,
    /// Style refinement applied to each table cell.
    pub table_cell: StyleRefinement,
    /// Style refinement applied to the bordered box that holds the rows.
    ///
    /// In scroll mode this is the track inside the scroll viewport, so it is
    /// the only way to reach the frame a table is drawn in.
    pub table_track: StyleRefinement,
    /// Style refinement applied to every row, after the default row rule.
    pub table_row: StyleRefinement,
    /// Style refinement applied to the header row, after `table_row`.
    pub table_head_row: StyleRefinement,
    /// Style refinement applied to a header cell, after `table_cell`.
    pub table_head_cell: StyleRefinement,
    /// Style refinement applied to a list, where its indent and item spacing live.
    pub list: StyleRefinement,
    /// Style refinement applied to the box a list item's marker sits in.
    ///
    /// Give it a minimum width to get CSS's `list-style-position: outside`
    /// gutter: the marker is pushed to the gutter's right edge and every item's
    /// text starts on one column.
    pub list_marker: StyleRefinement,
    /// Leave the mouse cursor alone over this document: no I-beam over
    /// selectable text, no pointing hand over a link, a decorated reference or a
    /// linked image. Selection, link clicks and the link context menu are
    /// unaffected; only the cursor shape is. Default `false`, which keeps the
    /// usual text and link cursors.
    pub default_cursor: bool,
    pub is_dark: bool,
}

impl PartialEq for TextViewStyle {
    fn eq(&self, other: &Self) -> bool {
        self.paragraph_gap == other.paragraph_gap
            && self.heading_base_font_size == other.heading_base_font_size
            && self.heading == other.heading
            && self.highlight_theme == other.highlight_theme
            && self.inline_code == other.inline_code
    }
}

impl Default for TextViewStyle {
    fn default() -> Self {
        Self {
            paragraph_gap: rems(1.),
            heading_base_font_size: px(14.),
            heading_font_size: None,
            heading: StyleRefinement::default(),
            highlight_theme: None,
            code_block: StyleRefinement::default(),
            inline_code: None,
            prose_swatch: None,
            table: StyleRefinement::default(),
            table_cell_max_width: None,
            table_wrap_cells: false,
            table_scrollbar: None,
            table_cell: StyleRefinement::default(),
            table_track: StyleRefinement::default(),
            table_row: StyleRefinement::default(),
            table_head_row: StyleRefinement::default(),
            table_head_cell: StyleRefinement::default(),
            list: StyleRefinement::default(),
            list_marker: StyleRefinement::default(),
            default_cursor: false,
            is_dark: false,
        }
    }
}

impl TextViewStyle {
    /// Set paragraph gap, default is 1 rem.
    pub fn paragraph_gap(mut self, gap: Rems) -> Self {
        self.paragraph_gap = gap;
        self
    }

    pub fn heading_font_size<F>(mut self, f: F) -> Self
    where
        F: Fn(u8, Pixels) -> Pixels + Send + Sync + 'static,
    {
        self.heading_font_size = Some(Arc::new(f));
        self
    }

    /// Set style for code blocks.
    pub fn code_block(mut self, style: StyleRefinement) -> Self {
        self.code_block = style;
        self
    }

    pub fn inline_code(mut self, style: InlineCodeStyle) -> Self {
        self.inline_code = Some(style);
        self
    }

    /// Set extra style for the table container.
    ///
    /// Set `overflow_x: scroll` on the refinement to make wide tables scroll
    /// horizontally (cells stop wrapping) instead of shrinking to fit.
    pub fn table(mut self, style: StyleRefinement) -> Self {
        self.table = style;
        self
    }

    /// Set extra style for each table cell.
    pub fn table_cell(mut self, style: StyleRefinement) -> Self {
        self.table_cell = style;
        self
    }

    /// Keep the plain arrow cursor over the whole document: over selectable
    /// text, over links, over decorated references and over linked images.
    /// Selection and clicks keep working; only the cursor shape changes.
    pub fn default_cursor(mut self, default_cursor: bool) -> Self {
        self.default_cursor = default_cursor;
        self
    }
}

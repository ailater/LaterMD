//! Table rendering for markdown tables.

use egui::emath::GuiRounding;
use egui::text::LayoutJob;
use egui::{
  Align, Color32, CornerRadius, FontFamily, FontId, Id, Layout, Margin, Rect, ScrollArea, Stroke, StrokeKind,
  TextFormat, Ui, UiBuilder, Vec2,
};
use egui_extras::{Column, TableBuilder};

use crate::layout::{append_link_to_job, render_link_in_ui};
use crate::link::LinkHandler;
use crate::style::{InlineCodeStyle, TableStyle};
use crate::types::{Alignment, TableData, Token, TokenStyle};

/// Geometry captured while rendering a table.
#[derive(Debug, Clone)]
pub struct TableGeometry {
  /// Full content rectangle, including horizontally clipped content.
  pub table: Rect,
  /// Visible horizontal viewport rectangle.
  pub viewport: Rect,
  /// Cell rectangles as `(row, column, header, rect)`.
  pub cells: Vec<(usize, usize, bool, Rect)>,
}

impl Default for TableGeometry {
  fn default() -> Self {
    Self { table: Rect::NOTHING, viewport: Rect::NOTHING, cells: Vec::new() }
  }
}

/// Read geometry captured by the last render for a stable table id.
pub fn table_geometry(ctx: &egui::Context, id: Id) -> Option<TableGeometry> {
  ctx.data(|d| d.get_temp::<TableGeometry>(id.with("geometry")))
}

/// Callback invoked after cell padding has been applied. Returning `true` replaces
/// the default rich cell renderer for that cell.
pub type CellRenderer<'a> = dyn FnMut(&mut Ui, usize, usize, bool, Rect) -> bool + 'a;

/// Options for [`render_table_with_options`].
pub struct TableRenderOptions<'a> {
  /// Optional callback that replaces the default renderer per cell.
  pub cell_renderer: Option<&'a mut CellRenderer<'a>>,
  /// Minimum content width before horizontal scrolling.
  pub min_width: f32,
}

impl<'a> Default for TableRenderOptions<'a> {
  fn default() -> Self {
    Self { cell_renderer: None, min_width: 0.0 }
  }
}

/// Render a markdown table using `egui_extras::TableBuilder`.
#[allow(clippy::too_many_arguments)]
pub fn render_table(
  ui: &mut Ui,
  id: Id,
  data: &TableData<'_>,
  font_id: &FontId,
  color: Color32,
  inline_code_style: &InlineCodeStyle,
  table_style: &TableStyle,
  link_handler: Option<&dyn LinkHandler>,
) {
  let mut options = TableRenderOptions::default();
  let _ =
    render_table_with_options(ui, id, data, font_id, color, inline_code_style, table_style, link_handler, &mut options);
}

/// Render a table with the same measurement, scrolling, and chrome as [`render_table`].
/// The returned geometry is also stored under `id.with("geometry")` for hit mapping.
#[allow(clippy::too_many_arguments)]
pub fn render_table_with_options(
  ui: &mut Ui,
  id: Id,
  data: &TableData<'_>,
  font_id: &FontId,
  color: Color32,
  inline_code_style: &InlineCodeStyle,
  table_style: &TableStyle,
  link_handler: Option<&dyn LinkHandler>,
  options: &mut TableRenderOptions<'_>,
) -> TableGeometry {
  let num_cols = data.alignments.len();
  if num_cols == 0 {
    return TableGeometry::default();
  }

  let hyperlink_color = ui.visuals().hyperlink_color;
  let strong_color = ui.visuals().strong_text_color();
  let dark_mode = ui.visuals().dark_mode;
  let bold_family = FontFamily::Name("bold".into());
  let has_bold = ui.ctx().fonts(|f| f.families().contains(&bold_family));

  let pad = table_style.cell_padding;
  let measured = measure_table(
    ui,
    data,
    font_id,
    color,
    hyperlink_color,
    strong_color,
    has_bold,
    dark_mode,
    inline_code_style,
    link_handler,
    pad,
  );

  let stroke = (table_style.stroke_width > 0.0)
    .then(|| Stroke::new(table_style.stroke_width, ui.visuals().widgets.noninteractive.bg_stroke.color));
  let last_body = data.rows.len().saturating_sub(1);
  let cell_margin = egui::Margin { left: pad[0] as i8, top: pad[1] as i8, right: pad[2] as i8, bottom: pad[3] as i8 };
  let mut col_widths = measured.col_widths;
  let header_h = measured.header_h;
  let row_heights = measured.row_heights;
  let intrinsic_width = col_widths.iter().sum::<f32>();
  if options.min_width > intrinsic_width && !col_widths.is_empty() {
    let extra = (options.min_width - intrinsic_width) / col_widths.len() as f32;
    for width in &mut col_widths {
      *width += extra;
    }
  }
  let table_width = col_widths.iter().sum::<f32>();
  let table_size = Vec2::new(table_width, header_h + row_heights.iter().sum::<f32>());
  let radius = table_style.corner_radius.round() as u8;
  let mut geometry = TableGeometry::default();

  let output = ScrollArea::horizontal()
    .id_salt(id.with("scroll"))
    .content_margin(Margin { top: 0, right: 0, bottom: 4, left: 0 })
    .show(ui, |ui| {
      let mut paint_table = |ui: &mut Ui| {
        if stroke.is_some() {
          ui.spacing_mut().item_spacing = Vec2::ZERO;
        }
        let mut builder = TableBuilder::new(ui)
          .id_salt(id)
          .striped(table_style.zebra_fill)
          .vscroll(false)
          .min_scrolled_height(0.0)
          .cell_layout(Layout::left_to_right(Align::Center));
        for &w in &col_widths {
          builder = builder.column(Column::exact(w));
        }
        builder
          .header(header_h, |mut header| {
            for (col_idx, cell_tokens) in data.headers.iter().enumerate() {
              let align = data.alignments.get(col_idx).copied().unwrap_or(Alignment::None);
              header.col(|ui| {
                if table_style.header_fill {
                  paint_header_fill(ui);
                }
                let cell_rect = ui.max_rect();
                geometry.cells.push((0, col_idx, true, cell_rect));
                padded_cell(ui, cell_margin, align, |ui| {
                  let replaced =
                    options.cell_renderer.as_deref_mut().is_some_and(|cb| cb(ui, 0, col_idx, true, cell_rect));
                  if !replaced {
                    render_cell(
                      ui,
                      cell_tokens,
                      font_id,
                      color,
                      hyperlink_color,
                      strong_color,
                      has_bold,
                      true,
                      dark_mode,
                      inline_code_style,
                      link_handler,
                    );
                  }
                });
                if let Some(stroke) = stroke {
                  paint_cell_separators(ui, ui.max_rect(), stroke, col_idx + 1 == num_cols, data.rows.is_empty());
                }
              });
            }
          })
          .body(|body| {
            body.heterogeneous_rows(row_heights.iter().copied(), |mut row| {
              let row_idx = row.index();
              let row_data = &data.rows[row_idx];
              for (col_idx, cell_tokens) in row_data.iter().enumerate() {
                let align = data.alignments.get(col_idx).copied().unwrap_or(Alignment::None);
                row.col(|ui| {
                  let cell_rect = ui.max_rect();
                  geometry.cells.push((row_idx + 1, col_idx, false, cell_rect));
                  padded_cell(ui, cell_margin, align, |ui| {
                    let replaced = options
                      .cell_renderer
                      .as_deref_mut()
                      .is_some_and(|cb| cb(ui, row_idx + 1, col_idx, false, cell_rect));
                    if !replaced {
                      render_cell(
                        ui,
                        cell_tokens,
                        font_id,
                        color,
                        hyperlink_color,
                        strong_color,
                        has_bold,
                        false,
                        dark_mode,
                        inline_code_style,
                        link_handler,
                      );
                    }
                  });
                  if let Some(stroke) = stroke {
                    paint_cell_separators(ui, ui.max_rect(), stroke, col_idx + 1 == num_cols, row_idx == last_body);
                  }
                });
              }
            });
          });
      };

      let top_left = ui.cursor().min;
      paint_table(ui);
      Rect::from_min_size(top_left, table_size)
    });

  geometry.table = output.inner;
  geometry.viewport = output.inner_rect;
  paint_table_chrome(ui, output.inner, output.inner_rect, stroke, radius);
  ui.ctx().data_mut(|d| d.insert_temp(id.with("geometry"), geometry.clone()));
  geometry
}

fn padded_cell(ui: &mut Ui, margin: Margin, align: Alignment, add: impl FnOnce(&mut Ui)) {
  let inner = ui.max_rect() - margin;
  ui.scope_builder(UiBuilder::new().max_rect(inner).layout(cell_layout(align)), add);
}

/// Outer stroke on the visible table, plus a light inner shadow on cut edges.
/// `ScrollAreaOutput::inner_rect` is the pre-shrink available rect, so height
/// comes from the table and width from the viewport. The visible rect is
/// always rounded; a cut edge is the same rounded path, just shorter.
fn paint_table_chrome(ui: &Ui, table_rect: Rect, viewport: Rect, stroke: Option<Stroke>, radius: u8) {
  let mut vis = table_rect.intersect(ui.clip_rect());
  vis.min.x = vis.min.x.max(viewport.min.x);
  vis.max.x = vis.max.x.min(viewport.max.x);
  if !vis.is_positive() {
    return;
  }

  let slop = stroke.map(|s| s.width.max(1.0)).unwrap_or(1.0);
  let cut_left = vis.left() > table_rect.left() + slop;
  let cut_right = vis.right() < table_rect.right() - slop;
  let cut_top = vis.top() > table_rect.top() + slop;
  let cut_bottom = vis.bottom() < table_rect.bottom() - slop;

  let painter = ui.painter_at(vis.expand(1.0));
  if let Some(stroke) = stroke {
    painter.rect_stroke(vis, CornerRadius::same(radius), stroke, StrokeKind::Inside);
  }

  if !(cut_left || cut_right || cut_top || cut_bottom) {
    return;
  }

  let dark = ui.visuals().dark_mode;
  if cut_right {
    paint_edge_shadow(&painter, vis, OverflowSide::Right, dark);
  }
  if cut_left {
    paint_edge_shadow(&painter, vis, OverflowSide::Left, dark);
  }
  if cut_top {
    paint_edge_shadow(&painter, vis, OverflowSide::Top, dark);
  }
  if cut_bottom {
    paint_edge_shadow(&painter, vis, OverflowSide::Bottom, dark);
  }
}

enum OverflowSide {
  Left,
  Right,
  Top,
  Bottom,
}

fn paint_edge_shadow(painter: &egui::Painter, vis: Rect, side: OverflowSide, dark: bool) {
  const SIZE: f32 = 6.0;
  let color = Color32::BLACK.gamma_multiply(if dark { 0.18 } else { 0.08 });
  let strip = match side {
    OverflowSide::Right => Rect::from_x_y_ranges((vis.right() - SIZE)..=vis.right(), vis.y_range()),
    OverflowSide::Left => Rect::from_x_y_ranges(vis.left()..=vis.left() + SIZE, vis.y_range()),
    OverflowSide::Top => Rect::from_x_y_ranges(vis.x_range(), vis.top()..=vis.top() + SIZE),
    OverflowSide::Bottom => Rect::from_x_y_ranges(vis.x_range(), (vis.bottom() - SIZE)..=vis.bottom()),
  };
  painter.add(egui::epaint::RectShape::filled(strip, 0.0, color).with_blur_width(SIZE));
}

/// Header row background, painted behind the cell's text. Same gapless
/// expansion `egui_extras` uses for its striped rows, so cell spacing leaves
/// no gaps; the rect stays within the cell, so a horizontally scrolled table
/// never fills past its viewport.
fn paint_header_fill(ui: &Ui) {
  let rect = ui.max_rect().expand2(0.5 * ui.spacing().item_spacing).round_ui();
  ui.painter().rect_filled(rect, CornerRadius::ZERO, ui.visuals().faint_bg_color);
}

/// Internal separators only. The outer rectangle is painted by [`paint_table_chrome`].
fn paint_cell_separators(ui: &Ui, rect: Rect, stroke: Stroke, last_col: bool, last_row: bool) {
  // Exact columns clip their cells; pull the lines in so the stroke is not discarded.
  let rect = rect.shrink(stroke.width * 0.5);
  let painter = ui.painter();
  if !last_col {
    painter.line_segment([rect.right_top(), rect.right_bottom()], stroke);
  }
  if !last_row {
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
  }
}

#[allow(clippy::too_many_arguments)]
fn render_cell(
  ui: &mut Ui,
  tokens: &[Token<'_>],
  font_id: &FontId,
  color: Color32,
  hyperlink_color: Color32,
  strong_color: Color32,
  has_bold: bool,
  is_header: bool,
  dark_mode: bool,
  inline_code_style: &InlineCodeStyle,
  link_handler: Option<&dyn LinkHandler>,
) {
  let has_links = tokens.iter().any(|t| matches!(t, Token::Link { .. }));
  if !has_links {
    let job = tokens_to_layout_job(
      ui,
      tokens,
      font_id,
      color,
      hyperlink_color,
      strong_color,
      has_bold,
      is_header,
      dark_mode,
      inline_code_style,
      link_handler,
    );
    ui.label(job);
    return;
  }

  // Render tokens individually so links are clickable and dispatch through the LinkHandler.
  for token in tokens {
    match token {
      Token::Link { text, href, .. } => {
        let (link_font, link_color) = link_font_and_color(font_id, hyperlink_color, strong_color, has_bold, is_header);
        let base_format = TextFormat::default();
        render_link_in_ui(ui, text.as_ref(), href.as_ref(), &link_font, &base_format, link_color, link_handler);
      }
      Token::Text { text, style } => {
        let format =
          token_style_to_format(style, font_id, color, strong_color, has_bold, is_header, dark_mode, inline_code_style);
        let job = LayoutJob::simple_singleline(text.to_string(), format.font_id, format.color);
        ui.label(job);
      }
      _ => {}
    }
  }
}

/// Return the font and color a link should use in a table cell, accounting for
/// header bold styling and missing bold-font fallback.
#[inline]
fn link_font_and_color(
  font_id: &FontId,
  hyperlink_color: Color32,
  strong_color: Color32,
  has_bold: bool,
  is_header: bool,
) -> (FontId, Color32) {
  let mut font = font_id.clone();
  let mut color = hyperlink_color;
  if is_header {
    if has_bold {
      font.family = FontFamily::Name("bold".into());
    } else {
      color = strong_color;
    }
  }
  (font, color)
}

struct Measured {
  col_widths: Vec<f32>,
  header_h: f32,
  row_heights: Vec<f32>,
}

#[allow(clippy::too_many_arguments)]
fn measure_table(
  ui: &Ui,
  data: &TableData<'_>,
  font_id: &FontId,
  color: Color32,
  hyperlink_color: Color32,
  strong_color: Color32,
  has_bold: bool,
  dark_mode: bool,
  inline_code_style: &InlineCodeStyle,
  link_handler: Option<&dyn LinkHandler>,
  pad: [f32; 4],
) -> Measured {
  let num_cols = data.alignments.len();
  let mut widths = vec![40.0_f32; num_cols];
  let hpad = pad[0] + pad[2];
  let vpad = pad[1] + pad[3];

  let measure = |tokens: &[Token<'_>], is_header: bool| -> Vec2 {
    let job = tokens_to_layout_job(
      ui,
      tokens,
      font_id,
      color,
      hyperlink_color,
      strong_color,
      has_bold,
      is_header,
      dark_mode,
      inline_code_style,
      link_handler,
    );
    ui.ctx().fonts_mut(|f| f.layout_job(job)).size()
  };

  let line_height = ui.fonts_mut(|fonts| fonts.row_height(font_id));
  let min_content_h = line_height.max(1.0);
  let mut header_h = min_content_h;
  for (col, cell) in data.headers.iter().enumerate() {
    let size = measure(cell, true);
    widths[col] = widths[col].max(size.x + hpad);
    header_h = header_h.max(size.y);
  }

  let mut row_heights = Vec::with_capacity(data.rows.len());
  for row in &data.rows {
    let mut h = min_content_h;
    for (col, cell) in row.iter().enumerate() {
      if col < num_cols {
        let size = measure(cell, false);
        widths[col] = widths[col].max(size.x + hpad);
        h = h.max(size.y);
      }
    }
    row_heights.push(h + vpad);
  }

  Measured { col_widths: widths, header_h: header_h + vpad, row_heights }
}

#[inline]
fn cell_layout(align: Alignment) -> Layout {
  match align {
    Alignment::None | Alignment::Left => Layout::left_to_right(Align::Center),
    Alignment::Center => Layout::top_down(Align::Center),
    Alignment::Right => Layout::right_to_left(Align::Center),
  }
}

#[allow(clippy::too_many_arguments)]
fn tokens_to_layout_job(
  ui: &Ui,
  tokens: &[Token<'_>],
  font_id: &FontId,
  color: Color32,
  hyperlink_color: Color32,
  strong_color: Color32,
  has_bold: bool,
  is_header: bool,
  dark_mode: bool,
  inline_code_style: &InlineCodeStyle,
  link_handler: Option<&dyn LinkHandler>,
) -> LayoutJob {
  let mut job = LayoutJob::default();
  for token in tokens {
    match token {
      Token::Text { text, style } => {
        let format =
          token_style_to_format(style, font_id, color, strong_color, has_bold, is_header, dark_mode, inline_code_style);
        job.append(text.as_ref(), 0.0, format);
      }
      Token::Link { text, href, .. } => {
        // Table cells never sit inside a heading, so there is no widget font
        // override to apply (None keeps the cell font everywhere).
        let (link_font, link_color) = link_font_and_color(font_id, hyperlink_color, strong_color, has_bold, is_header);
        let base_format = TextFormat::default();
        append_link_to_job(
          ui,
          &mut job,
          text.as_ref(),
          href.as_ref(),
          &link_font,
          None,
          &base_format,
          link_color,
          link_handler,
        );
      }
      Token::Newline => {
        job.append(" ", 0.0, TextFormat { font_id: font_id.clone(), color, ..Default::default() });
      }
      _ => {}
    }
  }
  job
}

#[inline]
fn apply_bold_to_format(format: &mut TextFormat, strong_color: Color32, has_bold: bool) {
  if has_bold {
    format.font_id.family = FontFamily::Name("bold".into());
  } else {
    format.color = strong_color;
  }
}

#[allow(clippy::too_many_arguments)]
fn token_style_to_format(
  style: &TokenStyle,
  font_id: &FontId,
  color: Color32,
  strong_color: Color32,
  has_bold: bool,
  is_header: bool,
  dark_mode: bool,
  inline_code_style: &InlineCodeStyle,
) -> TextFormat {
  let mut format = TextFormat { font_id: font_id.clone(), color, ..Default::default() };
  if style.bold || is_header {
    apply_bold_to_format(&mut format, strong_color, has_bold);
  }
  if style.italic {
    format.italics = true;
  }
  if style.strikethrough {
    format.strikethrough = egui::Stroke::new(1.0_f32, color);
  }
  if style.inline_code {
    crate::layout::apply_inline_code_bg(&mut format, dark_mode, inline_code_style);
  }
  format
}

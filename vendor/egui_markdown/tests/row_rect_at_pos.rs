use egui::{pos2, vec2, Context, Id, RawInput, Rect, UiBuilder};
use egui_markdown::{block_span_rects, row_rect_at_pos, MarkdownLabel};

fn screen() -> Rect {
  Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 2000.0))
}

/// Render `text` and return the painted text shapes with their absolute
/// per-row geometry (row y range plus the first glyph's absolute x), so tests
/// can target a specific row without modelling font metrics. Glyph positions
/// in epaint galleys are row-local on the y axis, so rows — not glyphs — are
/// the geometry source here.
struct PaintedRows {
  galley_left: f32,
  galley_right: f32,
  rows: Vec<(f32, f32)>, // absolute min_y..max_y per row, top-down
  first_glyph_x: Vec<f32>,
}

fn paint(text: &str, id: Id) -> Vec<PaintedRows> {
  let ctx = Context::default();
  let mut painted = Vec::new();
  let mut output = ctx.run_ui(RawInput { screen_rect: Some(screen()), ..Default::default() }, |ui| {
    let mut child = ui.new_child(UiBuilder::new().max_rect(screen()));
    MarkdownLabel::new(id, text).wrap().show(&mut child);
  });
  for clipped in &output.shapes {
    if let egui::Shape::Text(t) = &clipped.shape {
      painted.push(PaintedRows {
        galley_left: t.pos.x,
        galley_right: t.pos.x + t.galley.size().x,
        rows: t.galley.rows.iter().map(|row| (t.pos.y + row.min_y(), t.pos.y + row.max_y())).collect(),
        first_glyph_x: t
          .galley
          .rows
          .iter()
          .map(|row| t.pos.x + row.glyphs.first().map(|g| g.pos.x).unwrap_or(0.0))
          .collect(),
      });
    }
  }
  output.textures_delta.clear();
  painted
}

/// A position inside a row resolves to that row's full-width screen rect —
/// the anchor geometry live-preview hosts need to keep the clicked row
/// stationary across a rendered <-> source form switch.
#[test]
fn position_on_a_row_resolves_to_that_rows_rect() {
  let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron\n";
  let id = Id::new("rows");
  let painted = paint(text, id);
  let shape = painted.first().expect("paragraph rendered");
  assert!(shape.rows.len() >= 2, "long paragraph soft-wraps into several rows");
  // Probe strictly inside the second visual row: a hair below its top, at
  // the row's first glyph.
  let probe = pos2(shape.first_glyph_x[1] + 1.0, (shape.rows[1].0 + shape.rows[1].1) * 0.5);
  let ctx = Context::default();
  let mut row = None;
  let mut blocks = None;
  let mut output = ctx.run_ui(RawInput { screen_rect: Some(screen()), ..Default::default() }, |ui| {
    let mut child = ui.new_child(UiBuilder::new().max_rect(screen()));
    MarkdownLabel::new(id, text).wrap().show(&mut child);
    row = row_rect_at_pos(ui, id, probe);
    blocks = block_span_rects(ui, id);
  });
  output.textures_delta.clear();
  let row = row.expect("row interior resolves to its row rect");
  assert_eq!(row.min.y, shape.rows[1].0, "row rect top = the probed row's top");
  assert_eq!(row.max.y, shape.rows[1].1, "row rect bottom = the probed row's bottom");
  assert_eq!(row.min.x, shape.galley_left, "row rect starts at the galley left edge");
  assert_eq!(row.max.x, shape.galley_right, "row rect spans the full galley width");
  // The row is row-tall, not block-tall: it fits inside the block span that
  // holds it (one paragraph block wraps every row of this document).
  let blocks = blocks.expect("block table present");
  let block =
    blocks.iter().find(|b| b.rect.min.y <= probe.y && probe.y <= b.rect.max.y).expect("probed row sits in a block");
  assert!(row.max.y <= block.rect.max.y, "row rect fits inside its block span: row {row:?} block {:?}", block.rect);
}

/// Two rows of one document yield two distinct rects, and a point strictly
/// inside a row always resolves to that row — the per-row geometry callers
/// anchor to.
#[test]
fn stacked_rows_yield_distinct_rects() {
  let text = "row one\n\nrow two\n";
  let id = Id::new("stack");
  let painted = paint(text, id);
  let shape = painted.first().expect("paragraphs flush into one galley");
  assert!(shape.rows.len() >= 3, "content row, blank row, content row");
  let (top, bottom) = (shape.rows[0], shape.rows[2]);
  let ctx = Context::default();
  let mut rects = Vec::new();
  let mut output = ctx.run_ui(RawInput { screen_rect: Some(screen()), ..Default::default() }, |ui| {
    let mut child = ui.new_child(UiBuilder::new().max_rect(screen()));
    MarkdownLabel::new(id, text).wrap().show(&mut child);
    rects.push(row_rect_at_pos(ui, id, pos2(shape.first_glyph_x[0] + 1.0, (top.0 + top.1) * 0.5)));
    rects.push(row_rect_at_pos(ui, id, pos2(shape.first_glyph_x[2] + 1.0, (bottom.0 + bottom.1) * 0.5)));
  });
  output.textures_delta.clear();
  let (top_rect, bottom_rect) = (rects[0].expect("top row rect"), rects[1].expect("bottom row rect"));
  assert_eq!(top_rect.min.y, top.0, "top row resolves to the top row's geometry");
  assert_eq!(bottom_rect.min.y, bottom.0, "bottom row resolves to the bottom row's geometry");
  assert!(top_rect.max.y <= bottom_rect.min.y, "row rects are disjoint: {top_rect:?} vs {bottom_rect:?}");
}

/// Positions outside every painted text galley and unrendered labels report
/// None — same contract as `char_index_at_pos`.
#[test]
fn outside_galleys_and_unrendered_labels_yield_none() {
  let text = "para one\n";
  let id = Id::new("gap");
  let painted = paint(text, id);
  let last_row_bottom = painted.first().expect("paragraph rendered").rows.last().unwrap().1;
  let ctx = Context::default();
  let mut above_hit = None;
  let mut below_hit = None;
  let mut never_hit = None;
  let mut blocks = None;
  let mut output = ctx.run_ui(RawInput { screen_rect: Some(screen()), ..Default::default() }, |ui| {
    let mut child = ui.new_child(UiBuilder::new().max_rect(screen()));
    MarkdownLabel::new(id, text).wrap().show(&mut child);
    above_hit = row_rect_at_pos(ui, id, pos2(10.0, -10.0));
    below_hit = row_rect_at_pos(ui, id, pos2(10.0, last_row_bottom + 50.0));
    never_hit = row_rect_at_pos(ui, Id::new("never"), pos2(10.0, 10.0));
    blocks = block_span_rects(ui, id);
  });
  output.textures_delta.clear();
  assert!(above_hit.is_none(), "no text geometry above the galley");
  assert!(below_hit.is_none(), "no text geometry below the last row");
  assert_eq!(never_hit, None, "unrendered label has no geometry");
  // The block table still answers for out-of-galley positions (the documented
  // fallback path for hosts resolving a caret there).
  assert!(blocks.is_some_and(|b| !b.is_empty()), "block table present after render");
}

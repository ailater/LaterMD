use egui::{vec2, Color32, Context, Id, RawInput, Rect, UiBuilder};
use egui_markdown::{MarkdownLabel, MarkdownStyle};

const TABLE_MD: &str = "| H1 | H2 | H3 |\n|---|---|---|\n| a | b | c |\n| d | e | f |\n| g | h | i |\n";

fn md_style(header_fill: bool, zebra_fill: bool) -> MarkdownStyle {
  let mut style = MarkdownStyle::default();
  style.table.header_fill = header_fill;
  style.table.zebra_fill = zebra_fill;
  style
}

/// Render `TABLE_MD` for three frames on one context (stable widget ids across
/// frames), returning the painted text and the faint-background rects of the
/// last frame.
fn render(header_fill: bool, zebra_fill: bool) -> (Vec<String>, Vec<Rect>) {
  let ctx = Context::default();
  let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(500.0, 2000.0));
  let style = md_style(header_fill, zebra_fill);
  let mut faint = Color32::TRANSPARENT;
  let mut texts = Vec::new();
  let mut faint_rects = Vec::new();
  for _ in 0..3 {
    let mut output = ctx.run_ui(RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
      faint = ui.visuals().faint_bg_color;
      let mut child = ui.new_child(UiBuilder::new().max_rect(screen));
      MarkdownLabel::new(Id::new("table_style"), TABLE_MD).style(&style).show(&mut child);
    });
    texts.clear();
    faint_rects.clear();
    for clipped in &output.shapes {
      collect(&clipped.shape, faint, &mut texts, &mut faint_rects);
    }
    output.textures_delta.clear();
  }
  (texts, faint_rects)
}

fn collect(shape: &egui::epaint::Shape, faint: Color32, texts: &mut Vec<String>, faint_rects: &mut Vec<Rect>) {
  match shape {
    egui::epaint::Shape::Text(t) => texts.push(t.galley.text().to_owned()),
    egui::epaint::Shape::Rect(r) if r.fill == faint && r.rect.is_positive() => faint_rects.push(r.rect),
    egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect(s, faint, texts, faint_rects)),
    _ => {}
  }
}

/// Every toggle combination must keep rendering the table across frames without
/// panicking; `zebra_fill` also exercises `striped` + `vscroll(false)` together.
#[test]
fn toggle_combinations_render_three_frames_without_panic() {
  for header_fill in [false, true] {
    for zebra_fill in [false, true] {
      let (texts, _) = render(header_fill, zebra_fill);
      assert!(
        texts.iter().any(|t| t.contains("H1")),
        "header_fill={header_fill} zebra_fill={zebra_fill}: table did not render"
      );
    }
  }
}

#[test]
fn header_fill_paints_faint_bg_rect_behind_header_cells() {
  let (_, off) = render(false, false);
  let (_, on) = render(true, false);
  assert!(on.len() >= 3, "one faint rect per header cell expected, got {}", on.len());
  assert!(on.len() > off.len(), "baseline painted faint rects already: {off:?}");
}

#[test]
fn zebra_fill_paints_faint_bg_stripes_on_body_rows() {
  let (_, off) = render(false, false);
  let (_, on) = render(false, true);
  assert!(on.len() >= 3, "striped body rows should paint faint rects, got {}", on.len());
  assert!(on.len() > off.len(), "baseline painted faint rects already: {off:?}");
}

#[test]
fn table_geometry_has_grid_order_and_min_width() {
  use egui_markdown::{
    parse,
    table::{render_table_with_options, table_geometry, TableRenderOptions},
  };
  let ctx = Context::default();
  let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(500.0, 500.0));
  let md = parse(TABLE_MD);
  let data = md
    .tokens
    .iter()
    .find_map(|t| match t {
      egui_markdown::Token::Table(t) => Some(t),
      _ => None,
    })
    .unwrap();
  let id = Id::new("geometry-test");
  let mut output = ctx.run_ui(RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
    let font = egui::FontId::proportional(14.0);
    let style = MarkdownStyle::default();
    let mut opts = TableRenderOptions { cell_renderer: None, min_width: 480.0 };
    render_table_with_options(ui, id, data, &font, Color32::WHITE, &style.inline_code, &style.table, None, &mut opts);
  });
  output.textures_delta.clear();
  let geometry = table_geometry(&ctx, id).expect("geometry recorded");
  assert!(geometry.table.width() >= 480.0);
  assert_eq!(geometry.cells.len(), 12);
  for row in 0..4 {
    let cells: Vec<_> = geometry.cells.iter().filter(|(r, _, _, _)| *r == row).collect();
    assert_eq!(cells.len(), 3);
    assert!(cells[0].3.min.x < cells[1].3.min.x && cells[1].3.min.x < cells[2].3.min.x);
    if row > 0 {
      let prev = geometry.cells.iter().find(|(r, c, _, _)| *r == row - 1 && *c == 0).unwrap();
      assert!(cells[0].3.min.y > prev.3.min.y);
    }
  }
  assert!(geometry.viewport.is_positive());
}

#[test]
fn empty_rows_keep_a_font_line_height() {
  use egui_markdown::{
    parse,
    table::{render_table_with_options, table_geometry, TableRenderOptions},
  };
  let ctx = Context::default();
  let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(500.0, 500.0));
  let md = parse("| A | B |\n|---|---|\n|  |  |\n");
  let data = md
    .tokens
    .iter()
    .find_map(|t| match t {
      egui_markdown::Token::Table(t) => Some(t),
      _ => None,
    })
    .unwrap();
  let id = Id::new("empty-row-height");
  let mut output = ctx.run_ui(RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
    let font = egui::FontId::proportional(14.0);
    let style = MarkdownStyle::default();
    let mut opts = TableRenderOptions { cell_renderer: None, min_width: 0.0 };
    render_table_with_options(ui, id, data, &font, Color32::WHITE, &style.inline_code, &style.table, None, &mut opts);
  });
  output.textures_delta.clear();
  let geometry = table_geometry(&ctx, id).expect("geometry recorded");
  let header = geometry.cells.iter().find(|(r, _, h, _)| *r == 0 && *h).unwrap().3;
  let body = geometry.cells.iter().find(|(r, _, h, _)| *r == 1 && !*h).unwrap().3;
  assert!(body.height() >= header.height() - 0.1, "empty row collapsed: header={:?} body={:?}", header, body);
}

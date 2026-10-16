//! Estimated laid-out height of [`MarkdownLabel`] content, without shaping.
//!
//! Shaping is the dominant cost of laying out a large document, and a host that
//! manages its own viewport (a block-loop editor, a virtualized list) often
//! knows a block is off-screen before it renders it. For those blocks the exact
//! height is not needed — any close-enough placeholder is, because the block
//! will be shaped for real the moment it can become visible. This module
//! produces that placeholder: a structural walk over the parsed tokens that
//! mirrors the render paths' row semantics and converts them into an estimate
//! through per-glyph advances, which are exact in egui's kern-free shaper —
//! summing `Fonts::glyph_width` over a run reproduces its shaped advance.
//!
//! The walk simulates the layout job's text concatenation segment by segment:
//! every newline terminates a segment and opens a new one (an empty segment
//! costs one row, exactly as epaint's empty-paragraph height), content
//! accumulates advance and row height into the current segment, and a flowed
//! segment's rows are `ceil(advance / width)` — exact when the label breaks
//! anywhere, within word-wrap slack otherwise. Constructs (fences, tables,
//! images, block widgets, blockquotes) take the segmented render path, whose
//! flushed text ranges trim trailing newlines and charge inter-block spacing;
//! the walk mirrors both [`crate::layout::build_layout`] (text) and the
//! segmented renderer's `render_token_range` (flush boundaries, spacing,
//! trailing-newline skips) semantics.
//!
//! Accuracy by construct: plain text, headings, lists, inline code, horizontal
//! rules and task markers are exact to within a row per paragraph; fences —
//! scrolling frames and inline ones alike — are per-line exact; tables, images
//! and block widgets are structural approximations (a capped row each, a
//! loading box) that are corrected by measurement once the host renders them.
//!
//! The model is cached per widget id and text hash, so re-estimating at a new
//! width — every frame of an interactive resize — is arithmetic only.

use egui::{Color32, FontFamily, FontId, Ui};

use crate::label::hash_text;
use crate::layout::line_height_for;
use crate::link::LinkHandler;
use crate::parser;
use crate::style::MarkdownStyle;
use crate::types::Token;

/// Height placeholder for constructs whose real layout depends on data the
/// token stream does not carry: block-widget links (host-drawn cards) and
/// images (unknown until their texture loads). Both are corrected by
/// measurement on the first frame the construct can become visible.
const UNKNOWN_CONSTRUCT_HEIGHT: f32 = 24.0;

/// Header strip above a scrolling code block with copy controls — mirrors
/// `CODE_BLOCK_HEADER_HEIGHT` in `label.rs` (kept private there).
const CODE_BLOCK_HEADER_HEIGHT: f32 = 24.0;

/// The marker slot [`crate::layout::build_layout`] right-aligns list markers in.
const MARKER_SLOT_REFERENCE: &str = "99. ";

/// One segment of the simulated job text: flowed content wrapped at the width,
/// or a fixed-height row (heading spacers, horizontal rules).
#[derive(Clone, Copy, Debug)]
struct Segment {
  advance: f32,
  line_height: f32,
  /// When set, the segment is one row of exactly this height.
  fixed: Option<f32>,
}

impl Segment {
  fn empty() -> Self {
    Self { advance: 0.0, line_height: 0.0, fixed: None }
  }

  fn add(&mut self, advance: f32, line_height: f32) {
    self.advance += advance;
    self.line_height = self.line_height.max(line_height);
  }

  fn is_empty(&self) -> bool {
    self.advance <= 0.0 && self.fixed.is_none()
  }
}

/// The width-independent height structure of one label's content.
#[derive(Clone, Debug, Default)]
struct HeightModel {
  /// Flowed segments: their row count depends on the wrap width.
  runs: Vec<(f32, f32)>,
  /// Height that does not wrap: empty rows, spacer rows, horizontal rules,
  /// fenced code frames, tables, images, block widgets, inter-block spacing.
  fixed: f32,
}

impl HeightModel {
  fn push_run(&mut self, advance: f32, line_height: f32) {
    if advance > 0.0 {
      self.runs.push((advance, line_height));
    }
  }

  fn height_at(&self, wrap_width: f32) -> f32 {
    self.fixed
      + self
        .runs
        .iter()
        .map(|&(advance, line_height)| line_height * (advance / wrap_width.max(1.0)).ceil().max(1.0))
        .sum::<f32>()
  }
}

/// Cached model keyed by the same text hash the render cache uses, so the two
/// caches invalidate together.
#[derive(Clone, Debug)]
struct CachedModel {
  text_hash: u64,
  model: HeightModel,
}

/// Shared, label-derived configuration for the estimation walk.
struct EstCtx<'a> {
  style: &'a MarkdownStyle,
  handler: Option<&'a dyn LinkHandler>,
  scroll_code_blocks: bool,
  code_block_buttons: bool,
}

/// The walk's accumulated output.
struct WalkState {
  model: HeightModel,
  body_line_height: f32,
}

/// Everything the token walk needs per call, bound once.
struct Walker<'a, 'c> {
  ui: &'a Ui,
  ctx: &'a EstCtx<'c>,
  wrap_width: f32,
  /// Segments already terminated by a newline (or the walk's end).
  done: Vec<Segment>,
  /// The segment currently accepting content.
  current: Segment,
  /// Any token precedes this one within the current flushed range — the
  /// heading spacer fires only for a heading that is not first in its range
  /// (the renderer's check is on the slice index).
  flush_has_prior_token: bool,
  /// Simulated `text_start < i`: anything has been consumed since the last
  /// construct's trailing-newline skip. Drives the `before_block` spacing.
  text_start_lags: bool,
  /// A construct was seen: the label takes the segmented render path, whose
  /// flushed ranges trim trailing newlines.
  saw_construct: bool,
}

impl Walker<'_, '_> {
  #[inline]
  fn glyph(&self, font: &FontId, ch: char) -> f32 {
    self.ui.ctx().fonts_mut(|f| f.glyph_width(font, ch))
  }

  #[inline]
  fn text_advance(&self, font: &FontId, text: &str) -> f32 {
    let mut advance = 0.0;
    self.ui.ctx().fonts_mut(|f| {
      for ch in text.chars() {
        advance += f.glyph_width(font, ch);
      }
    });
    advance
  }

  #[inline]
  fn code_row_height(&self) -> f32 {
    let mono = FontId::monospace(self.ctx.style.code_font_size);
    self.ui.ctx().fonts_mut(|f| f.row_height(&mono))
  }

  fn bold_font(&self, font: &FontId) -> FontId {
    // Mirrors `apply_bold`: the bold family is only used when the host
    // registered one; otherwise bold is a color-only change with identical
    // metrics.
    let bold_family = FontFamily::Name("bold".into());
    let registered = self.ui.ctx().fonts(|f| f.families().contains(&bold_family));
    if registered {
      let mut font = font.clone();
      font.family = bold_family;
      font
    } else {
      font.clone()
    }
  }

  /// Simulate appending "\n": terminate the current segment, open a new one.
  fn newline(&mut self) {
    let empty = Segment::empty();
    self.done.push(std::mem::replace(&mut self.current, empty));
  }

  /// Add flowed content to the current segment.
  fn content(&mut self, advance: f32, line_height: f32) {
    self.current.add(advance, line_height);
  }

  /// Replace the current (newline-terminated, hence empty) segment with a
  /// fixed-height row — the heading spacer and horizontal rule patterns.
  fn fixed_row(&mut self, height: f32) {
    self.current = Segment { advance: 0.0, line_height: 0.0, fixed: Some(height) };
  }

  /// Emit everything terminated so far into the model, then reset.
  ///
  /// `trim_trailing_empty` mirrors the render path: the segmented path's
  /// flushed ranges trim trailing newlines, so their empty segments (in
  /// `done` and the unterminated `current`) cost nothing; the whole-document
  /// path renders them as empty rows.
  fn flush(&mut self, walk: &mut WalkState, trim_trailing_empty: bool) {
    if !(trim_trailing_empty && self.current.is_empty()) {
      self.done.push(self.current);
    }
    if trim_trailing_empty {
      while self.done.last().is_some_and(|seg| seg.is_empty()) {
        self.done.pop();
      }
    }
    let body = walk.body_line_height;
    for seg in &self.done {
      match seg.fixed {
        Some(fixed) => walk.model.fixed += fixed,
        None if seg.advance > 0.0 => walk.model.push_run(seg.advance, seg.line_height),
        None => walk.model.fixed += body,
      }
    }
    self.done.clear();
    self.current = Segment::empty();
    self.flush_has_prior_token = false;
  }

  /// The token walk over one render range (a whole label, or a blockquote's
  /// inner tokens).
  fn walk_tokens(&mut self, walk: &mut WalkState, tokens: &[Token<'_>], font: &FontId) {
    let body = walk.body_line_height;

    let mut i = 0;
    while i < tokens.len() {
      match &tokens[i] {
        Token::Newline => {
          self.newline();
          self.flush_has_prior_token = true;
          self.text_start_lags = true;
          i += 1;
        }
        Token::Text { text, style: ts } => {
          if !text.is_empty() {
            if ts.inline_code {
              // build_layout appends a transparent space on each side.
              let pad = self.glyph(font, ' ');
              self.content(pad + self.text_advance(font, text) + pad, body);
            } else {
              let token_font = if ts.heading.is_some() || ts.bold {
                let mut token_font = font.clone();
                // build_layout scales the size first, then applies the bold
                // family — a bold heading renders in the bold face at the
                // scaled size.
                if let Some(level) = ts.heading {
                  // A heading that follows other content gets a spacer row —
                  // but only when it is not first in its flushed range (the
                  // renderer checks the slice index, which restarts after
                  // every construct boundary).
                  if self.flush_has_prior_token && i > 0 && matches!(tokens[i - 1], Token::Newline) {
                    self.fixed_row(self.ctx.style.block_spacing + self.ctx.style.heading_space_above);
                    self.newline();
                  }
                  let idx = (level as usize).saturating_sub(1).min(5);
                  token_font.size *= self.ctx.style.heading.scales[idx];
                }
                if ts.bold {
                  token_font = self.bold_font(&token_font);
                }
                token_font
              } else {
                font.clone()
              };
              let line_height = if ts.heading.is_some() {
                line_height_for(token_font.size, self.ctx.style).unwrap_or(body)
              } else {
                body
              };
              self.content(self.text_advance(&token_font, text), line_height);
            }
          }
          self.flush_has_prior_token = true;
          self.text_start_lags = true;
          i += 1;
        }
        Token::CodeBlock { text, language } => {
          // Scroll frames and block-code widgets are constructs; a plain small
          // fence flows inline in the range's galley and is not.
          let is_construct = self.ctx.scroll_code_blocks
            || self.ctx.handler.is_some_and(|h| h.is_block_code_widget(language.as_deref()));
          if is_construct {
            self.flush(walk, true);
            // The scroll-fence arm charges `before_block`; the block-widget
            // arm does not.
            if self.text_start_lags && !self.ctx.handler.is_some_and(|h| h.is_block_code_widget(language.as_deref())) {
              walk.model.fixed += self.ctx.style.block_spacing;
            }
            let rows = text.lines().count().max(1) as f32;
            let pad = &self.ctx.style.code_block.padding;
            let header = if self.ctx.code_block_buttons { CODE_BLOCK_HEADER_HEIGHT } else { 0.0 };
            walk.model.fixed += pad[1] + header + rows * self.code_row_height() + pad[3];
            self.saw_construct = true;
            self.consume_trailing_newlines(&mut i, tokens, walk);
            continue;
          }
          // Inline (or single-fence-segment) fence: each source line is padded
          // with a leading space and wraps at the monospace metrics — one
          // segment per line keeps the per-line ceiling exact.
          let mono = FontId::monospace(self.ctx.style.code_font_size);
          let mono_row = self.code_row_height();
          for (idx, line) in text.lines().enumerate() {
            if idx > 0 {
              self.newline();
            }
            let mut padded = String::with_capacity(line.len() + 1);
            padded.push(' ');
            padded.push_str(line);
            self.content(self.text_advance(&mono, &padded), mono_row);
          }
          self.flush_has_prior_token = true;
          self.text_start_lags = true;
          i += 1;
        }
        Token::ListMarker { marker, indent_level } => {
          // Mirrors build_layout's slot math: a right-aligned marker inside a
          // fixed-width slot, nudged, preceded by nesting indentation.
          let list = &self.ctx.style.list;
          let is_ordered = marker.trim_start().starts_with(|c: char| c.is_ascii_digit());
          let nudge = if is_ordered { list.number_nudge } else { list.bullet_nudge };
          let mut marker_font = font.clone();
          if !is_ordered {
            marker_font.size *= list.bullet_scale;
          }
          let marker_width: f32 = marker.chars().map(|c| self.glyph(&marker_font, c)).sum();
          let slot_width: f32 = MARKER_SLOT_REFERENCE.chars().map(|c| self.glyph(font, c)).sum();
          let indent_width = self.glyph(font, ' ') * 2.0;
          // Token-level blockquote depth is not tracked here; a marker inside
          // a blockquote is estimated by the recursive call with a narrower
          // width, and the depth term only shifts text rightward.
          let nesting = (*indent_level as f32 - 1.0).max(0.0) * indent_width;
          let leading = (nesting + slot_width - nudge - marker_width).max(0.0);
          let trailing = nesting + slot_width + list.gap - leading - marker_width;
          self.content(leading + marker_width + trailing.max(0.0), body);
          self.flush_has_prior_token = true;
          self.text_start_lags = true;
          i += 1;
        }
        Token::Link { text, href, .. } => {
          let is_block_widget = self.ctx.handler.is_some_and(|h| h.is_block_widget(href));
          if is_block_widget {
            self.flush(walk, true);
            // The block-widget arm charges no `before_block` spacing.
            walk.model.fixed += UNKNOWN_CONSTRUCT_HEIGHT;
            self.saw_construct = true;
            self.consume_trailing_newlines(&mut i, tokens, walk);
            continue;
          }
          if let Some(widget) = self.ctx.handler.and_then(|h| h.inline_widget_size(href, font)) {
            // Transparent placeholder text at monospace metrics, fixed height.
            let mono = FontId::monospace(font.size);
            self.content(self.text_advance(&mono, text), widget.y);
          } else {
            self.content(self.text_advance(font, text), body);
          }
          self.flush_has_prior_token = true;
          self.text_start_lags = true;
          i += 1;
        }
        Token::Image { .. } => {
          self.flush(walk, true);
          if self.text_start_lags {
            walk.model.fixed += self.ctx.style.block_spacing;
          }
          // Loading fallback box; corrected by measurement once the texture is
          // known. Oversized images shrink to the column width, so the
          // placeholder only ever underestimates.
          walk.model.fixed += UNKNOWN_CONSTRUCT_HEIGHT;
          self.saw_construct = true;
          self.consume_trailing_newlines(&mut i, tokens, walk);
          continue;
        }
        Token::Table(data) => {
          self.flush(walk, true);
          if self.text_start_lags {
            walk.model.fixed += self.ctx.style.block_spacing;
          }
          self.estimate_table(data, font, walk);
          self.saw_construct = true;
          self.consume_trailing_newlines(&mut i, tokens, walk);
          continue;
        }
        Token::HorizontalRule => {
          // build_layout appends a newline, a fixed-height spacer row, and
          // another newline — all inline in the flushed range.
          self.newline();
          self.fixed_row(self.ctx.style.horizontal_rule.height);
          self.newline();
          self.flush_has_prior_token = true;
          self.text_start_lags = true;
          i += 1;
        }
        Token::TaskListMarker { .. } => {
          self.content(self.text_advance(font, "☑ "), body);
          self.flush_has_prior_token = true;
          self.text_start_lags = true;
          i += 1;
        }
        Token::FootnoteRef { label } => {
          let mut small = font.clone();
          small.size *= 0.75;
          let row = self.ui.ctx().fonts_mut(|f| f.row_height(&small));
          self.content(self.text_advance(&small, label), row);
          self.flush_has_prior_token = true;
          self.text_start_lags = true;
          i += 1;
        }
        Token::FootnoteDef { label } => {
          let bold = self.bold_font(font);
          self.content(self.text_advance(&bold, &format!("{label}. ")), body);
          self.flush_has_prior_token = true;
          self.text_start_lags = true;
          i += 1;
        }
        Token::BlockquoteStart => {
          self.flush(walk, true);
          // The blockquote arm charges no `before_block` spacing.
          self.saw_construct = true;
          // Find the matching end (same scan render_token_range uses).
          let mut depth = 1u32;
          let mut j = i + 1;
          while j < tokens.len() && depth > 0 {
            match &tokens[j] {
              Token::BlockquoteStart => depth += 1,
              Token::BlockquoteEnd => depth -= 1,
              _ => {}
            }
            if depth > 0 {
              j += 1;
            }
          }
          let inner = &tokens[i + 1..j.min(tokens.len())];
          let mut child_width = self.wrap_width - self.ctx.style.blockquote.indent_per_depth;
          if child_width < 1.0 {
            child_width = self.wrap_width;
          }
          let mut child = Walker {
            ui: self.ui,
            ctx: self.ctx,
            wrap_width: child_width,
            done: Vec::new(),
            current: Segment::empty(),
            flush_has_prior_token: false,
            text_start_lags: false,
            saw_construct: true, // a blockquote forces segmentation
          };
          child.walk_tokens(walk, inner, font);
          self.consume_trailing_newlines(&mut i, tokens, walk);
          continue;
        }
        Token::BlockquoteEnd => {
          i += 1;
        }
      }
    }
    // End of range: the segmented path trims trailing newlines, the
    // whole-document path renders them as empty rows.
    self.flush(walk, self.saw_construct);
  }

  /// `after_block`: consume the newlines that follow a construct (they open no
  /// segments), charge the after-spacing when anything follows, and reset the
  /// `text_start` simulation (it catches up to the cursor at a construct).
  fn consume_trailing_newlines(&mut self, i: &mut usize, tokens: &[Token<'_>], walk: &mut WalkState) {
    while *i + 1 < tokens.len() && matches!(tokens[*i + 1], Token::Newline) {
      *i += 1;
    }
    if *i + 1 < tokens.len() {
      walk.model.fixed += self.ctx.style.block_spacing;
    }
    *i += 1;
    self.text_start_lags = false;
    self.flush_has_prior_token = false;
  }

  /// Table estimate: one row per header/data row at the body row height, with
  /// the cell padding from the style. Cell wrapping is not modeled (columns
  /// are assumed wide enough) — wide cells underestimate and are corrected by
  /// measurement when the table approaches the viewport.
  fn estimate_table(&self, data: &crate::types::TableData<'_>, font: &FontId, walk: &mut WalkState) {
    let pad = &self.ctx.style.table.cell_padding;
    let vpad = pad[1] + pad[3];
    let hpad = pad[0] + pad[2];
    let num_cols = data.alignments.len().max(1);
    let cell_width = ((self.wrap_width - hpad * num_cols as f32) / num_cols as f32).max(1.0);
    let row_height = walk.body_line_height;

    let cell_advance = |tokens: &[Token<'_>]| -> f32 {
      tokens
        .iter()
        .map(|token| match token {
          Token::Text { text, .. } | Token::Link { text, .. } | Token::CodeBlock { text, .. } => {
            self.text_advance(font, text)
          }
          _ => 0.0,
        })
        .sum()
    };

    let header_advance = data.headers.iter().map(|cell| cell_advance(cell)).fold(0.0_f32, f32::max);
    walk.model.push_run(header_advance.min(cell_width * 4.0), row_height);
    walk.model.fixed += vpad;
    for row in &data.rows {
      let row_advance = row.iter().map(|cell| cell_advance(cell)).fold(0.0_f32, f32::max);
      walk.model.push_run(row_advance.min(cell_width * 4.0), row_height);
      walk.model.fixed += vpad;
    }
  }
}

/// Everything about a label the height walk needs, resolved by the widget
/// itself (its fields are private to `label.rs`).
pub(crate) struct HeightParams<'a> {
  pub id: egui::Id,
  pub text: &'a str,
  pub heal: bool,
  pub font: FontId,
  pub wrap_width: f32,
  pub style: &'a MarkdownStyle,
  pub color: Color32,
  pub dark_mode: bool,
  pub handler: Option<&'a dyn LinkHandler>,
  pub scroll_code_blocks: bool,
  pub code_block_buttons: bool,
}

/// Estimate the laid-out height of the described label content at its wrap
/// width, without shaping. See the [module docs](self) for accuracy bounds.
pub(crate) fn estimated_height(ui: &Ui, p: HeightParams<'_>) -> f32 {
  let cache_id = p.id.with("height-model");
  let text_hash = hash_text(p.text, p.style, &p.font, p.color, p.dark_mode, p.handler);

  if let Some(cached) = ui.data(|d| d.get_temp::<CachedModel>(cache_id)) {
    if cached.text_hash == text_hash {
      return cached.model.height_at(p.wrap_width);
    }
  }

  let healed;
  let text = if p.heal {
    healed = parser::heal(p.text);
    &*healed
  } else {
    p.text
  };
  // The cache is keyed by the healed text's hash; a healed text re-walks.
  let model = height_model(ui, text, &p.font, p.style, p.wrap_width, &p);
  ui.data_mut(|d| {
    d.insert_temp(
      cache_id,
      CachedModel {
        text_hash: hash_text(text, p.style, &p.font, p.color, p.dark_mode, p.handler),
        model: model.clone(),
      },
    )
  });
  model.height_at(p.wrap_width)
}

fn height_model(
  ui: &Ui,
  text: &str,
  font: &FontId,
  style: &MarkdownStyle,
  wrap_width: f32,
  p: &HeightParams<'_>,
) -> HeightModel {
  let ctx = EstCtx {
    style,
    handler: p.handler,
    scroll_code_blocks: p.scroll_code_blocks,
    code_block_buttons: p.code_block_buttons,
  };
  let md = parser::parse(text);
  let body = line_height_for(font.size, style).unwrap_or_else(|| ui.ctx().fonts_mut(|f| f.row_height(font)));
  let mut walker = Walker {
    ui,
    ctx: &ctx,
    wrap_width,
    done: Vec::new(),
    current: Segment::empty(),
    flush_has_prior_token: false,
    text_start_lags: false,
    saw_construct: false,
  };
  let mut walk = WalkState { model: HeightModel::default(), body_line_height: body };
  walker.walk_tokens(&mut walk, &md.tokens, font);
  walk.model
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::label::MarkdownLabel;
  use egui::{Context, Id, RawInput, Rect, ScrollArea, UiBuilder};

  /// Shared harness: run one frame at `width`, hand the content `Ui` to the
  /// closure, return what it captured.
  fn frame<T>(width: f32, f: impl FnOnce(&mut Ui) -> T) -> T {
    let ctx = Context::default();
    let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, 4000.0));
    let mut out = None;
    let mut f = Some(f);
    let mut full = ctx.run_ui(RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
      let mut child = ui.new_child(UiBuilder::new().max_rect(screen));
      ScrollArea::vertical().show(&mut child, |ui| {
        out = f.take().map(|f| f(ui));
      });
    });
    full.textures_delta.clear();
    out.expect("closure ran exactly once")
  }

  /// Real (shaped) content height of the label at `width`.
  pub(super) fn rendered_height(id: Id, text: &str, width: f32) -> f32 {
    frame(width, |ui| {
      MarkdownLabel::new(id, text).wrap().show(ui);
      ui.min_rect().height()
    })
  }

  pub(super) fn estimate_at(id: Id, text: &str, width: f32) -> f32 {
    frame(width, |ui| {
      MarkdownLabel::new(id, text).wrap().overflow_wrap(crate::OverflowWrap::BreakAll).estimated_height(ui)
    })
  }

  pub(super) fn sample_doc() -> String {
    let mut doc = String::new();
    use std::fmt::Write as _;
    for section in 1..=8 {
      let _ = writeln!(doc, "## Section {section}");
      let _ = writeln!(doc);
      let _ = writeln!(
        doc,
        "这是一段用于测量的中文正文,混排 English words 与 **加粗**、`行内代码`、[链接](https://example.com/{section})。"
      );
      let _ = writeln!(doc);
      let _ = writeln!(doc, "- 列表项一\n- 列表项二\n  - 嵌套项\n- 列表项三");
      let _ = writeln!(doc);
      if section % 2 == 0 {
        let _ = writeln!(doc, "```rust");
        let _ = writeln!(doc, "fn claim_{section}(log: &Log, seq: u64) -> Result<(), Error> {{");
        for step in 0..6 {
          let _ = writeln!(doc, "    let step_{step} = log.tail()?;");
        }
        let _ = writeln!(doc, "}}");
        let _ = writeln!(doc, "```");
        let _ = writeln!(doc);
      }
      if section == 3 {
        let _ = writeln!(doc, "| 列甲 | 列乙 |");
        let _ = writeln!(doc, "|---|---|");
        let _ = writeln!(doc, "| 单元 1 | cell 1 |");
        let _ = writeln!(doc);
      }
    }
    doc
  }

  #[test]
  fn estimate_tracks_real_height_within_tolerance() {
    let doc = sample_doc();
    for width in [320.0, 480.0, 700.0, 1000.0] {
      let real = rendered_height(Id::new("est-real"), &doc, width);
      let est = estimate_at(Id::new("est-calc"), &doc, width);
      let tolerance = (real * 0.10).max(48.0);
      assert!(
        (est - real).abs() <= tolerance,
        "width {width}: estimate {est:.1} vs real {real:.1} (tolerance {tolerance:.1})"
      );
    }
  }

  #[test]
  fn estimate_is_monotonic_in_width() {
    let doc = sample_doc();
    let narrow = estimate_at(Id::new("est-mono"), &doc, 360.0);
    let wide = estimate_at(Id::new("est-mono"), &doc, 900.0);
    assert!(
      wide <= narrow,
      "wider wrap must not lay out taller ({wide:.1} vs {narrow:.1}); ties are legal (runs that fit both widths)"
    );
  }

  #[test]
  fn estimate_is_stable_across_calls() {
    let doc = sample_doc();
    let a = estimate_at(Id::new("est-stable"), &doc, 600.0);
    let b = estimate_at(Id::new("est-stable"), &doc, 600.0);
    assert_eq!(a, b, "same inputs must produce the same estimate");
  }

  #[test]
  fn fence_estimate_is_near_exact() {
    let mut fence = String::from("```rust\nfn main() {\n");
    for i in 0..20 {
      use std::fmt::Write as _;
      let _ = writeln!(fence, "    let step_{i} = 1;");
    }
    fence.push_str("}\n```");
    let width = 700.0;
    let real = rendered_height(Id::new("fence-real"), &fence, width);
    let est = estimate_at(Id::new("fence-est"), &fence, width);
    assert!((est - real).abs() <= real * 0.05, "fence estimate {est:.1} vs real {real:.1}");
  }

  #[test]
  fn blank_lines_and_headings_cost_rows_in_both_paths() {
    let doc = "# Title\n\n\npara one\n\npara two\n";
    let real = rendered_height(Id::new("edge-real"), doc, 600.0);
    let est = estimate_at(Id::new("edge-est"), doc, 600.0);
    let tolerance = (real * 0.10).max(24.0);
    assert!((est - real).abs() <= tolerance, "estimate {est:.1} vs real {real:.1}");
  }
}

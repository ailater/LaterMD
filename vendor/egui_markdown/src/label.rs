//! The [`MarkdownLabel`] widget for rendering interactive markdown.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{
  text::LayoutJob, text_selection::LabelSelectionState, vec2, Align, Color32, Context, CursorIcon, FontId,
  FontSelection, Id, Layout, OpenUrl, Pos2, Rect, Response, Sense, Stroke, TextWrapMode, Ui, UiBuilder, Vec2,
};
use epaint::{
  pos2,
  text::{ByteRangeExt as _, Galley, Glyph, Row},
};

#[cfg(not(feature = "syntax_highlighting"))]
use crate::layout::highlight_code;
use crate::layout::{
  build_layout, code_block_admits_segmentation, needs_segmentation, section_for_char, CodeThemeArg, LayoutResult,
};
#[cfg(feature = "syntax_highlighting")]
use crate::layout::{scrolling_code_galley, StreamingCodeCache};
use crate::link::LinkHandler;
use crate::paint;
use crate::parser;
use crate::style::MarkdownStyle;
use crate::table;
use crate::types::{Markdown, Token};

/// Height reserved for the optional language/copy controls above a scrolling
/// code block. The controls are painted as an overlay, so the frame must move
/// the code galley below them to keep its first line readable.
const CODE_BLOCK_HEADER_HEIGHT: f32 = 24.0;

#[cfg(feature = "syntax_highlighting")]
pub use crate::theme::default_code_theme;

/// Cached parse + layout result stored in egui temp memory.
#[derive(Clone)]
struct CachedMarkdownLayout {
  text_hash: u64,
  /// `None` when the document takes the segmented render path, which lays out each flush
  /// range separately and so has no use for a whole-document layout.
  layout: Option<Arc<LayoutResult>>,
  /// Owned copy of the tokens for rendering (needed for hover/click after cache hit).
  tokens: Arc<Vec<Token<'static>>>,
  /// Source byte ranges parallel to `tokens`, so geometry recording (block span
  /// rects) keeps working on cache hits without re-parsing the document.
  spans: Arc<Vec<std::ops::Range<usize>>>,
}

/// Cached layout result for a single flush range (used by segmented render path).
#[derive(Clone)]
struct CachedFlushRange {
  /// True when this range's code blocks were built without syntect colors
  /// (deferred highlight, [`MarkdownLabel::defer_offscreen_highlight`]) and
  /// must be rebuilt with highlighting before the range can become visible.
  /// Text, metrics and geometry are already final, so sizes and block tables
  /// recorded while deferred stay valid.
  deferred_highlight: bool,
  ctx_hash: u64,
  layout: Arc<LayoutResult>,
  tokens: Arc<Vec<Token<'static>>>,
  /// The range's shaped galley from a previous frame, valid while the wrap it
  /// was shaped at still holds. See [`CachedShapedGalley`].
  galley: Option<CachedShapedGalley>,
}

/// A galley shaped from a flush range's cached job, at one wrap and zoom.
///
/// [`Fonts::layout_job`](egui::epaint::text::Fonts::layout_job) finds a cached
/// galley by hashing the whole job — for a large range that hash dominates an
/// otherwise fully-cached frame. The shaped result is kept here instead, keyed
/// by the wrap inputs that produced it (the job itself is pinned by the range's
/// `ctx_hash`, and `pixels_per_point` by egui's own galley cache). The inputs
/// re-shape deterministically, so a hit is exactly what `layout_job` would
/// return; any mismatch falls back to the normal shape path.
#[derive(Clone)]
struct CachedShapedGalley {
  max_width: f32,
  break_anywhere: bool,
  pixels_per_point: f32,
  galley: Arc<Galley>,
  /// Section anchors relative to the galley origin, from the shaping pass.
  anchors: Vec<SectionAnchor>,
}

/// What [`MarkdownLabel::render_galley`] shapes and paints.
enum ShapedJob {
  /// A fresh job to wrap and shape through the font layer.
  Job(LayoutJob),
  /// A galley already shaped from the identical job at the current wrap, with
  /// its section anchors as laid out relative to the galley origin.
  Galley { galley: Arc<Galley>, anchors: Vec<SectionAnchor> },
}

#[cfg(not(feature = "syntax_highlighting"))]
#[derive(Clone)]
struct CachedCodeBlock {
  ctx_hash: u64,
  galley: Arc<Galley>,
}

#[inline]
pub(crate) fn hash_text(
  text: &str,
  style: &MarkdownStyle,
  font: &FontId,
  color: Color32,
  dark_mode: bool,
  handler: Option<&dyn LinkHandler>,
) -> u64 {
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  text.hash(&mut hasher);
  style.hash(&mut hasher);
  // The layout job stores resolved text colors and font metrics. Keeping these
  // out of the whole-document key lets a light/dark switch reuse a galley from
  // the previous theme, which paints stale colors and can reuse stale geometry
  // after the host font chain changes.
  font.hash(&mut hasher);
  color.hash(&mut hasher);
  dark_mode.hash(&mut hasher);
  if let Some(h) = handler {
    h.id().hash(&mut hasher);
  }
  hasher.finish()
}

/// Identity of one block widget's own content: its token, plus the same context a
/// whole-document [`hash_text`] key covers (style, handler id).
///
/// The height cache under `("block_sz", index)` keys its entry by token index and
/// verifies it with this hash, so the hash range must be exactly the block: the
/// whole-document hash would invalidate every block on any edit. The block's
/// *source span* looks like a cheaper range (one contiguous `&str` hash), but spans
/// tile the source contiguously: a block's span absorbs the blank separators before
/// it and stops at the end of the block body (the closing fence line is not part of
/// it), so the same construct at a different position hashes differently. The token
/// is exact — an identical token renders an identical widget at the same height,
/// including nested table cells. Keeping style and handler id in the key preserves
/// the old invalidation: a style change still re-measures every block.
#[inline]
fn hash_block_content(token: &Token<'_>, style_context: u64) -> u64 {
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  token.hash(&mut hasher);
  style_context.hash(&mut hasher);
  hasher.finish()
}

/// The style-and-handler share of [`hash_block_content`]'s key, hashed once per
/// range instead of per block: hashing the full [`MarkdownStyle`] for every
/// block widget is measurable on table-heavy documents with hundreds of blocks
/// per frame.
fn hash_block_context(style: &MarkdownStyle, handler: Option<&dyn LinkHandler>) -> u64 {
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  style.hash(&mut hasher);
  if let Some(h) = handler {
    h.id().hash(&mut hasher);
  }
  hasher.finish()
}

/// Identity of everything a [`LayoutResult`] depends on.
///
/// Deliberately excludes the wrap width and break flag: `build_layout` only seeds
/// those on `job.wrap`, and `render_galley` overwrites them with the live values
/// before shaping. Hashing them here would rebuild every job on every frame of a resize.
#[inline]
fn hash_flush_context(
  tokens: &[Token<'_>],
  style: &MarkdownStyle,
  font: &FontId,
  color: Color32,
  dark_mode: bool,
  handler: Option<&dyn LinkHandler>,
) -> u64 {
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  // Token content must be hashed: a flush range is keyed by its start index, and an edit
  // can change a range's content while leaving its start index and length untouched.
  tokens.hash(&mut hasher);
  style.hash(&mut hasher);
  font.hash(&mut hasher);
  color.hash(&mut hasher);
  dark_mode.hash(&mut hasher);
  if let Some(h) = handler {
    h.id().hash(&mut hasher);
  }
  hasher.finish()
}

/// Identity of everything a scrolling code-block galley depends on except the source text.
#[cfg(feature = "syntax_highlighting")]
#[inline]
fn hash_code_block_identity(
  ui: &Ui,
  language: &str,
  code_font_size: f32,
  dark_mode: bool,
  pixels_per_point: f32,
  code_theme: CodeThemeArg<'_>,
) -> u64 {
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  language.hash(&mut hasher);
  code_font_size.to_bits().hash(&mut hasher);
  dark_mode.hash(&mut hasher);
  pixels_per_point.to_bits().hash(&mut hasher);
  crate::layout::theme_identity_ptr(ui, code_theme).hash(&mut hasher);
  hasher.finish()
}

#[cfg(not(feature = "syntax_highlighting"))]
#[inline]
fn hash_code_block_context(
  text: &str,
  language: &str,
  code_font_size: f32,
  dark_mode: bool,
  pixels_per_point: f32,
  code_theme: CodeThemeArg<'_>,
) -> u64 {
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  text.hash(&mut hasher);
  language.hash(&mut hasher);
  code_font_size.to_bits().hash(&mut hasher);
  dark_mode.hash(&mut hasher);
  pixels_per_point.to_bits().hash(&mut hasher);
  let _ = code_theme;
  hasher.finish()
}

/// Reserve space for an off-screen block using cached height and current available width.
/// Width is not cached so a panel shrink cannot leave a stale wider min-size.
/// Record a block widget's coarse entry: span is the token's source range,
/// rect spans from just before the widget rendered to just after, at the
/// available width. The widget's internal geometry stays opaque — block-level
/// granularity is what jump-to-block consumers need.
fn record_widget_block(ui: &egui::Ui, id: Id, span: std::ops::Range<usize>, before_rect: Rect, after_y: f32) {
  let rect = Rect::from_min_max(pos2(before_rect.left(), before_rect.min.y), pos2(before_rect.right(), after_y));
  record_block_rect(ui, id, span, rect);
}

/// Cull an off-screen block widget using its cached height.
///
/// The entry is keyed by the block's token index and verified by the block's
/// content hash ([`hash_block_content`]): the index is stable while edits append
/// at the end of the document, and an edit that changes the token count earlier
/// shifts later indices to fresh — never stale — entries. A hash match therefore
/// means the cached height was measured for this exact block.
///
/// Returns `Some(rect)` when the block was culled — the rect it would have
/// occupied, so callers can still record its geometry — and `None` when the
/// block must be rendered normally.
fn try_cull_block(ui: &mut Ui, block_sz_id: Id, content_hash: u64) -> Option<Rect> {
  let (cached_hash, cached_height) = ui.data(|d| d.get_temp::<(u64, f32)>(block_sz_id))?;
  if cached_hash != content_hash {
    return None;
  }
  let size = Vec2::new(ui.available_width(), cached_height);
  let est_rect = Rect::from_min_size(ui.available_rect_before_wrap().min, size);
  if ui.is_rect_visible(est_rect) {
    return None;
  }
  ui.allocate_space(size);
  Some(est_rect)
}

fn cache_block_height(ui: &mut Ui, block_sz_id: Id, content_hash: u64, before_y: f32) {
  let height = ui.min_rect().bottom() - before_y;
  ui.data_mut(|d| d.insert_temp(block_sz_id, (content_hash, height)));
}

/// Convert borrowed tokens to owned ('static) by converting CowStr::Borrowed to Boxed.
fn tokens_to_owned(tokens: &[Token<'_>]) -> Vec<Token<'static>> {
  tokens.iter().map(|t| token_to_owned(t)).collect()
}

fn cowstr_to_owned(s: &pulldown_cmark::CowStr<'_>) -> pulldown_cmark::CowStr<'static> {
  pulldown_cmark::CowStr::Boxed(s.to_string().into_boxed_str())
}

fn token_to_owned(t: &Token<'_>) -> Token<'static> {
  match t {
    Token::Newline => Token::Newline,
    Token::Text { text, style } => Token::Text { text: cowstr_to_owned(text), style: style.clone() },
    Token::CodeBlock { text, language } => {
      Token::CodeBlock { text: cowstr_to_owned(text), language: language.as_ref().map(|l| cowstr_to_owned(l)) }
    }
    Token::Link { text, href, title, heading } => Token::Link {
      text: cowstr_to_owned(text),
      href: cowstr_to_owned(href),
      title: title.as_ref().map(|t| cowstr_to_owned(t)),
      heading: *heading,
    },
    Token::ListMarker { marker, indent_level } => {
      Token::ListMarker { marker: cowstr_to_owned(marker), indent_level: *indent_level }
    }
    Token::Image { alt, url, title } => Token::Image {
      alt: cowstr_to_owned(alt),
      url: cowstr_to_owned(url),
      title: title.as_ref().map(|t| cowstr_to_owned(t)),
    },
    Token::Table(data) => Token::Table(crate::types::TableData {
      alignments: data.alignments.clone(),
      headers: data.headers.iter().map(|row| tokens_to_owned(row)).collect(),
      rows: data.rows.iter().map(|row| row.iter().map(|cell| tokens_to_owned(cell)).collect()).collect(),
    }),
    Token::HorizontalRule => Token::HorizontalRule,
    Token::BlockquoteStart => Token::BlockquoteStart,
    Token::BlockquoteEnd => Token::BlockquoteEnd,
    Token::TaskListMarker { checked, indent_level } => {
      Token::TaskListMarker { checked: *checked, indent_level: *indent_level }
    }
    Token::FootnoteRef { label } => Token::FootnoteRef { label: cowstr_to_owned(label) },
    Token::FootnoteDef { label } => Token::FootnoteDef { label: cowstr_to_owned(label) },
  }
}

/// What the layout does with a token that is wider than the available width.
///
/// This setting is independent of [`MarkdownLabel::wrap_mode`]. The wrap mode selects
/// whether the box wraps, truncates, or extends. This enum selects what happens when one
/// run of characters still does not fit.
///
/// The variants are close to the CSS `overflow-wrap` and `word-break` properties. A
/// `BreakWord` variant, which would wrap at a space and split a token only when that
/// token alone is wider than the box, needs support from the text engine. This crate
/// does not declare that variant until the support exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum OverflowWrap {
  /// Prefer a word boundary, and overrun the box when a token has none.
  ///
  /// This matches CSS `overflow-wrap: normal`.
  #[default]
  Normal,
  /// Break between any two characters when the row reaches `max_width`.
  ///
  /// This matches CSS `word-break: break-all`. It can split an ordinary sentence inside
  /// a word.
  BreakAll,
}

/// Resolved wrap settings for one layout/paint pass.
struct ResolvedWrap {
  mode: TextWrapMode,
  max_width: f32,
  break_anywhere: bool,
  max_rows: Option<u32>,
}

/// An interactive markdown-rendered label widget for egui.
///
/// Parses markdown text and renders it with formatting, clickable links,
/// selectable text, code block overlays, and more.
///
/// # Example
///
/// ```no_run
/// # use eframe::egui;
/// fn ui(ui: &mut egui::Ui) {
///     egui_markdown::MarkdownLabel::new(ui.id().with("md"), "**Hello** world")
///         .show(ui);
/// }
/// ```
pub struct MarkdownLabel<'a> {
  id: Id,
  text: &'a str,
  font: Option<FontId>,
  max_lines: Option<u32>,
  wrap_mode: Option<TextWrapMode>,
  overflow_wrap: OverflowWrap,
  selectable: bool,
  interactable: bool,
  link_handler: Option<&'a dyn LinkHandler>,
  #[allow(clippy::type_complexity)]
  code_block_buttons: Option<&'a dyn Fn(&mut Ui, &str, &str)>,
  scroll_code_blocks: bool,
  shrink_code_blocks: bool,
  defer_offscreen_highlight: bool,
  hug_content: bool,
  code_block_min_width: Option<f32>,
  style: Option<&'a MarkdownStyle>,
  heal: bool,
  #[allow(clippy::type_complexity)]
  map_job: Option<Box<dyn Fn(&Context, LayoutJob) -> LayoutJob + 'a>>,
  #[cfg(feature = "syntax_highlighting")]
  code_theme: Option<&'a syntect::highlighting::Theme>,
}

/// One laid-out section, with enough geometry for callers to scroll to it.
///
/// `MarkdownLabel` paints everything into a single galley, so a caller (e.g. an
/// outline pane) has no way to know where a given section ended up. Recording the
/// anchors lets it say "scroll the preview to this heading" without reaching into
/// galley internals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionAnchor {
  /// Byte offset of the section start **within the rendered text**
  /// (the text handed to [`MarkdownLabel::new`], after any `heal`/pre-processing).
  pub byte_start: usize,
  /// Y offset of the section's first row, relative to the label's top-left corner.
  pub y: f32,
}

/// Read the anchors recorded by the last render of the label with this `id`.
///
/// Returns `None` when the label has not been rendered in the current frame
/// (e.g. culled), in which case callers should simply not scroll.
pub fn section_anchors(ui: &egui::Ui, id: egui::Id) -> Option<Vec<SectionAnchor>> {
  ui.data(|d| d.get_temp(id.with("section-anchors")))
}

/// One rendered top-level block: where it came from in the source text, and
/// where it ended up on screen.
///
/// `span` is a byte range **within the text handed to [`MarkdownLabel::new`]**
/// (i.e. after any pre-processing the caller did — the label knows nothing
/// about text it was not given). `rect` is in screen coordinates for the frame
/// the label was rendered in, so it can be fed straight to [`Ui::scroll_to_rect`]
/// — within that same frame; in a later frame it is stale.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockSpanRect {
  /// Byte range of the block in the text handed to the label.
  pub span: std::ops::Range<usize>,
  /// Screen-space rect the block occupied in the last render pass.
  pub rect: Rect,
}

/// Data key under which a label's block table is stored.
fn block_rects_id(id: egui::Id) -> egui::Id {
  id.with("block-span-rects")
}

/// One painted text galley of a label, with its screen origin, kept for the
/// frame it was painted in so callers can hit-test click positions.
///
/// A label on the segmented render path paints several text galleys (one per
/// flush range); block widgets (code fences, tables) are separate widgets and
/// deliberately absent — clicking them resolves to `None` here, and callers
/// fall back to the block table ([`block_span_rects`]).
#[derive(Clone)]
struct HitGalley {
  galley: Arc<Galley>,
  /// Screen-space top-left of the galley in the frame it was painted.
  origin: Pos2,
}

fn hit_galleys_id(id: egui::Id) -> egui::Id {
  id.with("hit-galleys")
}

/// Read the block table recorded by the last render of the label with this `id`.
///
/// The table is only valid in the frame it was written (screen rects go stale
/// the moment anything scrolls or resizes), so reading it in a later frame —
/// or before the label ever rendered — returns `None`.
///
/// **The reset pass is explicit.** `render` clears the table for the current
/// frame before laying anything out, so a frame whose label rendered **only
/// estimated placeholders** (a host-side viewport cull; see `crate::estimate`)
/// reports `None` — the absence is observable, which is exactly what a host
/// needs to distinguish "rendered but culled by the layout cache" from "never
/// rendered this frame".
pub fn block_span_rects(ui: &egui::Ui, id: egui::Id) -> Option<Vec<BlockSpanRect>> {
  let (frame, blocks) = ui.data(|d| d.get_temp::<(u64, Vec<BlockSpanRect>)>(block_rects_id(id)))?;
  (frame == ui.ctx().cumulative_pass_nr()).then_some(blocks)
}

/// Whether the label with this `id` recorded a block table **this frame**.
///
/// A cheaper probe than [`block_span_rects`] for hosts that only need to know
/// whether a label really rendered (as opposed to being estimated away):
/// it returns `true` exactly when the table exists and belongs to the current
/// pass. Headless tests use it to assert viewport-culling behavior.
pub fn block_span_rects_present(ui: &egui::Ui, id: egui::Id) -> bool {
  let Some((frame, _)) = ui.data(|d| d.get_temp::<(u64, Vec<BlockSpanRect>)>(block_rects_id(id))) else {
    return false;
  };
  frame == ui.ctx().cumulative_pass_nr()
}

/// Look up the on-screen rect of the block containing a byte `offset` in the
/// text handed to [`MarkdownLabel::new`].
///
/// Resolution order: an offset inside a block's span hits that block; an offset
/// in a gap between blocks (blank lines between a table and the next paragraph
/// belong to no token's content span) resolves to the nearest **following**
/// block; an offset past the end of the document resolves to the last block, so
/// the document end is always a legal jump target. Returns `None` when the
/// label was not rendered this frame or produced no blocks.
pub fn block_rect_at_offset(ui: &egui::Ui, id: egui::Id, offset: usize) -> Option<BlockSpanRect> {
  let blocks = block_span_rects(ui, id)?;
  if let Some(hit) = blocks.iter().find(|b| b.span.start <= offset && offset < b.span.end) {
    return Some(hit.clone());
  }
  blocks
    .iter()
    .filter(|b| b.span.start >= offset)
    .min_by_key(|b| (b.span.start, b.rect.min.y.to_bits()))
    .cloned()
    .or_else(|| blocks.iter().max_by_key(|b| (b.span.start, b.rect.min.y.to_bits())).cloned())
}

/// Append one block record to the label's per-frame table, resetting the table
/// on the first write of a frame (stale entries from earlier frames never leak).
///
/// Records mutate the stored table in place (`get_temp_mut_or_insert_with`).
/// A read-modify-write via `get_temp` + `insert_temp` clones the whole `Vec`
/// for every record, making a frame with N blocks cost O(N²) memcpy — measured
/// at 4673 records/frame on a 20k-line document, that alone was ~16 ms/frame.
fn record_block_rect(ui: &egui::Ui, id: egui::Id, span: std::ops::Range<usize>, rect: Rect) {
  let table_id = block_rects_id(id);
  // Read outside the data lock: `cumulative_pass_nr` acquires the context lock
  // in egui 0.36 and would self-deadlock inside the `data_mut` closure.
  let frame = ui.ctx().cumulative_pass_nr();
  ui.ctx().data_mut(|d| {
    let (seen, blocks) = d.get_temp_mut_or_insert_with::<(u64, Vec<BlockSpanRect>)>(table_id, || (frame, Vec::new()));
    if *seen != frame {
      *seen = frame;
      blocks.clear();
    }
    blocks.push(BlockSpanRect { span, rect });
  });
}

/// Record one painted text galley under the label's per-frame hit table.
fn record_hit_galley(ui: &egui::Ui, id: egui::Id, galley: &Arc<Galley>, origin: Pos2) {
  let table_id = hit_galleys_id(id);
  // Same lock discipline as `record_block_rect`: read the frame number
  // outside `data_mut`.
  let frame = ui.ctx().cumulative_pass_nr();
  ui.ctx().data_mut(|d| {
    let (seen, galleys) = d.get_temp_mut_or_insert_with::<(u64, Vec<HitGalley>)>(table_id, || (frame, Vec::new()));
    if *seen != frame {
      *seen = frame;
      galleys.clear();
    }
    galleys.push(HitGalley { galley: Arc::clone(galley), origin });
  });
}

/// Reset the label's per-frame hit-galley table (same discipline as the block
/// table reset in [`MarkdownLabel::render`]).
fn reset_hit_galleys(ui: &egui::Ui, id: egui::Id) {
  let table_id = hit_galleys_id(id);
  let frame = ui.ctx().cumulative_pass_nr();
  ui.ctx().data_mut(|d| {
    let (seen, galleys) = d.get_temp_mut_or_insert_with::<(u64, Vec<HitGalley>)>(table_id, || (frame, Vec::new()));
    *seen = frame;
    galleys.clear();
  });
}

/// Map a screen position onto the character index in the text handed to the
/// label with this `id` (after any `heal`/pre-processing), using the galleys
/// painted by its last render.
///
/// Editor hit-testing semantics: the row containing `pos.y` is found first;
/// within it, positions left of the first glyph collapse to the row start,
/// past a glyph's midpoint collapse to that glyph, and past the last glyph
/// collapse to just before the row's newline. Below the last row resolves to
/// the end of the text.
///
/// Returns `None` when the label was not rendered in the current frame (stale
/// or never painted, e.g. culled), or when `pos.y` falls outside every painted
/// text galley — gaps between blocks and block widgets (code fences, tables)
/// have no text geometry; callers that need a caret there can resolve through
/// [`block_span_rects`] instead.
pub fn char_index_at_pos(ui: &egui::Ui, id: egui::Id, pos: Pos2) -> Option<usize> {
  let (frame, galleys) = ui.data(|d| d.get_temp::<(u64, Vec<HitGalley>)>(hit_galleys_id(id)))?;
  (frame == ui.ctx().cumulative_pass_nr()).then_some(())?;
  for hit in &galleys {
    let bottom = hit.origin.y + hit.galley.size().y;
    if pos.y >= hit.origin.y && pos.y <= bottom {
      return Some(pos_to_char_index(&hit.galley, pos - hit.origin.to_vec2()));
    }
  }
  None
}

/// Character index for a position inside one galley (origin-relative).
///
/// Rows are scanned top-down; the first row whose bottom reaches `pos.y` wins,
/// so a position on the seam between two rows resolves to the lower one.
fn pos_to_char_index(galley: &Galley, pos: Pos2) -> usize {
  let mut char_index = 0usize;
  for row in &galley.rows {
    if pos.y <= row.max_y() {
      let row_chars = row.char_count_excluding_newline().0;
      let Some(first) = row.glyphs.first() else {
        return char_index;
      };
      if pos.x <= first.pos.x {
        return char_index;
      }
      for (i, glyph) in row.glyphs.iter().enumerate() {
        if pos.x <= glyph.pos.x + glyph.advance_width * 0.5 {
          return char_index + i;
        }
      }
      // Past the last glyph: end of the row's content, before its newline.
      return char_index + row_chars;
    }
    char_index += row.char_count_including_newline().0;
  }
  char_index
}

/// Screen-space rect of the painted text row containing `pos`.
///
/// Sibling of [`char_index_at_pos`] that returns the row's geometry instead of
/// a character index: live-preview hosts anchor the scroll offset to the
/// clicked row when a block switches between rendered and source forms, so the
/// line under the pointer stays put across the form change. Row selection uses
/// the same rule as `char_index_at_pos` (a position on the seam between two
/// rows resolves to the lower one), and the rect spans the full galley width.
///
/// Returns `None` when the label was not rendered this frame or `pos` falls
/// outside every painted text galley — the same conditions as
/// [`char_index_at_pos`].
pub fn row_rect_at_pos(ui: &egui::Ui, id: egui::Id, pos: Pos2) -> Option<Rect> {
  let (frame, galleys) = ui.data(|d| d.get_temp::<(u64, Vec<HitGalley>)>(hit_galleys_id(id)))?;
  (frame == ui.ctx().cumulative_pass_nr()).then_some(())?;
  for hit in &galleys {
    let bottom = hit.origin.y + hit.galley.size().y;
    if pos.y >= hit.origin.y && pos.y <= bottom {
      let local_y = pos.y - hit.origin.y;
      for row in &hit.galley.rows {
        if local_y <= row.max_y() {
          return Some(Rect::from_min_max(
            hit.origin + vec2(0.0, row.min_y()),
            hit.origin + vec2(hit.galley.size().x, row.max_y()),
          ));
        }
      }
      return None;
    }
  }
  None
}

/// Does token `i` start a new block within a flushed text run?
///
/// Headings, fenced code blocks and horizontal rules always start a block; any
/// other content token starts one when preceded by a blank line (two or more
/// newlines — a single newline is a soft break inside a paragraph) or at the
/// very start of the run.
fn is_block_start(tokens: &[Token<'_>], i: usize) -> bool {
  match &tokens[i] {
    Token::Newline => false,
    Token::Table(_) | Token::CodeBlock { .. } | Token::HorizontalRule | Token::Image { .. } => true,
    Token::Text { style, .. } if style.heading.is_some() => {
      // A heading line split into several styled tokens is one block.
      i == 0
        || !matches!(
          &tokens[i - 1],
          Token::Text { style: prev, .. } if prev.heading.is_some()
        )
    }
    _ => {
      if i == 0 {
        return true;
      }
      matches!(tokens[i - 1], Token::Newline) && (i < 2 || matches!(tokens[i - 2], Token::Newline))
    }
  }
}

/// First searchable text of a block-start token, used to locate the block's top
/// row inside the laid-out text. `None` when the token has no stable literal in
/// the job text (callers fall back to the previous block's bottom).
fn block_needle<'a, 's>(token: &'a Token<'s>) -> Option<&'a str> {
  match token {
    Token::CodeBlock { text, .. } => text.lines().next().filter(|line| !line.is_empty()),
    Token::Text { text, .. } | Token::Link { text, .. } | Token::ListMarker { marker: text, .. } => {
      Some(text.as_ref()).filter(|text| !text.is_empty())
    }
    Token::Image { alt, url, .. } => {
      let needle = alt.as_ref();
      (!needle.is_empty()).then_some(needle).or(Some(url.as_ref()))
    }
    _ => None,
  }
}

/// Compute the anchors of a freshly laid-out galley.
///
/// Rows carry their character count (one glyph per char), so walking the rows and
/// accumulating gives each row's character range; a section starts at the row that
/// contains its first character. `PlacedRow::pos.y` is relative to the galley
/// origin, which is exactly what a `ScrollArea` caller needs.
///
/// Sections and rows are both in document order, so two cursors that only move
/// forward locate every section's row in one sweep. Restarting the row walk (and
/// re-counting the text prefix) for each section made this O(sections × rows),
/// which dominates large documents whose highlighting emits one section per code
/// line — exactly the shape a streaming AI response produces.
fn compute_section_anchors(galley: &egui::epaint::text::Galley) -> Vec<SectionAnchor> {
  let sections = &galley.job.sections;
  let text = &galley.job.text;
  let mut anchors = Vec::with_capacity(sections.len());
  let mut byte_cursor = 0usize;
  // Chars in `text[..byte_cursor]`, accumulated section by section.
  let mut chars_before = 0usize;
  let mut row_index = 0usize;
  // Chars the rows before `row_index` account for.
  let mut row_chars_before = 0usize;
  for (index, section) in sections.iter().enumerate() {
    // byte_start is already absolute within `galley.job.text`
    let byte_start = section.byte_range.start.0;
    // Convert to a char offset: rows are counted in chars, bytes are not.
    let limit = byte_start.min(text.len());
    chars_before += text[byte_cursor..limit].chars().count();
    byte_cursor = limit;
    let char_start = chars_before;
    let mut y = f32::NAN;
    // Advance the shared row cursor: an empty row contributes no glyphs, so it
    // must still advance the cursor by one visual line.
    while row_index < galley.rows.len() {
      let row = &galley.rows[row_index];
      let row_len = row.row.glyphs.len();
      if char_start < row_chars_before + row_len.max(1) {
        y = row.pos.y;
        break;
      }
      row_chars_before += row_len.max(1);
      row_index += 1;
    }
    if y.is_nan() && index + 1 == sections.len() {
      // Trailing empty section: park it at the very bottom rather than dropping it
      y = galley.rect.bottom();
    }
    if !y.is_nan() {
      anchors.push(SectionAnchor { byte_start, y });
    }
  }
  anchors
}

impl<'a> MarkdownLabel<'a> {
  /// Create a new markdown label with the given widget ID and markdown source text.
  pub fn new(id: Id, text: &'a str) -> Self {
    Self {
      id,
      text,
      font: None,
      max_lines: None,
      wrap_mode: None,
      overflow_wrap: OverflowWrap::Normal,
      selectable: true,
      interactable: true,
      link_handler: None,
      code_block_buttons: None,
      scroll_code_blocks: false,
      shrink_code_blocks: false,
      defer_offscreen_highlight: false,
      hug_content: false,
      code_block_min_width: None,
      style: None,
      heal: false,
      map_job: None,
      #[cfg(feature = "syntax_highlighting")]
      code_theme: None,
    }
  }

  /// Override the base font used for rendering.
  pub fn font(self, font: FontId) -> Self {
    Self { font: Some(font), ..self }
  }

  /// Limit the number of visible lines (rows).
  ///
  /// Under [`Self::truncate`], this is the elision row budget (default `1` when unset),
  /// so `.truncate().max_lines(3)` acts like a CSS line-clamp.
  ///
  /// Row limits apply only to the whole-document render path. Documents that need
  /// segmentation (tables, images, blockquotes, scrolling code blocks) ignore this.
  pub fn max_lines(self, n: u32) -> Self {
    Self { max_lines: Some(n), ..self }
  }

  /// Override the wrap mode for this widget.
  ///
  /// When unset, [`Ui::wrap_mode`] is used (same as egui's [`egui::Label`]).
  pub fn wrap_mode(self, wrap_mode: TextWrapMode) -> Self {
    Self { wrap_mode: Some(wrap_mode), ..self }
  }

  /// Set wrap mode to [`TextWrapMode::Wrap`].
  pub fn wrap(self) -> Self {
    self.wrap_mode(TextWrapMode::Wrap)
  }

  /// Set wrap mode to [`TextWrapMode::Truncate`].
  ///
  /// Forces [`OverflowWrap::BreakAll`] at layout time. An explicit
  /// [`OverflowWrap::Normal`] from [`Self::overflow_wrap`] does not survive truncate,
  /// matching egui/`epaint` guidance for `max_rows = 1`.
  ///
  /// Elides after [`Self::max_lines`] rows, or 1 when `max_lines` is unset.
  /// Same segmentation limitation as [`Self::max_lines`].
  pub fn truncate(self) -> Self {
    self.wrap_mode(TextWrapMode::Truncate)
  }

  /// Set wrap mode to [`TextWrapMode::Extend`], disabling wrapping and truncation.
  pub fn extend(self) -> Self {
    self.wrap_mode(TextWrapMode::Extend)
  }

  /// How to break tokens that are wider than the available width.
  ///
  /// Default: [`OverflowWrap::Normal`]. Ignored under [`Self::extend`] (nothing to
  /// break against) and overridden to [`OverflowWrap::BreakAll`] under [`Self::truncate`].
  pub fn overflow_wrap(self, overflow_wrap: OverflowWrap) -> Self {
    Self { overflow_wrap, ..self }
  }

  /// Enable or disable text selection. Default: `true`.
  pub fn selectable(self, selectable: bool) -> Self {
    Self { selectable, ..self }
  }

  /// Enable or disable all interaction (links, hover, selection). Default: `true`.
  pub fn interactable(self, interactable: bool) -> Self {
    Self { interactable, ..self }
  }

  /// Set a custom link handler for styling and click behavior.
  pub fn link_handler(self, handler: &'a dyn LinkHandler) -> Self {
    Self { link_handler: Some(handler), ..self }
  }

  /// Add a button overlay to code blocks. The callback receives `(ui, code_text, language)`.
  pub fn code_block_buttons(self, f: &'a dyn Fn(&mut Ui, &str, &str)) -> Self {
    Self { code_block_buttons: Some(f), ..self }
  }

  /// Enable horizontal scrolling for code blocks. Default: `false`.
  ///
  /// When enabled, code blocks render as standalone scrollable widgets instead of
  /// inline galley text. Long lines scroll horizontally rather than wrapping.
  pub fn scroll_code_blocks(self, scroll: bool) -> Self {
    Self { scroll_code_blocks: scroll, ..self }
  }

  /// Size code blocks to their content instead of filling the available width.
  /// Default: `false` (fill). Only applies with [`Self::scroll_code_blocks`].
  ///
  /// Useful for tooltips and popovers that should hug their content.
  pub fn shrink_code_blocks(self, shrink: bool) -> Self {
    Self { shrink_code_blocks: shrink, ..self }
  }

  /// Lay out flush ranges that start below the viewport without syntax
  /// highlighting, and highlight them the moment they can become visible.
  /// Default: `false`.
  ///
  /// A code block whose flush range starts entirely below the clip rect's
  /// bottom edge is built from the same padded text in the same monospace
  /// metrics — identical geometry, absent colors — and is rebuilt with full
  /// highlighting on the first frame where it is no longer below the fold.
  /// This removes the syntect run for every below-fold block from cache-miss
  /// frames (first paint of a long document) without any height estimation:
  /// the deferral decision reads the exact layout cursor, never a guess, so
  /// there is nothing to correct and nothing jumps.
  ///
  /// Only the segmented render path defers (ranges exist there); the
  /// whole-document path and builds without the `syntax_highlighting` feature
  /// are unaffected.
  pub fn defer_offscreen_highlight(self, defer: bool) -> Self {
    Self { defer_offscreen_highlight: defer, ..self }
  }

  /// Size the whole widget to the laid-out galley instead of filling available width.
  /// Default: `false` (fill). Wrapping still uses the available width as a cap.
  ///
  /// Decorations (code-block backgrounds, horizontal rules) span the allocated
  /// width, not the leftover space in the parent.
  pub fn hug_content(self, hug: bool) -> Self {
    Self { hug_content: hug, ..self }
  }

  /// Minimum width for shrunk code blocks (capped at the available width).
  ///
  /// Lets a container that sizes to content stretch blocks to match its widest
  /// sibling, typically using a width measured on the previous frame.
  pub fn code_block_min_width(self, width: Option<f32>) -> Self {
    Self { code_block_min_width: width, ..self }
  }

  /// Auto-close unclosed code fences before parsing.
  ///
  /// Useful for streaming LLM output where text is incomplete. An unclosed
  /// triple-backtick fence causes all subsequent text to be swallowed as code;
  /// enabling heal appends a closing fence before parsing.
  pub fn heal(self, heal: bool) -> Self {
    Self { heal, ..self }
  }

  /// Rewrite the laid-out [`LayoutJob`] just before it is shaped into a galley.
  ///
  /// The parse/layout cache is unchanged; only the painted job is rewritten. Pass `None` to
  /// leave the job as built.
  pub fn map_job<F>(self, f: Option<F>) -> Self
  where
    F: for<'c> Fn(&'c Context, LayoutJob) -> LayoutJob + 'a,
  {
    Self { map_job: f.map(|f| Box::new(f) as _), ..self }
  }

  /// Override visual styling for this widget.
  ///
  /// Without this, the widget uses the context-wide style from
  /// [`egui_markdown_style::global_style`], or [`MarkdownStyle::default`] when no context
  /// style is installed.
  pub fn style(self, style: &'a MarkdownStyle) -> Self {
    Self { style: Some(style), ..self }
  }

  /// Use a custom syntect theme for code block syntax highlighting.
  ///
  /// When set, this theme is used instead of the context-wide themes from
  /// [`crate::set_code_themes`]. When neither is set, a built-in theme is
  /// chosen based on dark/light mode.
  #[cfg(feature = "syntax_highlighting")]
  pub fn code_theme(self, theme: &'a syntect::highlighting::Theme) -> Self {
    Self { code_theme: Some(theme), ..self }
  }

  /// Resolve wrap mode, overflow, width, and row budget for this frame.
  fn resolve_wrap(&self, ui: &Ui) -> ResolvedWrap {
    let mode = self.wrap_mode.unwrap_or_else(|| ui.wrap_mode());
    match mode {
      TextWrapMode::Extend => {
        ResolvedWrap { mode, max_width: f32::INFINITY, break_anywhere: false, max_rows: self.max_lines }
      }
      TextWrapMode::Wrap => ResolvedWrap {
        mode,
        max_width: ui.available_width(),
        break_anywhere: matches!(self.overflow_wrap, OverflowWrap::BreakAll),
        max_rows: self.max_lines,
      },
      TextWrapMode::Truncate => ResolvedWrap {
        mode,
        max_width: ui.available_width(),
        break_anywhere: true,
        max_rows: Some(self.max_lines.unwrap_or(1)),
      },
    }
  }

  /// Estimated laid-out height of this label's content at the current wrap
  /// width, without shaping the text.
  ///
  /// Mirrors what [`Self::show`] would lay out — same heal, style, font and
  /// wrap resolution, same segmented-path flush semantics — with the accuracy
  /// bounds documented in [`crate::estimate`]. Intended for hosts that need a
  /// placeholder height for content they have decided not to render this frame
  /// (off-screen blocks in a virtualized layout). The structural model is
  /// cached per widget id and text hash, so repeated calls at changing widths
  /// cost arithmetic only.
  pub fn estimated_height(&self, ui: &Ui) -> f32 {
    let context_style = egui_markdown_style::global_style(ui.ctx());
    let style = self.style.unwrap_or(context_style.as_ref());
    let color = ui.visuals().text_color();
    let font = self.font.clone().unwrap_or_else(|| FontSelection::Default.resolve(ui.style()));
    let wrap = self.resolve_wrap(ui);
    crate::estimate::estimated_height(
      ui,
      crate::estimate::HeightParams {
        id: self.id,
        text: self.text,
        heal: self.heal,
        font,
        wrap_width: wrap.max_width,
        style,
        color,
        dark_mode: ui.visuals().dark_mode,
        handler: self.link_handler,
        scroll_code_blocks: self.scroll_code_blocks,
        code_block_buttons: self.code_block_buttons.is_some(),
      },
    )
  }

  /// Re-apply live wrap fields that are deliberately excluded from layout cache keys.
  fn apply_live_wrap(wrap: &ResolvedWrap, job: &mut LayoutJob) {
    job.wrap.max_width = wrap.max_width;
    job.wrap.break_anywhere = wrap.break_anywhere;
  }

  /// Return the code theme argument for passing to layout functions.
  #[cfg(feature = "syntax_highlighting")]
  fn code_theme_arg(&self) -> CodeThemeArg<'_> {
    self.code_theme
  }

  /// Return the code theme argument (stub when syntax highlighting is disabled).
  #[cfg(not(feature = "syntax_highlighting"))]
  fn code_theme_arg(&self) -> CodeThemeArg<'_> {
    None
  }

  /// Render the markdown into the UI.
  pub fn show(self, ui: &mut Ui) {
    self.render(ui);
  }

  /// Calculate the rendered size without painting.
  pub fn calculate_size(&self, ui: &mut Ui) -> Vec2 {
    let context_style = egui_markdown_style::global_style(ui.ctx());
    let style = self.style.unwrap_or(context_style.as_ref());
    let color = ui.visuals().text_color();
    let md = parser::parse(self.text);
    let font = self.font.clone().unwrap_or_else(|| FontSelection::Default.resolve(ui.style()));
    let code_theme = self.code_theme_arg();
    let wrap = self.resolve_wrap(ui);
    let result = build_layout(
      ui,
      &md.tokens,
      font,
      color,
      wrap.max_rows,
      wrap.max_width,
      wrap.break_anywhere,
      self.link_handler,
      false,
      false,
      true,
      style,
      code_theme,
    );
    let galley = ui.fonts_mut(|f| f.layout_job(result.job));
    galley.size()
  }

  /// Layout the markdown and return the galley position, galley, and response.
  pub fn layout_in_ui(self, ui: &mut Ui) -> (Pos2, Arc<Galley>, Response) {
    let context_style = egui_markdown_style::global_style(ui.ctx());
    let style = self.style.unwrap_or(context_style.as_ref());
    let color = ui.visuals().text_color();
    let md = parser::parse(self.text);
    let font = self.font.clone().unwrap_or_else(|| FontSelection::Default.resolve(ui.style()));
    let code_theme = self.code_theme_arg();
    let wrap = self.resolve_wrap(ui);
    let result = build_layout(
      ui,
      &md.tokens,
      font,
      color,
      wrap.max_rows,
      wrap.max_width,
      wrap.break_anywhere,
      self.link_handler,
      false,
      false,
      true,
      style,
      code_theme,
    );

    let mut job = result.job;
    job.halign = ui.layout().horizontal_placement();
    job.justify = ui.layout().horizontal_justify();

    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let sense = if self.interactable { Sense::click_and_drag() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(galley.size(), sense);

    let galley_pos = match galley.job.halign {
      Align::LEFT => rect.left_top(),
      Align::Center => rect.center_top(),
      Align::RIGHT => rect.right_top(),
    };

    (galley_pos, galley, response)
  }

  fn render(self, ui: &mut Ui) {
    let context_style = egui_markdown_style::global_style(ui.ctx());
    let style = self.style.unwrap_or(context_style.as_ref());
    let color = ui.visuals().text_color();
    let font = self.font.clone().unwrap_or_else(|| FontSelection::Default.resolve(ui.style()));

    let healed;
    let text = if self.heal {
      healed = parser::heal(self.text);
      &*healed
    } else {
      self.text
    };

    let text_hash = hash_text(text, style, &font, color, ui.visuals().dark_mode, self.link_handler);
    let cache_id = self.id.with("md_cache");

    // Reset the per-frame block table up front: every frame of this label starts
    // from an empty table, so records from the previous frame never leak into a
    // query (a render that records nothing — e.g. fully culled — yields an empty
    // table, which readers report as "no blocks" rather than stale geometry).
    // Cleared in place so the table's allocation survives frame-to-frame reuse
    // instead of being freed and regrown every frame on block-heavy documents.
    let frame = ui.ctx().cumulative_pass_nr();
    ui.data_mut(|d| {
      let (seen, blocks) =
        d.get_temp_mut_or_insert_with::<(u64, Vec<BlockSpanRect>)>(block_rects_id(self.id), || (frame, Vec::new()));
      *seen = frame;
      blocks.clear();
    });
    reset_hit_galleys(ui, self.id);

    // Check if we have a cached layout for this text.
    let cached: Option<CachedMarkdownLayout> = ui.data(|d| d.get_temp(cache_id));

    if let Some(ref cached) = cached {
      if cached.text_hash == text_hash {
        let tokens = Arc::clone(&cached.tokens);
        let spans = Arc::clone(&cached.spans);

        let Some(layout) = cached.layout.clone() else {
          // 布局缺失(分段渲染路径):缓存里留有 spans,块表记录照常可用。
          let md = Markdown { s: text, tokens: (*tokens).clone(), spans: (*spans).clone() };
          self.render_segmented(ui, &md, &font, color, style);
          return;
        };

        let _ = self.render_galley(
          ui,
          &tokens,
          ShapedJob::Job(layout.job.clone()),
          &layout.section_to_token,
          &layout.code_block_spans,
          &layout.code_block_info,
          &layout.hr_positions,
          &layout.inline_widget_spans,
          0,
          &spans,
          color,
          style,
          true,
        );
        return;
      }
    }

    // Cache miss - parse and build layout. The full-text parse and the owned-token copy
    // are a small fraction of what re-rendering costs (layout dominates), so this
    // whole-document gate keeps the full-text hash; per-block keying is a segmented-path
    // concern and lives in its own caches (per-range flushes, per-block heights).
    let md = parser::parse(text);
    let owned_tokens = Arc::new(tokens_to_owned(&md.tokens));

    // Decide the render path before laying anything out: a whole-document layout is useless
    // to the segmented path, and building one per keystroke doubles the cost of every edit.
    if needs_segmentation(&md.tokens, self.scroll_code_blocks, self.link_handler, style) {
      let owned_spans = Arc::new(md.spans.clone());
      ui.data_mut(|d| {
        d.insert_temp(
          cache_id,
          CachedMarkdownLayout { text_hash, layout: None, tokens: owned_tokens, spans: owned_spans },
        );
      });
      self.render_segmented(ui, &md, &font, color, style);
      return;
    }

    let code_theme = self.code_theme_arg();
    let wrap = self.resolve_wrap(ui);
    let layout = build_layout(
      ui,
      &md.tokens,
      font.clone(),
      color,
      wrap.max_rows,
      wrap.max_width,
      wrap.break_anywhere,
      self.link_handler,
      self.scroll_code_blocks,
      true,
      true,
      style,
      code_theme,
    );
    debug_assert!(layout.segment_breaks.is_empty(), "needs_segmentation disagrees with build_layout");
    let layout = Arc::new(layout);

    ui.data_mut(|d| {
      d.insert_temp(
        cache_id,
        CachedMarkdownLayout {
          text_hash,
          layout: Some(Arc::clone(&layout)),
          tokens: owned_tokens,
          spans: Arc::new(md.spans.clone()),
        },
      );
    });

    let _ = self.render_galley(
      ui,
      &md.tokens,
      ShapedJob::Job(layout.job.clone()),
      &layout.section_to_token,
      &layout.code_block_spans,
      &layout.code_block_info,
      &layout.hr_positions,
      &layout.inline_widget_spans,
      0,
      &md.spans,
      color,
      style,
      true,
    );
  }

  fn render_segmented(&self, ui: &mut Ui, md: &Markdown<'_>, font: &FontId, color: Color32, style: &MarkdownStyle) {
    self.render_token_range(ui, md, font, color, 0, md.tokens.len(), style);
  }

  /// Recursively render a range of tokens, handling blockquotes and tables as sub-regions.
  #[allow(clippy::too_many_arguments)]
  fn render_token_range(
    &self,
    ui: &mut Ui,
    md: &Markdown<'_>,
    font: &FontId,
    color: Color32,
    start: usize,
    end: usize,
    style: &MarkdownStyle,
  ) {
    let mut text_start = start;
    let mut i = start;

    // Block-widget height caches are keyed by the block's own token, not by the
    // document hash: appending at the end of the document leaves every earlier block's
    // cached height valid, so only the edited block re-measures. The style and
    // handler parts of the key are hashed once here — per-block re-hashing of the
    // full style is measurable on table-heavy documents.
    let block_context = hash_block_context(style, self.link_handler);
    let block_key = |index: usize| hash_block_content(&md.tokens[index], block_context);

    let block_spacing = style.block_spacing;
    // Before rendering a block element, add spacing if there was preceding content.
    let before_block = |had_content: bool, ui: &mut Ui| {
      if had_content {
        ui.add_space(block_spacing);
      }
    };
    // After rendering a block element, skip trailing newlines and add uniform spacing —
    // but only when more content follows, mirroring `before_block`, so a trailing block
    // doesn't leave a spacing gap at the bottom.
    let after_block = |i: &mut usize, text_start: &mut usize, end: usize, ui: &mut Ui| {
      while *text_start < end && matches!(md.tokens[*text_start], Token::Newline) {
        *text_start += 1;
        *i += 1;
      }
      if *text_start < end {
        ui.add_space(block_spacing);
      }
    };

    while i < end {
      match &md.tokens[i] {
        Token::Table(data) => {
          let had_content = text_start < i;
          self.flush_text_range(ui, md, font, color, text_start, i, style, true);
          before_block(had_content, ui);
          let block_sz_id = self.id.with(("block_sz", i));
          let before_rect = ui.available_rect_before_wrap();
          if let Some(cull_rect) = try_cull_block(ui, block_sz_id, block_key(i)) {
            record_block_rect(ui, self.id, md.spans[i].clone(), cull_rect);
            i += 1;
            text_start = i;
            after_block(&mut i, &mut text_start, end, ui);
            continue;
          }
          let before_y = before_rect.min.y;
          table::render_table(
            ui,
            self.id.with(("table", i)),
            data,
            font,
            color,
            &style.inline_code,
            &style.table,
            self.link_handler,
          );
          record_widget_block(ui, self.id, md.spans[i].clone(), before_rect, ui.min_rect().bottom());
          cache_block_height(ui, block_sz_id, block_key(i), before_y);
          i += 1;
          text_start = i;
          after_block(&mut i, &mut text_start, end, ui);
        }
        Token::CodeBlock { text, language }
          if self.link_handler.is_some_and(|h| h.is_block_code_widget(language.as_deref())) =>
        {
          self.flush_text_range(ui, md, font, color, text_start, i, style, true);
          let before_rect = ui.available_rect_before_wrap();
          let handler = self.link_handler.unwrap();
          handler.block_code_widget(ui, text, language.as_deref());
          record_widget_block(ui, self.id, md.spans[i].clone(), before_rect, ui.min_rect().bottom());
          i += 1;
          text_start = i;
        }
        Token::CodeBlock { text, language } if self.scroll_code_blocks => {
          let had_content = text_start < i;
          self.flush_text_range(ui, md, font, color, text_start, i, style, true);
          before_block(had_content, ui);
          let block_sz_id = self.id.with(("block_sz", i));
          let before_rect = ui.available_rect_before_wrap();
          if let Some(cull_rect) = try_cull_block(ui, block_sz_id, block_key(i)) {
            record_block_rect(ui, self.id, md.spans[i].clone(), cull_rect);
            i += 1;
            text_start = i;
            after_block(&mut i, &mut text_start, end, ui);
            continue;
          }
          let before_y = before_rect.min.y;
          render_code_block(
            ui,
            self.id.with(("code_block", i)),
            text,
            language.as_deref(),
            self.code_block_buttons,
            self.shrink_code_blocks,
            self.code_block_min_width,
            style,
            self.code_theme_arg(),
          );
          record_widget_block(ui, self.id, md.spans[i].clone(), before_rect, ui.min_rect().bottom());
          cache_block_height(ui, block_sz_id, block_key(i), before_y);
          i += 1;
          text_start = i;
          after_block(&mut i, &mut text_start, end, ui);
        }
        // A non-scrolling fence that reached the admission threshold becomes its own
        // flushed range: the same galley shape it would have inside a whole-document
        // galley (padding, highlighting, wrapping, background), but laid out, cached
        // and viewport-culled per block, so appending lines to it does not re-layout
        // the rest of the document. The single-fence flush passes `false` so the fence
        // renders inline within its own range instead of being reported as a break
        // again. Culling reuses the per-range size cache; block-widget culling
        // (`try_cull_block`) is unaffected.
        Token::CodeBlock { text, .. } if code_block_admits_segmentation(text.as_ref(), style) => {
          let had_content = text_start < i;
          self.flush_text_range(ui, md, font, color, text_start, i, style, true);
          before_block(had_content, ui);
          self.flush_text_range(ui, md, font, color, i, i + 1, style, false);
          i += 1;
          text_start = i;
          after_block(&mut i, &mut text_start, end, ui);
        }
        #[cfg(feature = "images")]
        Token::Image { alt, url, title, .. } => {
          let had_content = text_start < i;
          self.flush_text_range(ui, md, font, color, text_start, i, style, true);
          before_block(had_content, ui);
          let block_sz_id = self.id.with(("block_sz", i));
          let before_rect = ui.available_rect_before_wrap();
          if let Some(cull_rect) = try_cull_block(ui, block_sz_id, block_key(i)) {
            record_block_rect(ui, self.id, md.spans[i].clone(), cull_rect);
            i += 1;
            text_start = i;
            after_block(&mut i, &mut text_start, end, ui);
            continue;
          }
          let before_y = before_rect.min.y;
          // `ImageSize::default()` is `ImageFit::Fraction(1×1)` with aspect
          // ratio kept: while the image is loading — or once it has failed —
          // egui falls back to a 24×24 source size, which `Fraction` then
          // scales UP to fill the whole available area (measured: the widget
          // allocated 10 000 × 10 000 under a bare context, ~860×860 in a
          // real pane). Markdown previews never upscale: small images render
          // at their natural size, oversized ones still shrink to the column
          // via `max_width`, and the loading/error fallback stays at its
          // native 24×24 spinner/⚠ box.
          let image = egui::Image::new(url.as_ref())
            .max_width(ui.available_width())
            .fit_to_original_size(1.0)
            .show_loading_spinner(true);
          // Title convention (`types::CENTERED_IMAGE_TITLE`): a block image
          // titled with the marker is centered horizontally. Titles render
          // nowhere else, and the title already participates in the token
          // hash, so toggling the marker rotates the block cache key. The
          // centered layout only shifts x — block height bookkeeping
          // (before_y / min_rect) is unchanged.
          let image_response = if title.as_deref() == Some(crate::types::CENTERED_IMAGE_TITLE) {
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| ui.add(image)).inner
          } else {
            ui.add(image)
          };
          if let Some(handler) = &self.link_handler {
            handler.image_response(ui, alt.as_ref(), url.as_ref(), &image_response);
          }
          record_widget_block(ui, self.id, md.spans[i].clone(), before_rect, ui.min_rect().bottom());
          cache_block_height(ui, block_sz_id, block_key(i), before_y);
          i += 1;
          text_start = i;
          after_block(&mut i, &mut text_start, end, ui);
        }
        #[cfg(not(feature = "images"))]
        Token::Image { alt, url, .. } => {
          self.flush_text_range(ui, md, font, color, text_start, i, style, true);
          let block_sz_id = self.id.with(("block_sz", i));
          let before_rect = ui.available_rect_before_wrap();
          if let Some(cull_rect) = try_cull_block(ui, block_sz_id, block_key(i)) {
            record_block_rect(ui, self.id, md.spans[i].clone(), cull_rect);
            i += 1;
            text_start = i;
            after_block(&mut i, &mut text_start, end, ui);
            continue;
          }
          let before_y = before_rect.min.y;
          let text = if alt.is_empty() { url.as_ref() } else { alt.as_ref() };
          if ui.link(text).clicked() {
            ui.ctx().open_url(OpenUrl::new_tab(url.to_string()));
          }
          record_widget_block(ui, self.id, md.spans[i].clone(), before_rect, ui.min_rect().bottom());
          cache_block_height(ui, block_sz_id, block_key(i), before_y);
          i += 1;
          text_start = i;
          after_block(&mut i, &mut text_start, end, ui);
        }
        Token::Link { text, href, .. } if self.link_handler.is_some_and(|h| h.is_block_widget(href)) => {
          self.flush_text_range(ui, md, font, color, text_start, i, style, true);
          let before_rect = ui.available_rect_before_wrap();
          let handler = self.link_handler.unwrap();
          handler.block_widget(ui, text, href);
          record_widget_block(ui, self.id, md.spans[i].clone(), before_rect, ui.min_rect().bottom());
          i += 1;
          text_start = i;
        }
        Token::BlockquoteStart => {
          self.flush_text_range(ui, md, font, color, text_start, i, style, true);

          // Find the matching BlockquoteEnd.
          let bq_start = i + 1;
          let mut depth = 1u32;
          let mut j = bq_start;
          while j < end && depth > 0 {
            match &md.tokens[j] {
              Token::BlockquoteStart => depth += 1,
              Token::BlockquoteEnd => depth -= 1,
              _ => {}
            }
            if depth > 0 {
              j += 1;
            }
          }
          let bq_end = j; // index of matching BlockquoteEnd

          // Skip leading newlines inside the blockquote.
          let mut bq_content_start = bq_start;
          while bq_content_start < bq_end && matches!(md.tokens[bq_content_start], Token::Newline) {
            bq_content_start += 1;
          }

          let indent = style.blockquote.indent_per_depth;
          let mut child_rect = ui.available_rect_before_wrap();
          child_rect.min.x += indent;

          let mut child_ui = ui.new_child(UiBuilder::new().id_salt(self.id.with(("bq", i))).max_rect(child_rect));
          self.render_token_range(&mut child_ui, md, font, color, bq_content_start, bq_end, style);

          let bq_stroke =
            Stroke::new(style.blockquote.stroke_width, ui.visuals().widgets.noninteractive.bg_stroke.color);
          let line_x = child_rect.min.x - indent * 0.5;
          let top = child_rect.min.y;
          let bottom = child_ui.min_rect().bottom();
          // The blockquote is one jump target at the parent level; the child run
          // below it records its own finer blocks in the same screen space.
          if md.spans.len() > bq_end {
            let span = md.spans[i].start..md.spans[bq_end].end;
            record_widget_block(ui, self.id, span, child_rect, bottom);
          }
          ui.painter().line_segment([pos2(line_x, top), pos2(line_x, bottom)], bq_stroke);

          // Advance parent UI past the child content.
          let child_min_rect = child_ui.min_rect();
          ui.allocate_rect(child_min_rect, Sense::hover());

          i = bq_end + 1; // skip past BlockquoteEnd
          text_start = i;
          after_block(&mut i, &mut text_start, end, ui);
        }
        Token::BlockquoteEnd => {
          self.flush_text_range(ui, md, font, color, text_start, i, style, true);
          i += 1;
          text_start = i;
        }
        _ => {
          i += 1;
        }
      }
    }

    // Flush remaining text.
    self.flush_text_range(ui, md, font, color, text_start, end, style, true);
  }

  /// Render a range of non-break tokens as an interactive galley.
  ///
  /// `segment_large_code_blocks` is passed through to [`build_layout`]: plain text
  /// ranges pass `true` (they contain no admitted fence, and a disagreement here is
  /// what the segment-break debug assert catches), while the single-fence range an
  /// admitted fence renders as passes `false` so the fence renders inline.
  #[allow(clippy::too_many_arguments)]
  fn flush_text_range(
    &self,
    ui: &mut Ui,
    md: &Markdown<'_>,
    font: &FontId,
    color: Color32,
    start: usize,
    end: usize,
    style: &MarkdownStyle,
    segment_large_code_blocks: bool,
  ) {
    if start >= end {
      return;
    }
    // Skip if only newlines.
    if md.tokens[start..end].iter().all(|t| matches!(t, Token::Newline | Token::BlockquoteStart | Token::BlockquoteEnd))
    {
      return;
    }

    // Trim trailing newlines; spacing between blocks is handled by render_token_range.
    let mut trimmed_end = end;
    while trimmed_end > start && matches!(md.tokens[trimmed_end - 1], Token::Newline) {
      trimmed_end -= 1;
    }
    if trimmed_end <= start {
      return;
    }

    let token_slice = &md.tokens[start..trimmed_end];
    let apply_map_job = end == md.tokens.len();
    let wrap = self.resolve_wrap(ui);
    let max_width = wrap.max_width;
    let dark_mode = ui.visuals().dark_mode;
    // Deferral trigger: the range starts entirely below the viewport's bottom
    // edge. The cursor y is the sum of real heights (placeholders keep exact
    // heights), so this is an exact geometric fact, not an estimate.
    let below_fold = self.defer_offscreen_highlight && ui.available_rect_before_wrap().min.y > ui.clip_rect().max.y;
    let ctx_hash = hash_flush_context(token_slice, style, font, color, dark_mode, self.link_handler);
    let cache_id = self.id.with(("flush", start));
    let size_cache_id = self.id.with(("flush_sz", start));

    // Viewport culling: skip layout+paint for off-screen segments. Unlike the layout, a
    // measured size is only valid at the width it was measured at.
    if let Some((cached_hash, cached_width, cached_size)) = ui.data(|d| d.get_temp::<(u64, f32, Vec2)>(size_cache_id)) {
      if cached_hash == ctx_hash && cached_width == max_width {
        let est_rect = Rect::from_min_size(ui.available_rect_before_wrap().min, cached_size);
        if !ui.is_rect_visible(est_rect) {
          ui.allocate_space(cached_size);
          // Culled but not geometry-less: record the run as one coarse block so
          // the table still covers the whole document and gap resolution points
          // past it instead of stopping at the last visible block.
          if md.spans.len() >= trimmed_end {
            let span = md.spans[start].start..md.spans[trimmed_end - 1].end;
            record_block_rect(ui, self.id, span, est_rect);
          }
          return;
        }
      }
    }

    if let Some(cached) = ui.data(|d| d.get_temp::<CachedFlushRange>(cache_id)) {
      // A deferred range stops being deferred the moment it could become
      // visible: fall through to the full build below, which re-highlights and
      // stores `deferred_highlight: false`. One-time per range (~per-fence
      // syntect cost on the frame it scrolls into view).
      if cached.ctx_hash == ctx_hash && !(cached.deferred_highlight && !below_fold) {
        // The shaped galley can only be reused when no per-frame job transform
        // (`map_job` on the trailing range) would have shaped a different one.
        let reusable = !apply_map_job || self.map_job.is_none();
        // Read before any `data_mut` below: `pixels_per_point` acquires the
        // context write lock in egui 0.36 and would self-deadlock inside it.
        let pixels_per_point = ui.ctx().pixels_per_point();
        if let Some(shaped) = &cached.galley {
          if reusable
            && shaped.max_width == max_width
            && shaped.break_anywhere == wrap.break_anywhere
            && shaped.pixels_per_point == pixels_per_point
          {
            let (size, _, _) = self.render_galley(
              ui,
              &cached.tokens,
              ShapedJob::Galley { galley: Arc::clone(&shaped.galley), anchors: shaped.anchors.clone() },
              &cached.layout.section_to_token,
              &cached.layout.code_block_spans,
              &cached.layout.code_block_info,
              &cached.layout.hr_positions,
              &cached.layout.inline_widget_spans,
              start,
              &md.spans,
              color,
              style,
              apply_map_job,
            );
            ui.data_mut(|d| d.insert_temp(size_cache_id, (ctx_hash, max_width, size)));
            return;
          }
        }
        let (size, galley, relative_anchors) = self.render_galley(
          ui,
          &cached.tokens,
          ShapedJob::Job(cached.layout.job.clone()),
          &cached.layout.section_to_token,
          &cached.layout.code_block_spans,
          &cached.layout.code_block_info,
          &cached.layout.hr_positions,
          &cached.layout.inline_widget_spans,
          start,
          &md.spans,
          color,
          style,
          apply_map_job,
        );
        let shaped = reusable.then(|| CachedShapedGalley {
          max_width,
          break_anywhere: wrap.break_anywhere,
          pixels_per_point,
          galley,
          anchors: relative_anchors,
        });
        ui.data_mut(|d| {
          d.insert_temp(
            cache_id,
            CachedFlushRange {
              deferred_highlight: cached.deferred_highlight,
              ctx_hash,
              layout: Arc::clone(&cached.layout),
              tokens: Arc::clone(&cached.tokens),
              galley: shaped,
            },
          )
        });
        ui.data_mut(|d| d.insert_temp(size_cache_id, (ctx_hash, max_width, size)));
        return;
      }
    }

    let code_theme = self.code_theme_arg();
    // Segmented flush ranges ignore row limits (tables/images/etc. already split the doc).
    let layout = build_layout(
      ui,
      token_slice,
      font.clone(),
      color,
      None,
      wrap.max_width,
      wrap.break_anywhere,
      self.link_handler,
      false,
      segment_large_code_blocks,
      !below_fold,
      style,
      code_theme,
    );
    debug_assert!(layout.segment_breaks.is_empty());
    let owned = Arc::new(tokens_to_owned(token_slice));
    let layout = Arc::new(layout);
    let (size, galley, relative_anchors) = self.render_galley(
      ui,
      token_slice,
      ShapedJob::Job(layout.job.clone()),
      &layout.section_to_token,
      &layout.code_block_spans,
      &layout.code_block_info,
      &layout.hr_positions,
      &layout.inline_widget_spans,
      start,
      &md.spans,
      color,
      style,
      apply_map_job,
    );
    let reusable = !apply_map_job || self.map_job.is_none();
    // Read before taking the data lock: `pixels_per_point` acquires the context
    // write lock in egui 0.36 and would self-deadlock inside `data_mut`.
    let pixels_per_point = ui.ctx().pixels_per_point();
    let shaped = reusable.then(|| CachedShapedGalley {
      max_width,
      break_anywhere: wrap.break_anywhere,
      pixels_per_point,
      galley,
      anchors: relative_anchors,
    });
    ui.data_mut(|d| {
      d.insert_temp(
        cache_id,
        CachedFlushRange {
          deferred_highlight: below_fold,
          ctx_hash,
          layout: Arc::clone(&layout),
          tokens: Arc::clone(&owned),
          galley: shaped,
        },
      )
    });
    ui.data_mut(|d| d.insert_temp(size_cache_id, (ctx_hash, max_width, size)));
  }

  /// Publish the geometry of every section under this label's id.
  ///
  /// `anchors` are relative to the galley origin; `origin_y` is the label's own
  /// top in the parent's coordinate space, and anchors are stored absolute so a
  /// `ScrollArea` can `scroll_to_rect` them directly.
  fn record_section_anchors(&self, ui: &egui::Ui, anchors: &[SectionAnchor], origin_y: f32) {
    let anchors: Vec<SectionAnchor> =
      anchors.iter().map(|anchor| SectionAnchor { byte_start: anchor.byte_start, y: anchor.y + origin_y }).collect();
    ui.data_mut(|d| d.insert_temp(self.id.with("section-anchors"), anchors));
  }

  /// Record one block entry per top-level block flushed into this galley.
  ///
  /// A flushed text run may contain many blocks (several headings, paragraphs,
  /// lists); consumers like an outline pane want to jump to *one* of them, so
  /// the whole run is not enough. Block boundaries come from
  /// [`is_block_start`]; each block's top row is found via its first layout
  /// section. Rects are screen-space (`galley_rect` is the allocated rect), and
  /// each block extends down to the next block's top (the last one to the
  /// galley bottom). Skipped silently when `source_spans` does not cover the
  /// token range — geometry without a source span is useless.
  fn record_text_blocks(
    &self,
    ui: &egui::Ui,
    galley: &egui::epaint::text::Galley,
    galley_rect: Rect,
    tokens: &[Token<'_>],
    token_base: usize,
    source_spans: &[std::ops::Range<usize>],
  ) {
    if source_spans.len() < token_base + tokens.len() {
      return;
    }
    // Locate each block's top row via its first visible text in the laid-out
    // text. `LayoutJob::append` merges adjacent same-format sections, so
    // `galley.job.sections` cannot be indexed back to tokens (`section_to_token`
    // counts pushes, not surviving sections). Block starts are in document
    // order and so is the search cursor — one forward sweep over the job text.
    let job_text = &galley.job.text;
    let starts: Vec<usize> = (0..tokens.len()).filter(|&i| is_block_start(tokens, i)).collect();
    let mut block_row: Vec<Option<f32>> = Vec::with_capacity(starts.len());
    let mut search_cursor = 0usize;
    for &start in &starts {
      let row = block_needle(&tokens[start]).and_then(|needle| {
        let hay = &job_text[search_cursor.min(job_text.len())..];
        hay.find(needle).map(|offset| {
          let byte = search_cursor + offset;
          search_cursor = byte + needle.len().min(job_text.len() - byte);
          let char_start = job_text[..byte].chars().count();
          // `prefer_next_line`: a cursor sitting exactly at a row boundary reads
          // as the row below, which is where a block's first character lives.
          galley
            .pos_from_cursor(epaint::text::cursor::CCursor {
              index: epaint::text::CharIndex(char_start),
              prefer_next_row: true,
            })
            .min
            .y
        })
      });
      block_row.push(row);
    }

    let mut previous_bottom = galley_rect.min.y;
    for (index, &start) in starts.iter().enumerate() {
      let next_start = starts.get(index + 1).copied().unwrap_or(tokens.len());
      let top = block_row[index].map(|y| y + galley_rect.min.y).unwrap_or(previous_bottom).max(galley_rect.min.y);
      let bottom = block_row
        .get(index + 1)
        .and_then(|row| *row)
        .map(|y| y + galley_rect.min.y)
        .unwrap_or_else(|| galley_rect.max.y)
        .max(top);
      let span = source_spans[token_base + start].start..source_spans[token_base + next_start - 1].end;
      let rect = Rect::from_min_max(pos2(galley_rect.min.x, top), pos2(galley_rect.max.x, bottom));
      record_block_rect(ui, self.id, span, rect);
      previous_bottom = bottom;
    }
  }

  #[allow(clippy::too_many_arguments)]
  fn render_galley(
    &self,
    ui: &mut Ui,
    tokens: &[Token<'_>],
    shaped: ShapedJob,
    section_to_token: &[usize],
    code_block_spans: &[(usize, usize)],
    code_block_info: &[(String, String)],
    hr_positions: &[usize],
    inline_widget_spans: &[(usize, usize, usize)],
    token_base: usize,
    source_spans: &[std::ops::Range<usize>],
    color: Color32,
    style: &MarkdownStyle,
    apply_map_job: bool,
  ) -> (Vec2, Arc<Galley>, Vec<SectionAnchor>) {
    let wrap = self.resolve_wrap(ui);
    let (galley, relative_anchors) = match shaped {
      ShapedJob::Galley { galley, anchors } => (galley, anchors),
      ShapedJob::Job(mut job) => {
        Self::apply_live_wrap(&wrap, &mut job);
        if apply_map_job {
          if let Some(map_job) = &self.map_job {
            job = map_job(ui.ctx(), job);
          }
        }
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let anchors = compute_section_anchors(&galley);
        (galley, anchors)
      }
    };
    let code_block_rects = paint::compute_code_block_rects(ui, code_block_spans, &galley);
    let available_width = ui.available_width();
    // Decorations (code block backgrounds, horizontal rules) span the allocated width, so a
    // filling widget must claim the leftover space — otherwise they paint outside the rect.
    // Popovers that hug content skip that expand and paint decorations at the galley width.
    let mut size = galley.size();
    if !self.hug_content && wrap.mode != TextWrapMode::Extend {
      size.x = size.x.max(available_width);
    }
    let decoration_width = if self.hug_content { size.x } else { available_width };

    if !self.interactable {
      let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
      self.record_section_anchors(ui, &relative_anchors, rect.min.y);
      self.record_text_blocks(ui, &galley, rect, tokens, token_base, source_spans);
      record_hit_galley(ui, self.id, &galley, rect.min);
      paint_decorations(ui, hr_positions, &galley, &code_block_rects, rect.min, decoration_width, style);
      ui.painter().galley(rect.min, galley.clone(), color);
      if let Some(handler) = self.link_handler {
        paint_inline_widgets(ui, handler, tokens, inline_widget_spans, &galley, rect.min);
      }
      return (size, galley, relative_anchors);
    }

    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    self.record_section_anchors(ui, &relative_anchors, rect.min.y);
    self.record_text_blocks(ui, &galley, rect, tokens, token_base, source_spans);
    record_hit_galley(ui, self.id, &galley, rect.min);
    paint_decorations(ui, hr_positions, &galley, &code_block_rects, response.rect.min, decoration_width, style);

    let disable_text_selection = !self.selectable || ui.input(|input| input.modifiers.shift);
    if !disable_text_selection {
      LabelSelectionState::label_text_selection(ui, &response, rect.min, galley.clone(), color, Stroke::NONE);
    } else {
      ui.painter().galley(rect.min, galley.clone(), color);
    }

    if let Some(button_fn) = self.code_block_buttons {
      render_code_block_buttons(ui, button_fn, &code_block_rects, code_block_info, &response);
    }

    if let Some(handler) = self.link_handler {
      paint_inline_widgets(ui, handler, tokens, inline_widget_spans, &galley, response.rect.min);
    }

    let hovered_section =
      self.handle_hover(ui, tokens, &galley, section_to_token, inline_widget_spans, &response, rect, color);

    if self.interactable && response.clicked() {
      self.handle_click(ui, tokens, section_to_token, hovered_section);
    }

    // Secondary-click context menu on links: resolve the link under the
    // pointer and hand the label's Response to the handler for as long as
    // the popup is open. The payload is parked in temp storage because the
    // pointer leaves the link the moment the popup covers it — re-invoking
    // the hook every open frame is what lets the host re-attach its menu
    // closure. Right-clicking anywhere that is not a link parks `None`:
    // no hook, no empty popup.
    if let Some(handler) = &self.link_handler {
      let menu_key = self.id.with("link-ctx-menu");
      if response.secondary_clicked() {
        let link =
          self.token_at_pointer(ui, &galley, section_to_token, &response, rect).and_then(|(_section, token_index)| {
            let is_inline_widget = inline_widget_spans.iter().any(|&(_, _, span_ti)| span_ti == token_index);
            match tokens.get(token_index) {
              Some(Token::Link { text, href, .. }) if !is_inline_widget => Some((text.to_string(), href.to_string())),
              _ => None,
            }
          });
        ui.ctx().data_mut(|d| d.insert_temp(menu_key, link));
      }
      let payload: Option<Option<(String, String)>> = ui.ctx().data(|d| d.get_temp(menu_key));
      if let Some((text, href)) = payload.flatten() {
        handler.link_response(ui, &text, &href, &response);
        // The open command lands at frame end, so on the click frame itself
        // `context_menu_opened()` still reads false — only retire the hook
        // on a frame that neither has the menu open nor just opened it.
        if !response.context_menu_opened() && !response.secondary_clicked() {
          ui.ctx().data_mut(|d| d.insert_temp::<Option<(String, String)>>(menu_key, None));
        }
      }
    }
    (size, galley, relative_anchors)
  }

  /// Pointer hit test in galley-local coordinates: `(section_index,
  /// token_index)` under the pointer, or `None` when non-interactable,
  /// unhovered, or over empty space. Shared by hover decorations and the
  /// secondary-click link menu.
  fn token_at_pointer(
    &self,
    ui: &Ui,
    galley: &Arc<Galley>,
    section_to_token: &[usize],
    response: &Response,
    rect: Rect,
  ) -> Option<(u32, usize)> {
    if !self.interactable || !response.hovered() {
      return None;
    }

    let current_transform = ui.ctx().layer_transform_to_global(ui.layer_id());
    let transformed_pointer_pos = ui.input(|i| {
      i.pointer.latest_pos().map(|raw_pos| {
        if let Some(transform) = current_transform {
          transform.inverse() * raw_pos
        } else {
          raw_pos
        }
      })
    });

    let pos_in_galley = transformed_pointer_pos.map(|pos| pos - rect.min.to_vec2())?;
    let glyph_index = cursor_from_pos(galley, pos_in_galley)?;
    let section_index = section_for_char(&galley.job, glyph_index)?;
    let token_index = section_to_token.get(section_index as usize).copied()?;
    Some((section_index, token_index))
  }

  #[allow(clippy::too_many_arguments)]
  fn handle_hover(
    &self,
    ui: &mut Ui,
    tokens: &[Token<'_>],
    galley: &Arc<Galley>,
    section_to_token: &[usize],
    inline_widget_spans: &[(usize, usize, usize)],
    response: &Response,
    rect: Rect,
    _color: Color32,
  ) -> Option<u32> {
    let (section_index, token_index) = self.token_at_pointer(ui, galley, section_to_token, response, rect)?;
    let token = Some(token_index).and_then(|idx| tokens.get(idx));

    // Check if this token is an inline widget - show pointing hand but suppress underline/tooltip.
    let is_inline_widget = inline_widget_spans.iter().any(|&(_, _, span_ti)| span_ti == token_index);

    if is_inline_widget {
      ui.output_mut(|out| out.cursor_icon = CursorIcon::PointingHand);
      return Some(section_index);
    }

    let stroke = match token {
      Some(Token::Link { href, .. }) => {
        let link_color = if let Some(handler) = self.link_handler {
          handler.link_style(href).and_then(|s| s.color).unwrap_or(ui.visuals().hyperlink_color)
        } else {
          ui.visuals().hyperlink_color
        };
        Some(Stroke::new(1.0_f32, link_color))
      }
      _ => None,
    };

    if let Some(stroke) = stroke {
      // Find all sections belonging to the same token (for multi-section layout_link).
      let target_token = token_index;
      for (sec_idx, &tok_idx) in section_to_token.iter().enumerate() {
        if tok_idx != target_token {
          continue;
        }

        // Find the char range for this section.
        let mut sec_start_char = 0u32;
        for s in &galley.job.sections[..sec_idx] {
          sec_start_char += s.byte_range.slice(&galley.job.text).chars().count() as u32;
        }
        let section_text = galley.job.sections[sec_idx].byte_range.slice(&galley.job.text);
        let sec_char_count = section_text.chars().count();
        let sec_end_char = sec_start_char + sec_char_count as u32;
        if sec_end_char <= sec_start_char {
          continue;
        }

        let Some((sg, sr)) = glyph_at_index(galley, sec_start_char) else { continue };
        let Some((eg, er)) = glyph_at_index(galley, sec_end_char.saturating_sub(1)) else { continue };

        for row_index in sr..=er {
          let row = &galley.rows[row_index as usize];
          let row_sg = if row_index == sr { Some(sg) } else { row.glyphs.first() };
          let row_eg = if row_index == er { Some(eg) } else { last_non_whitespace_glyph(row) };
          if let Some((sg, eg)) = row_sg.zip(row_eg) {
            let row_rect =
              Rect::from_min_max(pos2(sg.pos.x, row.min_y()), pos2(eg.pos.x + eg.advance_width, row.max_y()))
                .translate(rect.min.to_vec2());
            ui.painter().line_segment([row_rect.left_bottom(), row_rect.right_bottom()], stroke);
          }
        }
      }
      ui.output_mut(|out| out.cursor_icon = CursorIcon::PointingHand);

      // Show URL in tooltip when url_in_tooltip is enabled (matches egui's Hyperlink behavior)
      if ui.style().url_in_tooltip {
        if let Some(Token::Link { href, .. }) = token {
          response.clone().on_hover_text_at_pointer(href.as_ref());
        }
      }
    }

    Some(section_index)
  }

  fn handle_click(&self, ui: &mut Ui, tokens: &[Token<'_>], section_to_token: &[usize], hovered_section: Option<u32>) {
    let section_index = match hovered_section {
      Some(s) => s,
      None => return,
    };
    let token_index = section_to_token.get(section_index as usize).copied();
    let token = token_index.and_then(|idx| tokens.get(idx));

    if let Some(Token::Link { text, href, .. }) = token {
      let handled = if let Some(handler) = self.link_handler { handler.click(text, href, ui) } else { false };
      if !handled {
        ui.ctx().open_url(OpenUrl::new_tab(href.to_string()));
      }
    }
  }
}

fn paint_decorations(
  ui: &Ui,
  hr_positions: &[usize],
  galley: &Arc<Galley>,
  code_block_rects: &[Rect],
  origin: Pos2,
  available_width: f32,
  style: &MarkdownStyle,
) {
  for bg_rect in code_block_rects {
    paint::paint_code_block_bg(ui, *bg_rect, origin, &style.code_block);
  }
  paint::paint_horizontal_rules(ui, hr_positions, galley, origin, available_width, &style.horizontal_rule);
}

fn render_code_block_buttons(
  ui: &mut Ui,
  button_fn: &dyn Fn(&mut Ui, &str, &str),
  code_block_rects: &[Rect],
  code_block_info: &[(String, String)],
  response: &Response,
) {
  for (bg_rect, (code_text, lang)) in code_block_rects.iter().zip(code_block_info.iter()) {
    let mut bg_rect = bg_rect.translate(response.rect.min.to_vec2());
    bg_rect.min.x -= 4.0;
    bg_rect.min.y -= 6.0;
    bg_rect.max.x += 12.0;
    bg_rect.max.y += 6.0;

    let header_rect = Rect::from_min_max(pos2(bg_rect.min.x, bg_rect.min.y), pos2(bg_rect.max.x, bg_rect.min.y + 24.0));
    let mut child_ui =
      ui.new_child(UiBuilder::new().max_rect(header_rect).layout(Layout::right_to_left(Align::Center)));
    button_fn(&mut child_ui, code_text, lang);
  }
}

/// Paint inline widgets over their transparent placeholder regions.
fn paint_inline_widgets(
  ui: &mut Ui,
  handler: &dyn LinkHandler,
  tokens: &[Token<'_>],
  spans: &[(usize, usize, usize)],
  galley: &Arc<Galley>,
  origin: Pos2,
) {
  for &(start_char, end_char, token_index) in spans {
    let (text, href) = match tokens.get(token_index) {
      Some(Token::Link { text, href, .. }) => (text.as_ref(), href.as_ref()),
      _ => continue,
    };

    let Some((start_glyph, start_row)) = glyph_at_index(galley, start_char as u32) else { continue };
    let Some((end_glyph, end_row)) = glyph_at_index(galley, (end_char as u32).saturating_sub(1)) else { continue };

    for row_index in start_row..=end_row {
      let row = &galley.rows[row_index as usize];
      let row_sg = if row_index == start_row { Some(start_glyph) } else { row.glyphs.first() };
      let row_eg = if row_index == end_row { Some(end_glyph) } else { row.glyphs.last() };
      if let (Some(sg), Some(eg)) = (row_sg, row_eg) {
        let row_rect = Rect::from_min_max(pos2(sg.pos.x, row.min_y()), pos2(eg.pos.x + eg.advance_width, row.max_y()))
          .translate(origin.to_vec2());
        ui.push_id(token_index, |ui| {
          handler.paint_inline_widget(ui, text, href, row_rect);
        });
      }
    }
  }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn render_code_block(
  ui: &mut Ui,
  id: Id,
  text: &pulldown_cmark::CowStr<'_>,
  language: Option<&str>,
  code_block_buttons: Option<&dyn Fn(&mut Ui, &str, &str)>,
  shrink_to_content: bool,
  min_width: Option<f32>,
  style: &MarkdownStyle,
  code_theme: CodeThemeArg<'_>,
) {
  let lang = language.unwrap_or(style.default_code_language.as_str());
  let cache_id = id.with("code_galley");

  #[cfg(feature = "syntax_highlighting")]
  let galley = {
    let identity_hash = hash_code_block_identity(
      ui,
      lang,
      style.code_font_size,
      ui.visuals().dark_mode,
      ui.ctx().pixels_per_point(),
      code_theme,
    );
    let cached = ui.data(|d| d.get_temp::<StreamingCodeCache>(cache_id));
    let updated =
      scrolling_code_galley(ui, text.as_ref(), lang, style.code_font_size, identity_hash, code_theme, cached);
    let galley = Arc::clone(&updated.galley);
    ui.data_mut(|d| d.insert_temp(cache_id, updated));
    galley
  };

  #[cfg(not(feature = "syntax_highlighting"))]
  let galley = {
    let ctx_hash = hash_code_block_context(
      text.as_ref(),
      lang,
      style.code_font_size,
      ui.visuals().dark_mode,
      ui.ctx().pixels_per_point(),
      code_theme,
    );
    match ui.data(|d| d.get_temp::<CachedCodeBlock>(cache_id)) {
      Some(cached) if cached.ctx_hash == ctx_hash => cached.galley,
      _ => {
        let galley = build_code_block_galley(ui, text.as_ref(), lang, style.code_font_size, code_theme);
        ui.data_mut(|d| {
          d.insert_temp(cache_id, CachedCodeBlock { ctx_hash, galley: Arc::clone(&galley) });
        });
        galley
      }
    }
  };

  let galley_width = galley.size().x;
  let galley_height = galley.size().y;
  let stroke = Stroke::new(style.code_block.stroke_width, ui.visuals().widgets.noninteractive.bg_stroke.color);

  let p = &style.code_block.padding;
  let header_height = code_block_buttons.map_or(0.0, |_| CODE_BLOCK_HEADER_HEIGHT);
  let frame = egui::Frame::NONE
    .fill(ui.visuals().code_bg_color)
    .stroke(stroke)
    .corner_radius(style.code_block.corner_radius)
    .inner_margin(egui::Margin {
      left: p[0] as i8,
      top: (p[1] + header_height) as i8,
      right: p[2] as i8,
      bottom: p[3] as i8,
    });

  let frame_response = frame.show(ui, |ui| {
    if !shrink_to_content {
      ui.set_min_width(ui.available_width());
    } else if let Some(width) = min_width {
      // `width` is the desired outer width; we're inside the frame's margins.
      ui.set_min_width((width - p[0] - p[2]).min(ui.available_width()));
    }
    egui::ScrollArea::horizontal().id_salt(id).show(ui, |ui| {
      let (rect, _) = ui.allocate_exact_size(epaint::vec2(galley_width, galley_height), Sense::hover());
      ui.painter().galley(rect.min, galley, ui.visuals().text_color());
    });
  });

  // Overlay buttons in the top-right, absolutely positioned over the frame.
  if let Some(button_fn) = code_block_buttons {
    let border = frame_response.response.rect;
    let header_rect = Rect::from_min_max(pos2(border.min.x, border.min.y), pos2(border.max.x, border.min.y + 24.0));
    let mut child_ui =
      ui.new_child(UiBuilder::new().max_rect(header_rect).layout(Layout::right_to_left(Align::Center)));
    child_ui.add_space(4.0);
    button_fn(&mut child_ui, text, lang);
  }
}

#[cfg(not(feature = "syntax_highlighting"))]
fn build_code_block_galley(
  ui: &mut Ui,
  text: &str,
  lang: &str,
  code_font_size: f32,
  code_theme: CodeThemeArg<'_>,
) -> Arc<Galley> {
  let padded_text = crate::layout::pad_code_body(text);
  let mut job = highlight_code(ui, &padded_text, lang, code_font_size, code_theme);
  job.wrap.max_width = f32::INFINITY;
  ui.fonts_mut(|f| f.layout_job(job))
}

/// Custom cursor-from-position that avoids egui's Galley::cursor_from_pos bug (#5796).
pub fn cursor_from_pos(galley: &Galley, pos: Pos2) -> Option<u32> {
  if !galley.rect.contains(pos) {
    return None;
  }
  let mut index = 0;
  for row in &galley.rows {
    let is_pos_within_row = row.min_y() <= pos.y && pos.y <= row.max_y();
    if is_pos_within_row {
      if let Some(column) =
        row.glyphs.iter().position(|glyph| glyph.pos.x <= pos.x && glyph.pos.x + glyph.advance_width >= pos.x)
      {
        return Some(index + column as u32);
      }
    }
    index += row.char_count_including_newline().0 as u32;
  }
  None
}

/// Finds the glyph and row_index from the given galley index.
pub fn glyph_at_index(galley: &Galley, index: u32) -> Option<(&Glyph, u32)> {
  let mut offset = 0;
  for (row_index, row) in galley.rows.iter().enumerate() {
    if index < offset + row.char_count_including_newline().0 as u32 {
      return row.glyphs.get((index - offset) as usize).map(|glyph| (glyph, row_index as u32));
    }
    offset += row.char_count_including_newline().0 as u32;
  }
  None
}

/// Helpful to avoid painting underlines for trailing whitespace.
pub fn last_non_whitespace_glyph(row: &Row) -> Option<&Glyph> {
  row.glyphs.iter().rfind(|glyph| !glyph.chr.is_whitespace())
}

#[cfg(all(test, feature = "images"))]
mod tests {
  use super::*;
  use egui::RawInput;
  use std::cell::Cell;

  /// The widget `Response` rect of the first image, captured through the
  /// `LinkHandler::image_response` hook (`block_span_rects` only records
  /// the full-width block strip, which cannot discriminate centering).
  struct CaptureRect(std::cell::Cell<Option<egui::Rect>>);

  impl crate::link::LinkHandler for CaptureRect {
    fn image_response(&self, _ui: &Ui, _alt: &str, _url: &str, response: &Response) {
      self.0.set(Some(response.rect));
    }
  }

  /// Title-centered convention (`types::CENTERED_IMAGE_TITLE`): a block
  /// image carrying the marker title is horizontally centered in the
  /// column; an unmarked (or differently-titled) image stays left. No
  /// image loader is installed here, so egui draws its 24×24 loading
  /// fallback — geometry is still valid evidence.
  #[test]
  fn titled_image_centers_and_untitled_stays_left() {
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
    let image_rect = |text: &str| -> Rect {
      let capture = CaptureRect(Cell::new(None));
      let id = egui::Id::new(text);
      let output = ctx.run_ui(RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
        MarkdownLabel::new(id, text).font(egui::FontId::monospace(14.0)).wrap().link_handler(&capture).show(ui);
      });
      output.drop_without_applying_deltas();
      capture.0.take().expect("image response hook fired")
    };

    let centered = image_rect("![](x.png \"latermd:center\")");
    assert!(
      (centered.center().x - screen.center().x).abs() < 1.0,
      "titled image should center in the column: rect {centered:?} vs width 400"
    );

    let left = image_rect("![](x.png)");
    assert!(
      left.center().x < screen.center().x - 100.0,
      "untitled image should stay near the left edge: rect {left:?}"
    );
    let left_titled_other = image_rect("![](x.png \"tooltip\")");
    assert!(
      left_titled_other.center().x < screen.center().x - 100.0,
      "an ordinary title must not trigger centering: rect {left_titled_other:?}"
    );
  }
}

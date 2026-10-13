//! `link_response` / `image_response`(①类扩展点):默认渲染的链接与图片
//! 把 Interaction Response 暴露给 LinkHandler,宿主得以挂右键菜单等
//! 附加交互。钉三件事:逐链接以正确 (text, href, response) 调用、
//! 图片臂以 (alt, url) 调用、默认实现零调用可安全忽略。

use std::cell::RefCell;

use egui::{vec2, Context, Id, RawInput, Rect, UiBuilder};
use egui_markdown::link::LinkHandler;
use egui_markdown::MarkdownLabel;

fn screen() -> Rect {
  Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(500.0, 2000.0))
}

struct Recorder {
  links: RefCell<Vec<(String, String, bool)>>,
  images: RefCell<Vec<(String, String, bool)>>,
}

impl Recorder {
  fn new() -> Self {
    Self { links: RefCell::new(Vec::new()), images: RefCell::new(Vec::new()) }
  }
}

impl LinkHandler for Recorder {
  fn link_response(&self, _ui: &egui::Ui, text: &str, href: &str, response: &egui::Response) {
    self.links.borrow_mut().push((text.to_owned(), href.to_owned(), response.id == egui::Id::NULL));
  }

  fn image_response(&self, _ui: &egui::Ui, alt: &str, url: &str, response: &egui::Response) {
    self.images.borrow_mut().push((alt.to_owned(), url.to_owned(), response.id == egui::Id::NULL));
  }
}

#[test]
fn link_response_fires_for_right_clicked_link_only() {
  // galley 主路径的 hook 由 secondary click 驱动:右键「甲」只交出「甲」,
  // 且 response 是整块 label 的真实 response(非 NULL)。
  let doc = "开头 [甲](https://a.example) 中 [乙](https://b.example) 结尾\n";
  let handler = Recorder::new();

  // 帧一:渲出字形,从帧输出里量「甲」的中心
  let ctx = Context::default();
  let mut output = ctx.run_ui(RawInput { screen_rect: Some(screen()), ..Default::default() }, |ui| {
    let mut child = ui.new_child(UiBuilder::new().max_rect(screen()));
    MarkdownLabel::new(Id::new("test"), doc).link_handler(&handler).show(&mut child);
  });
  // 整行是一个 galley,行中心未必落在链接上;按「甲」的字符位精确取点
  let target = output
    .shapes
    .iter()
    .find_map(|clipped| {
      let egui::epaint::Shape::Text(text) = &clipped.shape else {
        return None;
      };
      let char_index = text.galley.job.text.chars().position(|c| c == '甲')?;
      let rect = text.galley.pos_from_cursor(egui::text::CCursor::new(char_index));
      Some(
        egui::pos2(0.0, 0.0)
          + (text.galley.mesh_bounds.min.to_vec2()
            + rect.min.to_vec2()
            + egui::vec2(2.0, rect.height() / 2.0)
            + text.pos.to_vec2()),
      )
    })
    .expect("「甲」已渲染");
  output.textures_delta.clear();

  // 帧二/三:右键「甲」(按下帧 + 释放帧)
  let press = |pos, pressed| egui::Event::PointerButton {
    pos,
    button: egui::PointerButton::Secondary,
    pressed,
    modifiers: Default::default(),
  };
  let mut output = ctx.run_ui(
    RawInput {
      screen_rect: Some(screen()),
      events: vec![egui::Event::PointerMoved(target), press(target, true)],
      ..Default::default()
    },
    |ui| {
      let mut child = ui.new_child(UiBuilder::new().max_rect(screen()));
      MarkdownLabel::new(Id::new("test"), doc).link_handler(&handler).show(&mut child);
    },
  );
  output.textures_delta.clear();
  assert!(handler.links.borrow().is_empty(), "按下帧还只是塌缩/悬停,不交钩");

  let mut output = ctx.run_ui(
    RawInput { screen_rect: Some(screen()), events: vec![press(target, false)], ..Default::default() },
    |ui| {
      let mut child = ui.new_child(UiBuilder::new().max_rect(screen()));
      MarkdownLabel::new(Id::new("test"), doc).link_handler(&handler).show(&mut child);
    },
  );
  output.textures_delta.clear();

  assert_eq!(
    handler.links.borrow().clone(),
    vec![("甲".to_owned(), "https://a.example".to_owned(), false)],
    "右键哪个链接就只交哪个链接"
  );
}

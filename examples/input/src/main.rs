use gpui::*;
use gpui_component::{
    input::{InlineReplacement, Input, InputState},
    *,
};
use gpui::Focusable as _;
use gpui_component_assets::Assets;

const DRAFT: &str = "look at [Image #1](/Users/me/Pictures/a very long screenshot name.png) then open [File #2](apps/desktop/src/app/native_chat/composer.rs:42) and the [docs](https://example.com/guide) folder [Folder #3](packages/core-ui/chat/)\nsecond line with [File #4](README.md) inside it";

pub struct Example {
    input_state: Entity<InputState>,
}

fn replacements(text: &str) -> Vec<InlineReplacement> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while let Some(open) = text[cursor..].find('[') {
        let start = cursor + open;
        let Some(label_end) = text[start..].find("](") else { break };
        let Some(close) = text[start + label_end..].find(')') else { break };
        let end = start + label_end + close + 1;
        let label = &text[start + 1..start + label_end];
        let path = &text[start + label_end + 2..end - 1];
        let (icon, color) = if label.starts_with("Image") {
            ("icons/file.svg", rgb(0x5f9878))
        } else if label.starts_with("Folder") {
            ("icons/folder.svg", rgb(0x9a835b))
        } else if path.starts_with("http") {
            ("icons/external-link.svg", rgb(0x6687a3))
        } else {
            ("icons/file.svg", rgb(0x6c819b))
        };
        result.push(
            InlineReplacement::new(start..end, format!("\u{a0}\u{a0}\u{a0}\u{a0}\u{2009}{}\u{2009}", label.replace(' ', "\u{a0}")))
                .color(color)
                .icon(icon, px(14.), px(1.6))
                .pointer(!path.starts_with("http")),
        );
        cursor = end;
    }
    result
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input_state = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(3, 7)
                .default_value(DRAFT)
        });
        input_state.update(cx, |state, cx| {
            state.set_inline_replacements(replacements(DRAFT), cx)
        });
        input_state.read(cx).focus_handle(cx).focus(window, cx);
        Self { input_state }
    }
}

impl Render for Example {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.input_state.clone();
        let value = state.read(cx).value();
        v_flex()
            .p_5()
            .gap_4()
            .size_full()
            .child(
                div()
                    .w_full()
                    .p_4()
                    .rounded(px(22.))
                    .border_1()
                    .border_color(rgb(0x333333))
                    .text_size(px(14.))
                    .line_height(px(24.))
                    .child(Input::new(&self.input_state).appearance(false).bordered(false).p_0()),
            )
            .child(div().text_size(px(11.)).child(format!("{value:?}")))
            .child(div().text_size(px(11.)).child(format!(
                "cursor {} selection {:?}",
                state.read(cx).cursor(),
                state.read(cx).selected_range()
            )))
    }
}

fn main() {
    let app = gpui_platform::application().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(760.), px(420.)), cx)),
            ..Default::default()
        };
        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
                let view = cx.new(|cx| Example::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });
}

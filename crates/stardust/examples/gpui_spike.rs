//! G1 spike: one minimal native window built with `gpui-kit`.
//!
//! Run with:
//! `cargo run --example gpui_spike --features gpui-kit-spike`
//!
//! The window holds only synthetic text: a label, a button and a focusable
//! field. It uses no account, network, notification or payment code and is not
//! a replacement for any Stardust screen.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

/// The spike's single view: a label, a click counter button and a text field.
struct Spike {
    field: Entity<InputState>,
    clicks: usize,
}

impl Spike {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| InputState::new(window, cx).placeholder("Type here"));
        Self { field, clicks: 0 }
    }
}

impl Render for Spike {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .size_full()
            .child("Stardust gpui-kit spike")
            .child(
                Button::new("spike-button")
                    .primary()
                    .label(format!("Clicked {} times", self.clicks))
                    .on_click(move |_, _, cx| {
                        view.update(cx, |spike, cx| {
                            spike.clicks += 1;
                            cx.notify();
                        });
                    }),
            )
            .child(Input::new(&self.field))
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Spike::new(window, cx))
        })
        .expect("failed to open the spike window");
        cx.activate(true);
    });
}

//! sid — a native GPUI desktop cockpit for developer workflow.
//!
//! Entry point: open the global store (seeding a demo set on first run), then open the
//! window over the single [`app::AppState`] entity.

mod app;
mod ui;

use gpui::{Application, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};

fn main() {
    // A5 observation hook — remove in A6.
    // Run `cargo run -p sid -- --input-demo` to overlay a bare TextInput scratch view
    // (one plain + one masked) instead of the app, to eyeball typing/selection/IME/paste.
    let input_demo = std::env::args().any(|a| a == "--input-demo");

    Application::new().run(move |cx| {
        ui::init(cx);

        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);

        // A5 observation hook — remove in A6.
        if input_demo {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_window, cx| cx.new(input_demo::InputDemo::new),
            )
            .unwrap();
            cx.activate(true);
            return;
        }

        let store = app::open_store();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_cx| app::AppState::new(store)),
        )
        .unwrap();
        cx.activate(true);
    });
}

// A5 observation hook — remove in A6.
/// Scratch overlay for eyeballing the [`ui::TextInput`] element: one plain input and one
/// masked input on a dark backdrop. Not part of the shipping app; gated behind
/// `--input-demo`.
mod input_demo {
    use crate::ui::TextInput;
    use gpui::{Context, Entity, Window, div, prelude::*, px, rgb};

    pub struct InputDemo {
        plain: Entity<TextInput>,
        masked: Entity<TextInput>,
        focused: bool,
    }

    impl InputDemo {
        pub fn new(cx: &mut Context<Self>) -> Self {
            let plain = cx.new(|cx| TextInput::new(cx, "type here — unicode, IME, paste…"));
            let masked = cx.new(|cx| TextInput::new_masked(cx, "password (renders bullets)"));
            Self {
                plain,
                masked,
                focused: false,
            }
        }
    }

    impl Render for InputDemo {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // Focus the plain input once, on first paint, so typing works immediately.
            if !self.focused {
                self.plain.read(cx).focus(window);
                self.focused = true;
            }

            div()
                .flex()
                .flex_col()
                .gap_4()
                .size_full()
                .bg(rgb(0x161618))
                .text_color(rgb(0xdcdce0))
                .p(px(40.))
                .child(div().text_sm().child("TextInput observation (A5) — plain:"))
                .child(self.plain.clone())
                .child(div().text_sm().child("masked:"))
                .child(self.masked.clone())
        }
    }
}

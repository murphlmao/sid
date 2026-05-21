use sid_core::context::WidgetCtx;
use sid_core::event::Event;
use sid_core::layout::{Dir, Layout};
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

struct W {
    id: WidgetId,
    title: &'static str,
}

impl W {
    fn new(s: &'static str) -> Self {
        Self {
            id: WidgetId::new(s),
            title: s,
        }
    }
}

impl Widget for W {
    fn id(&self) -> &WidgetId { &self.id }
    fn title(&self) -> &str { self.title }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
}

#[test]
fn single_layout_holds_one_widget() {
    let layout: Layout = Layout::Single(Box::new(W::new("only")));
    let titles: Vec<String> = layout.iter_widgets().map(|w| w.title().to_string()).collect();
    assert_eq!(titles, vec!["only".to_string()]);
}

#[test]
fn split_layout_iterates_in_order() {
    let layout = Layout::Split {
        dir: Dir::Horizontal,
        ratio: 0.5,
        a: Box::new(Layout::Single(Box::new(W::new("a")))),
        b: Box::new(Layout::Single(Box::new(W::new("b")))),
    };
    let titles: Vec<String> = layout.iter_widgets().map(|w| w.title().to_string()).collect();
    assert_eq!(titles, vec!["a".to_string(), "b".to_string()]);
}

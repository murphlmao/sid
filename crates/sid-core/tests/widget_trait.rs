use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

struct Dummy {
    id: WidgetId,
    title: &'static str,
}

impl Widget for Dummy {
    fn id(&self) -> WidgetId {
        self.id.clone()
    }
    fn title(&self) -> &str {
        self.title
    }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(
        &mut self,
        _ev: &sid_core::event::Event,
        _ctx: &mut sid_core::context::WidgetCtx,
    ) -> EventOutcome {
        EventOutcome::Consumed
    }
    fn save_state(&self) -> Vec<u8> {
        Vec::new()
    }
    fn load_state(&mut self, _: &[u8]) {}
}

#[test]
fn dummy_widget_reports_metadata() {
    let d = Dummy {
        id: WidgetId::new("dummy"),
        title: "Dummy",
    };
    assert_eq!(d.id().as_str(), "dummy");
    assert_eq!(d.title(), "Dummy");
}

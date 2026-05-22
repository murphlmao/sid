use crate::widget::Widget;

/// Direction of a layout split.
///
/// # Examples
///
/// ```
/// use sid_core::Dir;
///
/// let h = Dir::Horizontal;
/// let v = Dir::Vertical;
/// assert_ne!(h, v);
/// assert_eq!(h, Dir::Horizontal);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dir {
    /// Children are placed side by side.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::Dir;
    ///
    /// let d = Dir::Horizontal;
    /// assert_eq!(d, Dir::Horizontal);
    /// ```
    Horizontal,

    /// Children are stacked top-to-bottom.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::Dir;
    ///
    /// let d = Dir::Vertical;
    /// assert_eq!(d, Dir::Vertical);
    /// ```
    Vertical,
}

/// Tree of widgets inside a single tab.
///
/// v1 only constructs `Single`. v2+ uses `Split` for Hyprland-style composition;
/// the variant is present here so future composition is a non-breaking addition.
pub enum Layout {
    Single(Box<dyn Widget>),
    Split {
        dir: Dir,
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

impl Layout {
    /// In-order traversal of every widget in the layout.
    ///
    /// Visits leaves left-to-right (for `Split`, `a` before `b`).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::layout::{Dir, Layout};
    /// use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// use sid_core::event::Event;
    /// use sid_core::context::WidgetCtx;
    ///
    /// struct W { id: WidgetId, title: &'static str }
    /// impl Widget for W {
    ///     fn id(&self) -> &WidgetId { &self.id }
    ///     fn title(&self) -> &str { self.title }
    ///     fn render(&self, _: &mut dyn RenderTarget) {}
    ///     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome {
    ///         EventOutcome::Bubble
    ///     }
    ///     fn as_any(&self) -> &dyn std::any::Any { self }
    ///     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    /// }
    ///
    /// let layout = Layout::Single(Box::new(W { id: WidgetId::new("w"), title: "W" }));
    /// assert_eq!(layout.iter_widgets().count(), 1);
    /// ```
    pub fn iter_widgets(&self) -> WidgetIter<'_> {
        WidgetIter { stack: vec![self] }
    }

    /// In-order mutable traversal of every widget in the layout.
    ///
    /// Visits leaves left-to-right (for `Split`, `a` before `b`).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::layout::{Dir, Layout};
    /// use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// use sid_core::event::Event;
    /// use sid_core::context::WidgetCtx;
    ///
    /// struct W { id: WidgetId, title: &'static str }
    /// impl Widget for W {
    ///     fn id(&self) -> &WidgetId { &self.id }
    ///     fn title(&self) -> &str { self.title }
    ///     fn render(&self, _: &mut dyn RenderTarget) {}
    ///     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome {
    ///         EventOutcome::Bubble
    ///     }
    ///     fn as_any(&self) -> &dyn std::any::Any { self }
    ///     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    /// }
    ///
    /// let mut layout = Layout::Single(Box::new(W { id: WidgetId::new("w"), title: "W" }));
    /// assert_eq!(layout.iter_widgets_mut().count(), 1);
    /// ```
    pub fn iter_widgets_mut(&mut self) -> WidgetIterMut<'_> {
        WidgetIterMut { stack: vec![self] }
    }
}

pub struct WidgetIter<'a> {
    stack: Vec<&'a Layout>,
}

impl<'a> Iterator for WidgetIter<'a> {
    type Item = &'a dyn Widget;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            match node {
                Layout::Single(w) => return Some(w.as_ref()),
                Layout::Split { a, b, .. } => {
                    self.stack.push(b);
                    self.stack.push(a);
                }
            }
        }
        None
    }
}

pub struct WidgetIterMut<'a> {
    stack: Vec<&'a mut Layout>,
}

impl<'a> Iterator for WidgetIterMut<'a> {
    type Item = &'a mut dyn Widget;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            match node {
                Layout::Single(w) => return Some(w.as_mut()),
                Layout::Split { a, b, .. } => {
                    self.stack.push(b);
                    self.stack.push(a);
                }
            }
        }
        None
    }
}

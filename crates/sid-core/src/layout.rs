use crate::widget::Widget;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dir {
    Horizontal,
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
    pub fn iter_widgets(&self) -> WidgetIter<'_> {
        WidgetIter { stack: vec![self] }
    }

    /// In-order mutable traversal of every widget in the layout.
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

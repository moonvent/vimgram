//! The message list, placed to the pixel.
//!
//! The list is a stack of items, oldest on top. Where it is scrolled to is
//! the item at the bottom of the screen and how many pixels of it are below
//! the screen (`shift`): new messages, older history arriving and heights
//! being corrected then leave what is on screen in place.
//!
//! Heights are first estimated; the [`Feed`] widget reports the ones it
//! actually laid out, which replace the estimates from then on.

use std::ops::Range;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Clipboard, Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Padding, Point, Rectangle, Size, Vector, window};

/// Slack for float comparisons, in pixels.
const EPS: f32 = 0.5;

/// Items stacked top to bottom: item `i` takes `[top(i), end(i))`, with a
/// screen `view` pixels tall over them.
pub struct Stack {
    ys: Vec<f32>,
    pub view: f32,
}

impl Stack {
    pub fn new(heights: impl IntoIterator<Item = f32>, view: f32) -> Self {
        let mut ys = vec![0.0];
        let mut y = 0.0;
        for h in heights {
            y += h.max(0.0);
            ys.push(y);
        }
        Self { ys, view }
    }

    pub fn len(&self) -> usize {
        self.ys.len() - 1
    }

    pub fn top(&self, i: usize) -> f32 {
        self.ys[i]
    }

    pub fn end(&self, i: usize) -> f32 {
        self.ys[i + 1]
    }

    pub fn height(&self, i: usize) -> f32 {
        self.end(i) - self.top(i)
    }

    pub fn total(&self) -> f32 {
        self.ys[self.len()]
    }

    /// Taller than the screen: seen a part at a time.
    pub fn tall(&self, i: usize) -> bool {
        self.height(i) > self.view + EPS
    }

    /// The bottom of the screen with `bottom` the lowest item on it and
    /// `shift` pixels of that item below the screen.
    pub fn position(&self, bottom: usize, shift: f32) -> f32 {
        if self.len() == 0 {
            return 0.0;
        }
        self.end(bottom.min(self.len() - 1)) - shift
    }

    /// Never past the newest item, nor above the oldest one (a list shorter
    /// than the screen sits at its bottom).
    pub fn clamp(&self, y: f32) -> f32 {
        y.clamp(self.view.min(self.total()), self.total())
    }

    /// The lowest item on screen at `y` and how much of it is below.
    pub fn anchor(&self, y: f32) -> (usize, f32) {
        if self.len() == 0 {
            return (0, 0.0);
        }
        let y = self.clamp(y);
        let i = self.ys[1..].partition_point(|&end| end < y - 0.01).min(self.len() - 1);
        (i, (self.end(i) - y).max(0.0))
    }

    /// Items at least partly on screen.
    pub fn visible(&self, y: f32) -> Range<usize> {
        if self.len() == 0 {
            return 0..0;
        }
        let top = y - self.view;
        let first = self.ys[1..].partition_point(|&end| end <= top + 0.01).min(self.len() - 1);
        first..self.anchor(y).0 + 1
    }

    pub fn fully_visible(&self, i: usize, y: f32) -> bool {
        self.top(i) >= y - self.view - EPS && self.end(i) <= y + EPS
    }

    /// Where the screen goes to show item `i`, moving as little as possible,
    /// with up to `context` items around it when they fit. Of an item taller
    /// than the screen, as much as fits from the side the cursor came from
    /// (`up`: from below, so its end; otherwise its beginning).
    pub fn reveal(&self, y: f32, i: usize, context: usize, up: bool) -> f32 {
        if i >= self.len() {
            return self.clamp(y);
        }
        let view = self.view;
        if self.tall(i) {
            let y = if up { self.end(i) } else { self.top(i) + view };
            return self.clamp(y);
        }
        let (top, end) = (self.top(i), self.end(i));
        let below = self.end((i + context).min(self.len() - 1)).min(top + view);
        let above = self.top(i.saturating_sub(context)).max(end - view);
        // What lies the way the cursor goes comes first.
        let show_below = |y: f32| y.max(below);
        let show_above = |y: f32| if above < y - view { above + view } else { y };
        let y = if up { show_above(show_below(y)) } else { show_below(show_above(y)) };
        self.clamp(y)
    }

    /// The item for the cursor after the screen moved to `y`: the cursor's
    /// own while it is in full view (or partly, when taller than the
    /// screen), else the nearest one in full view.
    pub fn follow(&self, y: f32, cursor: usize) -> usize {
        let shown = self.visible(y);
        if shown.is_empty() {
            return cursor;
        }
        let cursor = cursor.min(self.len() - 1);
        if self.fully_visible(cursor, y) || shown.contains(&cursor) && self.tall(cursor) {
            return cursor;
        }
        let nearest = |items: &mut dyn Iterator<Item = usize>| items.min_by_key(|&i| i.abs_diff(cursor));
        nearest(&mut shown.clone().filter(|&i| self.fully_visible(i, y)))
            .or_else(|| nearest(&mut shown.clone()))
            .unwrap_or(cursor)
    }
}

/// Laid-out heights, reported for these message ids.
pub type Measured = Vec<(i64, f32)>;

/// Items shown bottom up from `anchor`, whose bottom is `shift` pixels
/// below the bottom of the list (less `padding.bottom`); `spacing` below
/// each. Clipped to its bounds.
pub struct Feed<'a, Message> {
    items: Vec<Element<'a, Message>>,
    /// Message id of each item and the height it is known to have, for
    /// reporting the ones that turn out different.
    ids: Vec<(i64, Option<f32>)>,
    anchor: usize,
    shift: f32,
    spacing: f32,
    padding: Padding,
    on_measure: fn(Measured) -> Message,
}

impl<'a, Message> Feed<'a, Message> {
    pub fn new(
        items: Vec<(i64, Option<f32>, Element<'a, Message>)>,
        anchor: usize,
        shift: f32,
        on_measure: fn(Measured) -> Message,
    ) -> Self {
        let (ids, items) = items.into_iter().map(|(id, known, item)| ((id, known), item)).unzip();
        Self { items, ids, anchor, shift, spacing: 0.0, padding: Padding::ZERO, on_measure }
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn padding(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }
}

/// The cursor is for the items only over the list, not over what they are
/// scrolled under.
fn inside(cursor: mouse::Cursor, bounds: Rectangle) -> mouse::Cursor {
    if cursor.is_over(bounds) { cursor } else { mouse::Cursor::Unavailable }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Feed<'_, Message> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn children(&self) -> Vec<Tree> {
        self.items.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.items);
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        let size = limits.width(Length::Fill).height(Length::Fill).max();
        let width = (size.width - self.padding.left - self.padding.right).max(0.0);
        let item_limits = layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY));
        let mut nodes: Vec<layout::Node> = self
            .items
            .iter_mut()
            .zip(&mut tree.children)
            .map(|(item, tree)| item.as_widget_mut().layout(tree, renderer, &item_limits))
            .collect();
        if nodes.is_empty() {
            return layout::Node::new(size);
        }
        let anchor = self.anchor.min(nodes.len() - 1);
        // Bottom up from the anchor, then down from it.
        let mut end = size.height - self.padding.bottom + self.shift;
        for node in nodes[..=anchor].iter_mut().rev() {
            end -= node.size().height + self.spacing;
            node.move_to_mut(Point::new(self.padding.left, end));
        }
        let mut top = size.height - self.padding.bottom + self.shift;
        for node in nodes.iter_mut().skip(anchor + 1) {
            node.move_to_mut(Point::new(self.padding.left, top));
            top += node.size().height + self.spacing;
        }
        layout::Node::with_children(size, nodes)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((item, tree), layout) in self.items.iter_mut().zip(&mut tree.children).zip(layout.children()) {
                item.as_widget_mut().operate(tree, layout, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if let Event::Window(window::Event::RedrawRequested(_)) = event {
            let changed: Measured = self
                .ids
                .iter()
                .zip(layout.children())
                .map(|(&(id, known), layout)| (id, known, layout.bounds().height + self.spacing))
                .filter(|&(_, known, height)| known.is_none_or(|known| (known - height).abs() > EPS))
                .map(|(id, _, height)| (id, height))
                .collect();
            if !changed.is_empty() {
                shell.publish((self.on_measure)(changed));
            }
        }
        let cursor = inside(cursor, bounds);
        let Some(viewport) = bounds.intersection(viewport) else {
            return;
        };
        for ((item, tree), layout) in self.items.iter_mut().zip(&mut tree.children).zip(layout.children()) {
            if layout.bounds().intersects(&viewport) {
                item.as_widget_mut().update(tree, event, layout, cursor, renderer, clipboard, shell, &viewport);
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        let cursor = inside(cursor, bounds);
        let Some(viewport) = bounds.intersection(viewport) else {
            return mouse::Interaction::default();
        };
        self.items
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .filter(|(_, layout)| layout.bounds().intersects(&viewport))
            .map(|((item, tree), layout)| item.as_widget().mouse_interaction(tree, layout, cursor, &viewport, renderer))
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let Some(viewport) = bounds.intersection(viewport) else {
            return;
        };
        let cursor = inside(cursor, bounds);
        renderer.with_layer(viewport, |renderer| {
            for ((item, tree), layout) in self.items.iter().zip(&tree.children).zip(layout.children()) {
                if layout.bounds().intersects(&viewport) {
                    item.as_widget().draw(tree, renderer, theme, style, layout, cursor, &viewport);
                }
            }
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        overlay::from_children(&mut self.items, tree, layout, renderer, viewport, translation)
    }
}

impl<'a, Message: 'a> From<Feed<'a, Message>> for Element<'a, Message> {
    fn from(feed: Feed<'a, Message>) -> Self {
        Element::new(feed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Items 100 tall but the third 300, on a 250 tall screen.
    fn stack() -> Stack {
        Stack::new([100.0, 100.0, 300.0, 100.0, 100.0], 250.0)
    }

    #[test]
    fn anchor_and_position() {
        let s = stack();
        assert_eq!(s.total(), 700.0);
        assert_eq!(s.anchor(700.0), (4, 0.0));
        assert_eq!(s.anchor(650.0), (4, 50.0));
        assert_eq!(s.anchor(600.0), (3, 0.0));
        assert_eq!(s.anchor(450.0), (2, 50.0));
        assert_eq!(s.position(2, 50.0), 450.0);
        // Clamped to the list.
        assert_eq!(s.anchor(10_000.0), (4, 0.0));
        assert_eq!(s.anchor(0.0), (2, 250.0));
        // Shorter than the screen: at its bottom.
        assert_eq!(Stack::new([50.0, 50.0], 250.0).anchor(0.0), (1, 0.0));
    }

    #[test]
    fn visible_items() {
        let s = stack();
        assert_eq!(s.visible(700.0), 2..5);
        assert_eq!(s.visible(650.0), 2..5);
        assert_eq!(s.visible(450.0), 2..3);
        assert!(s.fully_visible(4, 700.0));
        assert!(s.fully_visible(3, 650.0));
        assert!(!s.fully_visible(2, 650.0));
    }

    #[test]
    fn reveal_moves_as_little_as_possible() {
        let s = Stack::new([100.0; 10], 250.0);
        // In full view already.
        assert_eq!(s.reveal(1000.0, 8, 0, true), 1000.0);
        // Up by one: just enough to show it whole.
        assert_eq!(s.reveal(1000.0, 7, 0, true), 950.0);
        // With one item of context above, as it fits.
        assert_eq!(s.reveal(1000.0, 7, 1, true), 850.0);
        // Down: the same from the other side.
        assert_eq!(s.reveal(250.0, 2, 0, false), 300.0);
        assert_eq!(s.reveal(250.0, 2, 1, false), 400.0);
        // Not past the ends.
        assert_eq!(s.reveal(1000.0, 9, 2, false), 1000.0);
        assert_eq!(s.reveal(250.0, 0, 2, true), 250.0);
    }

    #[test]
    fn reveal_tall_from_the_side_it_is_entered() {
        let s = stack();
        // From below: its end at the bottom of the screen.
        assert_eq!(s.reveal(700.0, 2, 2, true), 500.0);
        // From above, or jumped to: its beginning at the top.
        assert_eq!(s.reveal(250.0, 2, 2, false), 450.0);
    }

    #[test]
    fn cursor_follows_the_screen() {
        let s = Stack::new([100.0; 10], 250.0);
        // Still in full view.
        assert_eq!(s.follow(1000.0, 8), 8);
        // Scrolled off the bottom: the lowest one in full view.
        assert_eq!(s.follow(800.0, 9), 7);
        // Off the top: the highest one.
        assert_eq!(s.follow(1000.0, 2), 8);
        // Partly visible tall item keeps the cursor.
        assert_eq!(stack().follow(480.0, 2), 2);
    }
}

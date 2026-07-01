//! Reusable single-line text input, ported from gpui 0.2.2's `examples/input.rs`.
//!
//! This is the P3.2 spearhead element: an [`EntityInputHandler`] with IME-correct
//! range handling, cursor + selection rendering, mouse click/drag-to-position,
//! word-aware navigation, clipboard, and a `masked` mode that renders bullets while
//! keeping the real content (for password / passphrase fields).
//!
//! Ported from the gpui example; the window/demo `main` scaffolding is dropped. The
//! actions and keybindings live in [`super`] so a single [`super::init`] call wires
//! them once, scoped to the `TextInput` key context.
//!
//! # Masking and IME
//!
//! Masking is a *render-only* transform. The stored [`content`](TextInput::content)
//! and every offset/range the [`EntityInputHandler`] traffics in are always the real
//! text, so IME composition, selection, and clipboard read the true value. During
//! prepaint the display glyphs are swapped for bullets **one bullet per grapheme**, so
//! cursor and selection x-positions stay aligned with the real caret. Callers should
//! not enable IME composition affordances on masked fields (a password is not composed),
//! but if the platform delivers marked text anyway it is still handled correctly — the
//! underline just renders over bullets.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, Render, ShapedLine, SharedString, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, div, fill, hsla, point, prelude::*, px, relative, rgb, rgba, size,
};
use unicode_segmentation::UnicodeSegmentation;

use super::{
    Backspace, Copy, Cut, Delete, End, Home, Left, Paste, Right, SelectAll, SelectLeft,
    SelectRight, SelectToEnd, SelectToHome, ShowCharacterPalette, WordLeft, WordRight,
};

// Dark-theme palette, aligned with `app.rs`. Kept local so `ui` stays self-contained.
const FIELD_BG: u32 = 0x121215;
const FIELD_BORDER: u32 = 0x33343a;
const FIELD_FG: u32 = 0xdcdce0;
const CURSOR: u32 = 0x5a9ad0;
/// The masking glyph rendered in place of each real grapheme.
const BULLET: char = '\u{2022}';

/// A single-line text input entity.
///
/// Construct with [`TextInput::new`] (via `cx.new(|cx| TextInput::new(cx, "placeholder"))`)
/// or [`TextInput::new_masked`] for secret fields. Read/replace the value with
/// [`content`](Self::content) / [`set_content`](Self::set_content).
pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    masked: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl TextInput {
    /// A plain single-line input with the given placeholder text.
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: placeholder.into(),
            masked: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }

    /// A masked input (renders bullets) for passwords / passphrases.
    pub fn new_masked(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            masked: true,
            ..Self::new(cx, placeholder)
        }
    }

    // ---- public form API ---------------------------------------------------
    // Consumed by the host form in A6; the `#[allow]` keeps `-D warnings` green until
    // then. Remove the attribute once A6 wires these up.

    /// The current real content (never masked).
    #[allow(dead_code)]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Replace the entire content, clamping the selection to the new end.
    #[allow(dead_code)]
    pub fn set_content(&mut self, content: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = content.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    /// Whether the content is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Whether this input masks its content on render.
    #[allow(dead_code)]
    pub fn is_masked(&self) -> bool {
        self.masked
    }

    /// Move keyboard focus to this input.
    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    /// Clear all state (content, selection, IME marks, cached layout).
    #[allow(dead_code)]
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.content = SharedString::default();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_bounds = None;
        self.is_selecting = false;
        cx.notify();
    }

    // ---- action handlers ---------------------------------------------------

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn select_to_home(&mut self, _: &SelectToHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_to_end(&mut self, _: &SelectToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Single-line: collapse any newlines a paste might carry.
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        // Never expose masked content to the clipboard.
        if self.masked {
            return;
        }
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        // For masked fields, delete the selection but do not copy it out.
        if !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    // ---- selection primitives ---------------------------------------------

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        // Mouse hit-testing runs against the display line (possibly bullets), so translate
        // the returned display index back to a real content byte offset.
        let display_index = line.closest_index_for_x(position.x - bounds.left());
        self.real_offset_for_display(display_index)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    /// Translate a byte offset in the display string (bullets when masked) back to a byte
    /// offset in the real content.
    fn real_offset_for_display(&self, display_index: usize) -> usize {
        if !self.masked {
            return display_index;
        }
        let grapheme_ix = display_index / BULLET.len_utf8();
        self.content
            .grapheme_indices(true)
            .nth(grapheme_ix)
            .map(|(idx, _)| idx)
            .unwrap_or(self.content.len())
    }

    // ---- UTF-8 / UTF-16 / grapheme helpers (pure, unit-tested) -------------

    fn offset_from_utf16(&self, offset: usize) -> usize {
        offset_from_utf16(&self.content, offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        offset_to_utf16(&self.content, offset)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        previous_grapheme_boundary(&self.content, offset)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        next_grapheme_boundary(&self.content, offset)
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        previous_word_boundary(&self.content, offset)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        next_word_boundary(&self.content, offset)
    }
}

// ---- free-standing pure helpers (unit-testable without a window) -----------

/// UTF-16 code-unit offset → UTF-8 byte offset within `s`.
pub(crate) fn offset_from_utf16(s: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_count = 0;
    for ch in s.chars() {
        if utf16_count >= offset {
            break;
        }
        utf16_count += ch.len_utf16();
        utf8_offset += ch.len_utf8();
    }
    utf8_offset
}

/// UTF-8 byte offset → UTF-16 code-unit offset within `s`.
pub(crate) fn offset_to_utf16(s: &str, offset: usize) -> usize {
    let mut utf16_offset = 0;
    let mut utf8_count = 0;
    for ch in s.chars() {
        if utf8_count >= offset {
            break;
        }
        utf8_count += ch.len_utf8();
        utf16_offset += ch.len_utf16();
    }
    utf16_offset
}

/// Byte offset of the grapheme boundary strictly before `offset` (or 0).
pub(crate) fn previous_grapheme_boundary(s: &str, offset: usize) -> usize {
    s.grapheme_indices(true)
        .rev()
        .find_map(|(idx, _)| (idx < offset).then_some(idx))
        .unwrap_or(0)
}

/// Byte offset of the grapheme boundary strictly after `offset` (or `s.len()`).
pub(crate) fn next_grapheme_boundary(s: &str, offset: usize) -> usize {
    s.grapheme_indices(true)
        .find_map(|(idx, _)| (idx > offset).then_some(idx))
        .unwrap_or(s.len())
}

/// Byte offset of the word boundary strictly before `offset` (or 0).
///
/// Uses unicode word boundaries: lands at the start of the nearest word that begins
/// before the cursor, skipping trailing whitespace.
pub(crate) fn previous_word_boundary(s: &str, offset: usize) -> usize {
    s.split_word_bound_indices()
        .rfind(|(idx, word)| *idx < offset && !word.trim().is_empty())
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

/// Byte offset of the word boundary strictly after `offset` (or `s.len()`).
///
/// Lands at the end of the next non-whitespace word.
pub(crate) fn next_word_boundary(s: &str, offset: usize) -> usize {
    s.split_word_bound_indices()
        .filter(|(_, word)| !word.trim().is_empty())
        .map(|(idx, word)| idx + word.len())
        .find(|end| *end > offset)
        .unwrap_or(s.len())
}

/// Build the string actually shown on screen: bullets (one per grapheme) when
/// `masked`, otherwise the content itself. Kept a free function so it is unit-testable.
pub(crate) fn display_string(content: &str, masked: bool) -> String {
    if masked {
        content.graphemes(true).map(|_| BULLET).collect()
    } else {
        content.to_string()
    }
}

/// Map a byte offset in the *real* content to the corresponding byte offset in the
/// masked display string. Because each grapheme becomes exactly one bullet (a 3-byte
/// UTF-8 char), offsets must be translated so cursor/selection x-positions land on the
/// right bullet.
pub(crate) fn display_offset(content: &str, masked: bool, real_offset: usize) -> usize {
    if !masked {
        return real_offset;
    }
    let bullet_len = BULLET.len_utf8();
    content
        .grapheme_indices(true)
        .take_while(|(idx, _)| *idx < real_offset)
        .count()
        * bullet_len
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = display_offset(&self.content, self.masked, range.start);
        let end = display_offset(&self.content, self.masked, range.end);
        Some(Bounds::from_corners(
            point(bounds.left() + last_layout.x_for_index(start), bounds.top()),
            point(
                bounds.left() + last_layout.x_for_index(end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let display_index = last_layout.index_for_x(point.x - line_point.x)?;
        let real_index = self.real_offset_for_display(display_index);
        Some(self.offset_to_utf16(real_index))
    }
}

/// The custom element that shapes and paints the input line. Not public — it is an
/// implementation detail of [`TextInput`]'s [`Render`].
struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let masked = input.masked;
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color): (SharedString, _) = if content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 1., 0.28))
        } else {
            (display_string(&content, masked).into(), style.color)
        };

        // IME marked-text underline: mask-aware start/end within the display string.
        let marked_display = input.marked_range.as_ref().map(|r| {
            display_offset(&content, masked, r.start)..display_offset(&content, masked, r.end)
        });

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = marked_display {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        // Cursor / selection offsets are computed against the display string so bullets
        // and real glyphs line up identically.
        let cursor_display = display_offset(&content, masked, cursor);
        let cursor_pos = line.x_for_index(cursor_display);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    rgb(CURSOR),
                )),
            )
        } else {
            let sel_start = display_offset(&content, masked, selected_range.start);
            let sel_end = display_offset(&content, masked, selected_range.end);
            (
                Some(fill(
                    Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(sel_start), bounds.top()),
                        point(bounds.left() + line.x_for_index(sel_end), bounds.bottom()),
                    ),
                    rgba(0x3a7ad930),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(bounds.origin, window.line_height(), window, cx)
            .unwrap();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::select_to_home))
            .on_action(cx.listener(Self::select_to_end))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .bg(rgb(FIELD_BG))
            .border_1()
            .border_color(rgb(FIELD_BORDER))
            .rounded_md()
            .text_color(rgb(FIELD_FG))
            .line_height(px(22.))
            .text_size(px(14.))
            .child(
                div()
                    .h(px(22. + 6. * 2.))
                    .w_full()
                    .px(px(8.))
                    .py(px(6.))
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_roundtrip_ascii() {
        let s = "hello";
        for i in 0..=s.len() {
            let u16 = offset_to_utf16(s, i);
            assert_eq!(offset_from_utf16(s, u16), i, "ascii offset {i}");
        }
    }

    #[test]
    fn utf16_astral_char() {
        // "a😀b": bytes a=1, 😀=4, b=1 → len 6; UTF-16 units a=1, 😀=2, b=1 → 4.
        let s = "a😀b";
        assert_eq!(s.len(), 6);
        // byte 5 (start of 'b') is UTF-16 unit 3.
        assert_eq!(offset_to_utf16(s, 5), 3);
        assert_eq!(offset_from_utf16(s, 3), 5);
        // byte 1 (start of emoji) is UTF-16 unit 1.
        assert_eq!(offset_to_utf16(s, 1), 1);
        assert_eq!(offset_from_utf16(s, 1), 1);
    }

    #[test]
    fn grapheme_boundaries_are_char_safe() {
        let s = "a😀b";
        // from start of 'b' (byte 5) previous boundary is start of emoji (byte 1)
        assert_eq!(previous_grapheme_boundary(s, 5), 1);
        // from start of emoji (byte 1) previous is 0
        assert_eq!(previous_grapheme_boundary(s, 1), 0);
        // from 0 next boundary is byte 1 (the emoji spans 1..5)
        assert_eq!(next_grapheme_boundary(s, 0), 1);
        // from byte 1 next boundary is byte 5 — never lands mid-emoji.
        assert_eq!(next_grapheme_boundary(s, 1), 5);
        // clamps at ends
        assert_eq!(previous_grapheme_boundary(s, 0), 0);
        assert_eq!(next_grapheme_boundary(s, s.len()), s.len());
    }

    #[test]
    fn combining_grapheme_is_one_unit() {
        // "e" + combining acute accent = one grapheme, two chars, 3 bytes.
        let s = "e\u{0301}x";
        assert_eq!(s.len(), 4);
        // from 'x' (byte 3) the previous grapheme boundary is 0 (the é cluster), not 1.
        assert_eq!(previous_grapheme_boundary(s, 3), 0);
        // from 0 the next boundary skips the whole cluster to byte 3.
        assert_eq!(next_grapheme_boundary(s, 0), 3);
    }

    #[test]
    fn word_boundaries_jump_words() {
        let s = "foo bar baz";
        // forward from 0 lands at end of "foo" (byte 3)
        assert_eq!(next_word_boundary(s, 0), 3);
        // forward from 3 (the space) lands at end of "bar" (byte 7)
        assert_eq!(next_word_boundary(s, 3), 7);
        // backward from end lands at start of "baz" (byte 8)
        assert_eq!(previous_word_boundary(s, s.len()), 8);
        // backward from 8 lands at start of "bar" (byte 4)
        assert_eq!(previous_word_boundary(s, 8), 4);
        // clamps
        assert_eq!(previous_word_boundary(s, 0), 0);
        assert_eq!(next_word_boundary(s, s.len()), s.len());
    }

    #[test]
    fn masking_display_is_one_bullet_per_grapheme() {
        // 3 graphemes (é is one cluster) → 3 bullets.
        let s = "e\u{0301}ab"; // é a b = 3 graphemes
        let masked = display_string(s, true);
        assert_eq!(masked.chars().filter(|c| *c == BULLET).count(), 3);
        assert_eq!(masked.chars().count(), 3);
        // unmasked passes through unchanged
        assert_eq!(display_string(s, false), s);
    }

    #[test]
    fn masking_empty_is_empty() {
        assert_eq!(display_string("", true), "");
    }

    #[test]
    fn display_offset_maps_graphemes_to_bullets() {
        let s = "a😀b"; // 3 graphemes: a(1) 😀(4) b(1)
        let bullet_len = BULLET.len_utf8();
        // real offset 0 → display 0
        assert_eq!(display_offset(s, true, 0), 0);
        // real offset 1 (after 'a') → 1 bullet
        assert_eq!(display_offset(s, true, 1), bullet_len);
        // real offset 5 (after emoji) → 2 bullets
        assert_eq!(display_offset(s, true, 5), 2 * bullet_len);
        // real offset 6 (end) → 3 bullets
        assert_eq!(display_offset(s, true, 6), 3 * bullet_len);
        // unmasked is identity
        assert_eq!(display_offset(s, false, 5), 5);
    }
}

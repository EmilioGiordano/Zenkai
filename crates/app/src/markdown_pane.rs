use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::base::{TextView, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, Toggle, ToggleGroup};
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::text::TextViewState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use crate::interface_scale::rem_px;
use crate::markdown_depth;
use crate::markdown_highlight::{LANGUAGE, MarkdownHighlighter};
use crate::markdown_links::image_path;
use crate::text_document::{DiskNotice, ViewMode};

const READING_WIDTH: f32 = 860.0;
const MODES: [ViewMode; 3] = [ViewMode::Source, ViewMode::Split, ViewMode::Preview];
pub const CONTEXT: &str = "MarkdownDocument";

pub enum PaneEvent {
    Edited,
    Moved,
    Mode(ViewMode),
    OpenLink(String),
    Reload,
    KeepMine,
}

// What the pane shows of the document; the list owns it and pushes it here.
#[derive(Clone, Default)]
pub struct PaneState {
    pub folder: Option<PathBuf>,
    pub notice: DiskNotice,
}

pub struct MarkdownPane {
    editor: Entity<EditorState>,
    preview: Entity<TextViewState>,
    mode: ViewMode,
    state: PaneState,
    focus: FocusHandle,
    preview_stale: bool,
    too_deep: bool,
    images: Images,
    preview_task: Option<Task<()>>,
    follow_task: Option<Task<()>>,
    followed: usize,
    announced: usize,
    _subscriptions: Vec<Subscription>,
}

mod preview;

use preview::{Images, blocked_image, extensions, verdicts};

impl EventEmitter<PaneEvent> for MarkdownPane {}

impl MarkdownPane {
    pub fn new(
        text: &str,
        mode: ViewMode,
        state: PaneState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> MarkdownPane {
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language(LANGUAGE)
                .line_number(true)
                .soft_wrap(true)
                .searchable(true)
                .folding(false)
                .auto_close(false)
                .smart_indent(false)
                .default_value(text.to_owned());
            editor.set_highlighter_factory(MarkdownHighlighter::factory(), cx);
            editor
        });
        let too_deep = markdown_depth::too_deep(text);
        let shown = if too_deep { "" } else { text };
        let preview = cx.new(|cx| TextViewState::markdown(shown, cx));
        let changes = cx.subscribe_in(&editor, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.preview_stale = true;
                this.schedule_preview(cx);
                cx.emit(PaneEvent::Edited);
            }
        });
        let moves = cx.observe(&editor, |this, editor, cx| {
            let offset = editor.read(cx).cursor();
            if offset != this.announced {
                this.announced = offset;
                cx.emit(PaneEvent::Moved);
            }
            this.schedule_follow(cx);
        });
        let mut pane = MarkdownPane {
            editor,
            preview,
            mode,
            state,
            focus: cx.focus_handle(),
            preview_stale: false,
            too_deep,
            images: Images::default(),
            preview_task: None,
            follow_task: None,
            followed: 0,
            announced: 0,
            _subscriptions: vec![changes, moves],
        };
        pane.vet_images(text, cx);
        pane
    }

    pub fn text(&self, cx: &App) -> SharedString {
        self.editor.read(cx).value()
    }

    // One-based line and column, for the status bar.
    pub fn caret(&self, cx: &App) -> (usize, usize) {
        let position = self.editor.read(cx).cursor_position();
        (position.line as usize + 1, position.character as usize + 1)
    }

    pub fn show_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == ViewMode::Preview {
            self.set_mode(ViewMode::Split, window, cx);
        }
    }

    pub fn sync(&mut self, state: PaneState, cx: &mut Context<Self>) {
        let moved = self.state.folder != state.folder;
        self.state = state;
        if moved {
            verdicts(&self.images).clear();
            self.refresh_preview(cx);
        }
        cx.notify();
    }

    // The file was read again: the history of the old text goes with it.
    pub fn replace_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.set_value(text.to_owned(), window, cx)
        });
        self.show_in_preview(text, cx);
        cx.notify();
    }

    pub fn set_mode(&mut self, mode: ViewMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        if mode != ViewMode::Source && self.preview_stale {
            self.refresh_preview(cx);
        }
        self.focus(window, cx);
        cx.emit(PaneEvent::Mode(mode));
        cx.notify();
    }

    pub fn cycle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(self.mode.next(), window, cx);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            ViewMode::Preview => window.focus(&self.focus, cx),
            ViewMode::Split | ViewMode::Source => self
                .editor
                .update(cx, |editor, cx| editor.focus(window, cx)),
        }
    }

    // Find and Replace belong to the editor, so they are asked of it directly rather than by a
    // key action that would have to find the editor in a frame that may not have drawn it yet.
    pub fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.show_source(window, cx);
        self.focus(window, cx);
        self.editor
            .update(cx, |editor, cx| editor.open_search(replace, cx));
    }

    fn type_in_preview(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if self.mode != ViewMode::Preview
            || modifiers.control
            || modifiers.alt
            || modifiers.platform
            || modifiers.function
        {
            return;
        }
        let Some(typed) = keystroke
            .key_char
            .clone()
            .filter(|typed| !typed.chars().any(char::is_control))
        else {
            return;
        };
        self.set_mode(ViewMode::Split, window, cx);
        self.editor
            .update(cx, |editor, cx| editor.insert(typed, window, cx));
        // `insert` emits the editor's Change event, which marks the edit.
        cx.stop_propagation();
    }

    fn render_modes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        let current = self.mode;
        ToggleGroup::new("markdown-modes")
            .segmented()
            .small()
            .children(MODES.map(|mode| {
                Toggle::new(match mode {
                    ViewMode::Source => "markdown-mode-source",
                    ViewMode::Split => "markdown-mode-split",
                    ViewMode::Preview => "markdown-mode-preview",
                })
                .label(mode.label())
                .checked(mode == current)
            }))
            .on_click(move |next, window, cx| {
                let chosen = MODES
                    .iter()
                    .zip(next)
                    .find(|(mode, checked)| (**mode == current) != **checked)
                    .map(|(mode, _)| *mode);
                if let Some(mode) = chosen
                    && let Err(error) =
                        entity.update(cx, |pane, cx| pane.set_mode(mode, window, cx))
                {
                    tracing::debug!(%error, "markdown pane dropped");
                }
            })
    }

    fn render_notice(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let theme = cx.theme();
        let (text, offers) = match self.state.notice {
            DiskNotice::None => return None,
            DiskNotice::ChangedWithEdits => (t!("markdown.notice.changed"), true),
            DiskNotice::Deleted => (t!("markdown.notice.deleted"), false),
        };
        Some(
            h_flex()
                .px_3()
                .py_1p5()
                .gap_3()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.warning.opacity(0.12))
                .child(div().flex_none().text_color(theme.warning).child("⚠"))
                .child(div().flex_1().min_w_0().child(text))
                .when(offers, |bar| {
                    bar.child(
                        Button::new("markdown-reload")
                            .small()
                            .label(t!("markdown.notice.reload"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PaneEvent::Reload))),
                    )
                    .child(
                        Button::new("markdown-keep")
                            .small()
                            .label(t!("markdown.notice.keep"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PaneEvent::KeepMine))),
                    )
                }),
        )
    }

    fn render_source(&self) -> impl IntoElement {
        div()
            .id("markdown-source")
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                Editor::new(&self.editor)
                    .h(relative(1.))
                    .bordered(false)
                    .aria_label(t!("markdown.editor_label")),
            )
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let folder = self.state.folder.clone();
        let images = self.images.clone();
        let centered = self.mode == ViewMode::Preview;
        let too_deep = self.too_deep;
        div()
            .id("markdown-preview")
            .flex_1()
            .min_w_0()
            .h_full()
            .when(self.mode == ViewMode::Split, |pane| {
                pane.border_l_1().border_color(cx.theme().border)
            })
            .child(
                div()
                    .size_full()
                    .px_6()
                    .py_4()
                    .when(centered, |page| page.max_w(rem_px(READING_WIDTH)).mx_auto())
                    .when(!too_deep, |page| {
                        page.child(
                        TextView::new(&self.preview)
                            .markdown_extensions(extensions())
                            .selectable(true)
                            .scrollable(true)
                            .image_source(move |uri| {
                                match image_path(&uri.to_string(), folder.as_deref()) {
                                    Some(path) => match verdicts(&images).get(&path) {
                                        Some(Some(real)) => ImageSource::Resource(Resource::Path(
                                            Arc::from(real.as_path()),
                                        )),
                                        _ => blocked_image(),
                                    },
                                    None => blocked_image(),
                                }
                            })
                            .on_link_click(move |url, _, _, cx| {
                                let target = url.to_string();
                                if let Err(error) =
                                    weak.update(cx, |_, cx| cx.emit(PaneEvent::OpenLink(target)))
                                {
                                    tracing::debug!(%error, "markdown pane dropped a link click");
                                }
                            }),
                    )
                    })
                    .when(too_deep, |page| page.child(t!("markdown.too_deep"))),
            )
    }
}

impl Render for MarkdownPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (background, border, ring) = {
            let theme = cx.theme();
            (theme.background, theme.border, theme.ring)
        };
        let ring = if self.mode == ViewMode::Preview && self.focus.is_focused(window) {
            ring
        } else {
            background
        };
        let shows_source = self.mode != ViewMode::Preview;
        let shows_preview = self.mode != ViewMode::Source;
        v_flex()
            .id("markdown-pane")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|this, event, window, cx| this.type_in_preview(event, window, cx)),
            )
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(background)
            .border_2()
            .border_color(ring)
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .justify_end()
                    .border_b_1()
                    .border_color(border)
                    .child(self.render_modes(cx)),
            )
            .children(self.render_notice(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(shows_source, |body| body.child(self.render_source()))
                    .when(shows_preview, |body| body.child(self.render_preview(cx))),
            )
    }
}

pub fn folder_of(path: Option<&Path>) -> Option<PathBuf> {
    path.and_then(Path::parent).map(Path::to_path_buf)
}

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_types::WorkbookId;

use super::super::Workspace;
use super::WIDTH;
use crate::actions::*;
use crate::sidebar_item::{self, FileItem, FileState};
use crate::sidebar_rows::{Move, Row};
use crate::spaces::{Space, SpaceId};

#[derive(Clone)]
struct DraggedWorkbook {
    id: WorkbookId,
    label: SharedString,
}

impl Render for DraggedWorkbook {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .text_sm()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .shadow_md()
            .child(self.label.clone())
    }
}

struct Paint {
    focused: bool,
    active: WorkbookId,
    spaces: Vec<Space>,
    entity: WeakEntity<Workspace>,
}

struct NavRow {
    id: &'static str,
    row: Row,
    icon: IconName,
    label: &'static str,
    keys: &'static str,
}

fn state_mark(state: FileState, cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    match state {
        FileState::Ready => None,
        FileState::Dirty => Some(div().child("•").into_any_element()),
        FileState::NotLoaded => Some(
            div()
                .text_color(theme.muted_foreground)
                .child("○")
                .into_any_element(),
        ),
        FileState::Loading => Some(
            Spinner::new()
                .color(theme.muted_foreground)
                .into_any_element(),
        ),
        FileState::Missing => Some(
            Icon::new(IconName::TriangleAlert)
                .size_3()
                .text_color(theme.danger)
                .into_any_element(),
        ),
    }
}

fn entity_update(
    entity: &WeakEntity<Workspace>,
    cx: &mut App,
    update: impl FnOnce(&mut Workspace, &mut Context<Workspace>),
) {
    if let Err(error) = entity.update(cx, update) {
        tracing::debug!(%error, "workspace dropped");
    }
}

impl Workspace {
    pub(in crate::view) fn render_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.sidebar.visible {
            return None;
        }
        let theme = cx.theme();
        let (background, foreground, muted) = (
            theme.sidebar,
            theme.sidebar_foreground,
            theme.muted_foreground,
        );
        let paint = Paint {
            focused: self.sidebar.focus.contains_focused(window, cx),
            active: self.documents.active_id(),
            spaces: self.documents.spaces().iter().cloned().collect(),
            entity: cx.entity().downgrade(),
        };
        let sections: Vec<_> = paint
            .spaces
            .iter()
            .map(|space| self.render_space(space, &paint, cx))
            .collect();
        let recent: Vec<(String, String)> = self
            .unopened_recent()
            .iter()
            .map(|path| {
                (
                    path.file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    sidebar_item::folder_name(path).unwrap_or_default(),
                )
            })
            .collect();
        let new_workbook = NavRow {
            id: "new-workbook",
            row: Row::NewWorkbook,
            icon: IconName::FilePlus,
            label: "New workbook",
            keys: "Ctrl+N",
        };
        let search_files = NavRow {
            id: "search-files",
            row: Row::SearchFiles,
            icon: IconName::Search,
            label: "Search file",
            keys: "Ctrl+E",
        };
        let new_space = NavRow {
            id: "new-space",
            row: Row::NewSpace,
            icon: IconName::Plus,
            label: "New space",
            keys: "Ctrl+Alt+N",
        };
        Some(
            v_flex()
                .id("sidebar")
                .key_context("Sidebar")
                .track_focus(&self.sidebar.focus)
                .on_action(
                    cx.listener(|this, _: &SidebarUp, _, cx| this.sidebar_step(Move::Up, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SidebarDown, _, cx| this.sidebar_step(Move::Down, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SidebarFirst, _, cx| this.sidebar_step(Move::First, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SidebarLast, _, cx| this.sidebar_step(Move::Last, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SidebarCollapse, _, cx| {
                        this.sidebar_collapse(false, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &SidebarExpand, _, cx| this.sidebar_collapse(true, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &SidebarOpen, window, cx| this.sidebar_open(window, cx)),
                )
                .on_action(cx.listener(|this, _: &SidebarDelete, window, cx| {
                    this.sidebar_delete(window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &LeaveSidebar, window, cx| {
                        this.leave_sidebar(window, cx)
                    }),
                )
                .w(px(WIDTH))
                .flex_shrink_0()
                .h_full()
                .px(px(10.0))
                .pb_3()
                .bg(background)
                .text_color(foreground)
                .text_sm()
                .child(
                    h_flex().h(px(40.0)).items_center().child(
                        Button::new("hide-sidebar")
                            .ghost()
                            .compact()
                            .icon(IconName::PanelLeft)
                            .tooltip("Hide sidebar (Ctrl+Alt+B)")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(ToggleSidebar.boxed_clone(), cx)
                            }),
                    ),
                )
                .child(self.nav_row(new_workbook, &paint, cx))
                .child(self.nav_row(search_files, &paint, cx))
                .child(
                    v_flex()
                        .id("sidebar-spaces")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .mt_3()
                        .gap_3()
                        .children(sections)
                        .child(self.nav_row(new_space, &paint, cx))
                        .when(!recent.is_empty(), |list| {
                            list.child(self.render_recent(&recent, &paint, cx))
                        }),
                )
                .child(
                    h_flex()
                        .h(px(32.0))
                        .items_center()
                        .justify_end()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{} MB", self.memory_mb)),
                ),
        )
    }

    fn row_frame(
        &self,
        id: impl Into<ElementId>,
        row: Row,
        paint: &Paint,
        cx: &App,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let on_cursor = paint.focused && self.sidebar.cursor == Some(row);
        div()
            .id(id)
            .flex()
            .w_full()
            .rounded(px(8.0))
            .cursor_pointer()
            .border_1()
            .border_color(if on_cursor {
                theme.ring
            } else {
                gpui_kit::transparent_black()
            })
            .hover(|style| style.bg(theme.sidebar_accent.opacity(0.6)))
    }

    fn nav_row(&self, nav: NavRow, paint: &Paint, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let row = nav.row;
        self.row_frame(nav.id, row, paint, cx)
            .h(px(32.0))
            .px(px(8.0))
            .items_center()
            .gap(px(10.0))
            .child(Icon::new(nav.icon).small().text_color(muted))
            .child(div().flex_1().child(nav.label))
            .child(div().text_xs().text_color(muted).child(nav.keys))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.sidebar.cursor = Some(row);
                this.sidebar_open(window, cx);
            }))
            .into_any_element()
    }

    fn render_space(&self, space: &Space, paint: &Paint, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().sidebar_accent;
        let id = space.id;
        let files: Vec<FileItem> = self
            .documents
            .members(id)
            .map(|entry| sidebar_item::describe(entry, entry.id() == paint.active))
            .collect();
        let count = files.len();
        let renaming = self
            .sidebar
            .renaming
            .as_ref()
            .filter(|renaming| renaming.space == id);
        let title: AnyElement = match renaming {
            Some(renaming) => div()
                .key_context("SpaceRename")
                .flex_1()
                .child(Input::new(&renaming.input).small())
                .into_any_element(),
            None => div()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(space.name.clone())
                .into_any_element(),
        };
        let header = self
            .row_frame(("space", id.0), Row::Space(id), paint, cx)
            .h(px(28.0))
            .px(px(8.0))
            .items_center()
            .gap(px(6.0))
            .text_color(muted)
            .font_weight(FontWeight::MEDIUM)
            .child(
                Icon::new(if space.collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .small(),
            )
            .child(title)
            .child(div().text_xs().child(count.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sidebar.cursor = Some(Row::Space(id));
                this.documents.toggle_space(id);
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    this.sidebar.cursor = Some(Row::Space(id));
                    window.focus(&this.sidebar.focus, cx);
                }),
            )
            .context_menu({
                let focus = self.sidebar.focus.clone();
                move |menu, _, _| {
                    menu.action_context(focus.clone())
                        .menu("Rename space", Box::new(RenameSpace))
                        .menu("Delete space", Box::new(DeleteSpace))
                }
            });
        let items: Vec<_> = files
            .into_iter()
            .filter(|_| !space.collapsed)
            .map(|file| self.render_file(file, id, paint, cx))
            .collect();
        v_flex()
            .id(("section", id.0))
            .gap_0p5()
            .rounded(px(8.0))
            .drag_over::<DraggedWorkbook>(move |style, _, _, _| style.bg(accent.opacity(0.5)))
            .on_drop(cx.listener(move |this, dragged: &DraggedWorkbook, _, cx| {
                this.move_document(dragged.id, id, cx)
            }))
            .child(header)
            .children(items)
            .into_any_element()
    }

    fn render_file(
        &self,
        file: FileItem,
        space: SpaceId,
        paint: &Paint,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let active_background = cx.theme().sidebar_accent;
        let id = file.id;
        let ghost = DraggedWorkbook {
            id,
            label: file.name.clone().into(),
        };
        let close_label = match file.state {
            FileState::Ready | FileState::Dirty => "Close",
            _ => "Remove from sidebar",
        };
        let entity = paint.entity.clone();
        self.row_frame(("file", id.0), Row::File(id), paint, cx)
            .flex_col()
            .gap(px(3.0))
            .px(px(10.0))
            .py(px(8.0))
            .when(file.active, |frame| frame.bg(active_background))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .when(file.active, |name| name.font_weight(FontWeight::MEDIUM))
                            .child(file.name),
                    )
                    .children(state_mark(file.state, cx)),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_1p5()
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::FolderClosed).size_3())
                    .child(file.place),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.sidebar.cursor = Some(Row::File(id));
                this.switch_to(id, window, cx);
            }))
            .on_drag(ghost, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    this.sidebar.cursor = Some(Row::File(id));
                    window.focus(&this.sidebar.focus, cx);
                }),
            )
            .context_menu(move |menu, _, cx| {
                // Built when the menu opens, not on every draw of the sidebar.
                let others: Vec<(SpaceId, String)> = entity
                    .upgrade()
                    .map(|workspace| {
                        workspace
                            .read(cx)
                            .documents
                            .spaces()
                            .iter()
                            .filter(|other| other.id != space)
                            .map(|other| (other.id, other.name.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let close = entity.clone();
                let menu = menu.item(PopupMenuItem::new(close_label).on_click(
                    move |_, window, cx| {
                        entity_update(&close, cx, |this, cx| this.close_document(id, window, cx))
                    },
                ));
                others.iter().fold(menu, |menu, (target, name)| {
                    let entity = entity.clone();
                    let target = *target;
                    menu.item(PopupMenuItem::new(format!("Move to {name}")).on_click(
                        move |_, _, cx| {
                            entity_update(&entity, cx, |this, cx| {
                                this.move_document(id, target, cx)
                            })
                        },
                    ))
                })
            })
            .into_any_element()
    }

    fn render_recent(
        &self,
        recent: &[(String, String)],
        paint: &Paint,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let open = self.sidebar.recent_open;
        let header = self
            .row_frame("recent-header", Row::RecentHeader, paint, cx)
            .h(px(28.0))
            .px(px(8.0))
            .items_center()
            .gap(px(6.0))
            .text_color(muted)
            .font_weight(FontWeight::MEDIUM)
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .small(),
            )
            .child(div().flex_1().child("Recent"))
            .child(div().text_xs().child(recent.len().to_string()))
            .on_click(cx.listener(|this, _, _, cx| {
                this.sidebar.cursor = Some(Row::RecentHeader);
                this.sidebar.recent_open = !this.sidebar.recent_open;
                cx.notify();
            }));
        let items: Vec<_> = recent
            .iter()
            .enumerate()
            .filter(|_| open)
            .map(|(index, (name, folder))| {
                self.row_frame(("recent", index as u64), Row::Recent(index), paint, cx)
                    .flex_col()
                    .gap(px(3.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(name.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(muted)
                            .child(Icon::new(IconName::FolderClosed).size_3())
                            .child(folder.clone()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.sidebar.cursor = Some(Row::Recent(index));
                        this.sidebar_open(window, cx);
                    }))
            })
            .collect();
        v_flex()
            .gap_0p5()
            .child(header)
            .children(items)
            .into_any_element()
    }
}

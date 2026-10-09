mod draft;
mod generation;
mod layout;
mod options;
mod render;

use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use zenkai_datagen::{DatagenError, Locale};
use zenkai_types::{Range, SheetId, WorkbookId};

pub use draft::{Block, parse_range};
pub use layout::Layout;

use draft::{Draft, NameRole, Placement, Source, WriteIssue};
use options::KindChoice;

const DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub document: WorkbookId,
    pub generation: u64,
    pub sheet: SheetId,
}

pub struct Opening {
    pub layout: Layout,
    pub seed: u64,
    pub today: zenkai_datagen::Date,
    pub target: Target,
    pub sheet_name: SharedString,
}

pub struct Write {
    pub target: Target,
    pub block: Block,
    pub notice: String,
}

pub enum DialogEvent {
    Write,
    Relayout(Range),
    Close,
}

impl EventEmitter<DialogEvent> for GenerateDialog {}

struct Ready {
    request: u64,
    result: Result<Vec<Vec<String>>, DatagenError>,
}

struct ColumnInputs {
    header: Entity<InputState>,
    blanks: Entity<InputState>,
}

struct OptionsPanel {
    column: usize,
    inputs: Vec<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
}

enum Panel {
    Type(usize),
    Options(OptionsPanel),
}

pub struct GenerateDialog {
    draft: Draft,
    target: Target,
    sheet_name: SharedString,
    range: Entity<InputState>,
    rows: Entity<InputState>,
    seed: Entity<InputState>,
    columns: Vec<ColumnInputs>,
    panel: Option<Panel>,
    ready: Option<Ready>,
    working: bool,
    request: u64,
    submit_when_ready: bool,
    problem: Option<WriteIssue>,
    outgoing: Option<Write>,
    focus: FocusHandle,
    _fields: Vec<Subscription>,
    _column_fields: Vec<Subscription>,
}

impl Focusable for GenerateDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn text_input(
    value: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut Context<GenerateDialog>,
) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).default_value(value))
}

impl GenerateDialog {
    pub fn new(opening: Opening, window: &mut Window, cx: &mut Context<Self>) -> GenerateDialog {
        let draft = Draft::new(
            opening.layout,
            Locale::SpanishArgentina,
            opening.seed,
            opening.today,
        );
        let range = text_input(draft.range_text(), window, cx);
        let rows = text_input(draft.rows().to_string(), window, cx);
        let seed = text_input(draft.seed().to_string(), window, cx);
        let fields = vec![
            cx.subscribe_in(&range, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    this.range_edited(&text, cx);
                }
            }),
            cx.subscribe_in(&rows, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    this.draft.set_rows_text(&text);
                    this.changed(cx);
                }
            }),
            cx.subscribe_in(&seed, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value().to_string();
                    this.draft.set_seed_text(&text);
                    this.changed(cx);
                }
            }),
        ];
        let mut dialog = GenerateDialog {
            draft,
            target: opening.target,
            sheet_name: opening.sheet_name,
            range,
            rows,
            seed,
            columns: Vec::new(),
            panel: None,
            ready: None,
            working: false,
            request: 0,
            submit_when_ready: false,
            problem: None,
            outgoing: None,
            focus: cx.focus_handle(),
            _fields: fields,
            _column_fields: Vec::new(),
        };
        dialog.rebuild_columns(window, cx);
        dialog.schedule(cx);
        dialog
    }

    pub fn target(&self) -> Target {
        self.target
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(first) = self.columns.first() {
            let handle = first.header.focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    pub fn apply_layout(&mut self, layout: Layout, window: &mut Window, cx: &mut Context<Self>) {
        self.draft.relayout(layout);
        self.rebuild_columns(window, cx);
        self.sync_rows(window, cx);
        self.changed(cx);
    }

    fn sync_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.draft.rows().to_string();
        self.rows
            .update(cx, |input, cx| input.set_value(rows, window, cx));
    }

    fn rebuild_columns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = None;
        self.columns.clear();
        self._column_fields.clear();
        for (index, column) in self.draft.columns().iter().enumerate() {
            let header = text_input(column.header.clone(), window, cx);
            let blanks = text_input(format!("{}%", column.blanks.get()), window, cx);
            self._column_fields.push(cx.subscribe_in(
                &header,
                window,
                move |this, input, event, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        let text = input.read(cx).value().to_string();
                        this.header_edited(index, &text, cx);
                    }
                },
            ));
            self._column_fields.push(cx.subscribe_in(
                &blanks,
                window,
                move |this, input, event, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        let text = input.read(cx).value().to_string();
                        this.draft.set_blanks(index, &text);
                        this.changed(cx);
                    }
                },
            ));
            self.columns.push(ColumnInputs { header, blanks });
        }
    }

    fn range_edited(&mut self, text: &str, cx: &mut Context<Self>) {
        match parse_range(text) {
            Ok(range) if range == self.draft.layout().range => self.draft.accept_range(),
            Ok(range) => cx.emit(DialogEvent::Relayout(range)),
            Err(_) => self.draft.reject_range(text),
        }
        cx.notify();
    }

    fn header_edited(&mut self, index: usize, text: &str, cx: &mut Context<Self>) {
        let before = self.draft.columns()[index].kind.clone();
        self.draft.rename(index, text);
        if self.draft.columns()[index].kind != before {
            self.panel = None;
        }
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        self.schedule(cx);
        cx.notify();
    }

    pub(crate) fn add_column(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft.add_column();
        self.rebuild_columns(window, cx);
        self.changed(cx);
        if let Some(last) = self.columns.last() {
            let handle = last.header.focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    // Escape closes the open dropdown or popover first, then the dialog.
    pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.panel.take().is_some() {
            cx.notify();
        } else {
            cx.emit(DialogEvent::Close);
        }
    }

    fn toggle_type_menu(&mut self, column: usize, cx: &mut Context<Self>) {
        let open = matches!(self.panel, Some(Panel::Type(open)) if open == column);
        self.panel = (!open).then_some(Panel::Type(column));
        cx.notify();
    }

    fn toggle_options(&mut self, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(&self.panel, Some(Panel::Options(open)) if open.column == column) {
            self.panel = None;
            cx.notify();
            return;
        }
        let values = options::fields(&self.draft.columns()[column].kind);
        let inputs: Vec<Entity<InputState>> = values
            .iter()
            .map(|field| text_input(field.value.clone(), window, cx))
            .collect();
        let subscriptions = inputs
            .iter()
            .map(|input| {
                cx.subscribe_in(input, window, move |this, _, event, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.option_fields_edited(column, cx);
                    }
                })
            })
            .collect();
        if let Some(first) = inputs.first() {
            let handle = first.focus_handle(cx);
            window.focus(&handle, cx);
        }
        self.panel = Some(Panel::Options(OptionsPanel {
            column,
            inputs,
            _subscriptions: subscriptions,
        }));
        cx.notify();
    }

    fn option_fields_edited(&mut self, column: usize, cx: &mut Context<Self>) {
        let Some(Panel::Options(panel)) = &self.panel else {
            return;
        };
        let values: Vec<String> = panel
            .inputs
            .iter()
            .map(|input| input.read(cx).value().to_string())
            .collect();
        self.draft.set_option_fields(column, &values);
        self.changed(cx);
    }

    fn choose_kind(&mut self, column: usize, choice: KindChoice, cx: &mut Context<Self>) {
        self.draft.set_kind_choice(column, choice);
        self.panel = None;
        self.changed(cx);
    }

    fn set_kind(
        &mut self,
        column: usize,
        kind: zenkai_datagen::ColumnKind,
        cx: &mut Context<Self>,
    ) {
        self.draft.set_kind(column, kind);
        self.changed(cx);
    }

    fn set_source(
        &mut self,
        column: usize,
        role: NameRole,
        source: Source,
        cx: &mut Context<Self>,
    ) {
        self.draft.set_source(column, role, source);
        self.changed(cx);
    }

    fn discard_header(&mut self, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.draft.discard(column);
        let original = self.draft.columns()[column].header.clone();
        self.columns[column]
            .header
            .update(cx, |input, cx| input.set_value(original, window, cx));
        self.changed(cx);
    }

    fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.draft.set_locale(locale);
        self.panel = None;
        self.changed(cx);
    }

    fn set_placement(&mut self, placement: Placement, window: &mut Window, cx: &mut Context<Self>) {
        self.draft.set_placement(placement);
        self.sync_rows(window, cx);
        self.changed(cx);
    }

    fn toggle_unique(&mut self, column: usize, cx: &mut Context<Self>) {
        let unique = !self.draft.columns()[column].unique;
        self.draft.set_unique(column, unique);
        self.changed(cx);
    }

    fn remove_column(&mut self, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.draft.remove_added_column(column);
        self.rebuild_columns(window, cx);
        self.changed(cx);
    }
}

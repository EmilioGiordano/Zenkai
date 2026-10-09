use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::bridge::{Bridge, ENDPOINT_FILE};
use zenkai_agent::protected_view::FileOrigin;
use zenkai_agent::settings::{ExternalAgents, PermissionMode, Settings};
use zenkai_agent::tools::{
    self, PlannedWrite, ReadRequest, ToolCall, ToolEndpoint, ToolError, ToolReply, ToolRequest,
    WorkbookId, WriteRequest,
};

use super::{Severity, Workspace};
use crate::actions::{AllowAgentChange, DenyAgentChange, ShowAgentChange};
use crate::agent_routing;
use crate::agent_settings::{AgentConfig, BridgeStatus};
use crate::document::{self, Document};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const APPROVAL_CHECK: Duration = Duration::from_secs(1);
const READ_WAIT_STEP: Duration = Duration::from_millis(20);
const READ_WAIT_STEPS: u32 = 1_500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Allow,
    Deny,
}

pub(super) struct PendingWrite {
    call: ToolCall,
    plan: PlannedWrite,
    serial: u64,
    asked_at: Instant,
    id: WorkbookId,
    generation: u64,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
}

pub(super) enum BridgeState {
    Off,
    Starting,
    Running(Bridge),
    // Not retried until the settings change, or a failing start would loop forever.
    Failed(Settings),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BridgeStep {
    Start,
    Stop,
    Forget,
    Stay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SinceFailure {
    Unchanged,
    Changed,
}

fn bridge_step(state: &BridgeState, wanted: ExternalAgents, since: SinceFailure) -> BridgeStep {
    match (state, wanted) {
        (BridgeState::Off, ExternalAgents::Allowed) => BridgeStep::Start,
        (BridgeState::Running(_), ExternalAgents::Blocked) => BridgeStep::Stop,
        (BridgeState::Failed(_), _) if since == SinceFailure::Changed => match wanted {
            ExternalAgents::Allowed => BridgeStep::Start,
            ExternalAgents::Blocked => BridgeStep::Forget,
        },
        _ => BridgeStep::Stay,
    }
}

pub(super) struct AgentLink {
    endpoint: ToolEndpoint,
    bridge: BridgeState,
    pending: Option<PendingWrite>,
    next_serial: u64,
}

fn endpoint_file() -> Option<PathBuf> {
    Some(crate::recovery::directory()?.parent()?.join(ENDPOINT_FILE))
}

impl Workspace {
    // Tool calls from every agent arrive here, one at a time, on the UI thread.
    pub(super) fn start_tool_service(window: &mut Window, cx: &mut Context<Self>) -> AgentLink {
        let (endpoint, calls) = tools::channel();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(call) = calls.recv().await {
                let handled = this.update_in(cx, |this, window, cx| {
                    this.handle_tool_call(call, window, cx)
                });
                if handled.is_err() {
                    break;
                }
            }
        })
        .detach();
        AgentLink {
            endpoint,
            bridge: BridgeState::Off,
            pending: None,
            next_serial: 0,
        }
    }

    fn permission(cx: &App) -> PermissionMode {
        cx.global::<AgentConfig>().state.current.agents.permission
    }

    fn handle_tool_call(&mut self, call: ToolCall, window: &mut Window, cx: &mut Context<Self>) {
        match call.request.clone() {
            ToolRequest::ListWorkbooks => {
                let listed = agent_routing::summaries(&self.documents, Self::permission(cx));
                call.respond(Ok(ToolReply::Workbooks(listed)));
            }
            ToolRequest::GetSelection(id) => {
                let reply = self.selection_for_agent(id, cx);
                call.respond(reply);
            }
            ToolRequest::Read(id, request) => match agent_routing::target(&self.documents, id) {
                Ok(document) => {
                    let generation = document.generation();
                    self.read_for_agent(call, id, generation, request, cx)
                }
                Err(error) => call.respond(Err(error)),
            },
            ToolRequest::Write(id, request) => match self.plan_agent_write(id, &request, cx) {
                Ok(plan) => self.route_agent_write(call, id, plan, window, cx),
                Err(error) => call.respond(Err(error)),
            },
        }
    }

    // The workbook on screen has its live selection in the grid; the others keep the one
    // they had when they were put away.
    fn selection_for_agent(&self, id: WorkbookId, cx: &App) -> Result<ToolReply, ToolError> {
        let document = agent_routing::target(&self.documents, id)?;
        let sheet = document
            .sheets
            .iter()
            .find(|info| info.id == document.sheet)
            .map(|info| info.name.clone())
            .ok_or_else(|| ToolError::UnknownSheet(format!("#{}", document.sheet.0 + 1)))?;
        let range = if id == self.documents.active_id() {
            self.selection(cx)
        } else {
            document.view.selection.range()
        };
        Ok(ToolReply::Selection { sheet, range })
    }

    fn read_for_agent(
        &mut self,
        call: ToolCall,
        id: WorkbookId,
        generation: u64,
        request: ReadRequest,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            // Reads wait for queued edits, so an agent reading after its own write sees it.
            let mut shared = None;
            for _ in 0..READ_WAIT_STEPS {
                let state = this.update(cx, |this, _| {
                    this.documents
                        .get(id)
                        .filter(|document| document.is_current(generation))
                        .map(|document| document.begin_read())
                });
                match state {
                    Ok(Some(Some(workbook))) => {
                        shared = Some(workbook);
                        break;
                    }
                    Ok(Some(None)) => cx.background_executor().timer(READ_WAIT_STEP).await,
                    Ok(None) => {
                        call.respond(Err(ToolError::UnknownWorkbook(id)));
                        return;
                    }
                    Err(_) => {
                        call.respond(Err(ToolError::Closed));
                        return;
                    }
                }
            }
            let Some(shared) = shared else {
                call.respond(Err(ToolError::Busy));
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { tools::read(&request, &document::read_shared(&shared)) })
                .await;
            let saw_hidden = match &result {
                Ok(ToolReply::Cells(page)) => !page.hidden.is_empty(),
                Ok(ToolReply::Found(found)) => !found.hidden.is_empty(),
                _ => false,
            };
            call.respond(result);
            if saw_hidden {
                let notified = this.update(cx, |this, cx| {
                    this.notify(
                        Severity::Warning,
                        "An agent read content you cannot see (hidden sheets, rows or columns, or very long text).",
                        cx,
                    )
                });
                if notified.is_err() {
                    tracing::debug!("workspace closed after an agent read");
                }
            }
        })
        .detach();
    }

    fn plan_agent_write(
        &self,
        id: WorkbookId,
        request: &WriteRequest,
        cx: &App,
    ) -> Result<PlannedWrite, ToolError> {
        let document = agent_routing::target(&self.documents, id)?;
        agent_routing::access(document, Self::permission(cx)).check_write()?;
        // An agent write landing under the user's typing would mix the two edits.
        if id == self.documents.active_id() && self.user_is_editing(cx) {
            return Err(ToolError::UserEditing);
        }
        tools::plan_write(request, &document.sheets)
    }

    fn route_agent_write(
        &mut self,
        call: ToolCall,
        id: WorkbookId,
        plan: PlannedWrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if Self::permission(cx) == PermissionMode::Automatic {
            self.apply_agent_write(call, id, plan, window, cx);
            return;
        }
        if self.agent.pending.is_some() {
            call.respond(Err(ToolError::AwaitingApproval));
            return;
        }
        let Some(generation) = self.documents.get(id).map(Document::generation) else {
            call.respond(Err(ToolError::UnknownWorkbook(id)));
            return;
        };
        let focus = cx.focus_handle();
        let return_focus = self.take_focus_for_bar(&focus, window, cx);
        let serial = self.agent.next_serial;
        self.agent.next_serial += 1;
        self.agent.pending = Some(PendingWrite {
            call,
            plan,
            serial,
            asked_at: Instant::now(),
            id,
            generation,
            focus,
            return_focus,
        });
        self.sync_pending_highlight(cx);
        self.watch_pending_approval(serial, window, cx);
        cx.notify();
    }

    // A client that disconnected or gave up, or a user who never answers, must not leave
    // the approval slot taken: every later write would be refused with nobody to clear it.
    fn watch_pending_approval(&mut self, serial: u64, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(APPROVAL_CHECK).await;
                let finished = this.update_in(cx, |this, window, cx| {
                    let Some(pending) = this.agent.pending.as_ref() else {
                        return true;
                    };
                    if pending.serial != serial {
                        return true;
                    }
                    let timed_out = pending.asked_at.elapsed() >= APPROVAL_TIMEOUT;
                    if !timed_out && !pending.call.is_abandoned() {
                        return false;
                    }
                    if let Some(pending) = this.agent.pending.take() {
                        if timed_out {
                            pending.call.respond(Err(ToolError::ApprovalTimedOut));
                        }
                        this.release_focus_from_bar(
                            &pending.focus,
                            pending.return_focus.clone(),
                            window,
                            cx,
                        );
                        this.sync_pending_highlight(cx);
                        cx.notify();
                    }
                    true
                });
                if !matches!(finished, Ok(false)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_agent_write(
        &mut self,
        call: ToolCall,
        id: WorkbookId,
        plan: PlannedWrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self.documents.get(id).map(Document::name) else {
            call.respond(Err(ToolError::UnknownWorkbook(id)));
            return;
        };
        let summary = plan.summary();
        let description = plan.describe();
        let (done, finished) = async_channel::bounded::<Result<(), String>>(1);
        self.edit_document(id, window, cx, move |workbook| {
            let applied = plan.apply(workbook);
            let outcome = applied.as_ref().map(|_| ()).map_err(ToString::to_string);
            if done.try_send(outcome).is_err() {
                tracing::debug!("nobody waits for this agent write any more");
            }
            applied
        });
        self.notify(
            Severity::Info,
            format!("Agent in {name}: {description}"),
            cx,
        );
        cx.spawn(async move |_, _| {
            let reply = match finished.recv().await {
                Ok(Ok(())) => Ok(ToolReply::Written(summary)),
                Ok(Err(message)) => Err(ToolError::Engine(message)),
                Err(_) => Err(ToolError::Closed),
            };
            call.respond(reply);
        })
        .detach();
    }

    pub(super) fn decide_agent_change(
        &mut self,
        decision: Decision,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.agent.pending.take() else {
            return;
        };
        self.release_focus_from_bar(&pending.focus, pending.return_focus.clone(), window, cx);
        self.sync_pending_highlight(cx);
        cx.notify();
        if decision == Decision::Deny {
            pending.call.respond(Err(ToolError::Declined));
        } else if !self.holds_document(pending.id, pending.generation) {
            pending
                .call
                .respond(Err(ToolError::UnknownWorkbook(pending.id)));
        } else {
            self.apply_agent_write(pending.call, pending.id, pending.plan, window, cx);
        }
    }

    pub(super) fn render_agent_approval(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let pending = self.agent.pending.as_ref()?;
        let theme = cx.theme();
        let who = pending
            .call
            .client
            .clone()
            .unwrap_or_else(|| "An external agent".to_string());
        let workbook = self
            .documents
            .get(pending.id)
            .map_or_else(|| "a closed workbook".to_string(), Document::name);
        let on_screen = self.documents.active_id() == pending.id
            && self.documents.active().sheet == pending.plan.sheet();
        Some(
            v_flex()
                .key_context("AgentApproval")
                .track_focus(&pending.focus)
                .px_3()
                .py_2()
                .gap_1()
                .bg(theme.secondary)
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("{who} wants to change {workbook}")),
                        )
                        .when(!on_screen, |row| {
                            row.child(Button::new("agent-show").label("Show (Alt+W)").on_click(
                                |_, window, cx| {
                                    window.dispatch_action(Box::new(ShowAgentChange), cx)
                                },
                            ))
                        })
                        .child(
                            Button::new("agent-deny")
                                .primary()
                                .label("Deny (Enter)")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(DenyAgentChange), cx)
                                }),
                        )
                        .child(Button::new("agent-allow").label("Allow (Alt+Y)").on_click(
                            |_, window, cx| window.dispatch_action(Box::new(AllowAgentChange), cx),
                        )),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{}. Not saved; Ctrl+Z undoes it.",
                            pending.plan.headline()
                        )),
                )
                .children(pending.plan.sample().map(|sample| {
                    div()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(sample)
                })),
        )
    }

    // The amber outline follows the pending change only while its workbook and sheet are on screen.
    pub(super) fn sync_pending_highlight(&mut self, cx: &mut Context<Self>) {
        let range = self
            .agent
            .pending
            .as_ref()
            .filter(|pending| {
                pending.id == self.documents.active_id()
                    && pending.plan.sheet() == self.documents.active().sheet
            })
            .map(|pending| pending.plan.target());
        self.grid.update(cx, |grid, cx| grid.set_pending(range, cx));
    }

    pub(super) fn show_agent_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, sheet, target)) = self
            .agent
            .pending
            .as_ref()
            .map(|pending| (pending.id, pending.plan.sheet(), pending.plan.target()))
        else {
            return;
        };
        self.switch_to(id, window, cx);
        if self.documents.active_id() != id {
            return;
        }
        self.switch_sheet(sheet, window, cx);
        self.grid.update(cx, |grid, cx| {
            grid.show_range(target, cx);
        });
        self.sync_pending_highlight(cx);
    }

    pub(super) fn user_is_editing(&self, cx: &App) -> bool {
        self.formula_bar.is_some() || self.grid.read(cx).editor().is_some()
    }

    fn typing_in_a_field(&self, cx: &App) -> bool {
        self.user_is_editing(cx)
            || self.rename.is_some()
            || self.go_to.is_some()
            || self.palette.is_some()
            || self.search.is_some()
            || self.sidebar.renaming.is_some()
            || self.documents.active().find.is_some()
    }

    // A bar that asks the user something takes the keyboard only when the user is not
    // typing, so a keystroke meant for a field never answers it; the focus it took goes
    // back where it was.
    pub(super) fn take_focus_for_bar(
        &self,
        bar: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<FocusHandle> {
        if self.typing_in_a_field(cx) {
            return None;
        }
        let previous = window.focused(cx);
        window.focus(bar, cx);
        previous
    }

    pub(super) fn release_focus_from_bar(
        &self,
        bar: &FocusHandle,
        previous: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !bar.is_focused(window) {
            return;
        }
        let target = previous.unwrap_or_else(|| self.grid.focus_handle(cx));
        window.focus(&target, cx);
    }

    fn holds_document(&self, id: WorkbookId, generation: u64) -> bool {
        self.documents
            .get(id)
            .is_some_and(|document| document.is_current(generation))
    }

    // A change approved after its document closed, was replaced or unloaded would land in
    // another workbook or none. Called after every change to the list of documents.
    pub(super) fn refuse_orphaned_agent_change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let orphaned = self
            .agent
            .pending
            .as_ref()
            .is_some_and(|pending| !self.holds_document(pending.id, pending.generation));
        if !orphaned {
            return;
        }
        if let Some(pending) = self.agent.pending.take() {
            self.release_focus_from_bar(&pending.focus, pending.return_focus.clone(), window, cx);
            pending
                .call
                .respond(Err(ToolError::UnknownWorkbook(pending.id)));
            self.sync_pending_highlight(cx);
            cx.notify();
        }
    }

    pub(super) fn awaiting_approval(&self) -> Option<WorkbookId> {
        self.agent.pending.as_ref().map(|pending| pending.id)
    }

    // Dropping the bridge removes the endpoint file, so no client finds a dead pipe.
    pub(super) fn stop_bridge(&mut self) {
        self.agent.bridge = BridgeState::Off;
    }

    // As Excel's Enable Editing: the user vouches for this file, for this session only.
    pub(super) fn let_agents_edit(&mut self, cx: &mut Context<Self>) {
        let document = self.documents.active_mut();
        if document.origin == FileOrigin::Internet {
            document.origin = FileOrigin::Local;
            self.notify(
                Severity::Info,
                "Agents may now edit this file (until it is closed).",
                cx,
            );
        }
    }

    pub(super) fn sync_bridge(&mut self, cx: &mut Context<Self>) {
        let current = cx.global::<AgentConfig>().state.current.clone();
        let since = match &self.agent.bridge {
            BridgeState::Failed(at) if *at == current => SinceFailure::Unchanged,
            _ => SinceFailure::Changed,
        };
        match bridge_step(&self.agent.bridge, current.agents.external_agents, since) {
            BridgeStep::Start => self.start_bridge(cx),
            BridgeStep::Stop => {
                if let BridgeState::Running(bridge) =
                    std::mem::replace(&mut self.agent.bridge, BridgeState::Off)
                {
                    cx.background_executor()
                        .spawn(async move { drop(bridge) })
                        .detach();
                }
                set_bridge_status(BridgeStatus::Off, cx);
            }
            BridgeStep::Forget => {
                self.agent.bridge = BridgeState::Off;
                set_bridge_status(BridgeStatus::Off, cx);
            }
            BridgeStep::Stay => {}
        }
    }

    fn start_bridge(&mut self, cx: &mut Context<Self>) {
        self.agent.bridge = BridgeState::Starting;
        let endpoint = self.agent.endpoint.clone();
        cx.spawn(async move |this, cx| {
            let started = cx
                .background_executor()
                .spawn(async move { Bridge::start(endpoint, endpoint_file().as_deref()) })
                .await;
            let update = this.update(cx, |this, cx| {
                match started {
                    Ok(bridge) => {
                        this.agent.bridge = BridgeState::Running(bridge);
                        set_bridge_status(BridgeStatus::Listening, cx);
                    }
                    Err(error) => {
                        let settings = cx.global::<AgentConfig>().state.current.clone();
                        this.agent.bridge = BridgeState::Failed(settings);
                        tracing::warn!(%error, "could not start the MCP bridge");
                        set_bridge_status(BridgeStatus::Failed(error.to_string()), cx);
                    }
                }
                this.sync_bridge(cx);
            });
            if update.is_err() {
                tracing::debug!("workspace closed while the MCP bridge started");
            }
        })
        .detach();
    }
}

fn set_bridge_status(status: BridgeStatus, cx: &mut App) {
    cx.update_global::<AgentConfig, _>(|config, _| config.bridge = status);
}

#[cfg(test)]
mod tests {
    use super::{BridgeState, BridgeStep, ExternalAgents, Settings, SinceFailure, bridge_step};

    #[test]
    fn a_failed_bridge_waits_for_a_settings_change_before_starting_again() {
        let failed = BridgeState::Failed(Settings::default());
        let allowed = ExternalAgents::Allowed;
        assert_eq!(
            bridge_step(&failed, allowed, SinceFailure::Unchanged),
            BridgeStep::Stay
        );
        assert_eq!(
            bridge_step(&failed, allowed, SinceFailure::Changed),
            BridgeStep::Start
        );
        assert_eq!(
            bridge_step(&failed, ExternalAgents::Blocked, SinceFailure::Changed),
            BridgeStep::Forget
        );
    }

    #[test]
    fn the_bridge_follows_the_external_agents_setting() {
        let since = SinceFailure::Changed;
        assert_eq!(
            bridge_step(&BridgeState::Off, ExternalAgents::Allowed, since),
            BridgeStep::Start
        );
        assert_eq!(
            bridge_step(&BridgeState::Off, ExternalAgents::Blocked, since),
            BridgeStep::Stay
        );
        assert_eq!(
            bridge_step(&BridgeState::Starting, ExternalAgents::Allowed, since),
            BridgeStep::Stay
        );
    }
}

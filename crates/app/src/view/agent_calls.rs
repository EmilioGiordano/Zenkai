use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;
use zenkai_agent::bridge::{Bridge, ENDPOINT_FILE};
use zenkai_agent::protected_view::FileOrigin;
use zenkai_agent::settings::{ExternalAgents, PermissionMode, Settings};
use zenkai_agent::tools::{
    self, AgentAccess, PlannedWrite, ReadOnlyReason, ReadRequest, ToolCall, ToolEndpoint,
    ToolError, ToolReply, ToolRequest, WorkbookId, WriteRequest,
};

use super::{Severity, Workspace};
use crate::actions::{AllowAgentChange, DenyAgentChange};
use crate::agent_settings::{AgentConfig, BridgeStatus};
use crate::document;

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
        }
    }

    fn workbook_id(&self) -> WorkbookId {
        WorkbookId(self.document.generation())
    }

    fn agent_access(&self, cx: &App) -> AgentAccess {
        if self.document.read_only {
            AgentAccess::ReadOnly(ReadOnlyReason::ValuesOnlyFile)
        } else if self.document.origin == FileOrigin::Internet {
            AgentAccess::ReadOnly(ReadOnlyReason::ProtectedView)
        } else if cx.global::<AgentConfig>().state.current.agents.permission
            == PermissionMode::ReadOnly
        {
            AgentAccess::ReadOnly(ReadOnlyReason::Settings)
        } else {
            AgentAccess::Editable
        }
    }

    fn handle_tool_call(&mut self, call: ToolCall, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.workbook_id();
        match call.request.clone() {
            ToolRequest::ListWorkbooks => {
                let name = self.document.name();
                let summary = tools::workbook_summary(
                    open,
                    &name,
                    &self.document.sheets,
                    self.agent_access(cx),
                );
                call.respond(Ok(ToolReply::Workbooks(vec![summary])));
            }
            ToolRequest::GetSelection(id) => {
                let reply = tools::check_workbook(open, id).and_then(|()| {
                    let sheet = self
                        .document
                        .sheets
                        .iter()
                        .find(|info| info.id == self.document.sheet)
                        .map(|info| info.name.clone())
                        .ok_or_else(|| {
                            ToolError::UnknownSheet(format!("#{}", self.document.sheet.0 + 1))
                        })?;
                    Ok(ToolReply::Selection {
                        sheet,
                        range: self.selection(cx),
                    })
                });
                call.respond(reply);
            }
            ToolRequest::Read(id, request) => match tools::check_workbook(open, id) {
                Ok(()) => self.read_for_agent(call, request, cx),
                Err(error) => call.respond(Err(error)),
            },
            ToolRequest::Write(id, request) => {
                match self.plan_agent_write(open, id, &request, cx) {
                    Ok(plan) => self.route_agent_write(call, plan, window, cx),
                    Err(error) => call.respond(Err(error)),
                }
            }
        }
    }

    fn read_for_agent(&mut self, call: ToolCall, request: ReadRequest, cx: &mut Context<Self>) {
        let generation = self.document.generation();
        cx.spawn(async move |this, cx| {
            // Reads wait for queued edits, so an agent reading after its own write sees it.
            let mut shared = None;
            for _ in 0..READ_WAIT_STEPS {
                let state = this.update(cx, |this, _| {
                    this.document
                        .is_current(generation)
                        .then(|| this.document.begin_read())
                });
                match state {
                    Ok(Some(Some(workbook))) => {
                        shared = Some(workbook);
                        break;
                    }
                    Ok(Some(None)) => cx.background_executor().timer(READ_WAIT_STEP).await,
                    Ok(None) => {
                        call.respond(Err(ToolError::UnknownWorkbook(WorkbookId(generation))));
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
        open: WorkbookId,
        id: WorkbookId,
        request: &WriteRequest,
        cx: &App,
    ) -> Result<PlannedWrite, ToolError> {
        tools::check_workbook(open, id)?;
        self.agent_access(cx).check_write()?;
        // An agent write landing under the user's typing would mix the two edits.
        if self.user_is_editing(cx) {
            return Err(ToolError::UserEditing);
        }
        tools::plan_write(request, &self.document.sheets)
    }

    fn route_agent_write(
        &mut self,
        call: ToolCall,
        plan: PlannedWrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let permission = cx.global::<AgentConfig>().state.current.agents.permission;
        if permission == PermissionMode::Automatic {
            self.apply_agent_write(call, plan, window, cx);
            return;
        }
        if self.agent.pending.is_some() {
            call.respond(Err(ToolError::AwaitingApproval));
            return;
        }
        let focus = cx.focus_handle();
        let return_focus = self.take_focus_for_bar(&focus, window, cx);
        self.agent.pending = Some(PendingWrite {
            call,
            plan,
            generation: self.document.generation(),
            focus,
            return_focus,
        });
        cx.notify();
    }

    fn apply_agent_write(
        &mut self,
        call: ToolCall,
        plan: PlannedWrite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let summary = plan.summary();
        let description = plan.describe();
        let (done, finished) = async_channel::bounded::<Result<(), String>>(1);
        self.edit(window, cx, move |workbook| {
            let applied = plan.apply(workbook);
            let outcome = applied.as_ref().map(|_| ()).map_err(ToString::to_string);
            if done.try_send(outcome).is_err() {
                tracing::debug!("nobody waits for this agent write any more");
            }
            applied
        });
        self.notify(Severity::Info, format!("Agent: {description}"), cx);
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
        cx.notify();
        if decision == Decision::Deny {
            pending.call.respond(Err(ToolError::Declined));
        } else if !self.document.is_current(pending.generation) {
            pending
                .call
                .respond(Err(ToolError::UnknownWorkbook(WorkbookId(
                    pending.generation,
                ))));
        } else {
            self.apply_agent_write(pending.call, pending.plan, window, cx);
        }
    }

    pub(super) fn render_agent_approval(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let pending = self.agent.pending.as_ref()?;
        let theme = cx.theme();
        Some(
            h_flex()
                .key_context("AgentApproval")
                .track_focus(&pending.focus)
                .px_2()
                .py_1()
                .gap_3()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.secondary)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("An agent wants to:"),
                )
                .child(div().flex_1().min_w_0().child(pending.plan.describe()))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Not saved; Ctrl+Z undoes it."),
                )
                .child(Button::new("agent-allow").label("Allow (Alt+Y)").on_click(
                    |_, window, cx| window.dispatch_action(Box::new(AllowAgentChange), cx),
                ))
                .child(
                    Button::new("agent-deny")
                        .primary()
                        .label("Deny (Enter)")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(DenyAgentChange), cx)
                        }),
                ),
        )
    }

    pub(super) fn user_is_editing(&self, cx: &App) -> bool {
        self.formula_bar.is_some() || self.grid.read(cx).editor().is_some()
    }

    // A bar that asks the user something takes the keyboard only when the user is not
    // typing, so a keystroke meant for a cell never answers it; the focus it took goes
    // back where it was.
    pub(super) fn take_focus_for_bar(
        &self,
        bar: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<FocusHandle> {
        if self.user_is_editing(cx) {
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

    // A change approved after its document was replaced would land in the new one.
    pub(super) fn refuse_pending_agent_change(&mut self) {
        if let Some(pending) = self.agent.pending.take() {
            pending
                .call
                .respond(Err(ToolError::UnknownWorkbook(WorkbookId(
                    pending.generation,
                ))));
        }
    }

    // Dropping the bridge removes the endpoint file, so no client finds a dead pipe.
    pub(super) fn stop_bridge(&mut self) {
        self.agent.bridge = BridgeState::Off;
    }

    // As Excel's Enable Editing: the user vouches for this file, for this session only.
    pub(super) fn let_agents_edit(&mut self, cx: &mut Context<Self>) {
        if self.document.origin == FileOrigin::Internet {
            self.document.origin = FileOrigin::Local;
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

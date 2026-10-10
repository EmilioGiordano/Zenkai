use gpui_kit::*;
use zenkai_agent::chat::session::start;
use zenkai_agent::chat::thread::TurnEnd;
use zenkai_i18n::t;

use super::launch::{self, Live, Prepared};
use super::{ChatPanel, Gate, Link, Start, View, closed};
use crate::agent_settings::AgentConfig;

#[derive(Debug, PartialEq, Eq)]
enum LaunchStep {
    Wait,
    Arm,
    Confirm,
}

// A warm-up never installs or asks; a switch chosen by the user confirms an install first.
fn launch_step(start: Start, warming: bool, approved: bool, needs_install: bool) -> LaunchStep {
    if warming && (!approved || needs_install) {
        LaunchStep::Wait
    } else if approved && !(start == Start::Switch && needs_install) {
        LaunchStep::Arm
    } else {
        LaunchStep::Confirm
    }
}

impl ChatPanel {
    // Starts the agent when the chat opens so its models and modes are listed before the first
    // message. It never installs, asks or reports: only sending a message does that.
    pub(crate) fn warm_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view == View::Chat && self.thread.is_empty() && self.problem.is_none() {
            self.begin(Start::WarmUp, window, cx);
        }
    }

    pub(super) fn begin(&mut self, start: Start, window: &mut Window, cx: &mut Context<Self>) {
        if self.link != Link::Idle {
            return;
        }
        self.link = Link::Preparing;
        let epoch = self.epoch;
        let settings = cx.global::<AgentConfig>().state.current.clone();
        cx.spawn_in(window, async move |this, cx| {
            // Read after the caller returns: the workspace is still being updated when it opens the chat.
            let Ok(hint) = this.read_with(cx, |this, cx| this.folder_hint(cx)) else {
                return;
            };
            let planned = cx
                .background_executor()
                .spawn(async move { launch::plan_launch(&settings, &hint) })
                .await;
            closed(this.update_in(cx, |this, window, cx| {
                this.planned(epoch, start, planned, window, cx)
            }));
        })
        .detach();
    }

    pub(super) fn planned(
        &mut self,
        epoch: u64,
        start: Start,
        planned: Result<Prepared, launch::PrepareError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if epoch != self.epoch {
            return;
        }
        let warming = start == Start::WarmUp && self.queued.is_none();
        match planned {
            Err(_) if warming => self.link = Link::Idle,
            Err(error) => self.fail_before_start(error.to_string(), cx),
            Ok(prepared) => {
                let approved = cx.update_global::<AgentConfig, _>(|config, _| {
                    config
                        .approvals
                        .is_approved(&prepared.id, &prepared.server, &prepared.plan)
                });
                let needs_install = launch::needs_install(&prepared.plan);
                match launch_step(start, warming, approved, needs_install) {
                    LaunchStep::Wait => self.link = Link::Idle,
                    LaunchStep::Arm => self.arm(prepared, window, cx),
                    LaunchStep::Confirm => {
                        let focus = cx.focus_handle();
                        window.focus(&focus, cx);
                        self.link = Link::Confirming;
                        self.gate = Some(Gate { prepared, focus });
                        cx.notify();
                    }
                }
            }
        }
    }

    pub(super) fn arm(&mut self, prepared: Prepared, window: &mut Window, cx: &mut Context<Self>) {
        self.link = Link::Preparing;
        let epoch = self.epoch;
        let endpoint = self.endpoint.clone();
        cx.spawn_in(window, async move |this, cx| {
            let armed = {
                let prepared = prepared.clone();
                cx.background_executor()
                    .spawn(async move { launch::arm(&prepared, endpoint) })
                    .await
            };
            closed(this.update_in(cx, |this, _, cx| this.started(epoch, prepared, armed, cx)));
        })
        .detach();
    }

    pub(super) fn started(
        &mut self,
        epoch: u64,
        prepared: Prepared,
        armed: Result<launch::Armed, launch::PrepareError>,
        cx: &mut Context<Self>,
    ) {
        if epoch != self.epoch {
            return;
        }
        let armed = match armed {
            Ok(armed) => armed,
            Err(error) => return self.fail_before_start(error.to_string(), cx),
        };
        let (handle, events, run) = start(armed.config);
        cx.background_executor().spawn(run).detach();
        self.live = Some(Live {
            handle,
            agent: prepared.id,
            ready: false,
            folder: prepared.folder,
            _bridge: armed.bridge,
        });
        self.link = Link::Starting;
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| this.on_event(epoch, event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn fail_before_start(&mut self, text: String, cx: &mut Context<Self>) {
        self.link = Link::Idle;
        self.queued = None;
        self.finish_turn(TurnEnd::Failed(text), cx);
    }

    pub(super) fn confirm_launch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(gate) = self.gate.take() else {
            return;
        };
        cx.update_global::<AgentConfig, _>(|config, _| {
            config.approvals.approve(
                &gate.prepared.id,
                &gate.prepared.server,
                Some(&gate.prepared.plan),
            );
        });
        self.focus_composer(window, cx);
        self.arm(gate.prepared, window, cx);
    }

    pub(super) fn decline_launch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.gate.take().is_none() {
            return;
        }
        self.queued = None;
        self.link = Link::Idle;
        self.finish_turn(TurnEnd::Failed(t!("chat.not_confirmed").to_string()), cx);
        self.focus_composer(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{LaunchStep, Start, launch_step};

    #[test]
    fn a_warm_up_waits_instead_of_installing_or_asking() {
        assert_eq!(
            launch_step(Start::WarmUp, true, true, true),
            LaunchStep::Wait
        );
        assert_eq!(
            launch_step(Start::WarmUp, true, false, false),
            LaunchStep::Wait
        );
        assert_eq!(
            launch_step(Start::WarmUp, true, true, false),
            LaunchStep::Arm
        );
    }

    #[test]
    fn a_switch_confirms_an_install_even_when_approved() {
        assert_eq!(
            launch_step(Start::Switch, false, true, true),
            LaunchStep::Confirm
        );
        assert_eq!(
            launch_step(Start::Switch, false, false, false),
            LaunchStep::Confirm
        );
        assert_eq!(
            launch_step(Start::Switch, false, true, false),
            LaunchStep::Arm
        );
    }
}

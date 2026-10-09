use gpui_kit::*;
use zenkai_agent::chat::session::start;
use zenkai_agent::chat::thread::TurnEnd;

use super::launch::{self, Live, Prepared};
use super::{ChatPanel, Gate, Link, closed};
use crate::agent_settings::AgentConfig;

impl ChatPanel {
    pub(super) fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.link != Link::Idle {
            return;
        }
        self.link = Link::Preparing;
        let epoch = self.epoch;
        let settings = cx.global::<AgentConfig>().state.current.clone();
        cx.spawn_in(window, async move |this, cx| {
            let planned = cx
                .background_executor()
                .spawn(async move { launch::plan_launch(&settings) })
                .await;
            closed(this.update_in(cx, |this, window, cx| {
                this.planned(epoch, planned, window, cx)
            }));
        })
        .detach();
    }

    pub(super) fn planned(
        &mut self,
        epoch: u64,
        planned: Result<Prepared, launch::PrepareError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if epoch != self.epoch {
            return;
        }
        match planned {
            Err(error) => self.fail_before_start(error.to_string(), cx),
            Ok(prepared) => {
                let approved = cx
                    .global::<AgentConfig>()
                    .approvals
                    .is_approved(&prepared.id, &prepared.server);
                if approved {
                    self.arm(prepared, window, cx);
                } else {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    self.link = Link::Confirming;
                    self.gate = Some(Gate { prepared, focus });
                    cx.notify();
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
            config
                .approvals
                .approve(&gate.prepared.id, &gate.prepared.server);
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
        self.finish_turn(
            TurnEnd::Failed(
                "The agent was not started because you did not confirm its command.".to_string(),
            ),
            cx,
        );
        self.focus_composer(window, cx);
    }
}

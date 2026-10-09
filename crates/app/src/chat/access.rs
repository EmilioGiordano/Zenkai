use zenkai_agent::chat::state::Select;
use zenkai_agent::settings::PermissionMode;
use zenkai_i18n::t;

// What the user lets the agent do. Only these three exist: an agent's bypass or auto modes are
// never offered, and no mode id outside these lists is ever sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Access {
    AskBeforeWriting,
    EditAutomatically,
    Plan,
}

impl Access {
    pub const ALL: [Access; 3] = [
        Access::AskBeforeWriting,
        Access::EditAutomatically,
        Access::Plan,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Access::AskBeforeWriting => t!("chat.access.ask"),
            Access::EditAutomatically => t!("chat.access.edit"),
            Access::Plan => t!("chat.access.plan"),
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            Access::AskBeforeWriting => t!("chat.access.ask.note"),
            Access::EditAutomatically => t!("chat.access.edit.note"),
            Access::Plan => t!("chat.access.plan.note"),
        }
    }

    pub fn from_setting(mode: PermissionMode) -> Access {
        match mode {
            PermissionMode::ReadOnly => Access::Plan,
            PermissionMode::AskBeforeWrite => Access::AskBeforeWriting,
            PermissionMode::Automatic => Access::EditAutomatically,
        }
    }

    // Plan is read-only for the MCP bridge too: the agent's own plan mode does not promise to
    // block MCP write tools.
    pub fn setting(self) -> PermissionMode {
        match self {
            Access::Plan => PermissionMode::ReadOnly,
            Access::AskBeforeWriting => PermissionMode::AskBeforeWrite,
            Access::EditAutomatically => PermissionMode::Automatic,
        }
    }

    pub fn from_session_mode(mode: &str) -> Option<Access> {
        Access::ALL
            .into_iter()
            .find(|access| access.session_modes().contains(&mode))
    }

    fn session_modes(self) -> &'static [&'static str] {
        match self {
            Access::AskBeforeWriting => &["default", "ask"],
            Access::EditAutomatically => &["acceptEdits", "autoEdit"],
            Access::Plan => &["plan"],
        }
    }

    pub fn session_mode(self, modes: &Select) -> Option<String> {
        self.session_modes()
            .iter()
            .find(|id| modes.offers(id))
            .map(|id| (*id).to_string())
    }
}

#[cfg(test)]
mod tests {
    use zenkai_agent::chat::state::{Choice, ConfigId, ConfigKind, ConfigSource};

    use super::*;

    fn modes(ids: &[&str]) -> Select {
        Select {
            id: ConfigId::new("mode"),
            label: "Mode".to_string(),
            kind: ConfigKind::Mode,
            source: ConfigSource::ConfigOption,
            current: ids[0].to_string(),
            choices: ids
                .iter()
                .map(|id| Choice {
                    value: (*id).to_string(),
                    label: (*id).to_string(),
                    description: None,
                })
                .collect(),
        }
    }

    #[test]
    fn claude_modes_map_to_the_three_choices() {
        let claude = modes(&[
            "default",
            "acceptEdits",
            "plan",
            "auto",
            "bypassPermissions",
        ]);
        assert_eq!(
            Access::AskBeforeWriting.session_mode(&claude).as_deref(),
            Some("default")
        );
        assert_eq!(
            Access::EditAutomatically.session_mode(&claude).as_deref(),
            Some("acceptEdits")
        );
        assert_eq!(Access::Plan.session_mode(&claude).as_deref(), Some("plan"));
    }

    #[test]
    fn bypass_and_auto_modes_are_never_chosen() {
        let risky = modes(&["bypassPermissions", "auto", "yolo", "full-access"]);
        for access in Access::ALL {
            assert_eq!(access.session_mode(&risky), None);
        }
        for access in Access::ALL {
            for id in access.session_modes() {
                let lowered = id.to_lowercase();
                assert!(!lowered.contains("bypass") && !lowered.contains("yolo"));
                assert_ne!(*id, "auto");
            }
        }
    }

    #[test]
    fn only_edit_automatically_lets_zenkai_write_without_asking() {
        for access in Access::ALL {
            let writes_freely = access.setting() == PermissionMode::Automatic;
            assert_eq!(writes_freely, access == Access::EditAutomatically);
        }
    }

    #[test]
    fn plan_cannot_write_through_the_bridge() {
        assert_eq!(Access::Plan.setting(), PermissionMode::ReadOnly);
        for access in Access::ALL {
            assert_eq!(Access::from_setting(access.setting()), access);
        }
    }

    #[test]
    fn session_modes_map_back_to_a_choice() {
        assert_eq!(Access::from_session_mode("plan"), Some(Access::Plan));
        assert_eq!(
            Access::from_session_mode("acceptEdits"),
            Some(Access::EditAutomatically)
        );
        assert_eq!(Access::from_session_mode("bypassPermissions"), None);
    }
}

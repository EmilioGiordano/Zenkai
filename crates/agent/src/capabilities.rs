use crate::settings::PermissionMode;
use crate::tools::AgentAccess;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    ReadWorkbooks,
    OpenWorkbooks,
    EditCells,
    CreateWorkbooks,
    ChangeStructure,
    FileSystem,
    Macros,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grant {
    Always,
    AsksFirst,
    Never,
}

impl Capability {
    pub const ALL: [Capability; 7] = [
        Capability::ReadWorkbooks,
        Capability::OpenWorkbooks,
        Capability::EditCells,
        Capability::CreateWorkbooks,
        Capability::ChangeStructure,
        Capability::FileSystem,
        Capability::Macros,
    ];

    // The bridge tools behind each capability. A capability with no tools is something
    // Zenkai gives agents no way to do; a test keeps this list equal to the real catalog.
    pub fn tools(self) -> &'static [&'static str] {
        match self {
            Capability::ReadWorkbooks => &[
                "list_workbooks",
                "list_sheets",
                "get_selection",
                "read_range",
                "find",
            ],
            Capability::OpenWorkbooks => &["open_workbook"],
            Capability::EditCells => &[
                "write_cells",
                "set_formula",
                "format_range",
                "generate_data",
            ],
            Capability::CreateWorkbooks => &["create_workbook"],
            Capability::ChangeStructure | Capability::FileSystem | Capability::Macros => &[],
        }
    }

    pub fn tool_count() -> usize {
        Capability::ALL.iter().map(|c| c.tools().len()).sum()
    }
}

// `access` is the state of the workbook in front of the user: Protected View and values-only
// files stay read-only whatever the mode.
pub fn grant(capability: Capability, permission: PermissionMode, access: AgentAccess) -> Grant {
    let by_mode = match permission {
        PermissionMode::ReadOnly => Grant::Never,
        PermissionMode::AskBeforeWrite => Grant::AsksFirst,
        PermissionMode::Automatic => Grant::Always,
    };
    match capability {
        Capability::ReadWorkbooks | Capability::OpenWorkbooks => Grant::Always,
        Capability::EditCells => match access {
            AgentAccess::Editable => by_mode,
            AgentAccess::ReadOnly(_) => Grant::Never,
        },
        Capability::CreateWorkbooks => by_mode,
        Capability::ChangeStructure | Capability::FileSystem | Capability::Macros => Grant::Never,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::bridge;
    use crate::tools::ReadOnlyReason;

    const EDITABLE: AgentAccess = AgentAccess::Editable;

    fn grants(permission: PermissionMode, access: AgentAccess) -> Vec<Grant> {
        Capability::ALL
            .iter()
            .map(|capability| grant(*capability, permission, access))
            .collect()
    }

    #[test]
    fn capabilities_cover_exactly_the_bridge_tools() {
        let listed: Vec<&str> = Capability::ALL
            .iter()
            .flat_map(|capability| capability.tools().iter().copied())
            .collect();
        let unique: BTreeSet<&str> = listed.iter().copied().collect();
        assert_eq!(
            unique.len(),
            listed.len(),
            "a tool belongs to two capabilities"
        );
        let real = bridge::tools();
        let real: BTreeSet<&str> = real.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(unique, real);
        assert_eq!(Capability::tool_count(), real.len());
    }

    #[test]
    fn read_only_tools_are_always_granted_and_the_others_follow_the_mode() {
        let real = bridge::tools();
        for tool in &real {
            let read_only = tool
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint)
                == Some(true);
            let capability = Capability::ALL
                .iter()
                .find(|capability| capability.tools().contains(&tool.name.as_ref()))
                .unwrap();
            let in_plan = grant(*capability, PermissionMode::ReadOnly, EDITABLE);
            assert_eq!(in_plan == Grant::Always, read_only, "{}", tool.name);
        }
    }

    #[test]
    fn plan_asks_and_automatic_modes_differ_only_on_writes() {
        use Grant::{Always, AsksFirst, Never};
        assert_eq!(
            grants(PermissionMode::ReadOnly, EDITABLE),
            [Always, Always, Never, Never, Never, Never, Never]
        );
        assert_eq!(
            grants(PermissionMode::AskBeforeWrite, EDITABLE),
            [Always, Always, AsksFirst, AsksFirst, Never, Never, Never]
        );
        assert_eq!(
            grants(PermissionMode::Automatic, EDITABLE),
            [Always, Always, Always, Always, Never, Never, Never]
        );
    }

    #[test]
    fn a_read_only_file_blocks_cell_edits_in_every_mode_but_not_new_workbooks() {
        for reason in [
            ReadOnlyReason::ProtectedView,
            ReadOnlyReason::ValuesOnlyFile,
        ] {
            let access = AgentAccess::ReadOnly(reason);
            for permission in PermissionMode::ALL {
                assert_eq!(
                    grant(Capability::EditCells, permission, access),
                    Grant::Never
                );
            }
            assert_eq!(
                grant(
                    Capability::CreateWorkbooks,
                    PermissionMode::Automatic,
                    access
                ),
                Grant::Always
            );
        }
    }
}

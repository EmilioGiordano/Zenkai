use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

pub const NODE_INSTALL_HINT: &str = "Install Node.js LTS from https://nodejs.org or run \
     \"winget install OpenJS.NodeJS.LTS\", then detect again.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Program {
    Node,
    Npx,
    Claude,
    Gemini,
}

impl Program {
    pub const ALL: [Program; 4] = [
        Program::Node,
        Program::Npx,
        Program::Claude,
        Program::Gemini,
    ];

    pub fn executable(self) -> &'static str {
        match self {
            Program::Node => "node",
            Program::Npx => "npx",
            Program::Claude => "claude",
            Program::Gemini => "gemini",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Program::Node => "Node.js",
            Program::Npx => "npx",
            Program::Claude => "Claude Code",
            Program::Gemini => "Gemini CLI",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl NodeVersion {
    pub fn parse(text: &str) -> Option<NodeVersion> {
        let mut parts = text.trim().strip_prefix('v')?.splitn(3, '.');
        let mut next = || parts.next()?.parse().ok();
        Some(NodeVersion {
            major: next()?,
            minor: next()?,
            patch: next()?,
        })
    }
}

impl fmt::Display for NodeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeStatus {
    Missing,
    Installed(NodeVersion),
    Unreadable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    pub found: BTreeMap<Program, PathBuf>,
    pub node: NodeStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeProblem {
    Missing,
    TooOld { found: NodeVersion, needed: u32 },
    Unreadable(String),
}

impl fmt::Display for NodeProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeProblem::Missing => write!(f, "Node.js is not installed. {NODE_INSTALL_HINT}"),
            NodeProblem::TooOld { found, needed } => write!(
                f,
                "Node.js {found} is too old; this agent needs {needed} or newer. {NODE_INSTALL_HINT}"
            ),
            NodeProblem::Unreadable(why) => {
                write!(
                    f,
                    "Node.js was found but its version could not be read: {why}"
                )
            }
        }
    }
}

impl Detection {
    pub fn node_problem(&self, needed_major: u32) -> Option<NodeProblem> {
        match &self.node {
            NodeStatus::Missing => Some(NodeProblem::Missing),
            NodeStatus::Unreadable(why) => Some(NodeProblem::Unreadable(why.clone())),
            NodeStatus::Installed(version) if version.major < needed_major => {
                Some(NodeProblem::TooOld {
                    found: *version,
                    needed: needed_major,
                })
            }
            NodeStatus::Installed(_) => None,
        }
    }
}

pub fn detect(
    find: impl Fn(&str) -> Option<PathBuf>,
    node_version: impl Fn(&Path) -> std::io::Result<String>,
) -> Detection {
    let found: BTreeMap<Program, PathBuf> = Program::ALL
        .into_iter()
        .filter_map(|program| Some((program, find(program.executable())?)))
        .collect();
    let node = match found.get(&Program::Node) {
        None => NodeStatus::Missing,
        Some(path) => match node_version(path) {
            Ok(output) => NodeVersion::parse(&output).map_or_else(
                || NodeStatus::Unreadable(format!("unexpected output \"{}\"", output.trim())),
                NodeStatus::Installed,
            ),
            Err(error) => NodeStatus::Unreadable(error.to_string()),
        },
    };
    Detection { found, node }
}

// npm installs global command shims in %APPDATA%\npm, which a fresh login has on PATH
// but a process started before the install may not. Relative entries are dropped: they
// would resolve against whatever folder the search runs in, where anyone could plant a
// node.exe.
pub fn search_folders(path: &OsStr, appdata: Option<&OsStr>) -> Vec<PathBuf> {
    std::env::split_paths(path)
        .chain(appdata.map(|appdata| PathBuf::from(appdata).join("npm")))
        .filter(|folder| folder.is_absolute())
        .collect()
}

pub fn search_path() -> OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let folders = search_folders(&path, std::env::var_os("APPDATA").as_deref());
    match std::env::join_paths(folders) {
        Ok(joined) => joined,
        Err(error) => {
            tracing::warn!(%error, "agent search path has an unusable folder; detection is skipped");
            OsString::new()
        }
    }
}

pub fn find_in(search_path: &OsStr, name: &str) -> Option<PathBuf> {
    let cwd = std::env::temp_dir();
    which::which_in(name, Some(search_path), cwd).ok()
}

pub fn run_version(program: &Path) -> std::io::Result<String> {
    let mut command = std::process::Command::new(program);
    command.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "exited with {}",
            output.status
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> impl Fn(&str) -> Option<PathBuf> {
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        move |name| {
            names
                .contains(&name.to_string())
                .then(|| PathBuf::from(format!("C:\\bin\\{name}.exe")))
        }
    }

    #[test]
    fn versions_parse_like_node_prints_them() {
        assert_eq!(
            NodeVersion::parse("v22.11.0\r\n"),
            Some(NodeVersion {
                major: 22,
                minor: 11,
                patch: 0
            })
        );
        for bad in ["22.11.0", "v22", "vx.1.2", ""] {
            assert_eq!(NodeVersion::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn found_programs_and_the_node_version_are_reported() {
        let detection = detect(installed(&["node", "npx", "claude"]), |_| {
            Ok("v20.5.1".to_string())
        });
        assert_eq!(detection.found.len(), 3);
        assert!(!detection.found.contains_key(&Program::Gemini));
        assert_eq!(
            detection.node,
            NodeStatus::Installed(NodeVersion {
                major: 20,
                minor: 5,
                patch: 1
            })
        );
        assert_eq!(detection.node_problem(20), None);
        assert!(matches!(
            detection.node_problem(22),
            Some(NodeProblem::TooOld { needed: 22, .. })
        ));
    }

    #[test]
    fn missing_node_is_never_run_and_explains_the_install() {
        let detection = detect(installed(&["claude"]), |_| {
            panic!("node must not run when it is missing")
        });
        assert_eq!(detection.node, NodeStatus::Missing);
        let problem = detection.node_problem(22).unwrap();
        assert!(problem.to_string().contains("nodejs.org"));
    }

    #[test]
    fn a_node_that_fails_or_prints_garbage_is_unreadable() {
        let failing = detect(installed(&["node"]), |_| {
            Err(std::io::Error::other("access denied"))
        });
        assert!(matches!(failing.node, NodeStatus::Unreadable(_)));
        let garbage = detect(installed(&["node"]), |_| Ok("hello".to_string()));
        assert!(matches!(
            garbage.node_problem(20),
            Some(NodeProblem::Unreadable(_))
        ));
    }

    #[test]
    fn relative_path_entries_are_never_searched() {
        let absolute = std::env::temp_dir();
        let path =
            std::env::join_paths([PathBuf::from("."), PathBuf::from("bin"), absolute.clone()])
                .unwrap();
        let folders = search_folders(&path, None);
        assert_eq!(folders, [absolute]);
    }

    #[test]
    fn lookups_search_only_the_given_folders() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "fakeagent.exe"
        } else {
            "fakeagent"
        };
        let program = dir.path().join(name);
        std::fs::write(&program, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(find_in(&path, "fakeagent"), Some(program));
        let empty = tempfile::tempdir().unwrap();
        let elsewhere = std::env::join_paths([empty.path()]).unwrap();
        assert_eq!(find_in(&elsewhere, "fakeagent"), None);
    }
}
